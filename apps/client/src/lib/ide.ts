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

/** A viewer file read: content + the mtime token the save path checks. */
export type FileRead = { content: string; mtimeMs: number };

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
