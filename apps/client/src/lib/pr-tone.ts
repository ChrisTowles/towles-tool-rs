import type { PrItem } from "./data";

/**
 * The one PR-status → hue mapping, shared by every surface that colorizes a
 * PR (repo-header `PrChip`, Cockpit's `ChecksBadge`, the day bar's attention
 * feed, the Agentboard attention strip, Board link chips). Hues follow the
 * Folder Rail status palette (`statusColor()` in `lib/agentboard.ts`):
 *
 * - `running` → cyan, the rail's busy hue. CI in flight is progress, not a
 *   problem — never red/amber, which read as "act now".
 * - `failed` → red: checks failed, or the PR was closed without merging.
 * - `passing` → green, mirroring the rail's `complete`.
 * - `merged` → purple, matching `LandedBadge` — done, clean up the task.
 * - `review` → blue, the rail's waiting-on-you hue (review requested is a
 *   fact about *you*, orthogonal to the checks axis — callers pick it
 *   explicitly, `prTone` never returns it).
 * - `plain` → neutral: open with no checks reported.
 */
export type PrTone = "merged" | "failed" | "running" | "passing" | "review" | "plain";

/** The subset of tones a checks rollup alone can produce. */
export type ChecksTone = Extract<PrTone, "failed" | "passing" | "plain" | "running">;

/** Tone of a checks rollup alone, ignoring PR state — for surfaces that show
 * the CI axis by itself (e.g. `ChecksBadge`, where a merged PR still reads
 * "passing", not purple). */
export function checksTone(checks: string): ChecksTone {
  if (checks === "failing") return "failed";
  if (checks === "passing") return "passing";
  if (checks === "none") return "plain";
  // "pending" — and any collector value this map doesn't know yet, so a new
  // state degrades visibly (as in-flight) instead of vanishing into neutral.
  return "running";
}

/** Resolve a PR's tone from its collector-observed state + checks rollup. */
export function prTone(pr: Pick<PrItem, "state" | "checks">): PrTone {
  if (pr.state === "merged") return "merged";
  if (pr.state === "closed") return "failed";
  return checksTone(pr.checks);
}

/** Tailwind classes per tone, one facet per surface shape. */
export const PR_TONE: Record<
  PrTone,
  { chip: string; text: string; border: string; badge: string }
> = {
  merged: {
    chip: "border-purple-500/50 bg-purple-500/10 text-purple-600 hover:bg-purple-500/20 dark:text-purple-400",
    text: "text-purple-600 dark:text-purple-400",
    border: "border-l-purple-500",
    badge: "bg-purple-500/15 text-purple-700 dark:bg-purple-500/20 dark:text-purple-400",
  },
  failed: {
    chip: "border-red-500/50 bg-red-500/10 text-red-600 hover:bg-red-500/20 dark:text-red-400",
    text: "text-red-500 dark:text-red-400",
    border: "border-l-red-500",
    badge: "bg-red-500/15 text-red-700 dark:bg-red-500/20 dark:text-red-400",
  },
  running: {
    chip: "border-cyan-500/50 bg-cyan-500/10 text-cyan-600 hover:bg-cyan-500/20 dark:text-cyan-400",
    text: "text-cyan-600 dark:text-cyan-400",
    border: "border-l-cyan-500",
    badge: "bg-cyan-500/15 text-cyan-700 dark:bg-cyan-500/20 dark:text-cyan-400",
  },
  passing: {
    chip: "border-green-500/50 bg-green-500/10 text-green-600 hover:bg-green-500/20 dark:text-green-400",
    text: "text-green-600 dark:text-green-400",
    border: "border-l-green-500",
    badge: "bg-green-500/15 text-green-700 dark:bg-green-500/20 dark:text-green-400",
  },
  review: {
    chip: "border-blue-500/50 bg-blue-500/10 text-blue-600 hover:bg-blue-500/20 dark:text-blue-400",
    text: "text-blue-500 dark:text-blue-400",
    border: "border-l-blue-500",
    badge: "bg-blue-500/15 text-blue-700 dark:bg-blue-500/20 dark:text-blue-400",
  },
  plain: {
    chip: "border-border/70 text-muted-foreground hover:bg-accent hover:text-foreground",
    text: "text-muted-foreground",
    border: "border-l-border",
    badge: "bg-muted text-muted-foreground",
  },
};
