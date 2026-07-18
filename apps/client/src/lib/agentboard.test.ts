import { describe, expect, it } from "vitest";
import {
  agentRollup,
  cacheWarnMs,
  changedFolderDirs,
  cycleNeedsYou,
  colCount,
  diffPaneDir,
  diffPaneId,
  dragCol,
  dropPane,
  exitPaneId,
  exitPaneSession,
  fmtWaitingAge,
  folderActionableItems,
  folderSafeToDelete,
  hydrateWins,
  isDiffPane,
  isExitPane,
  isFilesPane,
  filesPaneDir,
  filesPaneId,
  folderPaneDir,
  isCacheExpiring,
  isFolderQuiet,
  needingSessionsOldestFirst,
  normalizeWins,
  paneRects,
  paneSession,
  snapCol,
  pathScope,
  placePane,
  prForFolder,
  prMergedButFolderHasWork,
  pruneWins,
  replacePane,
  QUIET_GRACE_MS,
  sessionNeeds,
  waitForFirstFrame,
  type AgentStatus,
  type FolderData,
  type Panes,
  type RepoData,
  type SessionData,
  type WindowsPayload,
  type WireWindowsPayload,
} from "./agentboard";
import type { PrItem } from "./data";

describe("waitForFirstFrame", () => {
  it("resolves immediately outside the Tauri shell (browser dev mode)", async () => {
    // vitest runs in a plain node environment (no `window` at all); stub one
    // without "__TAURI_INTERNALS__" to match plain-browser dev mode, where
    // this must hit the early-return fallback rather than waiting out the
    // full timeout.
    (globalThis as { window?: object }).window = {};
    try {
      const start = Date.now();
      await waitForFirstFrame("term-1", 5000);
      expect(Date.now() - start).toBeLessThan(100);
    } finally {
      delete (globalThis as { window?: object }).window;
    }
  });
});

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

describe("folderSafeToDelete", () => {
  it("is true for a clean folder with nothing unlanded", () => {
    expect(folderSafeToDelete(folder({}))).toBe(true);
  });

  it("is false for a dirty working tree", () => {
    expect(folderSafeToDelete(folder({ dirty: true }))).toBe(false);
  });

  it("is false with commits that haven't landed on comparedBase yet", () => {
    expect(folderSafeToDelete(folder({ commitsUnlanded: 1 }))).toBe(false);
  });

  it("stays true despite a nonzero commitsAhead — that's just SHA reachability, not unlanded work", () => {
    // The rebase/squash-merge case: commitsAhead never reaches 0, but
    // commitsUnlanded (patch-equivalence) does once the content has landed.
    expect(folderSafeToDelete(folder({ commitsAhead: 2, commitsUnlanded: 0 }))).toBe(true);
  });
});

