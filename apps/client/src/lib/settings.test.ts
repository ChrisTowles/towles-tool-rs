import { describe, expect, it } from "vitest";
import { nextCalendarSourceId, type CalendarSource } from "./settings";

const source = (id: string): CalendarSource => ({ id, label: id, enabled: false, prompt: "" });

describe("nextCalendarSourceId", () => {
  it("slugs the generated label", () => {
    expect(nextCalendarSourceId([], "Calendar 1")).toBe("calendar-1");
    expect(nextCalendarSourceId([], "Work Outlook")).toBe("work-outlook");
  });

  it("suffixes until the id is free, so a new lane never collides", () => {
    const existing = [source("google"), source("google-2")];
    expect(nextCalendarSourceId(existing, "Google")).toBe("google-3");
  });

  it("collides in practice when a source is removed and re-added", () => {
    // Two sources exist, one is removed, and the next add regenerates the same
    // label — the suffix is the only thing keeping the lanes apart.
    const existing = [source("calendar-1")];
    expect(nextCalendarSourceId(existing, "Calendar 1")).toBe("calendar-1-2");
  });
});
