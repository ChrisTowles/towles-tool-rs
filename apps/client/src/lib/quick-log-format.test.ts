import { describe, expect, it } from "vitest";
import { formatLogLine } from "./quick-log-format";

// Local wall-clock constructor so the test is timezone-independent (the fn uses local time).
const at = (h: number, m: number) => new Date(2026, 6, 12, h, m);

describe("formatLogLine", () => {
  it("prepends a zero-padded HH:MM time and a [context] bracket", () => {
    expect(formatLogLine("fixed the flaky test", { now: at(14, 32), context: "board" })).toBe(
      "- 14:32 [board] fixed the flaky test",
    );
  });

  it("zero-pads single-digit hours and minutes", () => {
    expect(formatLogLine("early note", { now: at(9, 5), context: "cockpit" })).toBe(
      "- 09:05 [cockpit] early note",
    );
  });

  it("renders midnight as 00:00", () => {
    expect(formatLogLine("just past midnight", { now: at(0, 0), context: "board" })).toBe(
      "- 00:00 [board] just past midnight",
    );
  });

  it("omits the bracket when context is empty", () => {
    expect(formatLogLine("no context here", { now: at(14, 32), context: "" })).toBe(
      "- 14:32 no context here",
    );
  });

  it("omits the bracket when context is undefined", () => {
    expect(formatLogLine("still no context", { now: at(8, 0) })).toBe("- 08:00 still no context");
  });

  it("omits the bracket when context is whitespace only", () => {
    expect(formatLogLine("blank ctx", { now: at(8, 0), context: "   " })).toBe(
      "- 08:00 blank ctx",
    );
  });

  it("trims the log body", () => {
    expect(formatLogLine("  padded body  ", { now: at(8, 0), context: "board" })).toBe(
      "- 08:00 [board] padded body",
    );
  });

  it("accepts an epoch-ms number for now", () => {
    const epoch = at(14, 32).getTime();
    expect(formatLogLine("from epoch", { now: epoch, context: "board" })).toBe(
      "- 14:32 [board] from epoch",
    );
  });
});
