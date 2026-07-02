// Fixed effective glyph set (the vestigial `theme.icons` indirection is skipped
// per UI-SPEC §4/§6 — components use these directly).

/** Spinner frames; ticks at 120ms while any agent is running. */
export const SPINNERS = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] as const;

/** Spinner cadence in ms. */
export const SPINNER_INTERVAL_MS = 120;

export const UNSEEN_ICON = "●";
export const DONE_ICON = "✓";
export const ERROR_ICON = "✗";
export const INTERRUPTED_ICON = "⚠";
export const WAITING_ICON = "◉";
export const QUESTION_ICON = "?";
export const IDLE_ICON = "○";

/** Truncation caps (hard char cuts). */
export const CAP_NAME = 18;
export const CAP_BRANCH = 45;
export const CAP_THREAD_NAME = 40;
export const CAP_SUBAGENT_DESC = 40;
export const CAP_LOOP_REASON = 36;
