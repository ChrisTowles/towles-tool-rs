import { describe, expect, it } from "vitest";
import { pathScope, prForFolder } from "./agentboard";
import type { PrItem } from "./data";

function pr(overrides: Partial<PrItem>): PrItem {
  return {
    repo: "ChrisTowles/towles-tool-rs",
    number: 42,
    title: "a pr",
    branch: "feature/x",
    state: "open",
    checks: "passing",
    reviewState: "none",
    url: "https://github.com/ChrisTowles/towles-tool-rs/pull/42",
    updatedTs: 0,
    ...overrides,
  };
}

describe("pathScope", () => {
  it("extracts the ~/code/<scope>/ prefix", () => {
    expect(pathScope("/home/me/code/p/towles-tool")).toBe("p/");
    expect(pathScope("/home/me/code/w/acme-web")).toBe("w/");
    expect(pathScope("/home/me/code/f/plannotator")).toBe("f/");
  });

  it("returns null outside the ~/code layout", () => {
    expect(pathScope("/tmp/somewhere")).toBeNull();
    expect(pathScope("/home/me/code/deep/nested")).toBeNull();
  });
});

describe("prForFolder", () => {
  it("matches on branch when the origin URL contains the PR's owner/name", () => {
    const found = prForFolder(
      [pr({ branch: "feature/x", number: 7 })],
      "git@github.com:ChrisTowles/towles-tool-rs.git",
      "feature/x",
    );
    expect(found?.number).toBe(7);
  });

  it("matches https origins case-insensitively", () => {
    const found = prForFolder(
      [pr({ repo: "ChrisTowles/Towles-Tool-RS" })],
      "https://github.com/christowles/towles-tool-rs.git",
      "feature/x",
    );
    expect(found).toBeDefined();
  });

  it("rejects a same-named branch from a different repo", () => {
    const found = prForFolder(
      [pr({ repo: "someone-else/other-repo" })],
      "git@github.com:ChrisTowles/towles-tool-rs.git",
      "feature/x",
    );
    expect(found).toBeUndefined();
  });

  it("matches on branch alone when the folder has no origin", () => {
    expect(prForFolder([pr({})], null, "feature/x")).toBeDefined();
    expect(prForFolder([pr({})], undefined, "other-branch")).toBeUndefined();
  });

  it("returns undefined for an empty branch", () => {
    expect(prForFolder([pr({})], "x", "")).toBeUndefined();
  });
});
