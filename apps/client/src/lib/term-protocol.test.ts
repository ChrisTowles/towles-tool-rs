import { describe, expect, it } from "vitest";
import { stepMatch, viewportMatches } from "./term-protocol";

describe("viewportMatches", () => {
  const matches = [
    { row: 2, col: 0, width: 4 },
    { row: 10, col: 3, width: 2 },
    { row: 11, col: 0, width: 6 },
    { row: 40, col: 1, width: 4 },
  ];

  it("maps absolute rows to viewport rows and keeps original indices", () => {
    expect(viewportMatches(matches, 10, 5)).toEqual([
      { y: 0, col: 3, width: 2, index: 1 },
      { y: 1, col: 0, width: 6, index: 2 },
    ]);
  });

  it("excludes matches above and below the viewport", () => {
    expect(viewportMatches(matches, 3, 5)).toEqual([]);
    expect(viewportMatches([], 0, 24)).toEqual([]);
  });
});

describe("stepMatch", () => {
  it("wraps around in both directions", () => {
    expect(stepMatch(3, 0, 1)).toBe(1);
    expect(stepMatch(3, 2, 1)).toBe(0);
    expect(stepMatch(3, 0, -1)).toBe(2);
  });

  it("returns -1 when there are no matches", () => {
    expect(stepMatch(0, 0, 1)).toBe(-1);
    expect(stepMatch(0, -1, -1)).toBe(-1);
  });
});
