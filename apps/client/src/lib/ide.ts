/**
 * Frontend half of the Claude Code IDE bridge (see docs/CLAUDE-CODE-IDE.md):
 * every embedded terminal hosts an IDE server in Rust; selecting lines in a
 * folder's file viewer or diff pane routes to the `claude` running in that
 * folder's terminal as selection context. This module wraps the `ide_*`
 * commands and the `ide://status` connect/disconnect event.
 */

import { useEffect, useMemo, useState } from "react";
import { invokeCmd, invokeOk, isTauri } from "@/lib/tauri";
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
      const initial = await invokeCmd<IdeStatus[]>("ide_status");
      if (disposed) return;
      if (initial) setStatuses(Object.fromEntries(initial.map((s) => [s.termId, s])));
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
 * session rooted at `dir`. Fire-and-forget. */
export function ideSetSelection(
  dir: string,
  filePath: string,
  startLine: number,
  endLine: number,
  startChar?: number,
  endChar?: number,
) {
  void invokeCmd("ide_set_selection", {
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
export function ideSetOpenFile(dir: string, filePath: string | null, dirty = false) {
  void invokeCmd("ide_set_open_file", { dir, filePath, dirty });
}

/** A viewer file read: content + the mtime token the save path checks. */
export type FileRead = { content: string; mtimeMs: number };

/** Read a repo file for the code viewer (size-capped, text-only). Returns
 * null in browser dev; throws with a readable message on binary/huge files. */
export function ideReadFile(dir: string, filePath: string): Promise<FileRead | null> {
  return invokeCmd<FileRead>("ide_read_file", { dir, filePath });
}

/** Save the viewer's buffer (atomic; refuses when the file changed on disk
 * since `expectedMtimeMs`). Resolves the new mtime token, or null after an
 * error toast. */
export async function ideWriteFile(
  dir: string,
  filePath: string,
  content: string,
  expectedMtimeMs: number | null,
): Promise<number | null> {
  const { toast } = await import("sonner");
  try {
    const { invokeOrThrow } = await import("@/lib/tauri");
    return await invokeOrThrow<number>("ide_write_file", {
      dir,
      filePath,
      content,
      expectedMtimeMs,
    });
  } catch (e) {
    toast.error(String(e));
    return null;
  }
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
export function ideClearSelection(dir: string, filePath: string) {
  void invokeCmd("ide_clear_selection", { dir, filePath });
}

/** Explicit "send to Claude" (@-mention). Omit the lines for a whole-file
 * mention (the Files tab). Resolves false — after an error toast — when no
 * Claude session is connected in that folder. */
export function ideAtMention(
  dir: string,
  filePath: string,
  startLine?: number,
  endLine?: number,
): Promise<boolean> {
  return invokeOk("ide_at_mention", {
    dir,
    filePath,
    startLine: startLine ?? null,
    endLine: endLine ?? null,
  });
}

/**
 * `ideAtMention` plus the success toast, so every gesture that mentions a file
 * reports itself the same way. `invokeOk` already owns the failure toast, so
 * this is the whole user-facing contract in one place — a null range means the
 * whole file.
 */
export async function ideMention(
  dir: string,
  filePath: string,
  range: MentionRange | null,
): Promise<void> {
  const ok = await ideAtMention(dir, filePath, range?.startLine, range?.endLine);
  if (!ok) return;
  const { toast } = await import("sonner");
  toast.success(`${formatMentionRef(filePath, range)} sent to claude`);
}
