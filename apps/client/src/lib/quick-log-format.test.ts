import { describe, expect, it } from "vitest";
import { formatLogLine, parseQuickLog } from "./quick-log-format";

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
    expect(formatLogLine("blank ctx", { now: at(8, 0), context: "   " })).toBe("- 08:00 blank ctx");
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

describe("parseQuickLog", () => {
  it("routes a /todo prefix to the Board and strips it", () => {
    expect(parseQuickLog("/todo ship the release")).toEqual({
      kind: "todo",
      body: "ship the release",
    });
  });

  it("routes the short /t prefix to the Board", () => {
    expect(parseQuickLog("/t call the vendor")).toEqual({
      kind: "todo",
      body: "call the vendor",
    });
  });

  it("is case-insensitive on the prefix", () => {
    expect(parseQuickLog("/TODO uppercase")).toEqual({ kind: "todo", body: "uppercase" });
    expect(parseQuickLog("/T short upper")).toEqual({ kind: "todo", body: "short upper" });
  });

  it("trims whitespace around the todo body", () => {
    expect(parseQuickLog("  /todo   padded body   ")).toEqual({
      kind: "todo",
      body: "padded body",
    });
  });

  it("treats a prefix with no body as an empty todo", () => {
    expect(parseQuickLog("/todo")).toEqual({ kind: "todo", body: "" });
    expect(parseQuickLog("/t")).toEqual({ kind: "todo", body: "" });
    expect(parseQuickLog("/todo    ")).toEqual({ kind: "todo", body: "" });
  });

  it("does not treat prefixes without a following space as routing", () => {
    expect(parseQuickLog("/todos are great")).toEqual({
      kind: "journal",
      body: "/todos are great",
    });
    expect(parseQuickLog("/team standup")).toEqual({
      kind: "journal",
      body: "/team standup",
    });
  });

  it("keeps plain text as a trimmed journal entry", () => {
    expect(parseQuickLog("  fixed the flaky test  ")).toEqual({
      kind: "journal",
      body: "fixed the flaky test",
    });
  });

  it("keeps a mid-line /todo as a journal entry", () => {
    expect(parseQuickLog("remember to /todo this")).toEqual({
      kind: "journal",
      body: "remember to /todo this",
    });
  });

  it("classifies empty input as an empty journal entry", () => {
    expect(parseQuickLog("   ")).toEqual({ kind: "journal", body: "" });
  });
});
