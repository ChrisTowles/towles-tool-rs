import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
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
  isWideRun,
  rgb,
  scrollbackKey,
  stepMatch,
  viewportMatches,
  TERM_CLEAR_COMMAND,
  type Cursor,
  type Frame,
  type Run,
  type SearchMatch,
  type TermExit,
} from "@/lib/term-protocol";
import { linkAt, linkLabel, type TermLink } from "@/lib/term-links";
import {
  rowsHaveSelection,
  selectionKindForDetail,
  shouldCopyOnSelect,
} from "@/lib/terminal-selection";
import { useCopyOnSelect } from "@/lib/terminal-prefs";
import { IS_MAC, matchesShortcut } from "@/lib/shortcuts";
import { openExternalUrl } from "@/lib/open-url";
import { Input } from "@/components/ui/input";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { IconBtn } from "@/components/agentboard-bits";

/** Scrollback-search highlight fills — alpha washes over the cell backgrounds
 * so they read on both light and dark terminal themes. The current match gets
 * the stronger orange + an outline; other matches a lighter amber. */
const MATCH_FILL = "rgba(250, 204, 21, 0.3)";
const CURRENT_MATCH_FILL = "rgba(249, 115, 22, 0.5)";
const CURRENT_MATCH_STROKE = "rgba(249, 115, 22, 0.9)";

const FONT_SIZE = 13;
const FONT_FAMILY = "ui-monospace, 'JetBrains Mono', 'Fira Code', monospace";
const LINE_HEIGHT = 1.25;

