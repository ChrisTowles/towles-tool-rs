import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  clearMode,
  parsePrompt,
  readMode,
  sessionStartContext,
  setMode,
  userPromptContext,
} from "./yagni-mode";

let dir: string;
let flagPath: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "yagni-mode-"));
  flagPath = join(dir, ".tt-yagni-mode");
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

describe("parsePrompt", () => {
  it("activates full mode for bare /yagni and namespaced /tt:yagni", () => {
    expect(parsePrompt("/yagni")).toEqual({ mode: "full" });
    expect(parsePrompt("/tt:yagni")).toEqual({ mode: "full" });
  });

  it("picks the requested level", () => {
    expect(parsePrompt("/yagni ultra")).toEqual({ mode: "ultra" });
    expect(parsePrompt("/tt:yagni lite")).toEqual({ mode: "lite" });
  });

  it("falls back to full for unknown levels", () => {
    expect(parsePrompt("/yagni bogus")).toEqual({ mode: "full" });
  });

  it("turns off via /yagni off and deactivation phrases", () => {
    expect(parsePrompt("/yagni off")).toEqual({ off: true });
    expect(parsePrompt("stop yagni please")).toEqual({ off: true });
    expect(parsePrompt("back to normal mode")).toEqual({ off: true });
  });

  it("ignores /yagni-review and unrelated prompts", () => {
    expect(parsePrompt("/yagni-review")).toBeNull();
    expect(parsePrompt("/tt:yagni-review")).toBeNull();
    expect(parsePrompt("explain yagni to me")).toBeNull();
  });
});

describe("flag file", () => {
  it("round-trips a mode and clears it", () => {
    expect(readMode(flagPath)).toBeNull();
    setMode(flagPath, "ultra");
    expect(readMode(flagPath)).toBe("ultra");
    clearMode(flagPath);
    expect(readMode(flagPath)).toBeNull();
  });

  it("treats garbage flag contents as off", () => {
    writeFileSync(flagPath, "banana");
    expect(readMode(flagPath)).toBeNull();
  });
});

describe("hook outputs", () => {
  it("emits nothing on SessionStart when mode is off", () => {
    expect(sessionStartContext(flagPath, join(dir, "SKILL.md"))).toBe("");
  });

  it("re-injects the skill body on SessionStart when active", () => {
    setMode(flagPath, "full");
    const skillPath = join(dir, "SKILL.md");
    writeFileSync(skillPath, "---\nname: yagni\n---\n\n# YAGNI Mode\n\nThe ladder.\n");
    const out = sessionStartContext(flagPath, skillPath);
    expect(out).toContain("YAGNI MODE ACTIVE — level: full");
    expect(out).toContain("The ladder.");
    expect(out).not.toContain("name: yagni");
  });

  it("sets the flag and confirms on /yagni prompts", () => {
    expect(userPromptContext(flagPath, "/yagni ultra")).toBe("YAGNI MODE ACTIVE — level: ultra");
    expect(readMode(flagPath)).toBe("ultra");
  });

  it("clears the flag on off, silently when already off", () => {
    setMode(flagPath, "full");
    expect(userPromptContext(flagPath, "/yagni off")).toBe("YAGNI MODE OFF");
    expect(readMode(flagPath)).toBeNull();
    expect(userPromptContext(flagPath, "stop yagni")).toBe("");
  });

  it("stays quiet on unrelated prompts", () => {
    expect(userPromptContext(flagPath, "fix the bug in foo.ts")).toBe("");
    expect(readMode(flagPath)).toBeNull();
  });
});
