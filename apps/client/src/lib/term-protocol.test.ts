import { describe, expect, it } from "vitest";
import { encodeKey, scrollbackKey, stepMatch, viewportMatches } from "./term-protocol";

type KeyEventLike = Parameters<typeof scrollbackKey>[0];

/** Minimal keydown for the pure key encoders (they read only these props). */
function key(k: string, mods: Partial<KeyEventLike> = {}): KeyEventLike {
  return { key: k, shiftKey: false, altKey: false, ctrlKey: false, metaKey: false, ...mods };
}

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

describe("scrollbackKey", () => {
  it("maps the bare-shift scrollback chords to actions", () => {
    expect(scrollbackKey(key("PageUp", { shiftKey: true }))).toBe("page-up");
    expect(scrollbackKey(key("PageDown", { shiftKey: true }))).toBe("page-down");
    expect(scrollbackKey(key("Home", { shiftKey: true }))).toBe("top");
    expect(scrollbackKey(key("End", { shiftKey: true }))).toBe("bottom");
  });

  it("ignores unshifted keys and other modifier combinations", () => {
    expect(scrollbackKey(key("PageUp"))).toBeNull();
    expect(scrollbackKey(key("PageUp", { shiftKey: true, ctrlKey: true }))).toBeNull();
    expect(scrollbackKey(key("Home", { shiftKey: true, altKey: true }))).toBeNull();
    expect(scrollbackKey(key("ArrowUp", { shiftKey: true }))).toBeNull();
  });
});

describe("encodeKey", () => {
  const modes = { appCursorKeys: false };

  it("returns null for the scrollback chords so the view handles them", () => {
    expect(encodeKey(key("PageUp", { shiftKey: true }), modes)).toBeNull();
    expect(encodeKey(key("PageDown", { shiftKey: true }), modes)).toBeNull();
    expect(encodeKey(key("Home", { shiftKey: true }), modes)).toBeNull();
    expect(encodeKey(key("End", { shiftKey: true }), modes)).toBeNull();
  });

  it("still encodes the unshifted keys as ordinary PTY input", () => {
    expect(encodeKey(key("PageUp"), modes)).toBe("\x1b[5~");
    expect(encodeKey(key("Home"), modes)).toBe("\x1b[H");
    expect(encodeKey(key("Home"), { appCursorKeys: true })).toBe("\x1bOH");
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