/**
 * Canvas terminal pane over the tt-vt engine. The Tauri bridge owns the PTY
 * and the terminal state (libghostty-vt), keyed by `termId`: `term_start`
 * spawns a shell in `cwd` sized to the measured grid, render frames arrive
 * as `terminal://frame` events (dirty-row style runs + cursor + modes), and
 * input/resize/scroll go back through `term_write`/`term_resize`/
 * `term_scroll`. When the shell exits, `onExit` hands the parent the exit
 * status (code + signal) so it can report how the shell died; unmounting kills
 * the shell (`term_kill`).
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
  onExit: (exit: TermExit) => void;
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

  // Scrollback search. The canvas paints from `searchRef` (mutable, read by
  // the render closure inside the effect); React state mirrors what the
  // overlay UI shows. `bridgeRef` exposes the effect's IPC + paint internals
  // to the overlay handlers once the Tauri side is up.
  const searchRef = useRef<{ matches: SearchMatch[]; current: number }>({
    matches: [],
    current: -1,
  });
  const bridgeRef = useRef<{
    search: (query: string) => Promise<SearchMatch[]>;
    scrollTo: (row: number) => void;
    repaint: () => void;
    focusTerm: () => void;
    copy: () => void;
    paste: () => void;
    selectAll: () => void;
    hasSelection: () => boolean;
    clearScrollback: () => void;
    /** Open a path link in the preferred editor (resolved against the cwd). */
    openPath: (link: Extract<TermLink, { kind: "path" }>) => void;
    /** The link under a canvas pixel (right-click point), or null. */
    linkAtPoint: (offsetX: number, offsetY: number) => TermLink | null;
  } | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [matchCount, setMatchCount] = useState(0);
  const [currentMatch, setCurrentMatch] = useState(-1);
  // Right-click menu: `copyEnabled` is sampled from the live selection when the
  // menu opens (Copy is dead when there's nothing selected). `menuLink` is the
  // URL under the click point, sampled on contextmenu (drives "Open link").
  const [copyEnabled, setCopyEnabled] = useState(false);
  const [menuLink, setMenuLink] = useState<TermLink | null>(null);
  // Copy-on-select preference, read live by the render effect's mouse handlers.
  const copyOnSelectRef = useCopyOnSelect();

  const runSearch = useCallback(async (q: string) => {
    const bridge = bridgeRef.current;
    if (!bridge) return;
    const matches = q ? await bridge.search(q).catch(() => [] as SearchMatch[]) : [];
    const current = matches.length - 1; // start at the most recent match
    searchRef.current = { matches, current };
    setMatchCount(matches.length);
    setCurrentMatch(current);
    if (current >= 0) bridge.scrollTo(matches[current].row);
    bridge.repaint();
  }, []);

  const step = useCallback((dir: 1 | -1) => {
    const sr = searchRef.current;
    const next = stepMatch(sr.matches.length, sr.current, dir);
    if (next < 0) return;
    sr.current = next;
    setCurrentMatch(next);
    bridgeRef.current?.scrollTo(sr.matches[next].row);
    bridgeRef.current?.repaint();
  }, []);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setQuery("");
    setMatchCount(0);
    setCurrentMatch(-1);
    searchRef.current = { matches: [], current: -1 };
    bridgeRef.current?.repaint();
    bridgeRef.current?.focusTerm();
  }, []);

  // Focus the overlay input when it opens; re-run the search as the query
  // changes (debounced — each keystroke otherwise round-trips the engine).
  useEffect(() => {
    if (searchOpen) searchInputRef.current?.focus();
  }, [searchOpen]);
  useEffect(() => {
    if (!searchOpen) return;
    const t = setTimeout(() => void runSearch(query), 150);
    return () => clearTimeout(t);
  }, [searchOpen, query, runSearch]);

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

    // Client-side grid mirror: rows of style runs (+ selection range), updated
    // per frame, so any row (cursor moves, resize repaints) can be repainted
    // from local state.
    const grid = {
      cols: Math.max(2, Math.floor(host.clientWidth / cellW)),
      rows: Math.max(1, Math.floor(host.clientHeight / cellH)),
      lines: [] as { runs: Run[]; sel?: [number, number] }[],
      cursor: null as Cursor | null,
      modes: { appCursorKeys: false, bracketedPaste: false, altScreen: false, mouseTracking: false },
      scrolledBack: false,
      /** URL under the mouse — underlined and Ctrl/Cmd-clickable. */
      hoveredLink: null as TermLink | null,
      /** Absolute row of the viewport top (from frames) — maps absolute
       * search-match rows onto viewport rows for highlighting. */
      viewportTop: 0,
    };

    const setFont = (flags: number) => {
      const bold = flags & BOLD ? "bold " : "";
      const italic = flags & ITALIC ? "italic " : "";
      ctx.font = `${italic}${bold}${FONT_SIZE}px ${FONT_FAMILY}`;
    };

    const paintRow = (y: number) => {
      ctx.fillStyle = theme.bg;
      ctx.fillRect(0, y * cellH, canvas.clientWidth, cellH);
      for (const run of grid.lines[y]?.runs ?? []) {
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
      // Hovered link: underline its cells on this row so it reads clickable.
      for (const seg of grid.hoveredLink?.segments ?? []) {
        if (seg.y !== y) continue;
        ctx.strokeStyle = theme.fg;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(seg.start * cellW, y * cellH + baseline + 2);
        ctx.lineTo((seg.end + 1) * cellW, y * cellH + baseline + 2);
        ctx.stroke();
      }
      const sel = grid.lines[y]?.sel;
      if (sel) {
        ctx.globalAlpha = 0.3;
        ctx.fillStyle = theme.fg;
        ctx.fillRect(sel[0] * cellW, y * cellH, (sel[1] - sel[0] + 1) * cellW, cellH);
        ctx.globalAlpha = 1;
      }
      // Search-match highlights: alpha washes over the drawn text, the
      // current match outlined so it stands apart from the rest.
      const sr = searchRef.current;
      if (sr.matches.length) {
        for (const m of viewportMatches(sr.matches, grid.viewportTop, grid.rows)) {
          if (m.y !== y) continue;
          const isCurrent = m.index === sr.current;
          ctx.fillStyle = isCurrent ? CURRENT_MATCH_FILL : MATCH_FILL;
          ctx.fillRect(m.col * cellW, y * cellH, m.width * cellW, cellH);
          if (isCurrent) {
            ctx.strokeStyle = CURRENT_MATCH_STROKE;
            ctx.lineWidth = 1;
            ctx.strokeRect(m.col * cellW + 0.5, y * cellH + 0.5, m.width * cellW - 1, cellH - 1);
          }
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
          const ch = charAt(grid.lines[c.y]?.runs ?? [], c.x);
          if (ch) {
            ctx.fillStyle = theme.bg;
            setFont(0);
            ctx.fillText(ch, px, py + baseline);
          }
        }
      }
    };

    const setHoveredLink = (link: TermLink | null) => {
      const prev = grid.hoveredLink;
      if (!prev && !link) return;
      if (
        prev &&
        link &&
        linkLabel(prev) === linkLabel(link) &&
        prev.segments[0].y === link.segments[0].y &&
        prev.segments[0].start === link.segments[0].start
      ) {
        return;
      }
      grid.hoveredLink = link;
      canvas.style.cursor = link ? "pointer" : "default";
      const openHint = link?.kind === "path" ? "open in editor" : "open";
      canvas.title = link ? `${linkLabel(link)}\nCtrl+Click (⌘+Click) to ${openHint}` : "";
      const rows = new Set([...(prev?.segments ?? []), ...(link?.segments ?? [])].map((s) => s.y));
      for (const y of rows) paintRow(y);
      paintCursor();
    };

    const paintAll = () => {
      ctx.fillStyle = theme.bg;
      ctx.fillRect(0, 0, canvas.clientWidth, canvas.clientHeight);
      for (let y = 0; y < grid.lines.length; y++) paintRow(y);
      paintCursor();
    };

    const applyFrame = (frame: Frame) => {
      const prevCursorY = grid.cursor?.y;
      const resized = frame.cols !== grid.cols || frame.rows !== grid.rows;
      if (frame.full) {
        grid.cols = frame.cols;
        grid.rows = frame.rows;
        grid.lines = Array.from({ length: frame.rows }, () => ({ runs: [] }));
      } else if (resized) {
        // Dimension change on a dirty-only frame (resize race): adjust the
        // row count but KEEP existing rows — wiping them here blanks rows
        // the engine considers clean and will never resend (#47).
        grid.cols = frame.cols;
        grid.rows = frame.rows;
        while (grid.lines.length < frame.rows) grid.lines.push({ runs: [] });
        grid.lines.length = frame.rows;
      }
      for (const row of frame.changed) grid.lines[row.y] = { runs: row.runs, sel: row.sel };
      // Text under a hovered link may have changed; drop the highlight rather
      // than underline stale cells (the next mousemove re-detects).
      if (
        grid.hoveredLink &&
        (frame.full ||
          resized ||
          grid.hoveredLink.segments.some((s) => frame.changed.some((r) => r.y === s.y)))
      ) {
        grid.hoveredLink = null;
        canvas.style.cursor = "default";
        canvas.title = "";
      }
      grid.cursor = frame.cursor;
      grid.modes = frame.modes;
      grid.viewportTop = frame.viewportTop;
      // The engine knows where the viewport really is (search navigation
      // scrolls it too, not just the wheel) — derive "scrolled back" from
      // the frame instead of trusting the wheel handler's optimistic flag.
      grid.scrolledBack = frame.viewportTop < frame.scrollbackRows;
      if (frame.title !== undefined) onTitleRef.current?.(termId, frame.title);

      if (frame.full || resized) {
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
      // Copy the engine's active selection to the system clipboard. Shared by
      // the Ctrl/⌘+Shift+C chord, the context menu, and copy-on-select.
      const copySelection = () =>
        void invoke<string>("term_copy", { termId })
          .then((text) => (text ? navigator.clipboard.writeText(text) : undefined))
          .catch(() => {});

      const onFrame = await listen<{ termId: string; frame: Frame }>("terminal://frame", (e) => {
        if (e.payload.termId === termId) applyFrame(e.payload.frame);
      });
      if (disposed) return onFrame();
      unlisteners.push(onFrame);

      const onExitEvent = await listen<TermExit>("terminal://exit", (e) => {
        if (e.payload.termId === termId) onExitRef.current(e.payload);
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
      // Paste from the system clipboard through the same bracketed-paste path
      // as a real paste event. Used by the context menu's Paste item.
      const pasteClipboard = () =>
        void navigator.clipboard
          .readText()
          .then((text) => {
            if (!text) return;
            backToLive();
            write(encodePaste(text, grid.modes.bracketedPaste));
          })
          .catch(() => {});

      const onKeyDown = (e: KeyboardEvent) => {
        if (e.isComposing) return;
        // The search chord is ours, not the shell's (Ctrl+F stays with the
        // shell) — checked before encodeKey turns it into a control byte.
        if (matchesShortcut("term-search", e)) {
          e.preventDefault();
          setSearchOpen(true);
          return;
        }
        if (e.ctrlKey && e.shiftKey && (e.key === "C" || e.key === "c")) {
          e.preventDefault();
          copySelection();
          return;
        }
        // Scrollback navigation: Shift+PageUp/PageDown scroll one page,
        // Shift+Home/End jump to the top / live bottom — driven through the
        // same `term_scroll` path as the wheel. On the alternate screen a
        // fullscreen TUI owns these keys, so we forward the unshifted key as
        // ordinary input instead (mirroring the wheel handler's alt-scroll
        // branch).
        const scrollback = scrollbackKey(e);
        if (scrollback) {
          e.preventDefault();
          if (grid.modes.altScreen) {
            const base = encodeKey(
              { key: e.key, shiftKey: false, altKey: false, ctrlKey: false, metaKey: false },
              grid.modes,
            );
            if (base !== null) write(base);
            return;
          }
          const page = Math.max(1, grid.rows - 1);
          switch (scrollback) {
            case "page-up":
              grid.scrolledBack = true;
              scroll(-page);
              break;
            case "page-down":
              scroll(page); // engine clamps at the live bottom
              break;
            case "top":
              if (grid.viewportTop > 0) {
                grid.scrolledBack = true;
                scroll(-grid.viewportTop);
              }
              break;
            case "bottom":
              backToLive();
              break;
          }
          return;
        }
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

      // Hand the overlay its IPC + paint hooks now that the shell is up.
      bridgeRef.current = {
        search: (q) => invoke<SearchMatch[]>("term_search", { termId, query: q }),
        scrollTo: (row) => void invoke("term_scroll_to", { termId, row }).catch(() => {}),
        repaint: paintAll,
        focusTerm: focusInput,
        copy: copySelection,
        paste: pasteClipboard,
        selectAll: () => void select("all"),
        hasSelection: () => rowsHaveSelection(grid.lines),
        clearScrollback: () => void invoke(TERM_CLEAR_COMMAND, { termId }).catch(() => {}),
        // Open a clicked file path in the editor. Relative paths resolve
        // against this pane's `cwd` (the backend joins them). Report-only —
        // this opens an editor, it never writes to the PTY.
        openPath: (link) =>
          void invoke("term_open_path", { path: link.path, cwd }).catch(() => {}),
        linkAtPoint: (offsetX, offsetY) => {
          const x = Math.max(0, Math.min(grid.cols - 1, Math.floor(offsetX / cellW)));
          const y = Math.max(0, Math.min(grid.rows - 1, Math.floor(offsetY / cellH)));
          return linkAt(grid.lines, grid.cols, x, y);
        },
      };

      // Mouse selection: drag = range, double-click = word, triple = line,
      // plain click = clear. Coordinates are viewport cells; the engine owns
      // the selection and reports highlight ranges back in frames.
      // The last selection IPC, so copy-on-select can wait for the engine to
      // apply the selection before `term_copy` reads it (both go over the same
      // engine channel, but the IPC calls are otherwise unordered).
      let lastSelect: Promise<unknown> = Promise.resolve();
      const select = (
        kind: "drag" | "word" | "line" | "all" | "clear",
        a?: { x: number; y: number },
        b?: { x: number; y: number },
      ) => {
        lastSelect = invoke("term_select", {
          termId,
          kind,
          ax: a?.x,
          ay: a?.y,
          bx: b?.x,
          by: b?.y,
        }).catch(() => {});
        return lastSelect;
      };
      // Copy a just-made selection to the clipboard when copy-on-select is on.
      const maybeCopyOnSelect = (kind: "drag" | "word" | "line") => {
        if (shouldCopyOnSelect(copyOnSelectRef.current, kind)) {
          void lastSelect.then(copySelection);
        }
      };
      const cellOf = (e: MouseEvent) => ({
        x: Math.max(0, Math.min(grid.cols - 1, Math.floor(e.offsetX / cellW))),
        y: Math.max(0, Math.min(grid.rows - 1, Math.floor(e.offsetY / cellH))),
      });
      let anchor: { x: number; y: number } | null = null;
      let dragged = false;
      const onMouseDown = (e: MouseEvent) => {
        focusInput();
        if (e.button !== 0) return;
        e.preventDefault(); // keep focus on the hidden input
        const cell = cellOf(e);
        // Ctrl/Cmd+click on a link opens it (VS Code terminal convention):
        // URLs in the system browser, file paths in the preferred editor.
        // Plain click keeps its select/focus meaning.
        if (e.ctrlKey || e.metaKey) {
          const link = linkAt(grid.lines, grid.cols, cell.x, cell.y);
          if (link) {
            if (link.kind === "url") void openExternalUrl(link.url);
            else bridgeRef.current?.openPath(link);
            return;
          }
        }
        const kind = selectionKindForDetail(e.detail);
        if (kind === "word" || kind === "line") {
          void select(kind, cell);
          maybeCopyOnSelect(kind);
        } else {
          anchor = cell;
          dragged = false;
        }
      };
      const onMouseMove = (e: MouseEvent) => {
        const cell = cellOf(e);
        if (!anchor) {
          setHoveredLink(linkAt(grid.lines, grid.cols, cell.x, cell.y));
          return;
        }
        if (!dragged && cell.x === anchor.x && cell.y === anchor.y) return;
        dragged = true;
        setHoveredLink(null);
        void select("drag", anchor, cell);
      };
      const onMouseUp = () => {
        if (anchor && !dragged) void select("clear");
        else if (dragged) maybeCopyOnSelect("drag");
        anchor = null;
      };
      const onMouseLeave = () => setHoveredLink(null);
      // Report focus so the backend can gate OSC 52 clipboard writes to the
      // focused terminal — a background pane must not hijack the clipboard.
      const setFocus = (focused: boolean) =>
        void invoke("term_focus", { termId, focused }).catch(() => {});
      const onFocus = () => setFocus(true);
      const onBlur = () => setFocus(false);

      input.addEventListener("keydown", onKeyDown);
      input.addEventListener("paste", onPaste);
      input.addEventListener("compositionend", onComposed);
      input.addEventListener("focus", onFocus);
      input.addEventListener("blur", onBlur);
      host.addEventListener("wheel", onWheel, { passive: false });
      canvas.addEventListener("mousedown", onMouseDown);
      canvas.addEventListener("mousemove", onMouseMove);
      canvas.addEventListener("mouseleave", onMouseLeave);
      window.addEventListener("mouseup", onMouseUp);
      disposers.push(() => {
        input.removeEventListener("keydown", onKeyDown);
        input.removeEventListener("paste", onPaste);
        input.removeEventListener("compositionend", onComposed);
        input.removeEventListener("focus", onFocus);
        input.removeEventListener("blur", onBlur);
        host.removeEventListener("wheel", onWheel);
        canvas.removeEventListener("mousedown", onMouseDown);
        canvas.removeEventListener("mousemove", onMouseMove);
        canvas.removeEventListener("mouseleave", onMouseLeave);
        window.removeEventListener("mouseup", onMouseUp);
        setFocus(false);
      });
      focusInput();
    })();

    // Panes are hidden with display:none (never unmounted), so the observer
    // sees them collapse to 0×0 and grow back on window switches.
    let wasHidden = false;
    const observer = new ResizeObserver(() => {
      if (host.clientWidth === 0 || host.clientHeight === 0) {
        // Hidden pane: never resize the PTY to a degenerate 2×1 grid — that
        // reflows the shell while offscreen and desyncs the local mirror
        // from the engine's grid, which is how panes came back stale (#47).
        wasHidden = true;
        return;
      }
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
      if (wasHidden) {
        // Re-shown: ask the engine for one full frame in case any dirty-only
        // frame was missed while hidden — the engine never resends rows it
        // considers clean, so a gap would otherwise persist until a scroll.
        wasHidden = false;
        void import("@tauri-apps/api/core").then(({ invoke }) =>
          invoke("term_request_full", { termId }).catch(() => {}),
        );
      }
    });
    observer.observe(host);

    return () => {
      disposed = true;
      bridgeRef.current = null;
      searchRef.current = { matches: [], current: -1 };
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
      {/* Right-click menu over the canvas. Copy is sampled live on open (dead
          when nothing is selected); items route through the render effect's
          IPC helpers via `bridgeRef`. `onCloseAutoFocus` returns focus to the
          hidden input so typing/IME keep working after the menu closes. */}
      <ContextMenu
        onOpenChange={(open) => {
          if (open) setCopyEnabled(bridgeRef.current?.hasSelection() ?? false);
        }}
      >
        <ContextMenuTrigger asChild>
          <canvas
            ref={canvasRef}
            className="block"
            // Sample the link under the click before the menu opens, so
            // "Open link" shows only when the right-click landed on a URL.
            onContextMenu={(e) =>
              setMenuLink(
                bridgeRef.current?.linkAtPoint(e.nativeEvent.offsetX, e.nativeEvent.offsetY) ??
                  null,
              )
            }
          />
        </ContextMenuTrigger>
        <ContextMenuContent
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            bridgeRef.current?.focusTerm();
          }}
        >
          {menuLink && (
            <>
              <ContextMenuItem
                onSelect={() =>
                  menuLink.kind === "url"
                    ? void openExternalUrl(menuLink.url)
                    : bridgeRef.current?.openPath(menuLink)
                }
              >
                {menuLink.kind === "url" ? "Open link" : "Open in editor"}
              </ContextMenuItem>
              <ContextMenuSeparator />
            </>
          )}
          <ContextMenuItem
            disabled={!copyEnabled}
            onSelect={() => bridgeRef.current?.copy()}
          >
            Copy
            <ContextMenuShortcut>{IS_MAC ? "⇧⌘C" : "Ctrl+Shift+C"}</ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => bridgeRef.current?.paste()}>
            Paste
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => bridgeRef.current?.selectAll()}>
            Select all
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => setSearchOpen(true)}>
            Search scrollback
            <ContextMenuShortcut>{IS_MAC ? "⇧⌘F" : "Ctrl+Shift+F"}</ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => bridgeRef.current?.clearScrollback()}>
            Clear scrollback
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      {/* Scrollback search overlay (Ctrl/⌘+Shift+F). Enter/Shift+Enter step
          through matches; Escape returns focus to the terminal. */}
      {searchOpen && (
        <div className="absolute right-1 top-1 z-10 flex items-center gap-1 rounded-md border bg-card p-1 shadow-md">
          <Input
            ref={searchInputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                step(e.shiftKey ? -1 : 1);
              } else if (e.key === "Escape" || matchesShortcut("term-search", e.nativeEvent)) {
                e.preventDefault();
                closeSearch();
              }
            }}
            placeholder="Search scrollback"
            className="h-6 w-44 px-2 text-xs md:text-xs"
            spellCheck={false}
            aria-label="search scrollback"
          />
          <span className="min-w-10 text-center font-mono text-[10px] tabular-nums text-muted-foreground">
            {matchCount > 0 ? `${currentMatch + 1}/${matchCount}` : "0/0"}
          </span>
          <IconBtn title="Previous match (Shift+Enter)" onClick={() => step(-1)}>
            <ChevronUp className="size-3" />
          </IconBtn>
          <IconBtn title="Next match (Enter)" onClick={() => step(1)}>
            <ChevronDown className="size-3" />
          </IconBtn>
          <IconBtn title="Close search (Esc)" onClick={closeSearch}>
            <X className="size-3" />
          </IconBtn>
        </div>
      )}
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
