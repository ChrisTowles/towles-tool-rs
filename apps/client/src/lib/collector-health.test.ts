import { describe, expect, it } from "vitest";
import {
  classifyRun,
  collectorHealth,
  DEFAULT_STALE_MS,
  KNOWN_COLLECTORS,
  type CollectorState,
} from "./collector-health";
import type { CollectRun } from "./data";

const NOW = 1_000_000_000;
const run = (over: Partial<CollectRun> = {}): CollectRun => ({
  collector: "prs",
  ranAt: NOW,
  ok: true,
  ...over,
});

describe("classifyRun", () => {
  const staleMs = 10 * 60_000;

  it("is never-ran with no run", () => {
    expect(classifyRun(undefined, NOW, staleMs)).toBe("never-ran");
  });

  it("is failing when the last run errored, regardless of age", () => {
    expect(classifyRun(run({ ok: false }), NOW, staleMs)).toBe("failing");
    expect(classifyRun(run({ ok: false, ranAt: NOW - 999 * 60_000 }), NOW, staleMs)).toBe(
      "failing",
    );
  });

  it("is fresh just inside the stale window", () => {
    expect(classifyRun(run({ ranAt: NOW - (staleMs - 1) }), NOW, staleMs)).toBe("fresh");
  });

  it("is stale exactly at the boundary", () => {
    expect(classifyRun(run({ ranAt: NOW - staleMs }), NOW, staleMs)).toBe("stale");
  });

  it("is stale past the boundary", () => {
    expect(classifyRun(run({ ranAt: NOW - (staleMs + 1) }), NOW, staleMs)).toBe("stale");
  });

  it("treats a run at now as fresh", () => {
    expect(classifyRun(run({ ranAt: NOW }), NOW, staleMs)).toBe("fresh");
  });
});

describe("collectorHealth", () => {
  it("returns one entry per known collector, in stable order", () => {
    const health = collectorHealth([], NOW);
    expect(health.map((h) => h.key)).toEqual([...KNOWN_COLLECTORS]);
    expect(health.every((h) => h.state === "never-ran")).toBe(true);
  });

  it("classifies each collector against its own default threshold", () => {
    // Aged 6 minutes: past the PR window (5m) but within the issues window (30m).
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 6 * 60_000 }),
      run({ collector: "issues", ranAt: NOW - 6 * 60_000 }),
    ];
    const byKey = Object.fromEntries(
      collectorHealth(runs, NOW).map((h) => [h.key, h.state]),
    ) as Record<string, CollectorState>;
    expect(byKey.prs).toBe("stale");
    expect(byKey.issues).toBe("fresh");
    expect(byKey["claude:calendar"]).toBe("never-ran");
    expect(byKey["slack:dm"]).toBe("never-ran");
  });

  it("uses the newest run per collector", () => {
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 60 * 60_000, ok: false }),
      run({ collector: "prs", ranAt: NOW - 1000, ok: true }),
    ];
    const prs = collectorHealth(runs, NOW).find((h) => h.key === "prs");
    expect(prs?.state).toBe("fresh");
    expect(prs?.run?.ok).toBe(true);
  });

  it("honours a per-call stale override", () => {
    const runs: CollectRun[] = [run({ collector: "prs", ranAt: NOW - 2 * 60_000 })];
    const health = collectorHealth(runs, NOW, { prs: 60_000 });
    expect(health.find((h) => h.key === "prs")?.state).toBe("stale");
  });

  it("carries the underlying run for the tooltip", () => {
    const runs: CollectRun[] = [
      run({ collector: "issues", ok: false, message: "gh auth expired" }),
    ];
    const issues = collectorHealth(runs, NOW).find((h) => h.key === "issues");
    expect(issues?.run?.message).toBe("gh auth expired");
    expect(DEFAULT_STALE_MS.issues).toBeGreaterThan(0);
  });
});