describe("prMergedButFolderHasWork", () => {
  it("is false for a merged PR when the folder is clean and everything landed", () => {
    expect(prMergedButFolderHasWork(pr({ state: "merged" }), folder({}))).toBe(false);
  });

  it("is true for a merged PR with a dirty working tree", () => {
    expect(
      prMergedButFolderHasWork(pr({ state: "merged" }), folder({ dirty: true })),
    ).toBe(true);
  });

  it("is true for a merged PR with commits that haven't landed on comparedBase yet", () => {
    expect(
      prMergedButFolderHasWork(pr({ state: "merged" }), folder({ commitsUnlanded: 1 })),
    ).toBe(true);
  });

  it("is false for a merged, rebase/squash-merged branch — commitsAhead alone isn't unlanded work", () => {
    expect(
      prMergedButFolderHasWork(
        pr({ state: "merged" }),
        folder({ commitsAhead: 2, commitsUnlanded: 0 }),
      ),
    ).toBe(false);
  });

  it("is false for an open PR even with local work — that's normal, not a data-loss warning", () => {
    expect(
      prMergedButFolderHasWork(pr({ state: "open" }), folder({ dirty: true, commitsUnlanded: 1 })),
    ).toBe(false);
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

const agent = (status: AgentStatus) => ({
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

describe("folder pane ids", () => {
  it("round-trips the folder dir and never collides with session ids", () => {
    const id = diffPaneId("/home/me/code/p/proj");
    expect(isDiffPane(id)).toBe(true);
    expect(diffPaneDir(id)).toBe("/home/me/code/p/proj");
    // Backend session ids are `s<16 hex>` (sessions.rs `gen_id`).
    expect(isDiffPane("s00deadbeef00cafe")).toBe(false);
    expect(diffPaneDir("s00deadbeef00cafe")).toBeNull();
  });

  it("keeps diff and files panes distinct while folderPaneDir spans both", () => {
    const diff = diffPaneId("/home/me/code/p/proj");
    const files = filesPaneId("/home/me/code/p/proj");
    expect(diff).not.toBe(files);
    expect(isFilesPane(files)).toBe(true);
    expect(isFilesPane(diff)).toBe(false);
    expect(isDiffPane(files)).toBe(false);
    expect(filesPaneDir(files)).toBe("/home/me/code/p/proj");
    expect(folderPaneDir(diff)).toBe("/home/me/code/p/proj");
    expect(folderPaneDir(files)).toBe("/home/me/code/p/proj");
    expect(folderPaneDir("s00deadbeef00cafe")).toBeNull();
  });
});

const win = (id: string, folderDir: string, panes: Panes) => ({
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

  it("reuses the folder's existing window when the active entry is stale", () => {
    const w: WindowsPayload = { windows: [win("w1", "/f", ["s1"])], activeWindows: {} };
    const next = placePane(w, "/f", "s2", () => "never-minted");
    expect(next.windows).toEqual([win("w1", "/f", ["s1", "s2"])]);
    expect(next.activeWindows["/f"]).toBe("w1");
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

  it("deletes an emptied window and refocuses a sibling", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/f", ["s2"])],
      activeWindows: { "/f": "w1" },
    };
    const next = dropPane(w, "s1");
    expect(next.windows).toEqual([win("w2", "/f", ["s2"])]);
    expect(next.activeWindows["/f"]).toBe("w2");
  });

  it("deletes the folder's last window with its last pane (no empty windows)", () => {
    const w: WindowsPayload = { windows: [win("w1", "/f", ["s1"])], activeWindows: { "/f": "w1" } };
    const next = dropPane(w, "s1");
    expect(next.windows).toEqual([]);
    expect(next.activeWindows).toEqual({});
  });

  it("only refocuses onto same-folder siblings", () => {
    // /f's only window dies; /g's window must not inherit /f's focus.
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/g", ["s2"])],
      activeWindows: { "/f": "w1", "/g": "w2" },
    };
    const next = dropPane(w, "s1");
    expect(next.windows).toEqual([win("w2", "/g", ["s2"])]);
    expect(next.activeWindows).toEqual({ "/g": "w2" });
  });

  it("is a no-op for a pane no window holds", () => {
    const w: WindowsPayload = { windows: [win("w1", "/f", ["s1"])], activeWindows: {} };
    expect(dropPane(w, "ghost")).toBe(w);
  });
});

describe("pruneWins", () => {
  const valid = (sessions: string[], folders: string[]) =>
    [new Set(sessions), new Set(folders)] as const;

  it("drops ghost session panes so survivors tile from the first slot", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["ghost", "s1"])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f).windows[0].panes).toEqual(["s1"]);
  });

  it("drops a window emptied by pruning when the folder has a surviving one", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["ghost"]), win("w2", "/f", ["s1"])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    const next = pruneWins(w, s, f);
    expect(next.windows).toEqual([win("w2", "/f", ["s1"])]);
    expect(next.activeWindows["/f"]).toBe("w2");
  });

  it("drops the folder's layout entirely when pruning empties every window", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["g1"]), win("w2", "/f", ["g2"])],
      activeWindows: { "/f": "w2" },
    };
    const [s, f] = valid([], ["/f"]);
    const next = pruneWins(w, s, f);
    expect(next.windows).toEqual([]);
    expect(next.activeWindows).toEqual({});
  });

  it("drops windows of folders no longer on the rail", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/gone", ["s1"]), win("w2", "/f", ["s2"])],
      activeWindows: { "/gone": "w1", "/f": "w2" },
    };
    const [s, f] = valid(["s1", "s2"], ["/f"]);
    const next = pruneWins(w, s, f);
    expect(next.windows).toEqual([win("w2", "/f", ["s2"])]);
    expect(next.activeWindows).toEqual({ "/f": "w2" });
  });

  it("keeps a valid folder's diff/files panes, drops a removed folder's", () => {
    const w: WindowsPayload = {
      windows: [
        win("w1", "/f", [
          diffPaneId("/f"),
          filesPaneId("/f"),
          diffPaneId("/gone"),
          filesPaneId("/gone"),
          "s1",
        ]),
      ],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f).windows[0].panes).toEqual([
      diffPaneId("/f"),
      filesPaneId("/f"),
      "s1",
    ]);
  });

  it("returns the same object when nothing changed", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f)).toBe(w);
  });

  it("keeps a tombstone while its session record lives", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", [exitPaneId("s1")])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f)).toBe(w);
  });

  it("prunes a tombstone once its session is gone — nothing left to name", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", [exitPaneId("s1"), "s2"])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s2"], ["/f"]);
    expect(pruneWins(w, s, f).windows[0].panes).toEqual(["s2"]);
  });
});

