import { describe, it, expect } from "vitest";
import { truncate, collapseWS } from "./truncate";

describe("truncate", () => {
  it("leaves short strings untouched", () => {
    expect(truncate("hello", 18)).toBe("hello");
    expect(truncate("exactly-eighteen!!", 18)).toBe("exactly-eighteen!!");
  });

  it("hard-cuts and appends an ellipsis when over the cap", () => {
    expect(truncate("this-name-is-way-too-long", 18)).toBe("this-name-is-way-…");
    expect(truncate("this-name-is-way-too-long", 18)).toHaveLength(18);
  });

  it("cuts to max-1 chars plus the ellipsis glyph", () => {
    expect(truncate("abcdef", 4)).toBe("abc…");
  });
});

describe("collapseWS", () => {
  it("collapses internal whitespace runs and trims", () => {
    expect(collapseWS("  a   b\n\tc  ")).toBe("a b c");
  });

  it("leaves a clean string unchanged", () => {
    expect(collapseWS("already clean")).toBe("already clean");
  });
});
