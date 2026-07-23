/**
 * Text logic behind the new-task form's goal editor: what to highlight, and
 * where a `#`-issue autocomplete is being typed.
 *
 * Pure and kept out of the component on purpose (this repo tests logic, not
 * components — see `apps/client/CLAUDE.md`): caret arithmetic and tokenizing
 * are exactly the parts that break silently in a DOM test-free codebase.
 */

/** One run of goal text, classified for the highlight overlay. */
export type GoalSegment = {
  text: string;
  kind: "plain" | "url" | "ref";
};

/** URLs and `#123` issue refs — the two things worth colouring in a task goal.
 * Deliberately narrow: this is a one-line-ish task description, not a markdown
 * document, so there's no bold/heading/code syntax to model. */
const TOKEN = /(https?:\/\/[^\s<>()]+)|(#\d+)\b/g;

/**
 * Split `text` into highlight runs. Always covers the input exactly — the
 * concatenated segment texts equal the input — so the overlay can't drift out
 * of alignment with the textarea it sits behind.
 */
export function highlightSegments(text: string): GoalSegment[] {
  const out: GoalSegment[] = [];
  let last = 0;
  // `matchAll` needs the /g flag, which TOKEN has; a fresh iterator per call
  // means no shared lastIndex between calls.
  for (const m of text.matchAll(TOKEN)) {
    const start = m.index;
    if (start > last) out.push({ text: text.slice(last, start), kind: "plain" });
    out.push({ text: m[0], kind: m[1] ? "url" : "ref" });
    last = start + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last), kind: "plain" });
  return out;
}

/**
 * The `#`-mention being typed at `caret`, or `null`.
 *
 * A mention only counts when its `#` starts a word — otherwise `foo#1` and, more
 * importantly, a URL fragment like `…/pull/4#issuecomment` would pop the issue
 * list mid-paste. `start` is the index of the `#` itself so
 * {@link applyMention} can replace the whole token.
 */
export function mentionQueryAt(
  text: string,
  caret: number,
): { start: number; query: string } | null {
  let i = caret;
  while (i > 0 && /[\w-]/.test(text[i - 1])) i -= 1;
  if (i === 0 || text[i - 1] !== "#") return null;
  const start = i - 1;
  if (start > 0 && !/\s/.test(text[start - 1])) return null;
  return { start, query: text.slice(i, caret) };
}

/**
 * Replace the mention token spanning `[start, caret)` with `#<number> `, and
 * report where the caret should land (after the inserted space, so typing
 * continues naturally rather than inside the reference).
 */
export function applyMention(
  text: string,
  start: number,
  caret: number,
  issueNumber: number,
): { text: string; caret: number } {
  const insert = `#${issueNumber} `;
  return {
    text: text.slice(0, start) + insert + text.slice(caret),
    caret: start + insert.length,
  };
}

/**
 * Insert a `#` at `caret` to start a mention, as the hint button does.
 *
 * Adds a leading space when the caret sits against a word, because
 * {@link mentionQueryAt} only recognises a `#` that starts one — without it the
 * button would type a character and pointedly do nothing.
 */
export function insertMentionTrigger(text: string, caret: number): { text: string; caret: number } {
  const before = text.slice(0, caret);
  const insert = before.length > 0 && !/\s$/.test(before) ? " #" : "#";
  return {
    text: before + insert + text.slice(caret),
    caret: caret + insert.length,
  };
}

/**
 * Filter issues for a mention query. An all-digit query matches on number
 * (typing `#12` should surface #12 and #123 before anything whose *title*
 * happens to contain "12"); otherwise it's a case-insensitive title match.
 * An empty query — just `#` typed — lists everything.
 */
export function matchIssues<T extends { number: number; title: string }>(
  issues: T[],
  query: string,
): T[] {
  const q = query.trim().toLowerCase();
  if (!q) return issues;
  if (/^\d+$/.test(q)) return issues.filter((i) => String(i.number).startsWith(q));
  return issues.filter((i) => i.title.toLowerCase().includes(q));
}

/**
 * The distinct issue numbers already referenced as `#N` in `text`, in the
 * order they first appear. Reuses {@link highlightSegments}'s classification
 * so a URL fragment like `…/pull/4#issuecomment` is never mistaken for a
 * reference — same rule, one source of truth.
 */
export function referencedIssueNumbers(text: string): number[] {
  const seen = new Set<number>();
  for (const seg of highlightSegments(text)) {
    if (seg.kind !== "ref") continue;
    seen.add(Number(seg.text.slice(1)));
  }
  return [...seen];
}
