import { describe, expect, it } from "vitest";
import { monacoLanguageFor } from "@/lib/markdown-code";

describe("monacoLanguageFor", () => {
  it("returns null for a fence with no language", () => {
    expect(monacoLanguageFor(undefined)).toBeNull();
    expect(monacoLanguageFor("")).toBeNull();
    expect(monacoLanguageFor("some-other-class")).toBeNull();
  });

  // The aliases that matter: Monaco renders an unknown id as plaintext with
  // no error, so a missed mapping reads as "highlighting is broken".
  it("maps short aliases to real Monaco ids", () => {
    expect(monacoLanguageFor("language-ts")).toBe("typescript");
    expect(monacoLanguageFor("language-tsx")).toBe("typescript");
    expect(monacoLanguageFor("language-js")).toBe("javascript");
    expect(monacoLanguageFor("language-rs")).toBe("rust");
    expect(monacoLanguageFor("language-py")).toBe("python");
    expect(monacoLanguageFor("language-yml")).toBe("yaml");
  });

  // Verified against the running app: `sh`/`bash`/`shell` all render as
  // plaintext; the grammar is registered as `shellscript`.
  it("maps every shell spelling to shellscript", () => {
    for (const alias of ["sh", "bash", "zsh", "shell", "console"]) {
      expect(monacoLanguageFor(`language-${alias}`)).toBe("shellscript");
    }
  });

  it("passes through ids that are already correct", () => {
    for (const id of ["typescript", "rust", "json", "css", "html", "yaml", "diff", "log"]) {
      expect(monacoLanguageFor(`language-${id}`)).toBe(id);
    }
  });

  it("is case-insensitive", () => {
    expect(monacoLanguageFor("language-TS")).toBe("typescript");
  });

  it("finds the language class among others", () => {
    expect(monacoLanguageFor("hljs language-rust extra")).toBe("rust");
  });

  it("handles ids containing + or -", () => {
    expect(monacoLanguageFor("language-objective-c")).toBe("objective-c");
    expect(monacoLanguageFor("language-c++")).toBe("c++");
  });
});
