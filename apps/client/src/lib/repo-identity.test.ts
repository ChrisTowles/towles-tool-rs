import { describe, expect, it } from "vitest";
import {
  DEFAULT_REPO_ICON,
  isHexColor,
  normalizeHex,
  repoAccentStyles,
  repoIcon,
  REPO_ICONS,
  REPO_PALETTE,
} from "./repo-identity";

describe("normalizeHex", () => {
  it("canonicalizes to lowercase #rrggbb", () => {
    expect(normalizeHex("#3B82F6")).toBe("#3b82f6");
    expect(normalizeHex("3b82f6")).toBe("#3b82f6");
    expect(normalizeHex("  #ABC  ")).toBe("#aabbcc");
    expect(normalizeHex("abc")).toBe("#aabbcc");
  });

  it("rejects anything that isn't a hex color", () => {
    for (const bad of ["", "#", "#12", "#12345", "#1234567", "red", "rgb(0,0,0)", "#zzzzzz"]) {
      expect([bad, normalizeHex(bad)]).toEqual([bad, null]);
      expect([bad, isHexColor(bad)]).toEqual([bad, false]);
    }
  });
});

describe("repoIcon", () => {
  it("falls back to the default for absent or unknown names", () => {
    expect(repoIcon(undefined)).toBe(DEFAULT_REPO_ICON);
    expect(repoIcon({})).toBe(DEFAULT_REPO_ICON);
    // The store is untrusted: a stale or hand-edited name must not crash.
    expect(repoIcon({ icon: "NotAnIcon" })).toBe(DEFAULT_REPO_ICON);
  });

  it("resolves an allowlisted name", () => {
    expect(repoIcon({ icon: "Rocket" })).toBe(REPO_ICONS.Rocket);
  });
});

describe("repoAccentStyles", () => {
  it("contributes nothing without a valid color — never invents one", () => {
    expect(repoAccentStyles(undefined)).toEqual({
      iconStyle: undefined,
      edgeStyle: undefined,
      surfaceStyle: undefined,
    });
    expect(repoAccentStyles({ icon: "Rocket", color: "nope" }).iconStyle).toBeUndefined();
  });

  it("tints the glyph and edge, but washes the surface only for style: tint", () => {
    const accent = repoAccentStyles({ color: "#3B82F6" });
    expect(accent.iconStyle).toEqual({ color: "#3b82f6" });
    expect(accent.edgeStyle?.borderLeftColor).toContain("#3b82f6");
    expect(accent.surfaceStyle).toBeUndefined();

    const tint = repoAccentStyles({ color: "#3b82f6", style: "tint" });
    expect(tint.surfaceStyle?.backgroundColor).toBe("color-mix(in srgb, #3b82f6 8%, transparent)");
  });

  it("mixes the wash into a caller-supplied opaque base (sticky surfaces)", () => {
    const tint = repoAccentStyles({ color: "#3b82f6", style: "tint" }, "var(--card)");
    expect(tint.surfaceStyle?.backgroundColor).toBe("color-mix(in srgb, #3b82f6 8%, var(--card))");
  });
});

describe("REPO_PALETTE", () => {
  it("is all canonical hex", () => {
    for (const swatch of REPO_PALETTE) expect(normalizeHex(swatch)).toBe(swatch);
  });
});
