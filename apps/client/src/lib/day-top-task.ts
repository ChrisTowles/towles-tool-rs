import type { TaskItem, TaskStatus } from "@/lib/data";

/**
 * Priority order for the day bar's single "top task": what you're actively
 * working on should win over stale backlog. Higher number = shown first.
 * `done` is excluded before ranking, so it needs no rank here.
 */
const STATUS_RANK: Record<Exclude<TaskStatus, "done">, number> = {
  doing: 3,
  review: 2,
  next: 1,
  backlog: 0,
};

/**
 * Pick the one task the day bar should surface: the most in-progress work,
 * not the oldest backlog item. Ranks by status (doing > review > next >
 * backlog), then by column position (the card nearer the top of its column
 * wins the tiebreak). `done` tasks are never eligible. Returns `undefined`
 * when there is nothing to show.
 */
export function pickTopTask(tasks: readonly TaskItem[]): TaskItem | undefined {
  let best: TaskItem | undefined;
  for (const task of tasks) {
    if (task.status === "done") continue;
    if (best === undefined || isHigherPriority(task, best)) {
      best = task;
    }
  }
  return best;
}

function isHigherPriority(a: TaskItem, b: TaskItem): boolean {
  const rankA = STATUS_RANK[a.status as Exclude<TaskStatus, "done">];
  const rankB = STATUS_RANK[b.status as Exclude<TaskStatus, "done">];
  if (rankA !== rankB) return rankA > rankB;
  return a.position < b.position;
}
