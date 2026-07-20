import { ownerRepoFromOrigin } from "@/lib/agentboard";
import { TASK_STATUSES, type TaskItem, type TaskStatus } from "@/lib/data";

/** The bucket for tasks with no discoverable repo. Sorts last. */
export const NO_REPO_GROUP = "__no_repo__";

/**
 * A Board swimlane: one repo's tasks, in the order the lanes are rendered.
 *
 * `key` stays stable across snapshots so React can keep lane identity as tasks
 * move — it must not be derived from anything that changes with a card's
 * status or position; repo identity only.
 */
export type TaskGroup = {
  key: string;
  /** What the lane header shows — the bare repo name, not `owner/name`. */
  label: string;
  tasks: TaskItem[];
};

/**
 * The repo a task belongs to, as a stable grouping key.
 *
 * Resolution order matters, and every GitHub-identity source is tried before
 * any filesystem path:
 *
 * 1. `slot.repo` — the `owner/name` bound at submit. It survives the worktree
 *    being removed (`slot_repo`/`slot_repo_root` are kept as historical fact
 *    when `slot_dir` is cleared), so a finished task stays in its lane instead
 *    of jumping to "No repo" the moment its slot is cleaned up.
 * 2. The first issue, then PR link — for tasks that were never slot-bound.
 * 3. `slot.repoRoot`'s basename, last and only as a weak guess: it's a local
 *    directory name, not `owner/name`, so tasks resolved this way can't merge
 *    into a lane keyed by GitHub identity. It only exists so a task bound to a
 *    repo with no parseable GitHub origin still gets a named lane.
 *
 * Exported for direct unit tests; production code goes through
 * [`groupTasksByRepo`].
 */
export function taskRepoKey(task: TaskItem): string {
  const slotRepo = task.slot?.repo?.trim();
  if (slotRepo) return slotRepo;

  const linked = task.issues[0]?.repo ?? task.prs[0]?.repo;
  if (linked) return linked;

  const root = task.slot?.repoRoot?.trim();
  if (root) {
    const base = root
      .replace(/[/\\]+$/, "")
      .split(/[/\\]/)
      .pop();
    if (base) return base;
  }

  return NO_REPO_GROUP;
}

/** The slice of an Agentboard rail repo row this module resolves against —
 * structural so tests don't have to build full `RepoData` values. */
export type RailRepoRow = {
  key: string;
  dir: string;
  originUrl?: string | null;
  folders: { dir: string }[];
};

/**
 * The Agentboard rail row a task belongs to, as the row's focus key
 * (`RepoData.key`) — the id `openTabWithFocus({ screen: "agentboard", kind:
 * "repo" })` scrolls to. `null` when the task's repo isn't on the rail.
 *
 * Path evidence outranks GitHub identity: a task's slot dir / repo root names
 * one specific checkout group, while `owner/name` could match a fork tracked
 * under a different local path. Untracked-repo tasks fall through to `null`
 * rather than guessing.
 */
export function railRepoKeyForTask(repos: RailRepoRow[], task: TaskItem): string | null {
  const dirs = [task.slot?.dir, task.slot?.repoRoot].filter((d): d is string => !!d?.trim());
  for (const repo of repos) {
    if (dirs.some((d) => d === repo.dir || repo.folders.some((f) => f.dir === d))) return repo.key;
  }

  const ghKey = taskRepoKey(task);
  if (ghKey === NO_REPO_GROUP) return null;
  for (const repo of repos) {
    if (ownerRepoFromOrigin(repo.originUrl) === ghKey) return repo.key;
  }
  return null;
}

/** The lane header text for a grouping key: `owner/name` renders as `name`.
 * Exported for direct unit tests. */
export function repoGroupLabel(key: string): string {
  if (key === NO_REPO_GROUP) return "No repo";
  return key.split("/").pop() || key;
}

/** The Board's card order within a status column: `position`, ties broken by
 * creation time. One comparator shared by the lane cells and the drop-time
 * insertion index, so they can never disagree about ordering. */
export function byBoardOrder(a: TaskItem, b: TaskItem): number {
  return a.position - b.position || a.createdAt - b.createdAt;
}

/**
 * Bucket tasks into the five status columns, each sorted in board order.
 */
export function bucketByStatus(tasks: TaskItem[]): Record<TaskStatus, TaskItem[]> {
  const byStatus = Object.fromEntries(TASK_STATUSES.map((s) => [s, [] as TaskItem[]])) as Record<
    TaskStatus,
    TaskItem[]
  >;
  // `?.`: `status` is a closed union in TS, but the value crosses IPC from
  // SQLite — an unknown status from an older/newer db drops that one card
  // rather than crashing the whole board render.
  for (const task of tasks) byStatus[task.status]?.push(task);
  for (const status of TASK_STATUSES) byStatus[status].sort(byBoardOrder);
  return byStatus;
}

/**
 * Bucket tasks into repo swimlanes, alphabetical by label with "No repo" last.
 *
 * Lanes are derived purely from the tasks passed in, so a lane appears and
 * disappears with its work rather than needing to be created or cleaned up —
 * that's what makes the grouping "automatic". Callers pass the already-filtered
 * task list, so filtering to nothing also removes the lane.
 */
export function groupTasksByRepo(tasks: TaskItem[]): TaskGroup[] {
  const byKey = new Map<string, TaskItem[]>();
  for (const task of tasks) {
    const key = taskRepoKey(task);
    const bucket = byKey.get(key);
    if (bucket) bucket.push(task);
    else byKey.set(key, [task]);
  }

  return [...byKey.entries()]
    .map(([key, groupTasks]) => ({ key, label: repoGroupLabel(key), tasks: groupTasks }))
    .toSorted((a, b) => {
      if (a.key === NO_REPO_GROUP) return 1;
      if (b.key === NO_REPO_GROUP) return -1;
      return a.label.localeCompare(b.label) || a.key.localeCompare(b.key);
    });
}
