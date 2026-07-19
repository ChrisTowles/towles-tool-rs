/**
 * Quick-add token parsing for the Board's New-task input. Lets a task be typed
 * as `ship the release @tomorrow`, pulling a due date (`@today` / `@tomorrow` /
 * `@YYYY-MM-DD`) out of the text and stripping the token from the stored
 * title. (The old `#owner/repo` tag died with the bare-repo association in
 * #339 — a task relates to repos through its issue/PR links and slot now.)
 * Pure and host-independent so it unit-tests without React or the Tauri
 * shell; `now` is injected (never read from the clock here) so
 * `@today`/`@tomorrow` resolve deterministically.
 */

/** A parsed New-task entry: the cleaned title plus any recognized tokens. */
export type QuickAdd = {
  /** The task text with recognized tokens removed and whitespace collapsed. */
  text: string;
  /** Due date as epoch ms at the end of the local day, when a `@` token was found. */
  dueTs?: number;
};

/** A due token (`@today` / `@tomorrow` / `@YYYY-MM-DD`) as a whole word. */
const DUE_TOKEN = /(?:^|\s)@(today|tomorrow|\d{4}-\d{2}-\d{2})(?=\s|$)/i;

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
 * Parse a New-task input into its title and due date. The first valid `@`
 * token wins; a token that doesn't fully match (a bare `@`, a bad date) is
 * left in the text verbatim. `now` is the shared wall clock, injected so
 * resolution is deterministic and testable.
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

  result.text = text.replace(/\s+/g, " ").trim();
  return result;
}
