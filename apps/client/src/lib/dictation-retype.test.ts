import { describe, expect, it } from "vitest";
import { committedDelta, isNoopStep, RetypeState } from "./dictation-retype";

describe("RetypeState", () => {
  it("first emission inserts full text", () => {
    const s = new RetypeState();
    const step = s.diff("hello");
    expect(step).toEqual({ backspaces: 0, insert: "hello" });
    expect(s.text).toBe("hello");
  });

  it("append-only avoids backspaces", () => {
    const s = new RetypeState();
    s.diff("hello");
    const step = s.diff("hello world");
    expect(step).toEqual({ backspaces: 0, insert: " world" });
  });

  it("correction backspaces the diverging tail", () => {
    const s = new RetypeState();
    s.diff("the quick brown fix");
    const step = s.diff("the quick brown fox");
    expect(step).toEqual({ backspaces: 2, insert: "ox" });
  });

  it("equal text is a noop", () => {
    const s = new RetypeState();
    s.diff("hello");
    expect(isNoopStep(s.diff("hello"))).toBe(true);
  });

  it("empty text is a noop and does not clear state", () => {
    const s = new RetypeState();
    s.diff("hello");
    expect(isNoopStep(s.diff(""))).toBe(true);
    expect(s.text).toBe("hello");
  });

  it("trailing spaces are ignored", () => {
    const s = new RetypeState();
    s.diff("hello");
    expect(isNoopStep(s.diff("hello   "))).toBe(true);
  });

  it("uses code-point counts for unicode", () => {
    const s = new RetypeState();
    s.diff("hello 🌊");
    const step = s.diff("hello 🌅");
    expect(step).toEqual({ backspaces: 1, insert: "🌅" });
  });

  it("full replacement backspaces everything", () => {
    const s = new RetypeState();
    s.diff("apple");
    const step = s.diff("banana");
    expect(step).toEqual({ backspaces: 5, insert: "banana" });
  });

  it("reset clears state", () => {
    const s = new RetypeState();
    s.diff("hello");
    s.reset();
    const step = s.diff("world");
    expect(step).toEqual({ backspaces: 0, insert: "world" });
  });

  it("simulated apply matches typed text across a session", () => {
    const s = new RetypeState();
    let buf = "";
    for (const t of ["the", "the quick", "the quick brown fix", "the quick brown fox"]) {
      const step = s.diff(t);
      buf = buf.slice(0, buf.length - step.backspaces) + step.insert;
    }
    expect(buf).toBe("the quick brown fox");
    expect(buf).toBe(s.text);
  });
});

describe("committedDelta", () => {
  it("returns the full text on first commit", () => {
    const result = committedDelta("", ["hello"]);
    expect(result).toEqual({ delta: "hello", sent: "hello" });
  });

  it("returns only the growth since prevSent", () => {
    const result = committedDelta("hello", ["hello", "world"]);
    expect(result).toEqual({ delta: " world", sent: "hello world" });
  });

  it("returns null when nothing changed", () => {
    expect(committedDelta("hello world", ["hello", "world"])).toBeNull();
  });

  it("never backspaces on divergence — emits nothing, rebases the baseline", () => {
    const result = committedDelta("hello world", ["hello"]);
    expect(result).toEqual({ delta: "", sent: "hello" });
  });
});
