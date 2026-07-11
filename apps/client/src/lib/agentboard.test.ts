import { describe, expect, it } from "vitest";
import {
  diffPaneDir,
  diffPaneId,
  dropPane,
  isDiffPane,
  normalizeWins,
  paneRects,
  pathScope,
  placePane,
  prForFolder,
  sessionNeeds,
  type SessionData,
  type WindowsPayload,
} from "./agentboard";
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

function session(overrides: Partial<SessionData>): SessionData {
  return {
    id: "s1",
    name: "shell 1",
    createdAt: 0,
    live: false,
    unseen: false,
    agents: [],
    ...overrides,
  };
}

const agent = (status: SessionData["agents"][number]["status"]) => ({
  agent: "claude-code",
  session: "",
  status,
  ts: 1,
});

describe("sessionNeeds", () => {
  // Mirrors `session_needs` in crates/tt-agentboard/src/bridge.rs — if these
  // rules change, change both.
  it("counts a live waiting/errored agent", () => {
    expect(sessionNeeds(session({ live: true, agentState: agent("waiting") }))).toBe(true);
    expect(sessionNeeds(session({ live: true, agentState: agent("error") }))).toBe(true);
  });

  it("ignores a stale waiting status on a session with no shell", () => {
    expect(sessionNeeds(session({ live: false, agentState: agent("waiting") }))).toBe(false);
  });

  it("counts an unseen finished turn (it's your move), but not a seen one", () => {
    expect(
      sessionNeeds(session({ live: true, unseen: true, agentState: agent("complete") })),
    ).toBe(true);
    expect(
      sessionNeeds(session({ live: true, unseen: true, agentState: agent("interrupted") })),
    ).toBe(true);
    expect(
      sessionNeeds(session({ live: true, unseen: false, agentState: agent("complete") })),
    ).toBe(false);
  });

  it("stays calm for busy/idle agents", () => {
    expect(sessionNeeds(session({ live: true, agentState: agent("busy") }))).toBe(false);
    expect(sessionNeeds(session({ live: true, agentState: agent("idle") }))).toBe(false);
    expect(sessionNeeds(session({ live: true }))).toBe(false);
  });
});

describe("diff pane ids", () => {
  it("round-trips the folder dir and never collides with session ids", () => {
    const id = diffPaneId("/home/me/code/p/proj");
    expect(isDiffPane(id)).toBe(true);
    expect(diffPaneDir(id)).toBe("/home/me/code/p/proj");
    // Backend session ids are `s<16 hex>` (sessions.rs `gen_id`).
    expect(isDiffPane("s00deadbeef00cafe")).toBe(false);
    expect(diffPaneDir("s00deadbeef00cafe")).toBeNull();
  });
});

const win = (id: string, folderDir: string, panes: string[]) => ({
  id,
  name: id,
  folderDir,
  panes,
});

describe("placePane", () => {
  const empty: WindowsPayload = { windows: [], activeWindows: {} };

  it("creates a primary window for a folder with none", () => {
    const next = placePane(empty, "/f", "s1", () => "w1");
    expect(next.windows).toEqual([{ id: "w1", name: "primary", folderDir: "/f", panes: ["s1"] }]);
    expect(next.activeWindows["/f"]).toBe("w1");
  });

  it("appends to the folder's active window", () => {
    const w: WindowsPayload = { windows: [win("w1", "/f", ["s1"])], activeWindows: { "/f": "w1" } };
    const next = placePane(w, "/f", "s2", () => "unused");
    expect(next.windows[0].panes).toEqual(["s1", "s2"]);
  });

  it("focuses (not duplicates) a pane already hosted in another window", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/f", ["s2"])],
      activeWindows: { "/f": "w1" },
    };
    const next = placePane(w, "/f", "s2", () => "unused");
    expect(next.windows).toEqual(w.windows);
    expect(next.activeWindows["/f"]).toBe("w2");
  });

  it("never places into another folder's window", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/other", ["s9"])],
      activeWindows: { "/other": "w1" },
    };
    const next = placePane(w, "/f", "s1", () => "w2");
    expect(next.windows.find((x) => x.id === "w2")?.folderDir).toBe("/f");
    expect(next.windows.find((x) => x.id === "w1")?.panes).toEqual(["s9"]);
  });
});

describe("dropPane", () => {
  it("removes the pane and leaves other windows alone", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1", "s2"]), win("w2", "/g", ["s3"])],
      activeWindows: { "/f": "w1" },
    };
    const next = dropPane(w, "s2");
    expect(next.windows[0].panes).toEqual(["s1"]);
    expect(next.windows[1].panes).toEqual(["s3"]);
  });
});

describe("normalizeWins", () => {
  it("drops active-window entries whose window is gone or moved folders", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", [])],
      activeWindows: { "/f": "w1", "/g": "w1", "/h": "gone" },
    };
    expect(normalizeWins(w).activeWindows).toEqual({ "/f": "w1" });
  });
});

describe("paneRects", () => {
  it("tiles up to three side-by-side at full height", () => {
    expect(paneRects(1)).toEqual([{ left: 0, top: 0, width: 100, height: 100 }]);
    expect(paneRects(3).map((r) => r.width)).toEqual([100 / 3, 100 / 3, 100 / 3]);
    expect(paneRects(3).every((r) => r.height === 100)).toBe(true);
  });

  it("switches to a 2-col grid from four panes, last odd pane spanning", () => {
    const four = paneRects(4);
    expect(four.map((r) => [r.left, r.top])).toEqual([
      [0, 0],
      [50, 0],
      [0, 50],
      [50, 50],
    ]);
    const five = paneRects(5);
    // 5 panes → 3 rows; the lone last pane spans the full width.
    expect(five[4]).toEqual({ left: 0, top: (200 / 3), width: 100, height: 100 / 3 });
  });

  it("returns nothing for an empty window", () => {
    expect(paneRects(0)).toEqual([]);
  });
});
