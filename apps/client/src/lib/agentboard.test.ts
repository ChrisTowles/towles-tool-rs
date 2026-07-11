import { describe, expect, it } from "vitest";
import {
  agentRollup,
  cacheWarnMs,
  changedFolderDirs,
  cycleNeedsYou,
  diffPaneDir,
  diffPaneId,
  dropEmptyWindows,
  dropPane,
  isDiffPane,
  isCacheExpiring,
  isFolderQuiet,
  normalizeWins,
  paneRects,
  pathScope,
  placePane,
  prForFolder,
  pruneWins,
  QUIET_GRACE_MS,
  sessionNeeds,
  type AgentStatus,
  type FolderData,
  type RepoData,
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

  it("keeps the folder's last window when its last pane closes", () => {
    const w: WindowsPayload = { windows: [win("w1", "/f", ["s1"])], activeWindows: { "/f": "w1" } };
    const next = dropPane(w, "s1");
    expect(next.windows).toEqual([win("w1", "/f", [])]);
    expect(next.activeWindows["/f"]).toBe("w1");
  });

  it("only counts same-folder windows as siblings", () => {
    // /f's only window empties; /g's window must not count as a sibling.
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/g", ["s2"])],
      activeWindows: { "/f": "w1", "/g": "w2" },
    };
    const next = dropPane(w, "s1");
    expect(next.windows).toEqual([win("w1", "/f", []), win("w2", "/g", ["s2"])]);
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

  it("keeps one (active-preferred) window when pruning empties them all", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["g1"]), win("w2", "/f", ["g2"])],
      activeWindows: { "/f": "w2" },
    };
    const [s, f] = valid([], ["/f"]);
    const next = pruneWins(w, s, f);
    expect(next.windows).toEqual([win("w2", "/f", [])]);
    expect(next.activeWindows["/f"]).toBe("w2");
  });

  it("never touches a deliberately-empty window", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/f", [])],
      activeWindows: { "/f": "w2" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f)).toBe(w);
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

  it("keeps a valid folder's diff pane, drops a removed folder's", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", [diffPaneId("/f"), diffPaneId("/gone"), "s1"])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f).windows[0].panes).toEqual([diffPaneId("/f"), "s1"]);
  });

  it("returns the same object when nothing changed", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"])],
      activeWindows: { "/f": "w1" },
    };
    const [s, f] = valid(["s1"], ["/f"]);
    expect(pruneWins(w, s, f)).toBe(w);
  });
});

describe("dropEmptyWindows", () => {
  it("sweeps zero-pane windows and their active entries", () => {
    const w: WindowsPayload = {
      windows: [win("w1", "/f", ["s1"]), win("w2", "/f", []), win("w3", "/g", [])],
      activeWindows: { "/f": "w2", "/g": "w3" },
    };
    const next = dropEmptyWindows(w);
    expect(next.windows).toEqual([win("w1", "/f", ["s1"])]);
    expect(next.activeWindows).toEqual({});
  });

  it("returns the same object when no window is empty", () => {
    const w: WindowsPayload = { windows: [win("w1", "/f", ["s1"])], activeWindows: {} };
    expect(dropEmptyWindows(w)).toBe(w);
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
      windows: [win("w1", "/f", []), win("w2", "/f", [])],
      activeWindows: { "/f": "w1" },
    };
    const b: WindowsPayload = { ...a, activeWindows: { "/f": "w2" } };
    expect(changedFolderDirs(a, b)).toEqual(["/f"]);
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

function folder(overrides: Partial<FolderData>): FolderData {
  return {
    name: "proj",
    dir: "/home/me/code/p/proj",
    branch: "main",
    isWorktree: false,
    filesChanged: 0,
    linesAdded: 0,
    linesRemoved: 0,
    commitsAhead: 0,
    commitsBehind: 0,
    sessions: [],
    needs: 0,
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
