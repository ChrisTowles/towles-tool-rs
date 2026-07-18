/**
 * Wire types for `terminal://frame` events (mirrors crates/tt-vt/src/frame.rs)
 * plus the DOM-key → escape-sequence encoder the canvas terminal view uses.
 * Pure module — no Tauri, no DOM rendering.
 */

export interface Run {
  x: number;
  width: number;
  text: string;
  /** Packed 0xRRGGBB; absent = terminal default (theme color). */
  fg?: number;
  bg?: number;
  flags?: number;
  /** OSC 8 hyperlink URI, when the run's cells carry one. Takes priority over
   * regex-based link detection (see term-links.ts) since the visible text
   * (e.g. a markdown-style link label) may not itself look like a URL. */
  link?: string;
  /** Underline style past single — 2 double, 3 curly, 4 dotted, 5 dashed
   * (SGR 4:x). Absent = none/single; UNDERLINE still flags "any". */
  ul?: number;
  /** SGR 58 underline color, packed 0xRRGGBB; absent = underline in fg. */
  ulc?: number;
}

export interface RowUpdate {
  y: number;
  runs: Run[];
  /** This row soft-wraps into the next: its content continues on row `y + 1`
   * (libghostty's per-row wrap bit; absent = false). term-links joins rows on
   * this flag rather than guessing from text that reaches the last column. */
  wrapped?: boolean;
  /** Row-local selected column range, inclusive. */
  sel?: [number, number];
}

export type CursorShape = "block" | "bar" | "underline" | "hollow";

export interface Cursor {
  x: number;
  y: number;
  visible: boolean;
  shape: CursorShape;
  blinking: boolean;
  /** Cursor color a program set (OSC 12), packed 0xRRGGBB; absent = theme. */
  color?: number;
  /** The program signalled password input — a lock hint renders in the cell. */
  password?: boolean;
}

/** Mode hints for input *routing* only — all encoding happens engine-side. */
export interface Modes {
  /** Alternate screen active (fullscreen TUI owns the scrollback chords). */
  altScreen: boolean;
  /** Clicks go to the program instead of local selection; Shift bypasses. */
  mouseTracking: boolean;
}

export interface Frame {
  full: boolean;
  cols: number;
  rows: number;
  changed: RowUpdate[];
  cursor: Cursor;
  colors: { fg: number; bg: number };
  modes: Modes;
  title?: string;
  scrollbackRows: number;
  /** Absolute row index of the viewport's top (0 = oldest scrollback row);
   * equals `scrollbackRows` at the live bottom. */
  viewportTop: number;
}

/** Payload of a `terminal://exit` event (mirrors `TermExit` in
 * crates-tauri/tt-app/src/terminal.rs): the dead shell's exit code and, when a
 * signal ended it, that signal's resolved name. A signal death leaves `code`
 * at portable-pty's placeholder, so consumers prefer `signal` when present. */
export interface TermExit {
  termId: string;
  code: number;
  /** Signal name ("Killed", "Terminated", …) when the shell was signalled;
   * null/absent for a normal exit. */
  signal?: string | null;
}

/** Human label for a dead shell's exit status: "exited" for a code-0 logout,
 * "exited · Killed" when a signal ended it, "exited · code 2" otherwise. */
export function exitLabel(code: number, signal?: string | null): string {
  if (signal) return `exited · ${signal}`;
  if (code === 0) return "exited";
  return `exited · code ${code}`;
}

/** Whether a shell's exit looks like a crash (nonzero code or a signal) rather
 * than a clean logout. A crashing pane vanishes like any other, so this is what
 * decides whether its death is worth a toast — a clean logout is expected and
 * says nothing; a crash is the one exit you'd otherwise never learn about. */
export function exitIsCrash(code: number, signal?: string | null): boolean {
  return code !== 0 || signal != null;
}

/** Tauri IPC command that drops a terminal's scrollback while leaving the
 * visible screen intact (right-click "Clear scrollback"). Handled by
 * `term_clear` in crates-tauri/tt-app/src/terminal.rs, which forces a full
 * frame so the view learns the scrollback depth collapsed. */
export const TERM_CLEAR_COMMAND = "term_clear";

/** One scrollback search hit (mirrors tt-vt's `SearchMatch`): absolute row
 * (0 = oldest scrollback row), starting column, width in columns. */
export interface SearchMatch {
  row: number;
  col: number;
  width: number;
}

/** The matches visible in the current viewport, mapped to viewport rows.
 * `index` is the match's position in the full list (to mark the current
 * match distinctly). */
export function viewportMatches(
  matches: SearchMatch[],
  viewportTop: number,
  rows: number,
): { y: number; col: number; width: number; index: number }[] {
  const out: { y: number; col: number; width: number; index: number }[] = [];
  for (let index = 0; index < matches.length; index++) {
    const m = matches[index];
    const y = m.row - viewportTop;
    if (y >= 0 && y < rows) out.push({ y, col: m.col, width: m.width, index });
  }
  return out;
}

/** Step a match index by ±1 with wrap-around; -1 when there are no matches. */
export function stepMatch(count: number, current: number, dir: 1 | -1): number {
  if (count <= 0) return -1;
  return (((current + dir) % count) + count) % count;
}

