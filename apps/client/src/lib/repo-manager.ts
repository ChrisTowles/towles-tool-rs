/**
 * Pure logic behind Settings → Agentboard → Repos, the *one* place repos are
 * managed (tracked, untracked, reordered, themed). The component stays thin;
 * everything decidable without a DOM lives here so it unit-tests (see
 * `apps/client/CLAUDE.md`'s logic-only testing convention).
 *
 * Order is expressed as a list of tracked repo **dirs** — the same key
 * `ab_set_repo_order` takes and the same key the rail's `repoPaths` is stored
 * under. Never the display name: names are collision-disambiguated on the Rust
 * side and shift as repos come and go.
 */

/** A row in the discovery list (`ab_discover_repos`). Re-declared structurally
 * rather than imported so this module stays free of the agentboard state
 * machinery. */
export type CandidateLike = { name: string; dir: string; active: boolean };

/**
 * Move `dragged` so it sits immediately before `beforeDir`, or at the end when
 * `beforeDir` is `"end"`. A dir that isn't in the list, or a no-op drop onto
 * itself, returns the list unchanged (identity is not preserved — callers
 * compare with {@link sameOrder}, not by reference).
 */
export function reorderDirs(
  dirs: readonly string[],
  dragged: string,
  beforeDir: string | "end",
): string[] {
  if (!dirs.includes(dragged) || dragged === beforeDir) return [...dirs];
  const rest = dirs.filter((d) => d !== dragged);
  if (beforeDir === "end") return [...rest, dragged];
  const at = rest.indexOf(beforeDir);
  if (at < 0) return [...rest, dragged];
  return [...rest.slice(0, at), dragged, ...rest.slice(at)];
}

/**
 * Apply an optimistic order to the repos of the latest backend snapshot.
 *
 * The snapshot is authoritative about *which* repos exist; `order` is only a
 * claim about their sequence, so a repo the order doesn't mention (tracked in
 * another window between the drop and the next poll) keeps its snapshot
 * position at the end rather than disappearing. `null` order = render the
 * snapshot as-is.
 */
export function applyRepoOrder<T extends { dir: string }>(
  repos: readonly T[],
  order: readonly string[] | null,
): T[] {
  if (!order) return [...repos];
  const rank = new Map(order.map((dir, i) => [dir, i]));
  return repos.toSorted((a, b) => {
    const ra = rank.get(a.dir) ?? Number.MAX_SAFE_INTEGER;
    const rb = rank.get(b.dir) ?? Number.MAX_SAFE_INTEGER;
    return ra - rb;
  });
}

/** True when two dir lists are the same sequence — how the optimistic overlay
 * knows the backend caught up and can be dropped. */
export function sameOrder(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((dir, i) => dir === b[i]);
}

/**
 * Has the backend caught up with an optimistic drag, so the overlay can go?
 *
 * "Settled" is *relative* order of the repos the drag actually named — not an
 * exact list match. `ab_set_repo_order` is deliberately merge-tolerant, so a
 * repo tracked in another window between the drop and the next poll shows up
 * in `snapshotDirs` and never in `order`; requiring equality there would pin
 * the overlay forever and permanently mask the backend's real order. Repos the
 * drag named that have since been untracked are likewise ignored.
 */
export function orderSettled(
  order: readonly string[] | null,
  snapshotDirs: readonly string[],
): boolean {
  if (order === null) return false;
  const tracked = new Set(snapshotDirs);
  return sameOrder(
    snapshotDirs.filter((dir) => order.includes(dir)),
    order.filter((dir) => tracked.has(dir)),
  );
}

/** Discovered repos that aren't tracked yet. Both `active` and the tracked-dir
 * set are consulted: `active` can lag a just-issued track by one poll. */
export function untrackedCandidates(
  candidates: readonly CandidateLike[],
  trackedDirs: ReadonlySet<string>,
): CandidateLike[] {
  return candidates.filter((c) => !c.active && !trackedDirs.has(c.dir));
}

/**
 * Should the "Add path" affordance show for this query? Only for an absolute
 * path that nothing on either list already covers — otherwise it duplicates a
 * row the user can just click.
 */
export function showAddPath(
  query: string,
  candidates: readonly CandidateLike[],
  trackedDirs: ReadonlySet<string>,
): boolean {
  const path = query.trim();
  if (!path.startsWith("/")) return false;
  if (trackedDirs.has(path)) return false;
  return !candidates.some((c) => c.dir === path);
}
