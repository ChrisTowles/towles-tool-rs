import { describe, it, expect } from "vitest";
import { BUILTIN_THEMES, DEFAULT_THEME, PALETTE_KEYS, resolveTheme } from "./themes";

describe("BUILTIN_THEMES", () => {
  it("includes the default and the spec's 18 named built-ins", () => {
    expect(DEFAULT_THEME).toBe("catppuccin-mocha");
    for (const name of [
      "catppuccin-mocha",
      "catppuccin-latte",
      "catppuccin-frappe",
      "catppuccin-macchiato",
      "tokyo-night",
      "gruvbox-dark",
      "nord",
      "dracula",
      "github-dark",
      "one-dark",
      "kanagawa",
      "everforest",
      "material",
      "flexoki",
      "ayu",
      "aura",
      "matrix",
      "transparent",
    ]) {
      expect(BUILTIN_THEMES[name]).toBeDefined();
    }
  });

  it("gives every theme all 21 palette keys", () => {
    expect(PALETTE_KEYS).toHaveLength(21);
    for (const theme of Object.values(BUILTIN_THEMES)) {
      for (const key of PALETTE_KEYS) {
        expect(typeof theme.palette[key]).toBe("string");
      }
    }
  });

  it("keeps catppuccin-mocha's signature accent", () => {
    expect(BUILTIN_THEMES["catppuccin-mocha"].palette.blue).toBe("#89b4fa");
  });

  it("renders transparent's bg tiers as transparent", () => {
    expect(BUILTIN_THEMES.transparent.palette.base).toBe("transparent");
    expect(BUILTIN_THEMES.transparent.palette.crust).toBe("transparent");
  });
});

describe("resolveTheme", () => {
  it("returns the default for undefined", () => {
    expect(resolveTheme(undefined)).toBe(BUILTIN_THEMES[DEFAULT_THEME]);
  });

  it("looks up a builtin by name", () => {
    expect(resolveTheme("nord")).toBe(BUILTIN_THEMES.nord);
  });

  it("falls back to the default for an unknown name", () => {
    expect(resolveTheme("does-not-exist")).toBe(BUILTIN_THEMES[DEFAULT_THEME]);
  });

  it("shallow-merges a partial over the default", () => {
    const merged = resolveTheme({ palette: { blue: "#000000" } });
    expect(merged.palette.blue).toBe("#000000");
    expect(merged.palette.red).toBe(BUILTIN_THEMES[DEFAULT_THEME].palette.red);
  });
});
