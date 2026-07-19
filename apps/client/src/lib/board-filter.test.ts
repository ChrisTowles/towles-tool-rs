import { describe, expect, it } from "vitest";
import { matchesTaskFilter } from "./board-filter";

/** A minimal filterable task with empty link lists. */
function task(
  fields: { text: string; notes?: string },
  extra?: Partial<Parameters<typeof matchesTaskFilter>[0]>,
) {
  return { issues: [], prs: [], ...fields, ...extra };
}

describe("matchesTaskFilter", () => {
  it("matches everything when the query is empty or whitespace", () => {
    expect(matchesTaskFilter(task({ text: "Ship the board" }), "")).toBe(true);
    expect(matchesTaskFilter(task({ text: "Ship the board" }), "   ")).toBe(true);
  });

  it("matches a case-insensitive substring of the task text", () => {
    expect(matchesTaskFilter(task({ text: "Ship the Board" }), "BOARD")).toBe(true);
    expect(matchesTaskFilter(task({ text: "Ship the Board" }), "ship")).toBe(true);
  });

  it("does not match when the substring is absent", () => {
    expect(matchesTaskFilter(task({ text: "Ship the board" }), "slack")).toBe(false);
  });

  it("matches against the notes, not just the text", () => {
    expect(
      matchesTaskFilter(task({ text: "Fix the flaky test", notes: "start with doctor" }), "doctor"),
    ).toBe(true);
    expect(matchesTaskFilter(task({ text: "Fix the flaky test" }), "doctor")).toBe(false);
  });

  it("matches against linked issue/PR repos and numbers", () => {
    const linked = task(
      { text: "Fix the bug" },
      {
        issues: [{ repo: "octo/towles-tool-rs", number: 339, url: "u", state: "open" }],
        prs: [{ repo: "octo/other", number: 7, url: "u", state: "open", checks: "none" }],
      },
    );
    expect(matchesTaskFilter(linked, "tool-rs")).toBe(true);
    expect(matchesTaskFilter(linked, "#339")).toBe(true);
    expect(matchesTaskFilter(linked, "octo/other")).toBe(true);
    expect(matchesTaskFilter(task({ text: "Fix the bug" }), "tool-rs")).toBe(false);
  });

  it("matches against the slot branch", () => {
    const slotted = task(
      { text: "Rate limits" },
      { slot: { repoRoot: "/r", branch: "fix/rate-limit-backoff" } },
    );
    expect(matchesTaskFilter(slotted, "rate-limit")).toBe(true);
  });

  it("matches against the slot repo — often a card's only repo identity", () => {
    const bound = task({ text: "Ship it" }, { slot: { repoRoot: "/r", repo: "octo/blog" } });
    expect(matchesTaskFilter(bound, "blog")).toBe(true);
    expect(matchesTaskFilter(task({ text: "Ship it" }), "blog")).toBe(false);
  });

  it("trims surrounding whitespace before comparing", () => {
    expect(matchesTaskFilter(task({ text: "Refresh cadence" }), "  refresh  ")).toBe(true);
  });
});
