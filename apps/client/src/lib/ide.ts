/**
 * Frontend half of the Claude Code IDE bridge (see docs/CLAUDE-CODE-IDE.md):
 * every embedded terminal hosts an IDE server in Rust; selecting lines in a
 * folder's file viewer or diff pane routes to the `claude` running in that
 * folder's terminal as selection context. This module wraps the `ide_*`
 * commands and the `ide://status` connect/disconnect event.
 */

import { useEffect, useMemo, useState } from "react";
import type { Result } from "better-result";
import { invoke, isTauri } from "@/lib/tauri";
import { errorMessage, type IpcError } from "@/lib/errors";
import { formatMentionRef, type MentionRange } from "@/lib/ide-selection";

/** One terminal's IDE pairing state (mirrors `IdeStatus` in ide.rs). */
export type IdeStatus = {
  termId: string;
  dir: string;
  port: number;
  connected: boolean;
};

const STATUS_EVENT = "ide://status";

/**
 * Whether any Claude Code CLI is currently connected to a terminal rooted at
 * `dir`. Seeds from `ide_status`, then tracks `ide://status` edges.
 */
export function useIdeConnected(dir: string | undefined): boolean {
  const [statuses, setStatuses] = useState<Record<string, IdeStatus>>({});

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const initial = await invoke<IdeStatus[]>("ide_status");
      if (disposed) return;
      if (initial.isOk()) setStatuses(Object.fromEntries(initial.value.map((s) => [s.termId, s])));
      if (!isTauri()) return;
      const { listen } = await import("@tauri-apps/api/event");
      const sub = await listen<IdeStatus>(STATUS_EVENT, (e) => {
        setStatuses((prev) => ({ ...prev, [e.payload.termId]: e.payload }));
      });
      if (disposed) sub();
      else unlisten = sub;
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return useMemo(
    () => !!dir && Object.values(statuses).some((s) => s.connected && s.dir === dir),
    [statuses, dir],
  );
}

/** Push a highlight (1-based inclusive lines; optional 0-based character
 * columns from the code viewer) as the ambient selection of every Claude
 * session rooted at `dir`. Safe to ignore the result — this is ambient context,
 * not an action the user is waiting on. */
export function ideSetSelection(
  dir: string,
  filePath: string,
  startLine: number,
  endLine: number,
  startChar?: number,
  endChar?: number,
): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_set_selection", {
    dir,
    filePath,
    startLine,
    endLine,
    startChar: startChar ?? null,
    endChar: endChar ?? null,
  });
}

/** Tell the folder's sessions which file the code viewer has open
 * (null = closed) and whether it has unsaved edits — surfaces in Claude's
 * getOpenEditors / checkDocumentDirty. */
export function ideSetOpenFile(
  dir: string,
  filePath: string | null,
  dirty = false,
): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_set_open_file", { dir, filePath, dirty });
}

/** Flip one diff-pane file's unsaved-edit state — same
 * getOpenEditors/checkDocumentDirty surface as `ideSetOpenFile`, but additive
 * rather than replacing: unlike the Files tab's single viewer, several diff
 * pane files can be dirty at once, so this upserts (or, on `dirty: false`,
 * clears) just this one path instead of replacing the whole open-file. */
export function ideSetDiffDirty(
  dir: string,
  filePath: string,
  dirty: boolean,
): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_set_diff_dirty", { dir, filePath, dirty });
}

/** A viewer file read: content + the mtime token the save path checks. */
export type FileRead = { content: string; mtimeMs: number };

/** Minimal stat (mirrors `FsStat` in ide.rs). An `Err` means the path does
 * not exist (or is unreadable) — which is exactly what the viewer's
 * deleted-on-disk detection needs to tell apart from a transient read
 * failure on a file that is still there. */
export type FsStat = { isDir: boolean; size: number; mtimeMs: number };

export function ideStat(dir: string, filePath: string): Promise<Result<FsStat, IpcError>> {
  return invoke<FsStat>("ide_stat", { dir, filePath });
}

/** Read a repo file for the code viewer (size-capped, text-only). Fails with a
 * readable message on binary/huge files, and with `NotInTauri` in browser dev. */
export function ideReadFile(dir: string, filePath: string): Promise<Result<FileRead, IpcError>> {
  return invoke<FileRead>("ide_read_file", { dir, filePath });
}

/** Save the viewer's buffer (atomic; refuses when the file changed on disk
 * since `expectedMtimeMs`). Resolves the new mtime token. */
export function ideWriteFile(
  dir: string,
  filePath: string,
  content: string,
  expectedMtimeMs: number | null,
): Promise<Result<number, IpcError>> {
  return invoke<number>("ide_write_file", { dir, filePath, content, expectedMtimeMs });
}

const FILE_CHANGED_EVENT = "ide://file-changed";

/** Payload of `ide://file-changed` — watched viewer files changed on disk
 * (an agent edit, a `git checkout`, any external writer). One event per
 * debounce batch, carrying every touched dir-relative path. */
export type FilesChangedEvent = { dir: string; filePaths: string[] };

/** Start watching open viewer files for on-disk changes; changes arrive as
 * `ide://file-changed` events (subscribe with `onFilesChangedOnDisk`). One
 * call for the whole list — a 50-file diff pane must not pay 50 IPC
 * round-trips. Pair with `ideUnwatchFiles` over the same list when the files
 * close. Failure just means no auto-refresh (browser dev, inotify limits) —
 * safe to fire-and-forget. */
