import type { CollectRun } from "./data";

/**
 * Always-on collector health, derived purely from the store's run bookkeeping
 * (`snapshot.runs`). The status bar renders one small dot per known collector
 * from this; keeping the classification here (not in the component) makes the
 * threshold behaviour unit-testable with an injected `now`.
 */

/** The four collector keys the app knows about (mirrors `tt-collect`). */
export type CollectorKey = "prs" | "issues" | "claude:calendar" | "slack:dm";

/**
 * - `fresh` — last run succeeded and is within the stale window.
 * - `stale` — last run succeeded but is older than the stale window.
 * - `failing` — last run errored (auth expired, network, …).
 * - `never-ran` — no run recorded yet.
 */
export type CollectorState = "fresh" | "stale" | "failing" | "never-ran";

export const KNOWN_COLLECTORS: readonly CollectorKey[] = [
  "prs",
  "issues",
  "claude:calendar",
  "slack:dm",
];

/** Human labels for tooltips (the raw keys are terse). */
export const COLLECTOR_LABELS: Record<CollectorKey, string> = {
  prs: "Pull requests",
  issues: "Issues",
  "claude:calendar": "Calendar",
  "slack:dm": "Slack DM",
};

/**
 * Age (ms) past a *successful* run after which a collector reads as stale.
 * Tuned per collector cadence — PRs/Slack refresh in seconds so they go stale
 * quickly, issues/calendar run on the minute scale — and overridable per call.
 */
export const DEFAULT_STALE_MS: Record<CollectorKey, number> = {
  prs: 5 * 60_000,
  issues: 30 * 60_000,
  "claude:calendar": 60 * 60_000,
  "slack:dm": 5 * 60_000,
};

export type CollectorHealth = {
  key: CollectorKey;
  label: string;
  state: CollectorState;
  run: CollectRun | undefined;
};

/**
 * Classify a single collector's latest run. `staleMs` is the boundary: a run
 * exactly `staleMs` old (or older) is `stale`, anything younger is `fresh`.
 */
export function classifyRun(
  run: CollectRun | undefined,
  now: number,
  staleMs: number,
): CollectorState {
  if (!run) return "never-ran";
  if (!run.ok) return "failing";
  return now - run.ranAt < staleMs ? "fresh" : "stale";
}

/**
 * Health for every known collector, newest-run-wins from `runs`. Order follows
 * {@link KNOWN_COLLECTORS} so the dot cluster is stable across renders.
 */
export function collectorHealth(
  runs: CollectRun[],
  now: number,
  staleMs: Partial<Record<CollectorKey, number>> = {},
): CollectorHealth[] {
  const latest = new Map<string, CollectRun>();
  for (const run of runs) {
    const prev = latest.get(run.collector);
    if (!prev || run.ranAt > prev.ranAt) latest.set(run.collector, run);
  }
  return KNOWN_COLLECTORS.map((key) => {
    const run = latest.get(key);
    const threshold = staleMs[key] ?? DEFAULT_STALE_MS[key];
    return { key, label: COLLECTOR_LABELS[key], state: classifyRun(run, now, threshold), run };
  });
}
