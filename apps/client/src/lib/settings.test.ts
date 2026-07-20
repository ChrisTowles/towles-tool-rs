import { describe, expect, it } from "vitest";
import { nextCalendarSourceId, type CalendarSource } from "./settings";

const source = (id: string): CalendarSource => ({ id, label: id, enabled: false, prompt: "" });

describe("nextCalendarSourceId", () => {
  it("slugs the label", () => {
    expect(nextCalendarSourceId([], "Work Outlook")).toBe("work-outlook");
    expect(nextCalendarSourceId([], "  Team / Ops  ")).toBe("team-ops");
  });

  it("falls back to `calendar` when nothing is sluggable", () => {
    expect(nextCalendarSourceId([], "…")).toBe("calendar");
    expect(nextCalendarSourceId([], "")).toBe("calendar");
  });

  it("suffixes until the id is free, so a new lane never collides", () => {
    const existing = [source("google"), source("google-2")];
    expect(nextCalendarSourceId(existing, "Google")).toBe("google-3");
  });

  it("keeps ids short enough to stay readable in the store", () => {
    expect(nextCalendarSourceId([], "a".repeat(80))).toHaveLength(32);
  });
});
