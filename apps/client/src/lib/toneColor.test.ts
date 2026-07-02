import { describe, it, expect } from "vitest";
import { toneColor } from "./toneColor";
import { BUILTIN_THEMES } from "./themes";

const palette = BUILTIN_THEMES["catppuccin-mocha"].palette;

describe("toneColor", () => {
  it("maps each tone to its palette color", () => {
    expect(toneColor("success", palette)).toBe(palette.green);
    expect(toneColor("error", palette)).toBe(palette.red);
    expect(toneColor("warn", palette)).toBe(palette.yellow);
    expect(toneColor("info", palette)).toBe(palette.blue);
  });

  it("falls back to overlay0 for neutral and undefined", () => {
    expect(toneColor("neutral", palette)).toBe(palette.overlay0);
    expect(toneColor(undefined, palette)).toBe(palette.overlay0);
  });
});
