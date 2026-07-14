/**
 * Frontend half of the Claude Code IDE bridge (see docs/CLAUDE-CODE-IDE.md):
 * every embedded terminal hosts an IDE server in Rust; highlighting lines in
 * a folder's diff pane routes to the `claude` running in that folder's
 * terminal as selection context. This module wraps the `ide_*` commands and
 * the `ide://status` connect/disconnect event.
 */

import { useEffect, useMemo, useState } from "react";
import { invokeCmd, invokeOk, isTauri } from "@/lib/tauri";

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

/** Push a highlight (1-based inclusive new-file lines) as the ambient
 * selection of every Claude session rooted at `dir`. Fire-and-forget. */
export function ideSetSelection(dir: string, filePath: string, startLine: number, endLine: number) {
  void invokeCmd("ide_set_selection", { dir, filePath, startLine, endLine });
}

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
