import { describe, expect, it } from "vitest";
import { matchesTaskFilter } from "./board-filter";

describe("matchesTaskFilter", () => {
  it("matches everything when the query is empty or whitespace", () => {
    expect(matchesTaskFilter({ text: "Ship the board" }, "")).toBe(true);
    expect(matchesTaskFilter({ text: "Ship the board" }, "   ")).toBe(true);
  });

  it("matches a case-insensitive substring of the todo text", () => {
    expect(matchesTaskFilter({ text: "Ship the Board" }, "BOARD")).toBe(true);
    expect(matchesTaskFilter({ text: "Ship the Board" }, "ship")).toBe(true);
  });

  it("does not match when the substring is absent", () => {
    expect(matchesTaskFilter({ text: "Ship the board" }, "slack")).toBe(false);
  });

  it("matches against the repo tag, not just the text", () => {
    expect(
      matchesTaskFilter({ text: "Fix the bug", repo: "towles-tool-rs" }, "tool-rs"),
    ).toBe(true);
    expect(matchesTaskFilter({ text: "Fix the bug" }, "tool-rs")).toBe(false);
  });

  it("trims surrounding whitespace before comparing", () => {
    expect(matchesTaskFilter({ text: "Refresh cadence" }, "  refresh  ")).toBe(true);
  });
});
