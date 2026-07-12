/**
 * Quick-add token parsing for the Board's New-todo input. Lets a todo be typed
 * as `ship the release @tomorrow #octo/widgets`, pulling a due date (`@today` /
 * `@tomorrow` / `@YYYY-MM-DD`) and a repo tag (`#owner/repo`) out of the text
 * and stripping the tokens from the stored title. Pure and host-independent so
 * it unit-tests without React or the Tauri shell; `now` is injected (never read
 * from the clock here) so `@today`/`@tomorrow` resolve deterministically.
 */

/** A parsed New-todo entry: the cleaned title plus any recognized tokens. */
export type QuickAdd = {
  /** The todo text with recognized tokens removed and whitespace collapsed. */
  text: string;
  /** Due date as epoch ms at the end of the local day, when a `@` token was found. */
  dueTs?: number;
  /** `owner/repo`, when a `#owner/repo` tag was found. */
  repo?: string;
};

/** A due token (`@today` / `@tomorrow` / `@YYYY-MM-DD`) as a whole word. */
const DUE_TOKEN = /(?:^|\s)@(today|tomorrow|\d{4}-\d{2}-\d{2})(?=\s|$)/i;
/** A repo tag (`#owner/repo`) as a whole word. */
const REPO_TOKEN = /(?:^|\s)#([\w.-]+\/[\w.-]+)(?=\s|$)/;

/** Epoch ms at the end of the local calendar day `n` days from `now`, matching
 * the Board's date-picker semantics (a card is not overdue until the day ends). */
function endOfDay(now: number, addDays: number): number {
  const d = new Date(now);
  d.setDate(d.getDate() + addDays);
  d.setHours(23, 59, 59, 999);
  return d.getTime();
}

/** Resolve a `@` token's captured value to an end-of-day epoch ms, or
 * `undefined` when it's a `YYYY-MM-DD` that isn't a real calendar date. */
function resolveDue(value: string, now: number): number | undefined {
  const lower = value.toLowerCase();
  if (lower === "today") return endOfDay(now, 0);
  if (lower === "tomorrow") return endOfDay(now, 1);
  const [y, m, d] = value.split("-").map(Number);
  const date = new Date(y, m - 1, d, 23, 59, 59, 999);
  // Reject overflow like `2026-13-40` that Date silently rolls forward.
  if (date.getFullYear() !== y || date.getMonth() !== m - 1 || date.getDate() !== d) {
    return undefined;
  }
  return date.getTime();
}

/**
 * Parse a New-todo input into its title, due date, and repo tag. The first
 * valid `@` and `#` token each win; a token that doesn't fully match (a bare
 * `@`, a bad date, a `#word` with no slash) is left in the text verbatim. `now`
 * is the shared wall clock, injected so resolution is deterministic and testable.
 */
export function parseQuickAdd(input: string, now: number): QuickAdd {
  let text = input;
  const result: QuickAdd = { text: input.trim() };

  const dueMatch = text.match(DUE_TOKEN);
  if (dueMatch) {
    const dueTs = resolveDue(dueMatch[1], now);
    if (dueTs !== undefined) {
      result.dueTs = dueTs;
      text = text.replace(dueMatch[0], " ");
    }
  }

  const repoMatch = text.match(REPO_TOKEN);
  if (repoMatch) {
    result.repo = repoMatch[1];
    text = text.replace(repoMatch[0], " ");
  }

  result.text = text.replace(/\s+/g, " ").trim();
  return result;
}
