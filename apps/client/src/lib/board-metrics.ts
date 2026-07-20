import { TASK_STATUSES, type TaskItem, type TaskStatus } from "@/lib/data";

/**
 * Pure column-load math for the Board kanban, factored out so it stays unit
 * tested without rendering the screen.
 */

/** Total cards in each status column. */
export function countByStatus(
  tasks: readonly Pick<TaskItem, "status">[],
): Record<TaskStatus, number> {
  const counts = Object.fromEntries(TASK_STATUSES.map((s) => [s, 0])) as Record<TaskStatus, number>;
  for (const t of tasks) counts[t.status] += 1;
  return counts;
}
