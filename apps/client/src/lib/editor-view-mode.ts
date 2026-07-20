/**
 * How the files pane divides its two halves — the Monaco editor and the
 * rendered Markdown/HTML preview — for a file that has a preview at all.
 *
 * Both halves stay *mounted* in every mode; a mode only collapses one of them.
 * That is load-bearing rather than an optimization: `CodeViewer`'s unmount
 * disposes the Monaco model and its remount re-reads the file from disk, so
 * unmounting the editor to show a full-width preview would drop the undo
 * stack, the scroll position, and any edit that autosave hadn't flushed yet.
 */
export type EditorViewMode = "code" | "split" | "preview";

/** Which panels a mode wants open. */
export function panelsFor(mode: EditorViewMode): { editor: boolean; preview: boolean } {
  return { editor: mode !== "preview", preview: mode !== "code" };
}

/**
 * The inverse — the mode a given pair of open panels represents. Dragging the
 * split handle far enough collapses a panel directly, so the toolbar has to
 * read its state back out of the panels or the highlighted button starts
 * lying about what's on screen.
 */
export function modeForPanels(editor: boolean, preview: boolean): EditorViewMode {
  if (!preview) return "code";
  if (!editor) return "preview";
  return "split";
}