describe("exit pane ids", () => {
  it("round-trips the session and never collides with the other pane kinds", () => {
    const id = exitPaneId("s00deadbeef00cafe");
    expect(isExitPane(id)).toBe(true);
    expect(exitPaneSession(id)).toBe("s00deadbeef00cafe");
    expect(isExitPane("s00deadbeef00cafe")).toBe(false);
    expect(isExitPane(diffPaneId("/f"))).toBe(false);
    // A tombstone is not a folder pane — folderPaneDir must not claim it.
    expect(folderPaneDir(id)).toBeNull();
    expect(exitPaneSession("s00deadbeef00cafe")).toBeNull();
  });

  it("paneSession names the session behind a live pane and a dead one alike", () => {
    expect(paneSession("s1")).toBe("s1");
    expect(paneSession(exitPaneId("s1"))).toBe("s1");
    expect(paneSession(diffPaneId("/f"))).toBeNull();
    expect(paneSession(filesPaneId("/f"))).toBeNull();
  });
});

describe("replacePane", () => {
  it("swaps in place, holding position and column widths", () => {
    const w: WindowsPayload = {
      windows: [{ ...win("w1", "/f", ["s1", "s2", "s3"]), cols: [200, 400, 400] }],
      activeWindows: { "/f": "w1" },
    };
    const next = replacePane(w, "s2", exitPaneId("s2"));
    expect(next.windows[0].panes).toEqual(["s1", exitPaneId("s2"), "s3"]);
    expect(next.windows[0].cols).toEqual([200, 400, 400]);
  });

  it("round-trips: a reclaimed tombstone lands back in its own slot", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1", "s2"])],
      activeWindows: { "/f": "w1" },
    };
    const dead = replacePane(w, "s2", exitPaneId("s2"));
    expect(replacePane(dead, exitPaneId("s2"), "s2")).toEqual(w);
  });

  it("leaves the payload untouched when the pane isn't in the layout", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"])],
      activeWindows: { "/f": "w1" },
    };
    expect(replacePane(w, "s9", exitPaneId("s9"))).toBe(w);
  });
});

