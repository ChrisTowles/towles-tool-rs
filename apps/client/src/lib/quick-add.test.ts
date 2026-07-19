import { describe, expect, it } from "vitest";
import { parseQuickAdd } from "./quick-add";

// A fixed local wall clock so @today/@tomorrow resolve deterministically:
// 2026-07-12 09:30 local.
const NOW = new Date(2026, 6, 12, 9, 30, 0, 0).getTime();

/** Epoch ms at the end of the local day `n` days from NOW, for expectations. */
function endOfDay(addDays: number): number {
  const d = new Date(NOW);
  d.setDate(d.getDate() + addDays);
  d.setHours(23, 59, 59, 999);
  return d.getTime();
}

describe("parseQuickAdd", () => {
  it("returns bare text unchanged when there are no tokens", () => {
    const r = parseQuickAdd("ship the release", NOW);
    expect(r).toEqual({ text: "ship the release" });
  });

  it("resolves @today to the end of the current local day", () => {
    const r = parseQuickAdd("write notes @today", NOW);
    expect(r.text).toBe("write notes");
    expect(r.dueTs).toBe(endOfDay(0));
  });

  it("resolves @tomorrow to the end of the next local day", () => {
    const r = parseQuickAdd("@tomorrow follow up", NOW);
    expect(r.text).toBe("follow up");
    expect(r.dueTs).toBe(endOfDay(1));
  });

  it("resolves an explicit @YYYY-MM-DD date", () => {
    const r = parseQuickAdd("release @2026-07-20", NOW);
    expect(r.text).toBe("release");
    expect(r.dueTs).toBe(new Date(2026, 6, 20, 23, 59, 59, 999).getTime());
  });

  it("leaves a #owner/repo tag in the text (the repo token died with #339)", () => {
    const r = parseQuickAdd("fix flaky test #octo/widgets", NOW);
    expect(r.text).toBe("fix flaky test #octo/widgets");
  });

  it("collapses the whitespace left where a mid-text token was removed", () => {
    const r = parseQuickAdd("triage @today the queue", NOW);
    expect(r.text).toBe("triage the queue");
  });

  it("leaves a bare @ or unmatched token in the text", () => {
    const r = parseQuickAdd("email bob@example.com", NOW);
    expect(r.text).toBe("email bob@example.com");
    expect(r.dueTs).toBeUndefined();
  });

  it("leaves an invalid calendar date in the text", () => {
    const r = parseQuickAdd("plan @2026-13-40", NOW);
    expect(r.text).toBe("plan @2026-13-40");
    expect(r.dueTs).toBeUndefined();
  });

  it("leaves a #tag in the text", () => {
    const r = parseQuickAdd("groom #backlog", NOW);
    expect(r.text).toBe("groom #backlog");
  });

  it("is case-insensitive on the @ keyword tokens", () => {
    const r = parseQuickAdd("nudge @Tomorrow", NOW);
    expect(r.dueTs).toBe(endOfDay(1));
    expect(r.text).toBe("nudge");
  });

  it("takes the first of repeated tokens and strips only that one", () => {
    const r = parseQuickAdd("@today @tomorrow ping", NOW);
    expect(r.dueTs).toBe(endOfDay(0));
    expect(r.text).toBe("@tomorrow ping");
  });
});
