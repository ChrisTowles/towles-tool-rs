import { describe, it, expect } from "vitest";
import { formatElapsed } from "./elapsed";

describe("formatElapsed", () => {
  it("formats sub-minute durations in seconds", () => {
    expect(formatElapsed(0)).toBe("0s");
    expect(formatElapsed(5_000)).toBe("5s");
    expect(formatElapsed(59_000)).toBe("59s");
  });

  it("formats sub-hour durations in whole minutes", () => {
    expect(formatElapsed(60_000)).toBe("1m");
    expect(formatElapsed(3 * 60_000)).toBe("3m");
    expect(formatElapsed(59 * 60_000)).toBe("59m");
  });

  it("formats hour+ durations in whole hours", () => {
    expect(formatElapsed(60 * 60_000)).toBe("1h");
    expect(formatElapsed(5 * 60 * 60_000)).toBe("5h");
  });

  it("clamps negative values to 0s", () => {
    expect(formatElapsed(-1000)).toBe("0s");
  });

  it("floors rather than rounds", () => {
    expect(formatElapsed(59_999)).toBe("59s");
    expect(formatElapsed(119_999)).toBe("1m");
  });
});
