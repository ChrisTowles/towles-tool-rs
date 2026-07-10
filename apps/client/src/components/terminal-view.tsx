import { useEffect, useRef } from "react";
import {
  BOLD,
  FAINT,
  INVERSE,
  INVISIBLE,
  ITALIC,
  OVERLINE,
  STRIKETHROUGH,
  UNDERLINE,
  encodeKey,
  encodePaste,
  rgb,
  type Cursor,
  type Frame,
  type Run,
} from "@/lib/term-protocol";

const FONT_SIZE = 13;
const FONT_FAMILY = "ui-monospace, 'JetBrains Mono', 'Fira Code', monospace";
const LINE_HEIGHT = 1.25;

/**
 * Canvas terminal pane over the tt-vt engine. The Tauri bridge owns the PTY
 * and the terminal state (libghostty-vt), keyed by `termId`: `term_start`
 * spawns a shell in `cwd` sized to the measured grid, render frames arrive
 * as `terminal://frame` events (dirty-row style runs + cursor + modes), and
 * input/resize/scroll go back through `term_write`/`term_resize`/
 * `term_scroll`. When the shell exits, `onExit` tells the parent to close
 * the pane; unmounting kills the shell (`term_kill`).
 *
 * Many of these can be mounted at once (one per agentboard terminal); each
 * filters the shared frame/exit events by its own `termId`.
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
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const onExitRef = useRef(onExit);
  onExitRef.current = onExit;
  const onTitleRef = useRef(onTitle);
  onTitleRef.current = onTitle;

  useEffect(() => {
    const host = hostRef.current;
    const canvas = canvasRef.current;
    const input = inputRef.current;
    if (!host || !canvas || !input) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    // Theme: resolved colors of the host (Tailwind bg-background /
    // text-foreground) so the grid matches light/dark.
    const cs = getComputedStyle(host);
    const theme = { bg: cs.backgroundColor || "#1e1e2e", fg: cs.color || "#cdd6f4" };

    // Cell metrics from the actual font.
    ctx.font = `${FONT_SIZE}px ${FONT_FAMILY}`;
    const cellW = ctx.measureText("M").width;
    const cellH = Math.ceil(FONT_SIZE * LINE_HEIGHT);
    const baseline = Math.round((cellH - FONT_SIZE) / 2 + FONT_SIZE * 0.8);

    // Client-side grid mirror: rows of style runs, updated per frame, so any
    // row (cursor moves, resize repaints) can be repainted from local state.
    const grid = {
      cols: Math.max(2, Math.floor(host.clientWidth / cellW)),
      rows: Math.max(1, Math.floor(host.clientHeight / cellH)),
      lines: [] as Run[][],
      cursor: null as Cursor | null,
      modes: { appCursorKeys: false, bracketedPaste: false, altScreen: false, mouseTracking: false },
      scrolledBack: false,
    };

    const setFont = (flags: number) => {
      const bold = flags & BOLD ? "bold " : "";
      const italic = flags & ITALIC ? "italic " : "";
      ctx.font = `${italic}${bold}${FONT_SIZE}px ${FONT_FAMILY}`;
    };

    const paintRow = (y: number) => {
      ctx.fillStyle = theme.bg;
      ctx.fillRect(0, y * cellH, canvas.clientWidth, cellH);
      for (const run of grid.lines[y] ?? []) {
        const flags = run.flags ?? 0;
        let fg = run.fg !== undefined ? rgb(run.fg) : theme.fg;
        let bg = run.bg !== undefined ? rgb(run.bg) : theme.bg;
        if (flags & INVERSE) [fg, bg] = [bg, fg];
        const px = run.x * cellW;
        const w = run.width * cellW;
        if (bg !== theme.bg || flags & INVERSE) {
          ctx.fillStyle = bg;
          ctx.fillRect(px, y * cellH, w, cellH);
        }
        if (flags & INVISIBLE) continue;
        ctx.fillStyle = fg;
        ctx.globalAlpha = flags & FAINT ? 0.6 : 1;
        setFont(flags);
        // Wide chars advance 2 columns; per-char placement keeps the grid
        // aligned regardless of what the canvas font measures.
        let cx = px;
        for (const ch of run.text) {
          ctx.fillText(ch, cx, y * cellH + baseline);
          cx += (isWideRun(run) && ch.charCodeAt(0) > 0xff ? 2 : 1) * cellW;
        }
        ctx.globalAlpha = 1;
        if (flags & (UNDERLINE | STRIKETHROUGH | OVERLINE)) {
          ctx.strokeStyle = fg;
          ctx.lineWidth = 1;
          const line = (ly: number) => {
            ctx.beginPath();
            ctx.moveTo(px, ly);
            ctx.lineTo(px + w, ly);
            ctx.stroke();
          };
          if (flags & UNDERLINE) line(y * cellH + baseline + 2);
          if (flags & STRIKETHROUGH) line(y * cellH + cellH / 2);
          if (flags & OVERLINE) line(y * cellH + 1);
        }
      }
    };

    const paintCursor = () => {
      const c = grid.cursor;
      if (!c || !c.visible || grid.scrolledBack) return;
      const px = c.x * cellW;
      const py = c.y * cellH;
      ctx.fillStyle = theme.fg;
      switch (c.shape) {
        case "bar":
          ctx.fillRect(px, py, 2, cellH);
          break;
        case "underline":
          ctx.fillRect(px, py + cellH - 2, cellW, 2);
          break;
        case "hollow":
          ctx.strokeStyle = theme.fg;
          ctx.strokeRect(px + 0.5, py + 0.5, cellW - 1, cellH - 1);
          break;
        default: {
          ctx.fillRect(px, py, cellW, cellH);
          const ch = charAt(grid.lines[c.y] ?? [], c.x);
          if (ch) {
            ctx.fillStyle = theme.bg;
            setFont(0);
            ctx.fillText(ch, px, py + baseline);
          }
        }
      }
    };

    const paintAll = () => {
      ctx.fillStyle = theme.bg;
      ctx.fillRect(0, 0, canvas.clientWidth, canvas.clientHeight);
      for (let y = 0; y < grid.lines.length; y++) paintRow(y);
      paintCursor();
    };

    const applyFrame = (frame: Frame) => {
      const prevCursorY = grid.cursor?.y;
      if (frame.full || frame.cols !== grid.cols || frame.rows !== grid.rows) {
        grid.cols = frame.cols;
        grid.rows = frame.rows;
        grid.lines = Array.from({ length: frame.rows }, () => []);
      }
      for (const row of frame.changed) grid.lines[row.y] = row.runs;
      grid.cursor = frame.cursor;
      grid.modes = frame.modes;
      if (frame.title !== undefined) onTitleRef.current?.(termId, frame.title);

      if (frame.full) {
        paintAll();
        return;
      }
      for (const row of frame.changed) paintRow(row.y);
      // The cursor cell is drawn over its row; repaint rows it left/entered
      // even when their content didn't change.
      if (prevCursorY !== undefined && !frame.changed.some((r) => r.y === prevCursorY)) {
        paintRow(prevCursorY);
      }
      if (!frame.changed.some((r) => r.y === frame.cursor.y) && frame.cursor.y !== prevCursorY) {
        paintRow(frame.cursor.y);
      }
      paintCursor();
    };

    const fitCanvas = () => {
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.max(1, Math.round(host.clientWidth * dpr));
      canvas.height = Math.max(1, Math.round(host.clientHeight * dpr));
      canvas.style.width = `${host.clientWidth}px`;
      canvas.style.height = `${host.clientHeight}px`;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.textBaseline = "alphabetic";
    };
    fitCanvas();

    // React 19 StrictMode double-mounts effects in dev; `disposed` keeps the
    // stale mount's async continuation from starting a second shell, and
    // `started` ensures only the mount that spawned the shell kills it.
    let disposed = false;
    let started = false;
    const unlisteners: (() => void)[] = [];
    const disposers: (() => void)[] = [];

    void (async () => {
      // Outside Tauri there is no PTY bridge; show a note instead of throwing
      // on the missing IPC internals.
      if (!("__TAURI_INTERNALS__" in window)) {
        ctx.fillStyle = theme.fg;
        setFont(0);
        ctx.fillText("terminals require the desktop app (browser dev mode)", 8, baseline + 8);
        return;
      }

      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");

      const write = (data: string) => void invoke("term_write", { termId, data }).catch(() => {});
      const scroll = (delta: number | null) =>
        void invoke("term_scroll", { termId, delta }).catch(() => {});

      const onFrame = await listen<{ termId: string; frame: Frame }>("terminal://frame", (e) => {
        if (e.payload.termId === termId) applyFrame(e.payload.frame);
      });
      if (disposed) return onFrame();
      unlisteners.push(onFrame);

      const onExitEvent = await listen<{ termId: string }>("terminal://exit", (e) => {
        if (e.payload.termId === termId) onExitRef.current();
      });
      if (disposed) return onExitEvent();
      unlisteners.push(onExitEvent);

      await invoke("term_start", { termId, cols: grid.cols, rows: grid.rows, cwd });
      started = true;
      if (disposed) return void invoke("term_kill", { termId }).catch(() => {});

      const backToLive = () => {
        if (grid.scrolledBack) {
          grid.scrolledBack = false;
          scroll(null);
        }
      };

      const onKeyDown = (e: KeyboardEvent) => {
        if (e.isComposing) return;
        const seq = encodeKey(e, grid.modes);
        if (seq !== null) {
          e.preventDefault();
          backToLive();
          write(seq);
        }
      };
      const onPaste = (e: ClipboardEvent) => {
        e.preventDefault();
        const text = e.clipboardData?.getData("text");
        if (text) {
          backToLive();
          write(encodePaste(text, grid.modes.bracketedPaste));
        }
      };
      // IME: composed text arrives on compositionend, not keydown.
      const onComposed = (e: CompositionEvent) => {
        if (e.data) write(e.data);
        input.value = "";
      };
      const onWheel = (e: WheelEvent) => {
        e.preventDefault();
        const lines =
          e.deltaMode === WheelEvent.DOM_DELTA_LINE
            ? Math.round(e.deltaY)
            : Math.round(e.deltaY / cellH) || Math.sign(e.deltaY);
        if (lines === 0) return;
        if (grid.modes.altScreen) {
          // Fullscreen TUIs: wheel becomes arrow keys (xterm alt-scroll).
          const key = lines < 0 ? (grid.modes.appCursorKeys ? "\x1bOA" : "\x1b[A") : grid.modes.appCursorKeys ? "\x1bOB" : "\x1b[B";
          write(key.repeat(Math.min(5, Math.abs(lines))));
        } else {
          grid.scrolledBack = true;
          scroll(lines);
        }
      };
      const focusInput = () => input.focus({ preventScroll: true });

      input.addEventListener("keydown", onKeyDown);
      input.addEventListener("paste", onPaste);
      input.addEventListener("compositionend", onComposed);
      host.addEventListener("wheel", onWheel, { passive: false });
      host.addEventListener("mousedown", focusInput);
      disposers.push(() => {
        input.removeEventListener("keydown", onKeyDown);
        input.removeEventListener("paste", onPaste);
        input.removeEventListener("compositionend", onComposed);
        host.removeEventListener("wheel", onWheel);
        host.removeEventListener("mousedown", focusInput);
      });
      focusInput();
    })();

    const observer = new ResizeObserver(() => {
      const cols = Math.max(2, Math.floor(host.clientWidth / cellW));
      const rows = Math.max(1, Math.floor(host.clientHeight / cellH));
      fitCanvas();
      paintAll(); // repaint from local state (pane may have been hidden at 0x0)
      if (cols !== grid.cols || rows !== grid.rows) {
        grid.cols = cols;
        grid.rows = rows;
        void import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke("term_resize", {
            termId,
            cols,
            rows,
            cellWidth: Math.round(cellW),
            cellHeight: cellH,
          }).catch(() => {}),
        );
      }
    });
    observer.observe(host);

    return () => {
      disposed = true;
      observer.disconnect();
      for (const dispose of disposers) dispose();
      for (const unlisten of unlisteners) unlisten();
      if (started) {
        void import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke("term_kill", { termId }).catch(() => {}),
        );
      }
    };
    // termId/cwd identify the shell; changing them means a different terminal.
  }, [termId, cwd]);

  return (
    <div ref={hostRef} className="relative size-full overflow-hidden bg-background p-1">
      <canvas ref={canvasRef} className="block" />
      {/* Hidden input: receives focus/keystrokes/IME composition/paste. */}
      <textarea
        ref={inputRef}
        className="absolute left-0 top-0 h-px w-px resize-none border-0 bg-transparent p-0 opacity-0 outline-none"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        aria-label="terminal input"
      />
    </div>
  );
}

/** Whether a run may contain wide (2-column) characters: its column width
 * exceeds its character count. */
function isWideRun(run: Run): boolean {
  return run.width > [...run.text].length;
}

/** The character at terminal column `x` in a row of runs, if any. */
function charAt(runs: Run[], x: number): string | null {
  for (const run of runs) {
    if (x < run.x || x >= run.x + run.width) continue;
    if (!isWideRun(run)) return [...run.text][x - run.x] ?? null;
    let cx = run.x;
    for (const ch of run.text) {
      const w = ch.charCodeAt(0) > 0xff ? 2 : 1;
      if (x >= cx && x < cx + w) return ch;
      cx += w;
    }
  }
  return null;
}
