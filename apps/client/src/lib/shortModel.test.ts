import { describe, it, expect } from "vitest";
import { shortModel } from "./shortModel";

describe("shortModel", () => {
  it("returns empty for empty input", () => {
    expect(shortModel("")).toBe("");
  });

  it("strips the leading claude- prefix", () => {
    expect(shortModel("claude-opus-4-6")).toBe("opus-4-6");
    expect(shortModel("claude-sonnet-5")).toBe("sonnet-5");
  });

  it("strips a trailing [1m] suffix (case-insensitive)", () => {
    expect(shortModel("claude-opus-4-6[1m]")).toBe("opus-4-6");
    expect(shortModel("claude-opus-4-6[1M]")).toBe("opus-4-6");
  });

  it("leaves non-matching models untouched", () => {
    expect(shortModel("gpt-4o")).toBe("gpt-4o");
  });
});
