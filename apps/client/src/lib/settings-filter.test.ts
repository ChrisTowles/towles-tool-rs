import { describe, expect, it } from "vitest";
import { isEmptyQuery, matchesFilter, normalizeQuery } from "./settings-filter";

describe("normalizeQuery", () => {
  it("trims surrounding whitespace and lowercases", () => {
    expect(normalizeQuery("  Slack  ")).toBe("slack");
  });

  it("collapses an all-whitespace query to empty", () => {
    expect(normalizeQuery("   ")).toBe("");
  });
});

describe("isEmptyQuery", () => {
  it("is true for blank and whitespace-only queries", () => {
    expect(isEmptyQuery("")).toBe(true);
    expect(isEmptyQuery("   ")).toBe(true);
  });

  it("is false once real characters are typed", () => {
    expect(isEmptyQuery("s")).toBe(false);
  });
});

describe("matchesFilter", () => {
  it("matches everything when the query is empty", () => {
    expect(matchesFilter("", "Preferred editor")).toBe(true);
    expect(matchesFilter("   ", "Preferred editor")).toBe(true);
  });

  it("matches a case-insensitive substring of the label", () => {
    expect(matchesFilter("EDIT", "Preferred editor")).toBe(true);
    expect(matchesFilter("editor", "Preferred editor")).toBe(true);
  });

  it("does not match when the substring is absent", () => {
    expect(matchesFilter("slack", "Preferred editor")).toBe(false);
  });

  it("matches against per-row keywords, not just the label", () => {
    // A row labelled "Enabled" is only discoverable by its section keyword.
    expect(matchesFilter("slack", "Enabled", ["slack", "dm"])).toBe(true);
    expect(matchesFilter("calendar", "Enabled", ["slack", "dm"])).toBe(false);
  });

  it("trims the query before comparing", () => {
    expect(matchesFilter("  refresh  ", "Refresh every", ["cadence"])).toBe(
      true,
    );
  });

  it("ignores keyword casing", () => {
    expect(matchesFilter("dm", "Watch member ID", ["Slack", "DM"])).toBe(true);
  });
});
