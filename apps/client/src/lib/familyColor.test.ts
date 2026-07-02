import { describe, it, expect } from "vitest";
import { familyOf, familyColor } from "./familyColor";
import { BUILTIN_THEMES } from "./themes";

const palette = BUILTIN_THEMES["catppuccin-mocha"].palette;
const FALLBACK = [palette.mauve, palette.blue, palette.green, palette.yellow, palette.red];

describe("familyOf", () => {
  it("groups -primary and -slot-N clones", () => {
    expect(familyOf("blog-primary")).toBe("blog");
    expect(familyOf("blog-slot-1")).toBe("blog");
    expect(familyOf("towles-tool-slot-9")).toBe("towles-tool");
  });

  it("returns the full name for solo sessions", () => {
    expect(familyOf("dotfiles")).toBe("dotfiles");
    expect(familyOf("foo")).toBe("foo");
  });

  it("only treats -primary/-slot-N as slot suffixes", () => {
    expect(familyOf("my-project-other")).toBe("my-project-other");
    expect(familyOf("my-project-primary")).toBe("my-project");
  });
});

describe("familyColor", () => {
  it("is deterministic for the same family", () => {
    expect(familyColor("some-repo", palette)).toBe(familyColor("some-repo", palette));
  });

  it("gives slot clones of one repo the same hue", () => {
    expect(familyColor("blog-primary", palette)).toBe(familyColor("blog-slot-2", palette));
  });

  it("always resolves to one of the five fallback hues", () => {
    for (const name of ["blog", "dotfiles", "toolbox", "towles-tool", "zzz", "a", "unknown"]) {
      expect(FALLBACK).toContain(familyColor(name, palette));
    }
  });
});
