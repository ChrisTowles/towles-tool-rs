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
}

export interface RowUpdate {
  y: number;
  runs: Run[];
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
}

export interface Modes {
  appCursorKeys: boolean;
  bracketedPaste: boolean;
  altScreen: boolean;
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

/** Whether a run may contain wide (2-column) characters: its column width
 * exceeds its character count. */
export function isWideRun(run: Run): boolean {
  return run.width > [...run.text].length;
}

/** The subset of `KeyboardEvent` the key encoders read — lets callers pass a
 * synthetic event (e.g. the alt-screen path forwarding an unshifted key). */
type KeyEventLike = Pick<
  KeyboardEvent,
  "key" | "shiftKey" | "altKey" | "ctrlKey" | "metaKey"
>;

/** xterm-style modifier parameter: 1 + shift(1) + alt(2) + ctrl(4) + meta(8). */
function modParam(e: KeyEventLike): number {
  return (
    1 +
    (e.shiftKey ? 1 : 0) +
    (e.altKey ? 2 : 0) +
    (e.ctrlKey ? 4 : 0) +
    (e.metaKey ? 8 : 0)
  );
}

const CURSOR_FINAL: Record<string, string> = {
  ArrowUp: "A",
  ArrowDown: "B",
  ArrowRight: "C",
  ArrowLeft: "D",
  Home: "H",
  End: "F",
};

const TILDE_CODE: Record<string, number> = {
  Insert: 2,
  Delete: 3,
  PageUp: 5,
  PageDown: 6,
};

const FN_SS3: Record<string, string> = { F1: "P", F2: "Q", F3: "R", F4: "S" };
const FN_TILDE: Record<string, number> = {
  F5: 15,
  F6: 17,
  F7: 18,
  F8: 19,
  F9: 20,
  F10: 21,
  F11: 23,
  F12: 24,
};

export type ScrollbackAction = "page-up" | "page-down" | "top" | "bottom";

/**
 * The terminal-emulator scrollback chords — Shift+PageUp/PageDown scroll a
 * page, Shift+Home/End jump to the top / live bottom. The canvas view drives
 * its own scrollback for these (see terminal-view.tsx), so `encodeKey` returns
 * null for them rather than sending them to the shell. Returns the action, or
 * null when the event isn't a bare-shift scrollback chord.
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

/**
 * Encode a keydown into the bytes a terminal expects, or `null` when the
 * event is not ours to handle (browser shortcut, plain char during IME
 * composition, copy/paste chords, scrollback-navigation chords).
 */
export function encodeKey(e: KeyEventLike, modes: Pick<Modes, "appCursorKeys">): string | null {
  // Leave clipboard chords to the paste/copy handlers.
  if (e.ctrlKey && e.shiftKey && (e.key === "V" || e.key === "C" || e.key === "v" || e.key === "c")) {
    return null;
  }
  // Scrollback-navigation chords belong to the view (it scrolls its own
  // buffer), not the shell.
  if (scrollbackKey(e)) return null;

  const mods = modParam(e);

  const cursorFinal = CURSOR_FINAL[e.key];
  if (cursorFinal) {
    if (mods > 1) return `\x1b[1;${mods}${cursorFinal}`;
    return modes.appCursorKeys ? `\x1bO${cursorFinal}` : `\x1b[${cursorFinal}`;
  }
  const tilde = TILDE_CODE[e.key];
  if (tilde) return mods > 1 ? `\x1b[${tilde};${mods}~` : `\x1b[${tilde}~`;
  const ss3 = FN_SS3[e.key];
  if (ss3) return mods > 1 ? `\x1b[1;${mods}${ss3}` : `\x1bO${ss3}`;
  const fnTilde = FN_TILDE[e.key];
  if (fnTilde) return mods > 1 ? `\x1b[${fnTilde};${mods}~` : `\x1b[${fnTilde}~`;

  switch (e.key) {
    case "Enter":
      return e.altKey ? "\x1b\r" : "\r";
    case "Tab":
      return e.shiftKey ? "\x1b[Z" : "\t";
    case "Backspace": {
      const base = e.ctrlKey ? "\x08" : "\x7f";
      return e.altKey ? `\x1b${base}` : base;
    }
    case "Escape":
      return "\x1b";
  }

  if (e.key.length === 1) {
    // Ctrl combos map into C0 control codes.
    if (e.ctrlKey && !e.altKey && !e.metaKey) {
      const c = e.key.toLowerCase().charCodeAt(0);
      if (c >= 97 && c <= 122) return String.fromCharCode(c - 96); // ctrl+a..z
      const special: Record<string, string> = {
        " ": "\x00",
        "[": "\x1b",
        "\\": "\x1c",
        "]": "\x1d",
        "^": "\x1e",
        _: "\x1f",
      };
      if (e.key in special) return special[e.key];
      return null;
    }
    if (e.metaKey) return null; // OS shortcuts
    // Alt+char sends ESC-prefixed char.
    if (e.altKey) return `\x1b${e.key}`;
    return e.key;
  }

  return null;
}

/** Wrap pasted text per bracketed-paste mode, normalizing newlines to CR. */
export function encodePaste(text: string, bracketed: boolean): string {
  const normalized = text.replace(/\r\n|\n/g, "\r");
  return bracketed ? `\x1b[200~${normalized}\x1b[201~` : normalized;
}
