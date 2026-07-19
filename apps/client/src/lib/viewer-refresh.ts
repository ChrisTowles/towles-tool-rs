/**
 * What the code viewer does when its open file changes on disk (the
 * `ide://file-changed` event) — the policy behind agent-edit refresh, kept
 * pure so it's testable without Monaco.
 *
 * The mtime compare is the own-save suppressor: the viewer's ⌘S lands on
 * disk and fires the watcher too, but by then the buffer's mtime token
 * already matches the disk, so the echo is ignored instead of re-read into
 * a self-reload loop.
 */

/** How long typing must pause before an editable buffer (the file viewer,
 * the diff pane's modified sides) auto-saves. ⌘S stays as save-now; a file
 * in conflict never auto-saves (resolution is the banner's explicit choice),
 * and neither does a deleted-on-disk one (recreating it is ⌘S's deliberate
 * act). */
export const AUTOSAVE_DELAY_MS = 1000;

export type DiskChangeAction =
  /** Disk matches what the buffer already knows (our own save's echo). */
  | "ignore"
  /** Clean buffer — take the disk content silently, in place. */
  | "reload"
  /** Unsaved edits — never clobber either side silently; raise the banner. */
  | "conflict";

export function diskChangeAction(opts: {
  /** The buffer has edits not yet saved to disk. */
  dirty: boolean;
  /** The mtime token from the buffer's last read/save, if known. */
  bufferMtimeMs: number | null;
  /** The mtime just re-read from disk. */
  diskMtimeMs: number;
}): DiskChangeAction {
  if (opts.bufferMtimeMs !== null && opts.diskMtimeMs === opts.bufferMtimeMs) return "ignore";
  return opts.dirty ? "conflict" : "reload";
}