// Run style flag bits (crates/tt-vt/src/frame.rs `flags` module).
export const BOLD = 1;
export const ITALIC = 1 << 1;
export const FAINT = 1 << 2;
export const UNDERLINE = 1 << 3;
export const INVERSE = 1 << 4;
export const INVISIBLE = 1 << 5;
export const STRIKETHROUGH = 1 << 6;
export const OVERLINE = 1 << 7;

export function rgb(packed: number): string {
  return `#${packed.toString(16).padStart(6, "0")}`;
}

const graphemeSegmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;

/** Split a run's text into grapheme clusters — the unit that fills exactly one
 * terminal cell. A cell may carry several codepoints (a base plus combining
 * marks, or an emoji with a variation selector); those must render as one glyph
 * and advance the grid by one cell, not one column per codepoint. Falls back to
 * codepoint iteration only where `Intl.Segmenter` is unavailable. */
export function graphemeClusters(text: string): string[] {
  if (!graphemeSegmenter) return [...text];
  const out: string[] = [];
  for (const { segment } of graphemeSegmenter.segment(text)) out.push(segment);
  return out;
}

/** Whether a run may contain wide (2-column) characters: its column width
 * exceeds its grapheme-cluster count (one cluster = one cell). Counting
 * clusters, not codepoints, keeps combining marks / emoji selectors from
 * looking like extra cells. */
export function isWideRun(run: Run): boolean {
  return run.width > graphemeClusters(run.text).length;
}

/** The subset of `KeyboardEvent` the key encoders read — lets callers pass a
 * synthetic event (e.g. the alt-screen path forwarding an unshifted key). */
type KeyEventLike = Pick<KeyboardEvent, "key" | "shiftKey" | "altKey" | "ctrlKey" | "metaKey">;

export type ScrollbackAction = "page-up" | "page-down" | "top" | "bottom";

/**
 * The terminal-emulator scrollback chords — Shift+PageUp/PageDown scroll a
 * page, Shift+Home/End jump to the top / live bottom. The canvas view drives
 * its own scrollback for these (see terminal-view.tsx) instead of sending
 * them to the shell. Returns the action, or null when the event isn't a
 * bare-shift scrollback chord.
 */
export function scrollbackKey(e: KeyEventLike): ScrollbackAction | null {
  if (!e.shiftKey || e.ctrlKey || e.altKey || e.metaKey) return null;
  switch (e.key) {
    case "PageUp":
      return "page-up";
    case "PageDown":
      return "page-down";
    case "Home":
      return "top";
    case "End":
      return "bottom";
    default:
      return null;
  }
}

/** A keystroke on the `term_key` wire (mirrors tt-app's `TermKey` /
 * tt-vt's `KeyEvent`): DOM `code`/`key` plus modifiers. The Rust engine
 * encodes it against live terminal state — kitty keyboard protocol, DECCKM,
 * keypad mode — so no escape sequences are built in the frontend. */
export interface KeyEventWire {
  code: string;
  key: string;
  action: "press" | "repeat" | "release";
  shift: boolean;
  alt: boolean;
  ctrl: boolean;
  meta: boolean;
  capsLock: boolean;
  numLock: boolean;
}

/** The subset of `KeyboardEvent` the wire mapper reads. */
export type KeyWireEventLike = KeyEventLike &
  Pick<KeyboardEvent, "code" | "repeat"> & {
    getModifierState?: (key: string) => boolean;
  };

/** Bare modifier keys: wired to the engine (kitty REPORT_ALL wants them) but
 * they never mean "the user typed something" — the view uses this to skip
 * its jump-to-live-bottom on a plain Shift press. */
export const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "NumLock"]);

/**
 * Map a DOM key event onto the `term_key` wire, or null when the keystroke
 * isn't the shell's to consume: Super/Cmd chords stay with the OS, and
 * Ctrl+Shift+C/V are the app's copy/paste chords (Ctrl+Shift+V must reach
 * the native paste event). Everything else is routed — the engine decides
 * what bytes, if any, the current terminal modes produce for it.
 */
export function keyEventWire(
  e: KeyWireEventLike,
  action: "press" | "release" = "press",
): KeyEventWire | null {
  if (e.metaKey) return null; // OS shortcuts stay with the OS
  if (
    e.ctrlKey &&
    e.shiftKey &&
    (e.key === "C" || e.key === "c" || e.key === "V" || e.key === "v")
  ) {
    return null; // copy/paste chords are the app's
  }
  return {
    code: e.code,
    key: e.key,
    action: action === "press" && e.repeat ? "repeat" : action,
    shift: e.shiftKey,
    alt: e.altKey,
    ctrl: e.ctrlKey,
    meta: e.metaKey,
    capsLock: e.getModifierState?.("CapsLock") ?? false,
    numLock: e.getModifierState?.("NumLock") ?? false,
  };
}

// Paste encoding lives in the Rust engine (`term_paste` → tt-vt's
// `Engine::paste`): libghostty's encoder strips bytes that could escape the
// paste bracket, which a frontend string-wrap cannot do safely.
