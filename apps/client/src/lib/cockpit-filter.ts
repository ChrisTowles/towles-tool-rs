/**
 * Client-side repo filter for the Cockpit. Narrows the PR and issue panels to a
 * single repo (the "All" chip clears it) so you can zero in on one repo when
 * zoning in. Pure and host-independent so it unit-tests without React or the
 * Tauri shell — the same functions feed the panels and their note counts, so
 * the two never drift.
 */
import type { IssueItem, PrItem } from "@/lib/data";

/**
 * The distinct repos present across the Cockpit's PRs and issues, sorted for a
 * stable chip order. Empty when nothing has been collected yet.
 */
export function cockpitRepos(
  prs: readonly Pick<PrItem, "repo">[],
  issues: readonly Pick<IssueItem, "repo">[],
): string[] {
  const set = new Set<string>();
  for (const p of prs) set.add(p.repo);
  for (const i of issues) set.add(i.repo);
  return [...set].toSorted();
}

/**
 * Narrow a list of repo-tagged items to a single selected repo. A `null`
 * selection (the "All" chip) matches everything, so both panels and the counts
 * derived from them stay consistent with the chip state.
 */
export function filterByRepo<T extends { repo: string }>(
  items: readonly T[],
  selected: string | null,
): T[] {
  if (selected === null) return [...items];
  return items.filter((item) => item.repo === selected);
}