describe("hydrateWins", () => {
  const wireWin = (id: string, folderDir: string, panes: string[]) => ({
    id,
    name: id,
    folderDir,
    panes,
  });

  it("sweeps legacy paneless windows and their active entries", () => {
    const w: WireWindowsPayload = {
      windows: [
        wireWin("w1", "/f", ["~diff:/f"]),
        wireWin("w2", "/f", []),
        wireWin("w3", "/g", []),
      ],
      activeWindows: { "/f": "w2", "/g": "w3" },
    };
    const next = hydrateWins(w);
    expect(next.windows).toEqual([win("w1", "/f", ["~diff:/f"])]);
    expect(next.activeWindows).toEqual({});
  });

  it("drops tombstones too — they report a crash from a run that's over", () => {
    const w: WireWindowsPayload = {
      windows: [wireWin("w1", "/f", [exitPaneId("s1"), "~diff:/f"]), wireWin("w2", "/g", [exitPaneId("s2")])],
      activeWindows: { "/f": "w1", "/g": "w2" },
    };
    const next = hydrateWins(w);
    expect(next.windows).toEqual([win("w1", "/f", ["~diff:/f"])]);
    expect(next.activeWindows).toEqual({ "/f": "w1" });
  });

  it("drops session panes — their PTYs died with the last run", () => {
    const w: WireWindowsPayload = {
      windows: [wireWin("w1", "/f", ["s1", "~diff:/f"]), wireWin("w2", "/g", ["s2"])],
      activeWindows: { "/f": "w1", "/g": "w2" },
    };
    const next = hydrateWins(w);
    expect(next.windows).toEqual([win("w1", "/f", ["~diff:/f"])]);
    expect(next.activeWindows).toEqual({ "/f": "w1" });
  });

  it("keeps a folder-pane layout intact, cols included", () => {
    const w: WireWindowsPayload = {
      windows: [{ ...wireWin("w1", "/f", ["~diff:/f", "~files:/f"]), cols: [333, 667] }],
      activeWindows: { "/f": "w1" },
    };
    expect(hydrateWins(w)).toEqual(w);
  });
});

describe("changedFolderDirs", () => {
  it("names only the folders whose windows or active entry differ", () => {
    const a: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/g", ["s2"])],
      activeWindows: { "/f": "w1", "/g": "w2" },
    };
    const b: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"])],
      activeWindows: { "/f": "w1" },
    };
    expect(changedFolderDirs(a, b)).toEqual(["/g"]);
    expect(changedFolderDirs(a, a)).toEqual([]);
  });

  it("catches an active-window-only change", () => {
    const a: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/f", ["s2"])],
      activeWindows: { "/f": "w1" },
    };
    const b: WindowsPayload = { ...a, activeWindows: { "/f": "w2" } };
    expect(changedFolderDirs(a, b)).toEqual(["/f"]);
  });
});

