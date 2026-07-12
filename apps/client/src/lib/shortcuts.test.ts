import { describe, expect, it } from "vitest";
import { matchesShortcut, SHORTCUTS } from "./shortcuts";

// `matches` only reads these fields, so a plain object stands in for a real
// KeyboardEvent (the test environment is node, without the DOM).
function key(overrides: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    key: "",
    ...overrides,
  } as KeyboardEvent;
}

describe("tab shortcuts", () => {
  it("registers a jump binding for each digit 1–9", () => {
    for (let n = 1; n <= 9; n++) {
      expect(SHORTCUTS[`tab-${n}`]).toBeDefined();
    }
  });

  it("collapses the nine tab jumps to a single help-overlay entry", () => {
    expect(SHORTCUTS["tab-1"].hideInHelp).toBeFalsy();
    for (let n = 2; n <= 9; n++) {
      expect(SHORTCUTS[`tab-${n}`].hideInHelp).toBe(true);
    }
  });

  it("matches mod+digit (Ctrl on non-mac)", () => {
    expect(matchesShortcut("tab-3", key({ ctrlKey: true, key: "3" }))).toBe(true);
    expect(matchesShortcut("tab-3", key({ ctrlKey: true, key: "4" }))).toBe(false);
    expect(matchesShortcut("tab-3", key({ key: "3" }))).toBe(false);
  });

  it("close-tab is plain mod+w — distinct from the shift+w session-close chord", () => {
    expect(matchesShortcut("close-tab", key({ ctrlKey: true, key: "w" }))).toBe(true);
    // mod+shift+w belongs to ab-close-session, so it must NOT close the tab.
    expect(matchesShortcut("close-tab", key({ ctrlKey: true, shiftKey: true, key: "w" }))).toBe(
      false,
    );
  });
});
