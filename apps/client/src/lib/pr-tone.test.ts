import { describe, expect, it } from "vitest";
import { prTone } from "./pr-tone";

describe("prTone", () => {
  it("merged wins over everything", () => {
    expect(prTone({ state: "merged", checks: "failing" })).toBe("merged");
  });

  it("closed-unmerged reads as failed, whatever the checks said", () => {
    expect(prTone({ state: "closed", checks: "passing" })).toBe("failed");
  });

  it("open PRs follow the checks rollup", () => {
    expect(prTone({ state: "open", checks: "failing" })).toBe("failed");
    expect(prTone({ state: "open", checks: "passing" })).toBe("passing");
    expect(prTone({ state: "open", checks: "pending" })).toBe("running");
    expect(prTone({ state: "open", checks: "none" })).toBe("plain");
  });

  it("an unknown checks value degrades visibly as running, not neutral", () => {
    expect(prTone({ state: "open", checks: "queued" })).toBe("running");
  });
});
