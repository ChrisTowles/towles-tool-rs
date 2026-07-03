import { describe, it, expect } from "vitest";
import { liveStatusIcon, unseenTerminalColor } from "./statusVisuals";
import { SPINNERS } from "./constants";
import { BUILTIN_THEMES } from "./themes";

const palette = BUILTIN_THEMES["catppuccin-mocha"].palette;

describe("liveStatusIcon", () => {
  it("returns the spinner frame at the given index for running", () => {
    expect(liveStatusIcon("busy", 0)).toBe(SPINNERS[0]);
    expect(liveStatusIcon("busy", 3)).toBe(SPINNERS[3]);
  });

  it("wraps the spinner index modulo the frame count", () => {
    expect(liveStatusIcon("busy", SPINNERS.length)).toBe(SPINNERS[0]);
    expect(liveStatusIcon("busy", SPINNERS.length + 2)).toBe(SPINNERS[2]);
  });

  it("returns a fixed glyph for waiting", () => {
    expect(liveStatusIcon("waiting", 0)).toBe("?");
  });

  it("returns empty for statuses without a live glyph", () => {
    expect(liveStatusIcon("idle", 0)).toBe("");
    expect(liveStatusIcon("complete", 0)).toBe("");
    expect(liveStatusIcon("error", 0)).toBe("");
    expect(liveStatusIcon("interrupted", 0)).toBe("");
  });
});

describe("unseenTerminalColor", () => {
  it("maps error→red, interrupted→peach, everything else→teal", () => {
    expect(unseenTerminalColor("error", palette)).toBe(palette.red);
    expect(unseenTerminalColor("interrupted", palette)).toBe(palette.peach);
    expect(unseenTerminalColor("complete", palette)).toBe(palette.teal);
    expect(unseenTerminalColor("idle", palette)).toBe(palette.teal);
  });
});
