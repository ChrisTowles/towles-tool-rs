/**
 * Pure helpers for the canvas terminal's mouse-selection and right-click
 * context menu. No DOM, no Tauri — unit-tested alongside `term-protocol`.
 */

/** A selection gesture understood by the `term_select` command. */
export type SelectionKind = "drag" | "word" | "line" | "all" | "clear";

/**
 * The selection kind a left mouse-down implies from its click count: a
 * double-click selects the word, a triple (or higher) click the line, and a
 * single click begins a drag range.
 */
export function selectionKindForDetail(detail: number): "word" | "line" | "drag" {
  if (detail === 2) return "word";
  if (detail >= 3) return "line";
  return "drag";
}

/**
 * Whether a completed selection gesture should copy to the clipboard under the
 * copy-on-select preference: only when the preference is enabled and the
 * gesture actually produced a selection (never on a `clear`).
 */
export function shouldCopyOnSelect(enabled: boolean, kind: SelectionKind): boolean {
  return enabled && kind !== "clear";
}

/**
 * Whether any row carries a selection range. Drives the context menu's Copy
 * item, which is enabled only when there is something to copy.
 */
export function rowsHaveSelection(lines: { sel?: [number, number] }[]): boolean {
  return lines.some((l) => l.sel !== undefined);
}