export function ideWatchFiles(dir: string, filePaths: string[]): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_watch_files", { dir, filePaths });
}

/** Drop one reference each to a batch of viewer file watches. Unmatched
 * entries are a no-op. */
export function ideUnwatchFiles(dir: string, filePaths: string[]): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_unwatch_files", { dir, filePaths });
}

/** Subscribe to on-disk-change events for every watched file in `dir` — the
 * diff pane's shape, where one listener covers the whole change set. The
 * callback receives one changed dir-relative path per call (event batches
 * fan out here). Returns an unsubscribe; a no-op outside Tauri. */
export function onFilesChangedOnDisk(dir: string, cb: (filePath: string) => void): () => void {
  if (!isTauri()) return () => {};
  let disposed = false;
  let unlisten: (() => void) | undefined;
  void (async () => {
    const { listen } = await import("@tauri-apps/api/event");
    const sub = await listen<FilesChangedEvent>(FILE_CHANGED_EVENT, (e) => {
      if (e.payload.dir !== dir) return;
      for (const filePath of e.payload.filePaths) cb(filePath);
    });
    if (disposed) sub();
    else unlisten = sub;
  })();
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/** Subscribe to on-disk-change events for one watched file. Returns an
 * unsubscribe; a no-op outside Tauri. */
export function onFileChangedOnDisk(dir: string, filePath: string, cb: () => void): () => void {
  return onFilesChangedOnDisk(dir, (changed) => {
    if (changed === filePath) cb();
  });
}

/** A Monaco model's minimal save-relevant surface — structural, not
 * `monaco-editor`'s `ITextModel`, so this lib module (IPC + IDE-bridge
 * concerns) doesn't need an editor dependency. */
type SavableModel = { getValue(): string; getAlternativeVersionId(): number };

/** A buffer's save-relevant surface, captured synchronously (see
 * `snapshotModel`) so a serialized save chain can write it later — after
 * earlier in-flight writes finished and the mtime token is fresh, possibly
 * after the model itself was disposed. */
export type BufferSnapshot = { value: string; versionAtSave: number };

export function snapshotModel(model: SavableModel): BufferSnapshot {
  return { value: model.getValue(), versionAtSave: model.getAlternativeVersionId() };
}

/**
 * The save sequence every editable Monaco buffer in this app uses —
 * `CodeViewer` (one file) and the diff pane's editable modified side (N
 * files) both need the identical write/error/version-capture steps, just
 * different storage shape for the per-path mtime/version bookkeeping, so
 * that bookkeeping stays with the caller. Callers serialize saves per file
 * (snapshot at request time, write when the previous save finished) —
 * overlapping writes of the same file would race each other's mtime tokens
 * and get one of them refused. On success, returns the new mtime token plus
 * the snapshot's version — comparing that to the model's *current* version
 * afterward tells the caller whether more was typed since and the buffer is
 * therefore still dirty. On failure, toasts and returns `null`: a refused
 * save leaves the buffer dirty and the file untouched, the one failure here
 * the user must never have to infer.
 */
export async function saveBufferSnapshot(
  dir: string,
  filePath: string,
  snapshot: BufferSnapshot,
  expectedMtimeMs: number | null,
): Promise<{ mtimeMs: number; versionAtSave: number } | null> {
  const written = await ideWriteFile(dir, filePath, snapshot.value, expectedMtimeMs);
  if (written.isErr()) {
    const { toast } = await import("sonner");
    toast.error(`Couldn't save ${filePath} — ${written.error.message}`);
    return null;
  }
  return { mtimeMs: written.value, versionAtSave: snapshot.versionAtSave };
}

/** Payload of the `ide://open-file` event (Claude called the openFile tool). */
export type OpenFileRequest = {
  dir: string;
  filePath: string;
  startText?: string | null;
  endText?: string | null;
  selectToEndOfLine?: boolean | null;
};

/** The highlight was dismissed — clear the sessions' selection context. */
export function ideClearSelection(dir: string, filePath: string): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_clear_selection", { dir, filePath });
}

/** Explicit "send to Claude" (@-mention). Omit the lines for a whole-file
 * mention (the Files tab). Fails when no Claude session is connected in that
 * folder. */
export function ideAtMention(
  dir: string,
  filePath: string,
  startLine?: number,
  endLine?: number,
): Promise<Result<void, IpcError>> {
  return invoke<void>("ide_at_mention", {
    dir,
    filePath,
    startLine: startLine ?? null,
    endLine: endLine ?? null,
  });
}

/**
 * `ideAtMention` plus its toasts, so every gesture that mentions a file reports
 * itself the same way — this is the whole user-facing contract in one place. A
 * null range means the whole file.
 */
export async function ideMention(
  dir: string,
  filePath: string,
  range: MentionRange | null,
): Promise<void> {
  const sent = await ideAtMention(dir, filePath, range?.startLine, range?.endLine);
  const { toast } = await import("sonner");
  sent.match({
    ok: () => toast.success(`${formatMentionRef(filePath, range)} sent to claude`),
    err: (e) => toast.error(errorMessage(e)),
  });
}
