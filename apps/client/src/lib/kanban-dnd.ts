import type { TaskStatus } from "./data";

/**
 * Pure drag-and-drop logic for the Board kanban (HTML5 drag events, no
 * dependency): encode a card into a `DataTransfer` payload on drag start,
 * recognize it during dragover, and turn a drop on a column into a status
 * change (or null when the drop is a no-op). Kept Tauri/DOM-free so vitest
 * covers it directly; `board.tsx` only wires events to these functions.
 */

/** Custom `DataTransfer` type identifying a Board card drag. */
export const TASK_DRAG_TYPE = "application/x-tt-task";

export type TaskDragPayload = {
  id: number;
  status: TaskStatus;
};

// Exhaustive by construction: adding a status to the `TaskStatus` union in
// data.ts without listing it here is a compile error (and vice versa).
const STATUS_SET: Record<TaskStatus, true> = {
  backlog: true,
  next: true,
  doing: true,
  review: true,
  done: true,
};

function isTaskStatus(s: unknown): s is TaskStatus {
  return typeof s === "string" && s in STATUS_SET;
}

/** Serialize a card for `dataTransfer.setData(TASK_DRAG_TYPE, …)`. */
export function encodeTaskDrag(payload: TaskDragPayload): string {
  return JSON.stringify(payload);
}

/**
 * Whether an in-flight drag carries a Board card (usable during `dragover`,
 * where the payload itself is not readable — only its types are).
 */
export function isTaskDrag(types: readonly string[]): boolean {
  return types.includes(TASK_DRAG_TYPE);
}

/** Parse a drag payload back into a card reference; null on anything malformed. */
export function decodeTaskDrag(data: string): TaskDragPayload | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const { id, status } = parsed as { id?: unknown; status?: unknown };
  if (typeof id !== "number" || !Number.isInteger(id)) return null;
  if (!isTaskStatus(status)) return null;
  return { id, status };
}

/**
 * Resolve a drop of `data` onto the `target` column: the status move to apply,
 * or null when nothing should happen (unparsable payload, or dropped back on
 * the column it came from).
 */
export function taskDropAction(
  data: string,
  target: TaskStatus,
): { id: number; status: TaskStatus } | null {
  const payload = decodeTaskDrag(data);
  if (!payload || payload.status === target) return null;
  return { id: payload.id, status: target };
}

/**
 * An optimistic `position` for a card dropped between two neighbors, so the
 * Board's `position ASC` sort places it in the new position before the backend
 * round-trips. Fractional values are fine locally — the next `store://snapshot`
 * replaces them with the store's renumbered integers. Pass `null` for a missing
 * neighbor (dropping at the very top or bottom, or into an empty column).
 */
export function reorderedPosition(prevPos: number | null, nextPos: number | null): number {
  if (prevPos === null && nextPos === null) return 0;
  if (prevPos === null) return (nextPos as number) - 1;
  if (nextPos === null) return prevPos + 1;
  return (prevPos + nextPos) / 2;
}
