/**
 * Client-side quick filter for the Board. Narrows kanban cards across every
 * column by a case-insensitive substring over the task's text, its notes, the
 * repos of its linked issues/PRs, and its slot branch. Pure and
 * host-independent so it unit-tests without React or the Tauri shell.
 */
import type { TaskItem } from "@/lib/data";

/**
 * Does a task match the quick filter? Case-insensitive substring test over
 * the task's text, its notes, its linked issue/PR repos and numbers, and its
 * slot branch. The query is trimmed first, so a whitespace-only query matches
 * everything.
 */
export function matchesTaskFilter(
  task: Pick<TaskItem, "text" | "notes" | "issues" | "prs" | "slot">,
  query: string,
): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  const haystack = [
    task.text,
    task.notes ?? "",
    ...task.issues.flatMap((l) => [l.repo, `#${l.number}`]),
    ...task.prs.flatMap((l) => [l.repo, `#${l.number}`]),
    task.slot?.branch ?? "",
  ]
    .join(" ")
    .toLowerCase();
  return haystack.includes(q);
}
