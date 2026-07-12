import { describe, expect, it } from "vitest";
import { rowsHaveSelection, selectionKindForDetail, shouldCopyOnSelect } from "./terminal-selection";

describe("selectionKindForDetail", () => {
  it("maps a double-click to a word selection", () => {
    expect(selectionKindForDetail(2)).toBe("word");
  });

  it("maps a triple (or higher) click to a line selection", () => {
    expect(selectionKindForDetail(3)).toBe("line");
    expect(selectionKindForDetail(5)).toBe("line");
  });

  it("treats a single click (or zero-detail synthetic event) as a drag", () => {
    expect(selectionKindForDetail(1)).toBe("drag");
    expect(selectionKindForDetail(0)).toBe("drag");
  });
});

describe("shouldCopyOnSelect", () => {
  it("copies a produced selection only when the preference is enabled", () => {
    expect(shouldCopyOnSelect(true, "drag")).toBe(true);
    expect(shouldCopyOnSelect(true, "word")).toBe(true);
    expect(shouldCopyOnSelect(true, "line")).toBe(true);
    expect(shouldCopyOnSelect(false, "drag")).toBe(false);
  });

  it("never copies on a clear, even when enabled", () => {
    expect(shouldCopyOnSelect(true, "clear")).toBe(false);
    expect(shouldCopyOnSelect(false, "clear")).toBe(false);
  });
});

describe("rowsHaveSelection", () => {
  it("is true when any row carries a selection range", () => {
    expect(rowsHaveSelection([{ runs: undefined }, { sel: [1, 4] }] as never)).toBe(true);
  });

  it("is false when no row is selected", () => {
    expect(rowsHaveSelection([{}, {}])).toBe(false);
    expect(rowsHaveSelection([])).toBe(false);
  });
});
