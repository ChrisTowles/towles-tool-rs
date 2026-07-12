/**
 * Client-side quick filter for the Board. Narrows kanban cards across every
 * column by a case-insensitive substring over the todo's text, its notes, and
 * its repo tag. Pure and host-independent so it unit-tests without React or the
 * Tauri shell.
 */
import type { TaskItem } from "@/lib/data";

/**
 * Does a task match the quick filter? Case-insensitive substring test over the
 * todo's text, its notes, plus its repo tag (if linked). The query is trimmed
 * first, so a whitespace-only query matches everything.
 */
export function matchesTaskFilter(
  task: Pick<TaskItem, "text" | "notes" | "repo">,
  query: string,
): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  const haystack = [task.text, task.notes ?? "", task.repo ?? ""].join(" ").toLowerCase();
  return haystack.includes(q);
}
