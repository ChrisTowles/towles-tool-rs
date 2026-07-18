/**
 * Monaco selection → Claude IDE-bridge conversions, shared by the file viewer
 * and the diff editor.
 *
 * Two wire shapes, and they disagree on purpose:
 * - **streaming** (`ide_set_selection`) wants 1-based lines and 0-based
 *   character columns, so it can quote the exact substring.
 * - **at-mention** (`ide_at_mention`) wants 1-based inclusive lines only, and
 *   omits them entirely for a whole-file mention.
 *
 * Both live here because the column arithmetic and the trailing-line rule are
 * easy to get subtly wrong and impossible to unit-test through a component.
 */

/** The shape of `monaco.Selection` this module needs. */
export type MonacoSelectionLike = {
  startLineNumber: number;
  endLineNumber: number;
  startColumn: number;
  endColumn: number;
};

/** `ide_set_selection`: 1-based lines, 0-based character columns. */
export type StreamRange = {
  startLine: number;
  endLine: number;
  startChar: number;
  endChar: number;
};

/** `ide_at_mention`: 1-based inclusive lines. */
export type MentionRange = { startLine: number; endLine: number };

export function streamRangeFrom(sel: MonacoSelectionLike): StreamRange {
  return {
    startLine: sel.startLineNumber,
    endLine: sel.endLineNumber,
    startChar: sel.startColumn - 1,
    endChar: sel.endColumn - 1,
  };
}

/**
 * The line range to @-mention, or `null` when nothing is selected — which is
 * what makes one code path serve both gestures: no range means a whole-file
 * mention.
 */
export function mentionRangeFrom(sel: MonacoSelectionLike | null | undefined): MentionRange | null {
  if (!sel) return null;
  const empty = sel.startLineNumber === sel.endLineNumber && sel.startColumn === sel.endColumn;
  if (empty) return null;
  const startLine = Math.min(sel.startLineNumber, sel.endLineNumber);
  let endLine = Math.max(sel.startLineNumber, sel.endLineNumber);
  // A triple-click or shift+down parks the caret in column 1 of the *next*
  // line. Visually that line isn't selected, so mentioning it would send
  // L12-41 for what the user sees as 12–40.
  if (sel.endColumn === 1 && endLine > startLine) endLine -= 1;
  return { startLine, endLine };
}

/** A one-line range collapses to a single "L12" in both spellings. */
function lines(range: MentionRange, dash: string): string {
  return range.startLine === range.endLine
    ? `L${range.startLine}`
    : `L${range.startLine}${dash}${range.endLine}`;
}

/** "L12" / "L12–40" — display text, so an en dash. */
export function formatLineRange(range: MentionRange): string {
  return lines(range, "–");
}

/** How the mention reads in Claude's prompt: "src/app.ts#L12-40". ASCII
 * hyphen here — Claude parses this one, it isn't display text. */
export function formatMentionRef(path: string, range: MentionRange | null): string {
  return range ? `${path}#${lines(range, "-")}` : path;
}

export function sameMentionRange(a: MentionRange | null, b: MentionRange | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.startLine === b.startLine && a.endLine === b.endLine;
}

/**
 * The repo-relative path a diff model belongs to, or `null` if this model
 * isn't a working-tree file in `dir`. The multi-diff editor holds both sides
 * of every file; only the modified side (`tt-diff-work`) maps to something
 * Claude can open.
 */
export function diffWorkPath(
  dir: string,
  uri: { scheme: string; path: string } | null | undefined,
): string | null {
  if (uri?.scheme !== "tt-diff-work") return null;
  const prefix = `${dir}/`;
  if (!uri.path.startsWith(prefix)) return null;
  return uri.path.slice(prefix.length);
}
