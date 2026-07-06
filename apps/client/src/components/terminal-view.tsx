import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

/**
 * xterm.js terminal pane. The Tauri bridge owns the PTY, keyed by `termId`:
 * `term_start` spawns a shell in `cwd` sized to the measured grid, output
 * arrives as base64 `terminal://output` events tagged with `termId`, and
 * input/resizes go back through `term_write`/`term_resize`. When the shell
 * exits, `onExit` tells the parent to close the pane; unmounting kills the
 * shell (`term_kill`).
 *
 * Many of these can be mounted at once (one per agentboard terminal); each
 * filters the shared output/exit events by its own `termId`.
 */
export function TerminalView({
  termId,
  cwd,
  onExit,
  onTitle,
}: {
  termId: string;
  cwd?: string;
  onExit: () => void;
  /** Fires when the PTY sets its window title (OSC 0/2) — e.g. Claude Code
   * emits `✳ <session title>`, which the rail uses as the live session label. */
  onTitle?: (termId: string, title: string) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const onExitRef = useRef(onExit);
  onExitRef.current = onExit;
  const onTitleRef = useRef(onTitle);
  onTitleRef.current = onTitle;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    // Track the app theme: read the resolved colors of the host (styled with
    // Tailwind bg-background/text-foreground) so xterm matches light/dark.
    const cs = getComputedStyle(host);
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "ui-monospace, 'JetBrains Mono', 'Fira Code', monospace",
      scrollback: 10_000,
      theme: {
        background: cs.backgroundColor || "#1e1e2e",
        foreground: cs.color || "#cdd6f4",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    // The PTY's window title (OSC 0/2). Claude Code sets `✳ <session title>`;
    // the rail reads it as the live agent label for this session.
    term.onTitleChange((title) => onTitleRef.current?.(termId, title));

    // React 19 StrictMode double-mounts effects in dev; `disposed` keeps the
    // stale mount's async continuation from starting a second shell, and
    // `started` ensures only the mount that spawned the shell kills it.
    let disposed = false;
    let started = false;
    const unlisteners: (() => void)[] = [];

    void (async () => {
      // Outside Tauri there is no PTY bridge; show a note instead of throwing
      // on the missing IPC internals.
      if (!("__TAURI_INTERNALS__" in window)) {
        term.write("terminals require the desktop app (browser dev mode)");
        return;
      }

      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");

      const onOutput = await listen<{ termId: string; data: string }>(
        "terminal://output",
        (e) => {
          if (e.payload.termId === termId) term.write(base64ToBytes(e.payload.data));
        },
      );
      if (disposed) return onOutput();
      unlisteners.push(onOutput);

      const onExitEvent = await listen<{ termId: string }>("terminal://exit", (e) => {
        if (e.payload.termId === termId) onExitRef.current();
      });
      if (disposed) return onExitEvent();
      unlisteners.push(onExitEvent);

      await invoke("term_start", { termId, cols: term.cols, rows: term.rows, cwd });
      started = true;
      if (disposed) return void invoke("term_kill", { termId }).catch(() => {});
      term.onData((data) => void invoke("term_write", { termId, data }).catch(() => {}));
      term.focus();
    })();

    const observer = new ResizeObserver(() => {
      fit.fit();
      // `fit()` on a pane that just went from `hidden` (0x0) to visible only
      // marks newly-added rows dirty; rows written while the pane was hidden
      // never got painted and stay blank until something (e.g. a scroll)
      // forces a full repaint. Force that repaint explicitly here.
      term.refresh(0, term.rows - 1);
      void import("@tauri-apps/api/core").then(({ invoke }) =>
        invoke("term_resize", { termId, cols: term.cols, rows: term.rows }).catch(() => {}),
      );
    });
    observer.observe(host);

    return () => {
      disposed = true;
      observer.disconnect();
      for (const unlisten of unlisteners) unlisten();
      term.dispose();
      if (started) {
        void import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke("term_kill", { termId }).catch(() => {}),
        );
      }
    };
    // termId/cwd identify the shell; changing them means a different terminal.
  }, [termId, cwd]);

  return <div ref={hostRef} className="size-full bg-background p-1" />;
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}
