/**
 * URL detection over the terminal grid mirror (rows of style runs), so the
 * canvas terminal can make links hoverable/clickable. Pure module — no DOM.
 *
 * The grid has no explicit hyperlink info (plain-text URLs from CLI output),
 * so links are found by regex over reconstructed row text. A row whose text
 * reaches the last column is treated as hard-wrapped into the next row, which
 * is how long URLs printed by CLIs (e.g. Claude Code) span lines. Besides the
 * URL itself, `linkAt` reports the exact cells it covers (one segment per
 * row), so the renderer can underline it on hover.
 */

import { isWideRun, type Run } from "@/lib/term-protocol";

export interface LinkSegment {
  y: number;
  /** Inclusive viewport column range. */
  start: number;
  end: number;
}

export interface TermLink {
  url: string;
  /** One segment per row the URL spans (consecutive rows when wrapped). */
  segments: LinkSegment[];
}

const URL_RE = /https?:\/\/[^\s"'`<>]+/g;
/** Punctuation that ends sentences around a URL, not the URL itself. */
const TRAILING = new Set([".", ",", ";", ":", "!", "?"]);
const CLOSERS: Record<string, string> = { ")": "(", "]": "[", "}": "{" };
/** How many rows a wrapped URL may span in either direction from the probe. */
const MAX_WRAP_ROWS = 4;

/** Reconstruct a row's text column-by-column (length = `cols`), so string
 * indices equal terminal columns. Wide characters fill their trailing column
 * with a space. */
export function rowText(runs: Run[], cols: number): string {
  const chars = new Array<string>(cols).fill(" ");
  for (const run of runs) {
    const wide = isWideRun(run);
    let x = run.x;
    for (const ch of run.text) {
      if (x >= cols) break;
      chars[x] = ch;
      x += wide && ch.charCodeAt(0) > 0xff ? 2 : 1;
    }
  }
  return chars.join("");
}

/** Drop sentence punctuation and unbalanced closing brackets off a match
 * (URLs in prose commonly end with `.` or a wrapping `)`). */
function trimUrl(url: string): string {
  let end = url.length;
  while (end > 0) {
    const ch = url[end - 1];
    if (TRAILING.has(ch)) {
      end--;
      continue;
    }
    const opener = CLOSERS[ch];
    if (opener) {
      const body = url.slice(0, end);
      const opens = [...body].filter((c) => c === opener).length;
      const closes = [...body].filter((c) => c === ch).length;
      if (closes > opens) {
        end--;
        continue;
      }
    }
    break;
  }
  return url.slice(0, end);
}

/**
 * The link under viewport cell (x, y), or null. `lines` is the grid mirror's
 * row array; rows are joined into one string across soft wraps (a row is
 * considered wrapped when its text runs to the last column) before matching.
 */
export function linkAt(
  lines: { runs: Run[] }[],
  cols: number,
  x: number,
  y: number,
): TermLink | null {
  if (cols <= 0 || x < 0 || y < 0 || y >= lines.length) return null;

  const text = (row: number) => rowText(lines[row]?.runs ?? [], cols);
  const wrapsToNext = (t: string) => t[cols - 1] !== " ";

  // Find the wrapped block containing row y: walk up while the row above
  // flows into ours, then down while rows keep flowing.
  let startRow = y;
  while (y - startRow < MAX_WRAP_ROWS && startRow > 0 && wrapsToNext(text(startRow - 1))) {
    startRow--;
  }
  let endRow = y;
  while (endRow - y < MAX_WRAP_ROWS && endRow + 1 < lines.length && wrapsToNext(text(endRow))) {
    endRow++;
  }

  const rows: string[] = [];
  for (let r = startRow; r <= endRow; r++) rows.push(text(r));
  const joined = rows.join("");
  const probe = (y - startRow) * cols + x;

  for (const m of joined.matchAll(URL_RE)) {
    const url = trimUrl(m[0]);
    if (url.length <= "https://".length) continue;
    const start = m.index;
    const end = start + url.length - 1; // inclusive
    if (probe < start || probe > end) continue;

    const segments: LinkSegment[] = [];
    for (let r = Math.floor(start / cols); r <= Math.floor(end / cols); r++) {
      segments.push({
        y: startRow + r,
        start: r === Math.floor(start / cols) ? start % cols : 0,
        end: r === Math.floor(end / cols) ? end % cols : cols - 1,
      });
    }
    return { url, segments };
  }
  return null;
}
