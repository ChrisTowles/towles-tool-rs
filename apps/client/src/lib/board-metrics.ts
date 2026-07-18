import { TASK_STATUSES, type TaskItem, type TaskStatus } from "@/lib/data";

/**
 * Pure due-date + column-load math for the Board kanban, factored out so the
 * boundary cases (local-day rollover, exactly-now, missing due date) are unit
 * tested without rendering the screen. All times are epoch ms; `now` is the
 * shared wall clock (`useNow`), never read from the clock in here.
 */

/** How a card's due date reads against `now`:
 * - `overdue` — past its due instant (drives the red accent)
 * - `today` — due later in the current local calendar day (amber accent)
 * - `future` — due on a later day (no accent)
 * - `none` — no due date set */
export type DueState = "overdue" | "today" | "future" | "none";

/** Classify a due date relative to `now`. "today" uses the local-day boundary,
 * so a card due at end-of-today reads amber until the day actually ends, and
 * one due yesterday flips to overdue the moment midnight passes. */
export function dueState(dueTs: number | null | undefined, now: number): DueState {
  if (dueTs == null) return "none";
  if (dueTs < now) return "overdue";
  const endOfToday = new Date(now);
  endOfToday.setHours(23, 59, 59, 999);
  return dueTs <= endOfToday.getTime() ? "today" : "future";
}

/** Total cards in each status column. */
export function countByStatus(
  tasks: readonly Pick<TaskItem, "status">[],
): Record<TaskStatus, number> {
  const counts = Object.fromEntries(TASK_STATUSES.map((s) => [s, 0])) as Record<TaskStatus, number>;
  for (const t of tasks) counts[t.status] += 1;
  return counts;
}

/** Overdue cards in each status column. `done` is never counted — a shipped
 * card is not "late" no matter when it was due, matching the card accent. */
export function overdueByStatus(
  tasks: readonly Pick<TaskItem, "status" | "dueTs">[],
  now: number,
): Record<TaskStatus, number> {
  const counts = Object.fromEntries(TASK_STATUSES.map((s) => [s, 0])) as Record<TaskStatus, number>;
  for (const t of tasks) {
    if (t.status !== "done" && dueState(t.dueTs, now) === "overdue") counts[t.status] += 1;
  }
  return counts;
}
