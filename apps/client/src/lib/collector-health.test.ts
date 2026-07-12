import { describe, expect, it } from "vitest";
import {
  ALWAYS_ON_COLLECTORS,
  alwaysOnHealth,
  classifyRun,
  collectorHealth,
  dataRefreshedAt,
  DEFAULT_STALE_MS,
  KNOWN_COLLECTORS,
  worstCollectorState,
  type CollectorHealth,
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

describe("dataRefreshedAt", () => {
  it("is undefined when no refresh collector has run", () => {
    expect(dataRefreshedAt([], NOW)).toBeUndefined();
  });

  it("takes the newest successful run across prs and issues", () => {
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 10 * 60_000 }),
      run({ collector: "issues", ranAt: NOW - 3 * 60_000 }),
    ];
    expect(dataRefreshedAt(runs, NOW)).toBe(NOW - 3 * 60_000);
  });

  it("ignores calendar and slack collectors", () => {
    const runs: CollectRun[] = [
      run({ collector: "claude:calendar", ranAt: NOW - 1000 }),
      run({ collector: "slack:dm", ranAt: NOW - 1000 }),
      run({ collector: "prs", ranAt: NOW - 4 * 60_000 }),
    ];
    expect(dataRefreshedAt(runs, NOW)).toBe(NOW - 4 * 60_000);
  });

  it("skips a collector whose latest run errored, falling back to the other", () => {
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 1000, ok: false }),
      run({ collector: "issues", ranAt: NOW - 5 * 60_000 }),
    ];
    expect(dataRefreshedAt(runs, NOW)).toBe(NOW - 5 * 60_000);
  });

  it("is undefined when every latest run errored", () => {
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 1000, ok: false }),
      run({ collector: "issues", ranAt: NOW - 2000, ok: false }),
    ];
    expect(dataRefreshedAt(runs, NOW)).toBeUndefined();
  });
});

describe("worstCollectorState", () => {
  const h = (state: CollectorState): CollectorHealth => ({
    key: "prs",
    label: "Pull requests",
    state,
    run: undefined,
  });

  it("is fresh for an empty list (nothing to judge)", () => {
    expect(worstCollectorState([])).toBe("fresh");
  });

  it("stays fresh when every collector is fresh", () => {
    expect(worstCollectorState([h("fresh"), h("fresh")])).toBe("fresh");
  });

  it("prefers stale over fresh", () => {
    expect(worstCollectorState([h("fresh"), h("stale")])).toBe("stale");
  });

  it("prefers never-ran over stale", () => {
    expect(worstCollectorState([h("stale"), h("never-ran")])).toBe("never-ran");
  });

  it("failing beats everything", () => {
    expect(worstCollectorState([h("failing"), h("never-ran"), h("stale"), h("fresh")])).toBe(
      "failing",
    );
  });
});

describe("alwaysOnHealth", () => {
  it("covers only the always-on collectors, in declared order", () => {
    const health = alwaysOnHealth([], NOW);
    expect(health.map((x) => x.key)).toEqual([...ALWAYS_ON_COLLECTORS]);
    expect(ALWAYS_ON_COLLECTORS).not.toContain("claude:calendar");
  });

  it("ignores a disabled calendar collector when colouring the dot", () => {
    // Calendar never runs (off by default); prs/issues are fresh. The dot must
    // read fresh, not amber, despite calendar's perpetual never-ran.
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 1000 }),
      run({ collector: "issues", ranAt: NOW - 1000 }),
    ];
    expect(worstCollectorState(alwaysOnHealth(runs, NOW))).toBe("fresh");
  });

  it("surfaces a failing always-on collector as the dot state", () => {
    const runs: CollectRun[] = [
      run({ collector: "prs", ranAt: NOW - 1000, ok: false }),
      run({ collector: "issues", ranAt: NOW - 1000 }),
    ];
    expect(worstCollectorState(alwaysOnHealth(runs, NOW))).toBe("failing");
  });
});
