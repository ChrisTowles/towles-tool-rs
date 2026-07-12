import { describe, expect, it } from "vitest";
import { COLD_START_TAB, loadWorkspaceTabs } from "@/lib/workspace-persistence";

describe("loadWorkspaceTabs", () => {
  it("cold start (nothing stored) falls back to cockpit", () => {
    expect(loadWorkspaceTabs(null, null)).toEqual({
      visited: [COLD_START_TAB],
      activeTab: COLD_START_TAB,
    });
  });

  it("restores a valid stored active tab and visited list", () => {
    const result = loadWorkspaceTabs("board", JSON.stringify(["cockpit", "board"]));
    expect(result).toEqual({ visited: ["cockpit", "board"], activeTab: "board" });
  });

  it("falls back to cockpit for an unknown/removed active screen id", () => {
    const result = loadWorkspaceTabs("some-deleted-screen", JSON.stringify(["cockpit"]));
    expect(result.activeTab).toBe(COLD_START_TAB);
  });

  it("drops stale/unknown ids from the visited list", () => {
    const result = loadWorkspaceTabs("board", JSON.stringify(["board", "gone", "gh-prs"]));
    expect(result.visited).toEqual(["board", "gh-prs"]);
  });

  it("ensures the active tab is always present in visited", () => {
    // A closed cockpit tab shouldn't be resurrected, but the active tab must
    // still be mounted.
    const result = loadWorkspaceTabs("gh-prs", JSON.stringify(["board"]));
    expect(result.visited).toContain("gh-prs");
    expect(result.visited).not.toContain("cockpit");
  });

  it("does not resurrect a closed tab (only stored ids are restored)", () => {
    const result = loadWorkspaceTabs("board", JSON.stringify(["board", "gh-prs"]));
    expect(result.visited).not.toContain("cockpit");
  });

  it("degrades to cockpit on malformed JSON", () => {
    expect(loadWorkspaceTabs("cockpit", "{not json")).toEqual({
      visited: ["cockpit"],
      activeTab: "cockpit",
    });
  });

  it("degrades to cockpit seed when stored visited is not an array", () => {
    const result = loadWorkspaceTabs("board", JSON.stringify({ nope: true }));
    // Non-array → cockpit seed, then the valid active tab is appended.
    expect(result.visited).toEqual(["cockpit", "board"]);
    expect(result.activeTab).toBe("board");
  });

  it("de-duplicates repeated ids in stored order", () => {
    const result = loadWorkspaceTabs("cockpit", JSON.stringify(["cockpit", "board", "cockpit"]));
    expect(result.visited).toEqual(["cockpit", "board"]);
  });

  it("empty stored array falls back to cockpit but keeps a valid active tab", () => {
    const result = loadWorkspaceTabs("board", JSON.stringify([]));
    // Empty → cockpit seed, then active tab appended.
    expect(result.visited).toEqual(["cockpit", "board"]);
    expect(result.activeTab).toBe("board");
  });
});
