import { describe, expect, it } from "vitest";
import { matchesEditableOverride, matchesShortcut, SHORTCUTS } from "./shortcuts";

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

  it("zen is a global mod+shift+f binding shown in the help overlay", () => {
    const zen = SHORTCUTS["zen"];
    expect(zen).toBeDefined();
    expect(zen.scope).toBe("global");
    expect(zen.hideInHelp).toBeFalsy();
    expect(matchesShortcut("zen", key({ ctrlKey: true, shiftKey: true, key: "f" }))).toBe(true);
    // Plain mod+f (no shift) must not toggle zen.
    expect(matchesShortcut("zen", key({ ctrlKey: true, key: "f" }))).toBe(false);
  });

  it("close-tab is plain mod+w — distinct from the shift+w session-close chord", () => {
    expect(matchesShortcut("close-tab", key({ ctrlKey: true, key: "w" }))).toBe(true);
    // mod+shift+w belongs to ab-close-session, so it must NOT close the tab.
    expect(matchesShortcut("close-tab", key({ ctrlKey: true, shiftKey: true, key: "w" }))).toBe(
      false,
    );
  });

  it("next-tab/prev-tab are mod+] and mod+[", () => {
    expect(matchesShortcut("next-tab", key({ ctrlKey: true, key: "]" }))).toBe(true);
    expect(matchesShortcut("prev-tab", key({ ctrlKey: true, key: "[" }))).toBe(true);
    expect(matchesShortcut("next-tab", key({ ctrlKey: true, key: "[" }))).toBe(false);
  });
});

describe("board shortcuts", () => {
  it("registers the board-scoped filter binding", () => {
    expect(SHORTCUTS["board-filter"].scope).toBe("board");
    expect(matchesShortcut("board-filter", key({ key: "/" }))).toBe(true);
  });

  it("has no new-task binding — tasks are created on the Agentboard", () => {
    expect("board-new-todo" in SHORTCUTS).toBe(false);
  });
});

describe("editable-target override", () => {
  it("jump-next/prev opt out of the editable guard so they work with a terminal focused", () => {
    expect(SHORTCUTS["ab-jump-next"].allowInEditable).toBe(true);
    expect(SHORTCUTS["ab-jump-prev"].allowInEditable).toBe(true);
    expect(matchesEditableOverride(key({ ctrlKey: true, shiftKey: true, key: "n" }))).toBe(true);
    expect(matchesEditableOverride(key({ ctrlKey: true, shiftKey: true, key: "p" }))).toBe(true);
  });

  it("slot lifecycle chords work with a terminal focused — they act on board state", () => {
    expect(SHORTCUTS["ab-new-slot"].allowInEditable).toBe(true);
    expect(SHORTCUTS["ab-remove-slot"].allowInEditable).toBe(true);
    expect(matchesEditableOverride(key({ ctrlKey: true, shiftKey: true, key: "d" }))).toBe(true);
    expect(matchesEditableOverride(key({ ctrlKey: true, shiftKey: true, key: "Backspace" }))).toBe(
      true,
    );
  });

  it("remove-slot is mod+shift+backspace — plain backspace stays with the shell", () => {
    expect(
      matchesShortcut("ab-remove-slot", key({ ctrlKey: true, shiftKey: true, key: "Backspace" })),
    ).toBe(true);
    expect(matchesShortcut("ab-remove-slot", key({ key: "Backspace" }))).toBe(false);
    expect(matchesShortcut("ab-remove-slot", key({ ctrlKey: true, key: "Backspace" }))).toBe(false);
  });

  it("new-session (mod+d) stays gated — Ctrl+D is EOF at a shell prompt", () => {
    expect(SHORTCUTS["ab-new-session"].allowInEditable).toBeFalsy();
    expect(matchesEditableOverride(key({ ctrlKey: true, key: "d" }))).toBe(false);
  });

  it("a plain, unmatched key never triggers the override", () => {
    expect(matchesEditableOverride(key({ key: "n" }))).toBe(false);
  });
});
