/**
 * Link detection over the terminal grid mirror (rows of style runs), so the
 * canvas terminal can make URLs and file paths hoverable/clickable. Pure
 * module — no DOM.
 *
 * A cell may carry a real OSC 8 hyperlink URI (`Run.link`); when it does,
 * `linkAt` trusts it outright — the visible text needn't look like a URL at
 * all (e.g. `gh`/markdown-style link labels like "here"). Otherwise links are
 * found by regex over reconstructed row text. Rows are joined across soft
 * wraps using the engine's real per-row wrap flag (`RowUpdate.wrapped`, from
 * libghostty), which is how long links printed by CLIs (e.g. Claude Code)
 * span lines. Two kinds are recognised: `http(s)` URLs, and file paths
 * (absolute, or repo-relative with an extension) that dominate agent output,
 * e.g. `crates/tt-vt/src/search.rs:42`. Besides the link text, `linkAt`
 * reports the exact cells it covers (one segment per row), so the renderer
 * can underline it on hover.
 */

import { isWideRun, type Run } from "@/lib/term-protocol";

export interface LinkSegment {
  y: number;
  /** Inclusive viewport column range. */
  start: number;
  end: number;
}

/** An `http(s)` URL — opened in the system browser. */
export interface UrlLink {
  kind: "url";
  url: string;
  /** One segment per row the link spans (consecutive rows when wrapped). */
  segments: LinkSegment[];
}

/** A file path (optionally with a `:line` suffix) — opened in the editor. */
export interface PathLink {
  kind: "path";
  /** The filesystem path, without any `:line[:col]` suffix. */
  path: string;
  /** The 1-based line from a `:line` suffix, if present. */
  line: number | null;
  segments: LinkSegment[];
}

export type TermLink = UrlLink | PathLink;

/** Display text for a link (URL, or `path[:line]`) — drives the hover tooltip
 * and the hover-dedup identity. */
export function linkLabel(link: TermLink): string {
  if (link.kind === "url") return link.url;
  return link.line != null ? `${link.path}:${link.line}` : link.path;
}

