import { describe, expect, it } from "vitest";
import {
  exitIsCrash,
  exitLabel,
  graphemeClusters,
  isWideRun,
  keyEventWire,
  scrollbackKey,
  stepMatch,
  viewportMatches,
  type Run,
} from "./term-protocol";

type KeyEventLike = Parameters<typeof scrollbackKey>[0];

/** Minimal keydown for the pure key encoders (they read only these props). */
function key(k: string, mods: Partial<KeyEventLike> = {}): KeyEventLike {
  return { key: k, shiftKey: false, altKey: false, ctrlKey: false, metaKey: false, ...mods };
}

describe("exitLabel", () => {
  it("labels a clean logout without a code", () => {
    expect(exitLabel(0)).toBe("exited");
    expect(exitLabel(0, null)).toBe("exited");
  });

  it("shows the numeric code for a nonzero exit", () => {
    expect(exitLabel(2)).toBe("exited · code 2");
    expect(exitLabel(127)).toBe("exited · code 127");
  });

  it("prefers the signal name over the placeholder code", () => {
    expect(exitLabel(1, "Killed")).toBe("exited · Killed");
    expect(exitLabel(0, "Terminated")).toBe("exited · Terminated");
  });
});

describe("exitIsCrash", () => {
  it("stays quiet for a code-0, no-signal exit — a clean logout gets no toast", () => {
    expect(exitIsCrash(0)).toBe(false);
    expect(exitIsCrash(0, null)).toBe(false);
  });

  it("toasts a nonzero code or any signal", () => {
    expect(exitIsCrash(2)).toBe(true);
    expect(exitIsCrash(0, "Killed")).toBe(true);
    expect(exitIsCrash(1, "Segmentation fault")).toBe(true);
  });
});

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

type WireEventLike = Parameters<typeof keyEventWire>[0];

/** Minimal keydown for the wire mapper. */
function wireKey(k: string, code: string, mods: Partial<WireEventLike> = {}): WireEventLike {
  return {
    key: k,
    code,
    repeat: false,
    shiftKey: false,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    ...mods,
  };
}

describe("keyEventWire", () => {
  it("routes a plain key with its DOM identity intact", () => {
    const wire = keyEventWire(wireKey("a", "KeyA"));
    expect(wire).toMatchObject({ code: "KeyA", key: "a", action: "press", shift: false });
  });

  it("marks held-key repeats", () => {
    expect(keyEventWire(wireKey("a", "KeyA", { repeat: true }))?.action).toBe("repeat");
  });

  it("passes the release action through", () => {
    expect(keyEventWire(wireKey("a", "KeyA"), "release")?.action).toBe("release");
  });

  it("yields OS chords and the app's clipboard chords", () => {
    expect(keyEventWire(wireKey("v", "KeyV", { metaKey: true }))).toBeNull();
    expect(keyEventWire(wireKey("V", "KeyV", { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(keyEventWire(wireKey("C", "KeyC", { ctrlKey: true, shiftKey: true }))).toBeNull();
  });

  it("keeps plain ctrl combos for the shell", () => {
    const wire = keyEventWire(wireKey("c", "KeyC", { ctrlKey: true }));
    expect(wire).toMatchObject({ code: "KeyC", ctrl: true, shift: false });
  });

  it("reads lock-state modifiers when the event exposes them", () => {
    const wire = keyEventWire(
      wireKey("A", "KeyA", { getModifierState: (k: string) => k === "CapsLock" }),
    );
    expect(wire?.capsLock).toBe(true);
    expect(wire?.numLock).toBe(false);
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

describe("graphemeClusters", () => {
  it("keeps a base codepoint and its combining mark as one cluster", () => {
    // "e" + U+0301 (combining acute) is one cell, not two.
    expect(graphemeClusters("e\u{301}llo")).toEqual(["e\u{301}", "l", "l", "o"]);
  });

  it("keeps an emoji and its variation selector as one cluster", () => {
    expect(graphemeClusters("\u{2764}\u{FE0F}")).toEqual(["\u{2764}\u{FE0F}"]);
  });

  it("splits plain ASCII one cluster per character", () => {
    expect(graphemeClusters("hi")).toEqual(["h", "i"]);
  });
});

describe("isWideRun", () => {
  const run = (text: string, width: number): Run => ({ x: 0, width, text });

  it("counts grapheme clusters, not codepoints, against the column width", () => {
    // A combining-mark cell has more codepoints than columns but is not wide.
    expect(isWideRun(run("e\u{301}", 1))).toBe(false);
    // An emoji-selector cluster in a single narrow column is likewise not wide.
    expect(isWideRun(run("\u{2764}\u{FE0F}", 1))).toBe(false);
  });

  it("still flags a run whose column width exceeds its cluster count", () => {
    // Two CJK glyphs occupy four columns: genuinely wide.
    expect(isWideRun(run("\u{6F22}\u{5B57}", 4))).toBe(true);
  });
});
