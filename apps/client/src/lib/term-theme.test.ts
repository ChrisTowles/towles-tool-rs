import { describe, expect, it } from "vitest";
import { ANSI_DARK, ANSI_LIGHT, cssColorToPacked } from "@/lib/term-theme";

describe("cssColorToPacked", () => {
  it("parses the rgb() form getComputedStyle emits", () => {
    expect(cssColorToPacked("rgb(30, 30, 46)")).toBe(0x1e1e2e);
    expect(cssColorToPacked("rgba(205, 214, 244, 0.9)")).toBe(0xcdd6f4);
    expect(cssColorToPacked("rgb(0 0 0)")).toBe(0x000000);
  });

  it("parses hex", () => {
    expect(cssColorToPacked("#1e1e2e")).toBe(0x1e1e2e);
    expect(cssColorToPacked("#CDD6F4")).toBe(0xcdd6f4);
  });

  it("rejects what it cannot parse rather than guessing", () => {
    expect(cssColorToPacked("color(srgb 0.1 0.2 0.3)")).toBeNull();
    expect(cssColorToPacked("transparent")).toBeNull();
    expect(cssColorToPacked("")).toBeNull();
  });

  it("clamps out-of-range channels", () => {
    expect(cssColorToPacked("rgb(300, 0, 0)")).toBe(0xff0000);
  });
});

describe("ANSI palettes", () => {
  it("carry exactly 16 entries each", () => {
    expect(ANSI_DARK).toHaveLength(16);
    expect(ANSI_LIGHT).toHaveLength(16);
  });

  it("brights differ from normals only where the scheme says so", () => {
    // Catppuccin reuses the accent for normal+bright; the greys differ.
    expect(ANSI_DARK[8]).not.toBe(ANSI_DARK[0]);
    expect(ANSI_LIGHT[15]).not.toBe(ANSI_LIGHT[7]);
  });
});