describe("normalizeWins", () => {
  it("drops active-window entries whose window is gone or moved folders", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"])],
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

  it("returns nothing for zero panes", () => {
    expect(paneRects(0)).toEqual([]);
  });

  it("applies dragged column widths in the row layout", () => {
    const rects = paneRects(2, [333, 667]);
    expect(rects.map((r) => [r.left, r.width])).toEqual([
      [0, 33.3],
      [33.3, 66.7],
    ]);
  });

  it("applies the shared column split to every grid row except a solo one", () => {
    const rects = paneRects(5, [200, 800]);
    expect(rects[0].width).toBe(20);
    expect(rects[1]).toMatchObject({ left: 20, width: 80 });
    expect(rects[3]).toMatchObject({ left: 20, width: 80 });
    expect(rects[4]).toMatchObject({ left: 0, width: 100 }); // solo last row spans
  });

  it("falls back to equal columns when cols don't match the layout", () => {
    // Wrong length (stored before a pane was added), wrong sum, sub-minimum.
    expect(paneRects(3, [333, 667]).map((r) => r.width)).toEqual([100 / 3, 100 / 3, 100 / 3]);
    expect(paneRects(2, [900, 99]).map((r) => r.width)).toEqual([50, 50]);
    expect(paneRects(2, [950, 50]).map((r) => r.width)).toEqual([50, 50]);
  });
});

describe("column drag + snap", () => {
  it("counts columns: row of n up to three, then the grid's two", () => {
    expect([1, 2, 3, 4, 7].map(colCount)).toEqual([1, 2, 3, 2, 2]);
  });

  it("snaps a divider onto thirds and fifths within the threshold", () => {
    expect(snapCol(340)).toBe(333); // pulled onto 1/3
    expect(snapCol(190)).toBe(200); // pulled onto 1/5
    expect(snapCol(510)).toBe(500); // pulled onto the even split
    expect(snapCol(450)).toBe(450); // free between snap points
  });

  it("drags a two-pane divider, snapping and keeping the per-mille total", () => {
    expect(dragCol(2, undefined, 0, 660)).toEqual([667, 333]);
    expect(dragCol(2, undefined, 0, 450)).toEqual([450, 550]);
  });

  it("moves only the divider's two columns in a three-pane row", () => {
    const cols = dragCol(3, undefined, 1, 810); // second divider toward 4/5
    expect(cols).toEqual([333, 467, 200]);
    expect(cols.reduce((a, b) => a + b, 0)).toBe(1000);
  });

  it("clamps so both adjacent columns keep the minimum width", () => {
    expect(dragCol(2, undefined, 0, 20)).toEqual([100, 900]);
    expect(dragCol(2, undefined, 0, 990)).toEqual([900, 100]);
  });

  it("rebases from equal columns when the stored cols no longer fit", () => {
    // Stored for a 2-col layout, now three panes: stale cols are ignored.
    expect(dragCol(3, [500, 500], 0, 200)).toEqual([200, 466, 334]);
  });
});

function folder(overrides: Partial<FolderData>): FolderData {
  return {
    name: "proj",
    dir: "/home/me/code/p/proj",
    dirMissing: false,
    branch: "main",
    isWorktree: false,
    filesChanged: 0,
    linesAdded: 0,
    linesRemoved: 0,
    commitsAhead: 0,
    commitsBehind: 0,
    dirty: false,
    commitsUnlanded: 0,
    sessions: [],
    needs: 0,
    hasPortDrift: false,
    ...overrides,
  };
}

describe("isFolderQuiet", () => {
  // Far enough past the ts:1 the `agent()` helper stamps that the grace
  // window (QUIET_GRACE_MS) has long expired for those events.
  const NOW = 100 * 60 * 60_000;

  it("is quiet with no sessions and a clean, non-ahead tree", () => {
    expect(isFolderQuiet(folder({}), NOW)).toBe(true);
  });

  it("is quiet for a worktree checkout that was created but never used", () => {
    expect(isFolderQuiet(folder({ isWorktree: true, sessions: [] }), NOW)).toBe(true);
  });

  it("is not quiet with a live session", () => {
    expect(isFolderQuiet(folder({ sessions: [session({ live: true })] }), NOW)).toBe(false);
  });

  it("is not quiet with a dirty working tree", () => {
    expect(isFolderQuiet(folder({ filesChanged: 3 }), NOW)).toBe(false);
  });

  it("is not quiet with unpushed local commits", () => {
    expect(isFolderQuiet(folder({ commitsAhead: 2 }), NOW)).toBe(false);
  });

  it("stays quiet when only behind origin — that's staleness, not work", () => {
    expect(isFolderQuiet(folder({ commitsBehind: 4 }), NOW)).toBe(true);
  });

  it("is not quiet with a session that catches the eye (unseen/waiting/errored)", () => {
    expect(
      isFolderQuiet(folder({ sessions: [session({ live: true, unseen: true })] }), NOW),
    ).toBe(false);
    expect(
      isFolderQuiet(
        folder({ sessions: [session({ live: true, agentState: agent("waiting") })] }),
        NOW,
      ),
    ).toBe(false);
  });

  it("stays quiet with a non-live, fully-acknowledged session sitting around", () => {
    expect(isFolderQuiet(folder({ sessions: [session({ live: false })] }), NOW)).toBe(true);
  });

  it("stays active through the grace window after an agent stops", () => {
    const stopped = folder({
      sessions: [session({ live: false, agentState: { ...agent("complete"), ts: NOW - 60_000 } })],
    });
    expect(isFolderQuiet(stopped, NOW)).toBe(false);
    expect(isFolderQuiet(stopped, NOW - 60_000 + QUIET_GRACE_MS)).toBe(true);
  });

  it("counts agent history and details.lastActivityAt toward recency", () => {
    const history = folder({
      sessions: [session({ agents: [{ ...agent("complete"), ts: NOW - 60_000 }] })],
    });
    expect(isFolderQuiet(history, NOW)).toBe(false);

    const details = folder({
      sessions: [
        session({
          agentState: { ...agent("complete"), ts: 1, details: { lastActivityAt: NOW - 60_000 } },
        }),
      ],
    });
    expect(isFolderQuiet(details, NOW)).toBe(false);
  });

  it("counts agent-pushed folder metadata toward recency", () => {
    const status = folder({ metadata: { status: { text: "wrapping up", ts: NOW - 60_000 } } });
    expect(isFolderQuiet(status, NOW)).toBe(false);
    const logs = folder({ metadata: { logs: [{ message: "done", ts: NOW - 60_000 }] } });
    expect(isFolderQuiet(logs, NOW)).toBe(false);
  });

  it("goes quiet once all activity is older than the grace window", () => {
    const old = folder({
      sessions: [session({ agentState: { ...agent("complete"), ts: NOW - QUIET_GRACE_MS } })],
      metadata: { status: { text: "old news", ts: NOW - QUIET_GRACE_MS - 1 } },
    });
    expect(isFolderQuiet(old, NOW)).toBe(true);
  });
});

function repo(key: string, folders: FolderData[]): RepoData {
  return { key, name: key, folders, needs: 0 };
}

describe("cycleNeedsYou", () => {
  // Board order: a1 (waiting), a2 (idle), b1 (unseen), b2 (error)
  const repos = [
    repo("a", [
      folder({
        dir: "a/f1",
        sessions: [
          session({ id: "a1", live: true, agentState: agent("waiting") }),
          session({ id: "a2", live: true }),
        ],
      }),
    ]),
    repo("b", [
      folder({
        dir: "b/f1",
        sessions: [
          session({ id: "b1", live: true, unseen: true }),
          session({ id: "b2", live: true, agentState: agent("error") }),
        ],
      }),
    ]),
  ];

  it("returns null when nothing catches the eye", () => {
    const calm = [
      repo("a", [
        folder({
          dir: "a/f1",
          sessions: [session({ id: "a1", live: true }), session({ id: "a2", live: true })],
        }),
      ]),
    ];
    expect(cycleNeedsYou(calm, null, "next")).toBeNull();
  });

  it("skips idle sessions and starts from the beginning when nothing is selected", () => {
    expect(cycleNeedsYou(repos, null, "next")?.id).toBe("a1");
  });

  it("cycles forward across folders and repos, skipping idle sessions", () => {
    expect(cycleNeedsYou(repos, "a1", "next")?.id).toBe("b1");
    expect(cycleNeedsYou(repos, "b1", "next")?.id).toBe("b2");
  });

  it("wraps around from the last match back to the first", () => {
    expect(cycleNeedsYou(repos, "b2", "next")?.id).toBe("a1");
  });

  it("cycles backward, wrapping from the first match to the last", () => {
    expect(cycleNeedsYou(repos, "b1", "prev")?.id).toBe("a1");
    expect(cycleNeedsYou(repos, "a1", "prev")?.id).toBe("b2");
  });

  it("starts from the end when nothing is selected and direction is prev", () => {
    expect(cycleNeedsYou(repos, null, "prev")?.id).toBe("b2");
  });

  it("treats an unrecognized fromSessionId the same as nothing selected", () => {
    expect(cycleNeedsYou(repos, "not-a-real-id", "next")?.id).toBe("a1");
  });

  it("anchors on the currently idle session's board position, not just other targets", () => {
    // From "a2" (idle, sits between a1 and b1) — next should be b1, prev should be a1.
    expect(cycleNeedsYou(repos, "a2", "next")?.id).toBe("b1");
    expect(cycleNeedsYou(repos, "a2", "prev")?.id).toBe("a1");
  });
});

describe("fmtWaitingAge", () => {
  const NOW = 10_000_000;

  it("returns null when there's no stamp or it's in the future", () => {
    expect(fmtWaitingAge(null, NOW)).toBeNull();
    expect(fmtWaitingAge(undefined, NOW)).toBeNull();
    expect(fmtWaitingAge(NOW + 5_000, NOW)).toBeNull();
  });

  it("renders sub-minute, minutes, hours, and days", () => {
    expect(fmtWaitingAge(NOW - 30_000, NOW)).toBe("waiting <1m");
    expect(fmtWaitingAge(NOW - 12 * 60_000, NOW)).toBe("waiting 12m");
    expect(fmtWaitingAge(NOW - 3 * 60 * 60_000, NOW)).toBe("waiting 3h");
    expect(fmtWaitingAge(NOW - 2 * 24 * 60 * 60_000, NOW)).toBe("waiting 2d");
  });
});

describe("needingSessionsOldestFirst", () => {
  it("orders needing sessions oldest-first, stamp-less last, stable on ties", () => {
    const repos = [
      repo("a", [
        folder({
          dir: "a/f1",
          sessions: [
            session({ id: "fresh", live: true, agentState: agent("waiting"), needsSinceMs: 500 }),
            session({ id: "nostamp", live: true, agentState: agent("error") }),
          ],
        }),
      ]),
      repo("b", [
        folder({
          dir: "b/f1",
          sessions: [
            session({ id: "old", live: true, agentState: agent("waiting"), needsSinceMs: 100 }),
            // Not needing (busy) — excluded entirely.
            session({ id: "busy", live: true, agentState: agent("busy"), needsSinceMs: 50 }),
          ],
        }),
      ]),
    ];
    expect(needingSessionsOldestFirst(repos).map((s) => s.id)).toEqual(["old", "fresh", "nostamp"]);
  });
});

describe("folderActionableItems", () => {
  it("returns nothing for a calm folder", () => {
    expect(folderActionableItems(folder({}), undefined)).toEqual([]);
  });

  it("flags a worktree whose merged PR left nothing uncommitted or unlanded", () => {
    const f = folder({ isWorktree: true });
    const items = folderActionableItems(f, pr({ state: "merged", number: 9 }));
    expect(items).toEqual([
      expect.objectContaining({ kind: "safe-to-delete", pr: { number: 9, url: expect.any(String) } }),
    ]);
  });

  it("does not flag a worktree whose PR is still open", () => {
    const f = folder({ isWorktree: true });
    expect(folderActionableItems(f, pr({ state: "open" }))).toEqual([]);
  });

  it("does not flag a merged worktree with uncommitted changes or unlanded commits", () => {
    const dirty = folder({ isWorktree: true, dirty: true });
    const unlanded = folder({ isWorktree: true, commitsUnlanded: 1 });
    expect(folderActionableItems(dirty, pr({ state: "merged" }))).toEqual([]);
    expect(folderActionableItems(unlanded, pr({ state: "merged" }))).toEqual([]);
  });

  it("does not flag a merged non-worktree checkout — that's the primary clone, not a slot", () => {
    const f = folder({ isWorktree: false });
    expect(folderActionableItems(f, pr({ state: "merged" }))).toEqual([]);
  });

  it("does not flag anything when there's no PR at all", () => {
    const f = folder({ isWorktree: true });
    expect(folderActionableItems(f, undefined)).toEqual([]);
  });

  it("flags sessions waiting on you, pluralized by count", () => {
    expect(folderActionableItems(folder({ needs: 2 }), undefined)).toEqual([
      expect.objectContaining({ kind: "needs-you", subtitle: "2 sessions waiting on you" }),
    ]);
    expect(folderActionableItems(folder({ needs: 1 }), undefined)).toEqual([
      expect.objectContaining({ kind: "needs-you", subtitle: "1 session waiting on you" }),
    ]);
  });

  it("flags drifted ports, listing every drift entry", () => {
    const f = folder({
      sessions: [session({ portDrift: [{ key: "APP_PORT", spawnedPort: 3000, currentPort: 3010 }] })],
    });
    expect(folderActionableItems(f, undefined)).toEqual([
      expect.objectContaining({ kind: "port-drift", subtitle: "APP_PORT 3000 → 3010" }),
    ]);
  });

  it("can surface all three signals at once, in a stable order", () => {
    const f = folder({
      isWorktree: true,
      needs: 1,
      sessions: [session({ portDrift: [{ key: "APP_PORT", spawnedPort: 3000, currentPort: 3010 }] })],
    });
    const items = folderActionableItems(f, pr({ state: "merged", number: 3 }));
    expect(items.map((i) => i.kind)).toEqual(["safe-to-delete", "needs-you", "port-drift"]);
  });
});

describe("cache expiry warning", () => {
  const FIVE_MIN = 300_000;
  const ONE_HOUR = 3_600_000;
  const now = 1_000_000_000;
  const details = (ttlMs: number, msLeft: number) => ({
    cacheTtlMs: ttlMs,
    cacheExpiresAt: now + msLeft,
  });

  it("warns inside the last 2m of a 5m cache", () => {
    expect(isCacheExpiring(details(FIVE_MIN, 90_000), now)).toBe(true);
    expect(isCacheExpiring(details(FIVE_MIN, 120_000), now)).toBe(true);
    expect(isCacheExpiring(details(FIVE_MIN, 180_000), now)).toBe(false);
  });

  it("warns inside the last 10m of a 1h cache", () => {
    expect(isCacheExpiring(details(ONE_HOUR, 9 * 60_000), now)).toBe(true);
    expect(isCacheExpiring(details(ONE_HOUR, 11 * 60_000), now)).toBe(false);
  });

  it("stops warning once the cache is cold — that's isCold's job", () => {
    expect(isCacheExpiring(details(FIVE_MIN, 0), now)).toBe(false);
    expect(isCacheExpiring(details(FIVE_MIN, -60_000), now)).toBe(false);
  });

  it("never warns without cache activity", () => {
    expect(isCacheExpiring(null, now)).toBe(false);
    expect(isCacheExpiring({ cacheTtlMs: FIVE_MIN }, now)).toBe(false);
  });

  it("falls back to the 5m warn window for an unknown TTL", () => {
    expect(cacheWarnMs(null)).toBe(120_000);
    expect(cacheWarnMs(ONE_HOUR)).toBe(600_000);
  });
});

describe("agentRollup expiring count", () => {
  const now = 1_000_000_000;
  const repoOf = (sessions: SessionData[]): RepoData =>
    repo("r", [{ ...folder({}), sessions }]);

  it("counts running agents whose warm cache is inside the warn window", () => {
    const expiring = session({
      live: true,
      agentState: {
        ...agent("idle"),
        details: { cacheTtlMs: 300_000, cacheExpiresAt: now + 60_000 },
      },
    });
    const warm = session({
      id: "s2",
      live: true,
      agentState: {
        ...agent("busy"),
        details: { cacheTtlMs: 300_000, cacheExpiresAt: now + 240_000 },
      },
    });
    const r = agentRollup([repoOf([expiring, warm])], now, 30);
    expect(r.total).toBe(2);
    expect(r.expiring).toBe(1);
  });
});