const URL_RE = /https?:\/\/[^\s"'`<>]+/g;
/**
 * A file path: an optional `/`, `./`, `../`, or `~/` prefix, any number of
 * `dir/` segments, then a filename with an extension, and an optional
 * `:line[:col]` suffix. Over-matches (any `word.ext` token); `isPathLike`
 * then keeps only candidates anchored by a `/` or a `:line`, so prose like
 * `example.com` or a bare `1.2.3` version is rejected.
 */
const PATH_RE = /(?:\/|\.\.?\/|~\/)?(?:[\w.@~+-]+\/)*[\w.@~+-]+\.[A-Za-z0-9]+(?::\d+(?::\d+)?)?/g;
/** Punctuation that ends sentences around a link, not the link itself. */
const TRAILING = new Set([".", ",", ";", ":", "!", "?"]);
const CLOSERS: Record<string, string> = { ")": "(", "]": "[", "}": "{" };
/** How many rows a wrapped link may span in either direction from the probe. */
const MAX_WRAP_ROWS = 4;

/** Reconstruct a row's text column-by-column (length = `cols`), so string
 * indices equal terminal columns. Wide characters fill their trailing column
 * with a space. */
export function rowText(runs: Run[], cols: number): string {
  // Runs once per row per frame — `new Array().fill()` allocates once;
  // `Array.from({length}).fill()` would visit every index twice.
  // oxlint-disable-next-line unicorn/no-new-array
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

/** Reconstruct a row's per-column hyperlink URI (length = `cols`), mirroring
 * `rowText`. A run's `link` (or its absence) applies to every column it
 * spans — the engine only merges cells sharing the same link, so a run never
 * straddles a link boundary. */
export function rowLinks(runs: Run[], cols: number): (string | undefined)[] {
  // See rowText above: same per-row hot path, same reason to avoid Array.from.
  // oxlint-disable-next-line unicorn/no-new-array
  const out = new Array<string | undefined>(cols).fill(undefined);
  if (cols <= 0) return out;
  for (const run of runs) {
    if (!run.link) continue;
    const end = Math.min(run.x + run.width, cols);
    for (let x = run.x; x < end; x++) out[x] = run.link;
  }
  return out;
}

/** Drop sentence punctuation and unbalanced closing brackets off a match
 * (links in prose commonly end with `.` or a wrapping `)`). */
function trimTrailing(text: string): string {
  let end = text.length;
  while (end > 0) {
    const ch = text[end - 1];
    if (TRAILING.has(ch)) {
      end--;
      continue;
    }
    const opener = CLOSERS[ch];
    if (opener) {
      const body = text.slice(0, end);
      const opens = [...body].filter((c) => c === opener).length;
      const closes = [...body].filter((c) => c === ch).length;
      if (closes > opens) {
        end--;
        continue;
      }
    }
    break;
  }
  return text.slice(0, end);
}

/** Blank out `http(s)` URL spans so the path matcher never re-claims a URL's
 * tail (e.g. `example.com/x.html`). Indices stay aligned (same length). */
function maskUrls(joined: string): string {
  return joined.replace(URL_RE, (m) => " ".repeat(m.length));
}

/** Keep only path candidates anchored by a `/` or a `:line` suffix, so a bare
 * `foo.rs` or a prose `example.com` / `1.2.3` isn't treated as a path. */
function isPathLike(raw: string): boolean {
  return raw.includes("/") || /:\d/.test(raw);
}

/**
 * A bare `name.ext` (no `/`, no `:line`) is still a path when it is the whole
 * argument of an agent tool-call header — `Update(README.md)`,
 * `Write(vitest.config.ts)` — which is how Claude Code prints root-level
 * files. Anchoring on the `CapitalizedWord(…)` wrapper keeps prose
 * (`example.com`, `1.2.3`, `e.g.`) unlinked. `start`/`end` are the match's
 * inclusive offsets into `text`.
 */
function isToolHeaderArg(text: string, start: number, end: number): boolean {
  if (text[start - 1] !== "(" || text[end + 1] !== ")") return false;
  let i = start - 2;
  while (i >= 0 && /[A-Za-z]/.test(text[i])) i--;
  return /^[A-Z][A-Za-z]+$/.test(text.slice(i + 1, start - 1));
}

/** Split a matched path into its filesystem path and 1-based line (paths never
 * contain `:`, so the first colon starts the `:line[:col]` suffix). */
function splitPathLine(raw: string): { path: string; line: number | null } {
  const colon = raw.indexOf(":");
  if (colon < 0) return { path: raw, line: null };
  const line = Number.parseInt(raw.slice(colon + 1), 10);
  return { path: raw.slice(0, colon), line: Number.isNaN(line) ? null : line };
}

/** The cells a link covers, given its inclusive `[start, end]` offsets into the
 * wrap-joined block that starts at viewport row `startRow`. */
function segmentsFor(start: number, end: number, startRow: number, cols: number): LinkSegment[] {
  const segments: LinkSegment[] = [];
  const first = Math.floor(start / cols);
  const last = Math.floor(end / cols);
  for (let r = first; r <= last; r++) {
    segments.push({
      y: startRow + r,
      start: r === first ? start % cols : 0,
      end: r === last ? end % cols : cols - 1,
    });
  }
  return segments;
}

/**
 * The link under viewport cell (x, y), or null. `lines` is the grid mirror's
 * row array; rows the engine marked soft-wrapped (`wrapped`) are joined into
 * one string before matching. URLs win over paths where both could match
 * (URL spans are masked out before path detection).
 */
export function linkAt(
  lines: { runs: Run[]; wrapped?: boolean }[],
  cols: number,
  x: number,
  y: number,
): TermLink | null {
  if (cols <= 0 || x < 0 || y < 0 || y >= lines.length) return null;

  const text = (row: number) => rowText(lines[row]?.runs ?? [], cols);

  // Find the wrapped block containing row y: walk up while the row above
  // flows into ours, then down while rows keep flowing.
  let startRow = y;
  while (y - startRow < MAX_WRAP_ROWS && startRow > 0 && lines[startRow - 1]?.wrapped) {
    startRow--;
  }
  let endRow = y;
  while (endRow - y < MAX_WRAP_ROWS && endRow + 1 < lines.length && lines[endRow]?.wrapped) {
    endRow++;
  }

  const rows: string[] = [];
  const linkRows: (string | undefined)[] = [];
  for (let r = startRow; r <= endRow; r++) {
    rows.push(text(r));
    linkRows.push(...rowLinks(lines[r]?.runs ?? [], cols));
  }
  const joined = rows.join("");
  const probe = (y - startRow) * cols + x;

  // A real OSC 8 hyperlink is an unambiguous signal — trust it over the
  // regex heuristics below, since the visible text may not look like a link.
  const hyperlink = linkRows[probe];
  if (hyperlink) {
    let start = probe;
    while (start > 0 && linkRows[start - 1] === hyperlink) start--;
    let end = probe;
    while (end < linkRows.length - 1 && linkRows[end + 1] === hyperlink) end++;
    return { kind: "url", url: hyperlink, segments: segmentsFor(start, end, startRow, cols) };
  }

  for (const m of joined.matchAll(URL_RE)) {
    const url = trimTrailing(m[0]);
    if (url.length <= "https://".length) continue;
    const start = m.index;
    const end = start + url.length - 1; // inclusive
    if (probe < start || probe > end) continue;
    return { kind: "url", url, segments: segmentsFor(start, end, startRow, cols) };
  }

  const masked = maskUrls(joined);
  for (const m of masked.matchAll(PATH_RE)) {
    const raw = trimTrailing(m[0]);
    const start = m.index;
    const end = start + raw.length - 1; // inclusive
    if (!isPathLike(raw) && !isToolHeaderArg(masked, start, end)) continue;
    if (probe < start || probe > end) continue;
    const { path, line } = splitPathLine(raw);
    return { kind: "path", path, line, segments: segmentsFor(start, end, startRow, cols) };
  }
  return null;
}
