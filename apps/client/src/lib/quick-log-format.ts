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
 * Matches the `ttr journal jot` bullet format (`- HH:MM text`) exactly so app and CLI
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
