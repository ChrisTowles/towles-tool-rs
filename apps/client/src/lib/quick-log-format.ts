/**
 * Formatting for the ⌘J quick-log timeline. Pure and clock-free: the caller injects
 * `now`, so this module can be unit-tested without mocking time.
 */

export type FormatLogOpts = {
  /**
   * When the line is being logged, as a `Date` or epoch-ms number. The pure function
   * never reads the clock itself — the caller passes `new Date()` / `Date.now()`.
   */
  now: Date | number;
  /**
   * Active screen id (or repo name) the log was captured from. Trimmed; when empty the
   * `[context]` bracket is omitted entirely.
   */
  context?: string;
};

/**
 * Build one journal timeline bullet: `- HH:MM [context] text`.
 *
 * Matches the `tt journal jot` bullet format (`- HH:MM text`) exactly so app and CLI
 * captures interleave cleanly in the same daily note — the optional `[context]` prefix
 * just annotates the free-text body. The time is local (matching the daily-note
 * convention) and zero-padded to `HH:MM`. The bracket is dropped when `context` is empty.
 */
export function formatLogLine(text: string, opts: FormatLogOpts): string {
  const when = typeof opts.now === "number" ? new Date(opts.now) : opts.now;
  const hh = String(when.getHours()).padStart(2, "0");
  const mm = String(when.getMinutes()).padStart(2, "0");
  const body = text.trim();
  const context = opts.context?.trim();
  const prefix = context ? `[${context}] ` : "";
  return `- ${hh}:${mm} ${prefix}${body}`;
}

/** Where a quick-log line is routed: the daily journal note or the Board's todo store. */
export type QuickLogKind = "journal" | "todo";

export type ParsedQuickLog = {
  kind: QuickLogKind;
  /** The remaining text after stripping a routing prefix, trimmed. */
  body: string;
};

/** Leading tokens that route a quick-log line to the Board instead of the journal. */
const TODO_PREFIXES = ["/todo", "/t"];

/**
 * Classify a ⌘J quick-log line by its leading prefix.
 *
 * A leading `/todo ` or `/t ` (case-insensitive, requiring at least one space after the
 * token) routes the remainder to the todo store; anything else stays a journal entry. The
 * returned `body` is trimmed and has the routing prefix removed. A prefix with no body
 * after it (e.g. `/todo` or `/t   `) yields an empty `todo` body — the caller decides
 * whether to submit. This is pure so the routing rule can be unit-tested directly.
 */
export function parseQuickLog(text: string): ParsedQuickLog {
  const trimmed = text.trim();
  for (const prefix of TODO_PREFIXES) {
    if (trimmed.toLowerCase() === prefix) {
      return { kind: "todo", body: "" };
    }
    if (trimmed.slice(0, prefix.length).toLowerCase() === prefix) {
      const rest = trimmed.slice(prefix.length);
      if (/^\s/.test(rest)) {
        return { kind: "todo", body: rest.trim() };
      }
    }
  }
  return { kind: "journal", body: trimmed };
}
