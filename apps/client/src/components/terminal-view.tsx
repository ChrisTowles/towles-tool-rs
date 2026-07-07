import { useEffect, useRef } from "react";
import { FitAddon, init, Terminal } from "ghostty-web";

/**
 * Terminal pane rendered with ghostty-web — Ghostty's VT engine compiled to
 * WASM behind an xterm.js-compatible API, painting into a canvas. The Tauri
 * bridge owns the PTY, keyed by `termId`: `term_start` spawns a shell in
 * `cwd` sized to the measured grid, output arrives as base64
 * `terminal://output` events tagged with `termId`, and input/resizes go back
 * through `term_write`/`term_resize`. When the shell exits, `onExit` tells
 * the parent to close the pane; unmounting kills the shell (`term_kill`).
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

    // React 19 StrictMode double-mounts effects in dev; `disposed` keeps the
    // stale mount's async continuation from starting a second shell, and
    // `started` ensures only the mount that spawned the shell kills it.
    let disposed = false;
    let started = false;
    let term: Terminal | undefined;
    const unlisteners: (() => void)[] = [];

    void (async () => {
      // Loads the shared Ghostty WASM instance (embedded in the JS bundle);
      // idempotent, so every mount can await it.
      await init();
      if (disposed) return;

      // Track the app theme: read the resolved colors of the host (styled with
      // Tailwind bg-background/text-foreground) so the terminal matches
      // light/dark.
      const cs = getComputedStyle(host);
      term = new Terminal({
        cursorBlink: true,
        fontSize: 13,
        fontFamily: "ui-monospace, 'JetBrains Mono', 'Fira Code', monospace",
        // ghostty's WASM engine caps history around ~1.5k short lines
        // regardless of this value (tried 1M: eviction onset unchanged), so
        // this is aspirational until upstream honors it.
        scrollback: 10_000,
        // No wheel animation: the ~100ms ease steps in whole lines (janky on
        // WebKitGTK), and an in-flight animation fights the write-time
        // viewport correction in preserveScrollOnWrite.
        smoothScrollDuration: 0,
        theme: {
          background: cs.backgroundColor || "#1e1e2e",
          foreground: cs.color || "#cdd6f4",
        },
      });
      preserveScrollOnWrite(term);
      if (import.meta.env.VITE_WDIO) {
        // Expose live terminals to the drive/e2e harness (wdio builds only).
        const w = window as unknown as { __tt_terms?: Record<string, Terminal> };
        (w.__tt_terms ??= {})[termId] = term;
        unlisteners.push(() => delete w.__tt_terms?.[termId]);
      }
      const fit = new FitAddon();
      term.loadAddon(fit);

      // Paste: ghostty-web lets Ctrl+V fall through to the browser's native
      // paste event, but its own paste handler is text/plain-only — an image
      // on the clipboard (Claude Code's Ctrl+V attach) is silently dropped.
      // Intercept the paste event ourselves instead: an image becomes 0x16
      // (SYN) on the PTY — the byte a Linux terminal sends for Ctrl+V — so
      // TUIs like Claude Code read the image from the system clipboard
      // themselves; text goes through term.paste(), which adds the
      // bracketed-paste markers ghostty-web's native path omits. Registered
      // before term.open() because ghostty-web attaches its own paste
      // listener to this same element there, and same-node listeners fire in
      // registration order — ours must run first to claim the event.
      let ptyWrite: ((data: string) => void) | undefined;
      const onPaste = (e: ClipboardEvent) => {
        const items = e.clipboardData ? Array.from(e.clipboardData.items) : [];
        const hasImage = items.some((it) => it.type.startsWith("image/"));
        const text = e.clipboardData?.getData("text/plain") ?? "";
        if (!hasImage && !text) return; // nothing we handle; leave it to ghostty
        e.preventDefault();
        e.stopImmediatePropagation();
        if (hasImage) ptyWrite?.("\x16");
        else term?.paste(text);
      };
      host.addEventListener("paste", onPaste, { capture: true });
      unlisteners.push(() => host.removeEventListener("paste", onPaste, { capture: true }));

      term.open(host);
      fit.fit();

      // The PTY's window title (OSC 0/2). Claude Code sets `✳ <session title>`;
      // the rail reads it as the live agent label for this session.
      term.onTitleChange((title) => onTitleRef.current?.(termId, title));

      // Outside Tauri there is no PTY bridge; show a note instead of throwing
      // on the missing IPC internals.
      if (!("__TAURI_INTERNALS__" in window)) {
        term.write("terminals require the desktop app (browser dev mode)");
        return;
      }

      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");
      if (disposed) return;

      const onOutput = await listen<{ termId: string; data: string }>(
        "terminal://output",
        (e) => {
          if (e.payload.termId === termId) term?.write(base64ToBytes(e.payload.data));
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
      ptyWrite = (data) => void invoke("term_write", { termId, data }).catch(() => {});
      term.onData((data) => ptyWrite?.(data));
      // FitAddon watches the host with its own ResizeObserver and refits;
      // mirror every grid change to the PTY. Both are torn down by
      // `term.dispose()`, which disposes loaded addons.
      term.onResize(({ cols, rows }) => {
        void invoke("term_resize", { termId, cols, rows }).catch(() => {});
      });
      fit.observeResize();
      term.focus();
    })();

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
      term?.dispose();
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

/**
 * ghostty-web ≤0.4.0 calls scrollToBottom() on every write() when the
 * viewport is scrolled up (coder/ghostty-web#127), so streaming PTY output —
 * Claude Code repaints its status line constantly — yanks the user out of
 * scrollback within a repaint or two. Until the upstream fix (PR #150) ships,
 * wrap the private writeInternal to restore the viewport after each write,
 * shifted by however many lines the write pushed into scrollback so the same
 * content stays on screen. At the bottom (viewportY 0) nothing changes:
 * output still follows live.
 */
function preserveScrollOnWrite(term: Terminal) {
  const t = term as unknown as {
    writeInternal: (data: string | Uint8Array, callback?: () => void) => void;
    targetViewportY: number;
  };
  const original = t.writeInternal.bind(term);
  t.writeInternal = (data, callback) => {
    // viewportY counts lines up from the bottom; 0 means pinned to live output.
    const viewportY = term.getViewportY();
    const scrollbackBefore = term.getScrollbackLength();
    original(data, callback);
    if (viewportY > 0) {
      // Growth can read negative when the engine evicts old pages mid-write;
      // eviction at the top doesn't move content relative to the bottom, so
      // never scroll down for it — only compensate for added lines.
      const grown = Math.max(0, term.getScrollbackLength() - scrollbackBefore);
      term.scrollToLine(viewportY + grown); // clamps to scrollback length
      // Keep the wheel-scroll target in sync so a queued smooth-scroll step
      // can't ease back toward a stale position.
      t.targetViewportY = term.getViewportY();
    }
  };
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}
