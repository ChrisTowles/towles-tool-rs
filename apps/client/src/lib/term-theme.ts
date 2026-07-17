/**
 * Terminal color theme pushed into the Rust engine (mirrors `tt_vt::Theme` /
 * the `TermTheme` arg of `term_start`/`term_theme`), so OSC 10/11 and
 * color-scheme queries (`CSI ? 996 n`) answer the app's real colors — that's
 * how programs like Claude Code decide dark vs light — and so indexed ANSI
 * colors resolve against a palette that matches the app theme.
 *
 * Default fg/bg come from the host element's *computed* style, so they track
 * whatever dark/light + color-theme combination is active without this file
 * knowing the token system. The ANSI 0–15 palette is Catppuccin Mocha (dark)
 * / Latte (light) — the palettes the app's existing terminal fallback colors
 * already came from — taken from catppuccin's official terminal ports, not
 * hand-derived. Per-color-theme ANSI palettes (Nord, …) can layer on later by
 * keying this table off `data-color-theme`.
 */

export interface TermTheme {
  /** Packed 0xRRGGBB. */
  fg: number;
  bg: number;
  cursor?: number;
  /** ANSI colors 0–15, packed 0xRRGGBB. */
  palette16: number[];
  dark: boolean;
}

/** Catppuccin Mocha ANSI 0–15 (normal 0–7, bright 8–15). */
export const ANSI_DARK: number[] = [
  0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
  0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
];

/** Catppuccin Latte ANSI 0–15. */
export const ANSI_LIGHT: number[] = [
  0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
  0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
];

/** Fallbacks when a computed color can't be parsed (Mocha text/base, Latte
 * text/base) — the same constants the canvas used before themes flowed
 * through the engine. */
const FALLBACK = {
  dark: { fg: 0xcdd6f4, bg: 0x1e1e2e },
  light: { fg: 0x4c4f69, bg: 0xeff1f5 },
};

/** Parse a resolved CSS color — `#rrggbb`, `rgb(r, g, b)`, or
 * `rgba(r, g, b, a)`, the forms getComputedStyle emits — into packed
 * 0xRRGGBB; null when unparseable (e.g. a `color(srgb …)` form). */
export function cssColorToPacked(css: string): number | null {
  const s = css.trim();
  const hex = /^#([0-9a-f]{6})$/i.exec(s);
  if (hex) return Number.parseInt(hex[1], 16);
  const rgb = /^rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/i.exec(s);
  if (!rgb) return null;
  const [r, g, b] = [rgb[1], rgb[2], rgb[3]].map((v) => Math.min(255, Number.parseInt(v, 10)));
  return (r << 16) | (g << 8) | b;
}

/** Resolve the terminal theme from the host element's computed colors (which
 * already reflect the active dark/light + color-theme tokens) and the root
 * `dark` class. DOM-reading by necessity; the parsing lives in
 * `cssColorToPacked` so it stays unit-testable. */
export function resolveTermTheme(host: Element): TermTheme {
  const dark = document.documentElement.classList.contains("dark");
  const cs = getComputedStyle(host);
  const fallback = dark ? FALLBACK.dark : FALLBACK.light;
  return {
    fg: cssColorToPacked(cs.color) ?? fallback.fg,
    bg: cssColorToPacked(cs.backgroundColor) ?? fallback.bg,
    palette16: dark ? ANSI_DARK : ANSI_LIGHT,
    dark,
  };
}
