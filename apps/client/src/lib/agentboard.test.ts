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
  ownerRepoFromOrigin,
  dropPane,
  exitPaneId,
  exitPaneSession,
  fmtContext,
  fmtTokens,
  fmtWaitingAge,
  folderActionableItems,
  folderLanded,
  folderLandedButHasWork,
  folderRemovableTask,
  forceDeleteLabel,
  branchRedundant,
  humanizeFolderName,
  modelContextLabel,
  modelLetter,
  stoppablePort,
  type TaskBlocker,
  folderHoldsNoWork,
  folderSafeToDelete,
  hydrateWins,
  isDiffPane,
  isExitPane,
  isFilesPane,
  filesPaneDir,
  filesPaneId,
  filesPanePathFor,
  folderPaneDir,
  isCacheExpiring,
  claudeCommand,
  dynamicFlowPrompt,
  isFolderQuiet,
  isPasteableImage,
  issuesForFolder,
  needingSessionsOldestFirst,
  normalizeWins,
  paneRects,
  paneSession,
  snapCol,
  pathScope,
  placePane,
  prForFolder,
  taskForFolder,
  promptWithImages,
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
  nextOpenFileNonce,
  nextWindowId,
} from "./agentboard";
import type { PrItem, TaskItem } from "./data";

describe("nextWindowId", () => {
  it("never repeats an id, even when minted within one millisecond", () => {
    // Window ids key `activeWindows`; a duplicate makes two folders resolve to
    // the same window and only one folder's panes ever mount. Restoring
    // several panes after a crash mints them all in a single tick, so this is
    // the realistic collision, not a theoretical one.
    const ids = Array.from({ length: 200 }, () => nextWindowId());
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("stays monotonically increasing so ordering is preserved", () => {
    const a = nextWindowId();
    const b = nextWindowId();
    expect(Number(b.slice(1))).toBeGreaterThan(Number(a.slice(1)));
  });
});

describe("nextOpenFileNonce", () => {
  it("changes on every call so back-to-back opens both re-trigger", () => {
    // Claude's openFile tool calls arrive with no human delay between them;
    // a repeated nonce reads as "nothing changed" and the second open would
    // never scroll to its anchor.
    const a = nextOpenFileNonce();
    const b = nextOpenFileNonce();
    expect(b).not.toBe(a);
  });
});

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

describe("filesPanePathFor", () => {
  const dir = "/home/u/code/repo";

  it("keeps checkout-relative paths as-is", () => {
    expect(filesPanePathFor(dir, "crates/tt-vt/src/lib.rs")).toBe("crates/tt-vt/src/lib.rs");
  });

  it("strips a ./ prefix", () => {
    expect(filesPanePathFor(dir, "./src/main.rs")).toBe("src/main.rs");
    expect(filesPanePathFor(dir, "././src/main.rs")).toBe("src/main.rs");
  });

  it("relativizes an absolute path inside the checkout", () => {
    expect(filesPanePathFor(dir, `${dir}/apps/client/src/App.tsx`)).toBe("apps/client/src/App.tsx");
  });

  it("rejects paths outside the checkout", () => {
    expect(filesPanePathFor(dir, "/etc/hosts.conf")).toBeNull();
    expect(filesPanePathFor(dir, "/home/u/code/other-repo/src/main.rs")).toBeNull();
    expect(filesPanePathFor(dir, "~/notes/todo.md")).toBeNull();
    expect(filesPanePathFor(dir, "../sibling/file.rs")).toBeNull();
    expect(filesPanePathFor(dir, "./../file.rs")).toBeNull();
  });
});

describe("fmtTokens", () => {
  it("abbreviates to K and M", () => {
    expect(fmtTokens(0)).toBe("0");
    expect(fmtTokens(950)).toBe("950");
    expect(fmtTokens(53_159)).toBe("53K");
    expect(fmtTokens(412_000)).toBe("412K");
    expect(fmtTokens(1_000_000)).toBe("1M");
    expect(fmtTokens(1_500_000)).toBe("1.5M");
  });

  it("promotes to M on the rounded value, never reading '1000K'", () => {
    expect(fmtTokens(999_499)).toBe("999K");
    expect(fmtTokens(999_500)).toBe("1M");
    expect(fmtTokens(999_999)).toBe("1M");
  });

  it("drops a trailing .0 after rounding", () => {
    expect(fmtTokens(1_020_000)).toBe("1M");
    expect(fmtTokens(1_049_999)).toBe("1M");
    expect(fmtTokens(1_550_000)).toBe("1.6M");
  });
});

describe("fmtContext", () => {
  it("reads used against the window", () => {
    expect(fmtContext({ contextUsed: 412_000, contextMax: 1_000_000 })).toBe("412K / 1M");
  });

  it("names the window rather than inventing a count", () => {
    // The state after a journal rotation: the window survives, the counter doesn't.
    expect(fmtContext({ contextMax: 200_000 })).toBe("200K window");
  });

  it("is null without a window, since a bare used-count answers nothing", () => {
    expect(fmtContext({ contextUsed: 5_000 })).toBeNull();
    expect(fmtContext(null)).toBeNull();
  });
});

describe("modelContextLabel", () => {
  it("joins model and context", () => {
    expect(
      modelContextLabel({ model: "claude-opus-4-8", contextUsed: 412_000, contextMax: 1_000_000 }),
    ).toBe("claude-opus-4-8 · 412K / 1M");
  });

  it("drops the separator when only one side is known", () => {
    expect(modelContextLabel({ model: "claude-opus-4-8" })).toBe("claude-opus-4-8");
    expect(modelContextLabel({ contextUsed: 412_000, contextMax: 1_000_000 })).toBe("412K / 1M");
  });

  it("is null when nothing is known", () => {
    expect(modelContextLabel({})).toBeNull();
    expect(modelContextLabel(null)).toBeNull();
  });
});

describe("branchRedundant", () => {
  it("matches a worktree task's folder against its slugged branch", () => {
    expect(branchRedundant("feat-model-indicator-badge", "feat/model-indicator-badge")).toBe(true);
    expect(branchRedundant("feature-4-fix-thing", "feature/4-fix-thing")).toBe(true);
  });

  it("keeps the label when they differ", () => {
    expect(branchRedundant("towles-tool-rs", "main")).toBe(false);
    expect(branchRedundant("feat-model-indicator-badge", "feat/other-branch")).toBe(false);
    expect(branchRedundant("feat-model-indicator-badge", null)).toBe(false);
    expect(branchRedundant("feat-model-indicator-badge", undefined)).toBe(false);
  });

  it("collapses runs and strips trailing dashes like tt-git's slug", () => {
    expect(branchRedundant("feat-a-b", "feat//a--b!!")).toBe(true);
  });
});

describe("humanizeFolderName", () => {
  it("strips a conventional prefix and turns dashes into a sentence", () => {
    expect(humanizeFolderName("feat-today-we-use-the-worktree-name-as-the-title-for-al")).toBe(
      "Today we use the worktree name as the title for al",
    );
  });

  it("keeps the whole name when there's no recognized prefix", () => {
    expect(humanizeFolderName("quick-hotfix")).toBe("Quick hotfix");
  });

  it("leaves a bare word alone but still capitalizes it", () => {
    expect(humanizeFolderName("sandbox")).toBe("Sandbox");
  });
});

describe("modelLetter", () => {
  it("maps each family to its letter, ignoring the version", () => {
    expect(modelLetter("claude-haiku-4-5-20251001")).toBe("H");
    expect(modelLetter("claude-sonnet-5")).toBe("S");
    expect(modelLetter("claude-opus-4-8")).toBe("O");
    expect(modelLetter("claude-fable-5")).toBe("F");
    expect(modelLetter("claude-mythos-5")).toBe("M");
  });

  it("matches the family token, not a substring", () => {
    expect(modelLetter("us.anthropic.claude-opus-4-8")).toBe("O");
    expect(modelLetter("claude-opusish-1")).toBeNull();
  });

  it("is null for unknown families and missing models", () => {
    expect(modelLetter("gpt-5")).toBeNull();
    expect(modelLetter("")).toBeNull();
    expect(modelLetter(null)).toBeNull();
    expect(modelLetter(undefined)).toBeNull();
  });
});

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

function task(overrides: Partial<TaskItem>): TaskItem {
  return {
    id: 1,
    text: "a task",
    status: "doing",
    position: 0,
    createdAt: 0,
    issues: [],
    prs: [],
    ...overrides,
  };
}

describe("taskForFolder / issuesForFolder", () => {
  const linked = task({
    id: 5,
    worktree: { repoRoot: "/r", dir: "/r/.claude/worktrees/feat-x" },
    issues: [{ repo: "o/r", number: 12, url: "https://github.com/o/r/issues/12", state: "open" }],
  });

  it("matches a task by its worktree dir", () => {
    expect(taskForFolder([linked], "/r/.claude/worktrees/feat-x")?.id).toBe(5);
  });

  it("returns undefined when no task is bound to the dir", () => {
    expect(taskForFolder([linked], "/r/.claude/worktrees/other")).toBeUndefined();
    // A task with no worktree binding never matches.
    expect(taskForFolder([task({})], "/r/.claude/worktrees/feat-x")).toBeUndefined();
  });

  it("surfaces the bound task's issue links, empty when nothing is bound", () => {
    expect(issuesForFolder([linked], "/r/.claude/worktrees/feat-x")).toHaveLength(1);
    expect(issuesForFolder([linked], "/r/.claude/worktrees/feat-x")[0].number).toBe(12);
    expect(issuesForFolder([linked], "/r/.claude/worktrees/gone")).toEqual([]);
  });
});

describe("folderHoldsNoWork", () => {
  it("is true for a clean folder with nothing unlanded", () => {
    expect(folderHoldsNoWork(folder({}))).toBe(true);
  });

  it("is false for a dirty working tree", () => {
    expect(folderHoldsNoWork(folder({ dirty: true }))).toBe(false);
  });

  it("is false with commits that haven't landed on comparedBase yet", () => {
    expect(folderHoldsNoWork(folder({ commitsUnlanded: 1 }))).toBe(false);
  });

  it("stays true despite a nonzero commitsAhead — that's just SHA reachability, not unlanded work", () => {
    // The rebase/squash-merge case: commitsAhead never reaches 0, but
    // commitsUnlanded (patch-equivalence) does once the content has landed.
    expect(folderHoldsNoWork(folder({ commitsAhead: 2, commitsUnlanded: 0 }))).toBe(true);
  });

  it("is true for a squash-merged branch with a clean tree", () => {
    expect(
      folderHoldsNoWork(
        folder({ landed: "squash-merged", commitsAhead: 3, commitsUnlanded: 0, dirty: false }),
      ),
    ).toBe(true);
  });

  it("is false for a squash-merged branch with uncommitted changes — landing doesn't save those", () => {
    // The two axes are independent: the commits are all on the base, but the
    // working tree still holds files that exist nowhere else.
    expect(
      folderHoldsNoWork(folder({ landed: "squash-merged", commitsUnlanded: 0, dirty: true })),
    ).toBe(false);
  });

  it("is false for a branch squash-merged and then committed to again", () => {
    // commitsUnlanded counts only the commits made after the squash.
    expect(folderHoldsNoWork(folder({ landed: null, commitsAhead: 3, commitsUnlanded: 1 }))).toBe(
      false,
    );
  });

  it("ignores landed itself — 'upstream gone' is not proof the commits are anywhere", () => {
    expect(
      folderHoldsNoWork(folder({ landed: "upstream gone", commitsAhead: 2, commitsUnlanded: 2 })),
    ).toBe(false);
  });
});

describe("folderSafeToDelete", () => {
  it("is true for a merged PR over a checkout holding nothing", () => {
    expect(folderSafeToDelete(folder({ landed: "squash-merged" }), pr({ state: "merged" }))).toBe(
      true,
    );
  });

  it("is false with no PR at all, even when git proves the branch landed", () => {
    // The rule: git can prove content reached the base, but not that the work
    // was accepted rather than abandoned. No PR, no affirmative claim.
    expect(folderSafeToDelete(folder({ landed: "squash-merged" }), undefined)).toBe(false);
  });

  it("is false for a clean checkout with no PR — the PR-less scratch task", () => {
    expect(folderSafeToDelete(folder({}), undefined)).toBe(false);
  });

  it("is false while the PR is still open", () => {
    expect(folderSafeToDelete(folder({ landed: "squash-merged" }), pr({ state: "open" }))).toBe(
      false,
    );
  });

  it("is false for a closed-unmerged PR — that's abandoned work, not landed work", () => {
    // The case the badge must never fire on: closed-without-merge means the
    // branch may hold the only copy.
    expect(folderSafeToDelete(folder({}), pr({ state: "closed" }))).toBe(false);
  });

  it("is false when the PR merged but the tree is dirty", () => {
    expect(folderSafeToDelete(folder({ dirty: true }), pr({ state: "merged" }))).toBe(false);
  });

  it("is false when the PR merged but commits were added after", () => {
    expect(folderSafeToDelete(folder({ commitsUnlanded: 1 }), pr({ state: "merged" }))).toBe(false);
  });
});

describe("folderLanded", () => {
  it("is true when git proves it, with no PR at all", () => {
    expect(folderLanded(folder({ landed: "squash-merged" }), undefined)).toBe(true);
  });

  it("is true for a merged PR even when git can't see the landing", () => {
    expect(folderLanded(folder({ landed: null }), pr({ state: "merged" }))).toBe(true);
  });

  it("is false for an unlanded branch with an open PR", () => {
    expect(folderLanded(folder({ landed: null }), pr({ state: "open" }))).toBe(false);
  });

  it("is false for an unlanded branch with no PR", () => {
    expect(folderLanded(folder({ landed: null }), undefined)).toBe(false);
  });
});

describe("folderRemovableTask", () => {
  it("is true only for a worktree that still exists on disk", () => {
    expect(folderRemovableTask(folder({ isWorktree: true }))).toBe(true);
    expect(folderRemovableTask(folder({}))).toBe(false); // main checkout
    expect(folderRemovableTask(folder({ isWorktree: true, dirMissing: true }))).toBe(false); // ghost
  });
});

const blocker = (over: Partial<TaskBlocker> = {}): TaskBlocker => ({
  kind: "dirtyTree",
  message: "task working tree is not clean (1 changed/untracked path(s))",
  remedy: "Commit or stash the changes to keep them.",
  losesWork: true,
  ...over,
});

const portBlocker = (port: number | null): TaskBlocker =>
  blocker({
    kind: "foreignPort",
    message: `port ${port} in use`,
    remedy: "Stop it.",
    losesWork: false,
    port,
  });

describe("forceDeleteLabel", () => {
  it("names what is discarded, so the consequence is in the button", () => {
    expect(forceDeleteLabel([blocker()])).toBe("Delete and discard the changes");
    expect(forceDeleteLabel([blocker({ kind: "unreachableCommits" })])).toBe(
      "Delete and discard the commits",
    );
    expect(forceDeleteLabel([blocker(), blocker({ kind: "unreachableCommits" })])).toBe(
      "Delete and discard the changes and commits",
    );
  });

  it("does not claim work is lost when only a stray listener blocks", () => {
    // Forcing past a dev server orphans a process; it destroys nothing, and
    // borrowing the destructive wording here would train the user to ignore it.
    const port = blocker({ kind: "foreignPort", losesWork: false, port: 4424 });
    expect(forceDeleteLabel([port])).toBe("Delete anyway");
    expect(forceDeleteLabel([])).toBe("Delete anyway");
  });

  it("keys off losesWork, not the kind", () => {
    expect(forceDeleteLabel([blocker({ kind: "somethingNew", losesWork: false })])).toBe(
      "Delete anyway",
    );
  });

  it("stays unspecific about a guard it doesn't recognize", () => {
    // A newer backend's losesWork guard must not be described as discarding
    // "commits" just because that was the last branch available.
    expect(forceDeleteLabel([blocker({ kind: "liveContainer", losesWork: true })])).toBe(
      "Delete anyway",
    );
    // …but a recognized kind alongside it is still named.
    expect(forceDeleteLabel([blocker({ kind: "liveContainer", losesWork: true }), blocker()])).toBe(
      "Delete and discard the changes",
    );
  });

  it("names each discarded noun once, however many blockers carry it", () => {
    // Two dirty-tree blockers can't produce "the changes and changes".
    expect(forceDeleteLabel([blocker(), blocker()])).toBe("Delete and discard the changes");
  });
});

describe("stoppablePort", () => {
  it("offers the port of a foreignPort blocker", () => {
    expect(stoppablePort(portBlocker(4424))).toBe(4424);
  });

  it("offers nothing when there is nothing to pass to task_stop_port", () => {
    // A port blocker whose number didn't survive still renders its remedy as
    // text — it just gets no button.
    expect(stoppablePort(portBlocker(null))).toBeNull();
    // And a port on a non-port blocker is not an action: only `foreignPort`
    // is something `task_stop_port` will act on.
    expect(stoppablePort(blocker({ port: 4424 }))).toBeNull();
  });
});

describe("folderLandedButHasWork", () => {
  it("is false for a merged PR when the folder is clean and everything landed", () => {
    expect(folderLandedButHasWork(folder({}), pr({ state: "merged" }))).toBe(false);
  });

  it("is true for a merged PR with a dirty working tree", () => {
    expect(folderLandedButHasWork(folder({ dirty: true }), pr({ state: "merged" }))).toBe(true);
  });

  it("is true for a merged PR with commits that haven't landed on comparedBase yet", () => {
    expect(folderLandedButHasWork(folder({ commitsUnlanded: 1 }), pr({ state: "merged" }))).toBe(
      true,
    );
  });

  it("is false for a merged, rebase/squash-merged branch — commitsAhead alone isn't unlanded work", () => {
    expect(
      folderLandedButHasWork(
        folder({ commitsAhead: 2, commitsUnlanded: 0 }),
        pr({ state: "merged" }),
      ),
    ).toBe(false);
  });

  it("is false for an open PR even with local work — that's normal, not a data-loss warning", () => {
    expect(
      folderLandedButHasWork(folder({ dirty: true, commitsUnlanded: 1 }), pr({ state: "open" })),
    ).toBe(false);
  });

  it("is false for a squash-merged, clean folder with no PR", () => {
    expect(folderLandedButHasWork(folder({ landed: "squash-merged" }), undefined)).toBe(false);
  });

  it("is true for a squash-merged folder with uncommitted changes and no PR", () => {
    // The case the PR-only check could never see: git knows the branch is
    // done, and the checkout still holds files that removal would destroy.
    expect(
      folderLandedButHasWork(folder({ landed: "squash-merged", dirty: true }), undefined),
    ).toBe(true);
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
    expect(sessionNeeds(session({ live: true, unseen: true, agentState: agent("complete") }))).toBe(
      true,
    );
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

const valid = (sessions: string[], folders: string[]) =>
  [new Set(sessions), new Set(folders)] as const;

describe("pruneWins", () => {
  it("drops ghost session panes so survivors tile from the first task", () => {
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

  it("round-trips: a reclaimed tombstone lands back in its own task", () => {
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

const wireWin = (id: string, folderDir: string, panes: string[]) => ({
  id,
  name: id,
  folderDir,
  panes,
});

describe("hydrateWins", () => {
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
      windows: [
        wireWin("w1", "/f", [exitPaneId("s1"), "~diff:/f"]),
        wireWin("w2", "/g", [exitPaneId("s2")]),
      ],
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
    expect(five[4]).toEqual({ left: 0, top: 200 / 3, width: 100, height: 100 / 3 });
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
    landed: null,
    sessions: [],
    needs: 0,
    hasPortDrift: false,
    hasLaunchConfig: false,
    quiet: false,
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

  it("the quiet override forces quiet even with a live session", () => {
    expect(isFolderQuiet(folder({ quiet: true, sessions: [session({ live: true })] }), NOW)).toBe(
      true,
    );
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
    expect(isFolderQuiet(folder({ sessions: [session({ live: true, unseen: true })] }), NOW)).toBe(
      false,
    );
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
  return { key, dir: key, name: key, folders, needs: 0 };
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
      expect.objectContaining({
        kind: "safe-to-delete",
        pr: { number: 9, url: expect.any(String) },
      }),
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

  it("does not flag a merged non-worktree checkout — that's the primary clone, not a task", () => {
    const f = folder({ isWorktree: false });
    expect(folderActionableItems(f, pr({ state: "merged" }))).toEqual([]);
  });

  it("does not flag anything when there's no PR and git can't prove a landing", () => {
    const f = folder({ isWorktree: true });
    expect(folderActionableItems(f, undefined)).toEqual([]);
  });

  it("does NOT flag a squash-merged worktree with no PR at all", () => {
    // Deliberate: git proves the content reached the base, but not that the
    // work was accepted. A branch abandoned and reset away leaves the same
    // trace as one that shipped, so no PR means no affirmative signal.
    const f = folder({
      isWorktree: true,
      landed: "squash-merged",
      commitsAhead: 3,
      commitsUnlanded: 0,
      comparedBase: "origin/main",
    });
    expect(folderActionableItems(f, undefined)).toEqual([]);
  });

  it("flags a merged PR and names git's account of the landing alongside it", () => {
    const f = folder({
      isWorktree: true,
      landed: "squash-merged",
      commitsAhead: 3,
      commitsUnlanded: 0,
      comparedBase: "origin/main",
    });
    expect(folderActionableItems(f, pr({ state: "merged", number: 42 }))).toEqual([
      {
        kind: "safe-to-delete",
        subtitle:
          "PR #42 merged, squash-merged into main, no uncommitted changes, every commit landed",
        pr: { number: 42, url: pr({ number: 42 }).url },
      },
    ]);
  });

  it("flags a merged PR that git can't corroborate, naming only the PR", () => {
    // The base this checkout compares against may not have been fetched since
    // the merge, so `landed` stays null while the PR is authoritative.
    const f = folder({ isWorktree: true, landed: null, commitsUnlanded: 0 });
    expect(folderActionableItems(f, pr({ state: "merged", number: 7 }))).toEqual([
      {
        kind: "safe-to-delete",
        subtitle: "PR #7 merged, no uncommitted changes, every commit landed",
        pr: { number: 7, url: pr({ number: 7 }).url },
      },
    ]);
  });

  it("does not flag a squash-merged worktree that still has uncommitted changes", () => {
    const f = folder({ isWorktree: true, landed: "squash-merged", dirty: true });
    expect(folderActionableItems(f, pr({ state: "merged" }))).toEqual([]);
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
      sessions: [
        session({ portDrift: [{ key: "APP_PORT", spawnedPort: 3000, currentPort: 3010 }] }),
      ],
    });
    expect(folderActionableItems(f, undefined)).toEqual([
      expect.objectContaining({ kind: "port-drift", subtitle: "APP_PORT 3000 → 3010" }),
    ]);
  });

  it("can surface all three signals at once, in a stable order", () => {
    const f = folder({
      isWorktree: true,
      needs: 1,
      sessions: [
        session({ portDrift: [{ key: "APP_PORT", spawnedPort: 3000, currentPort: 3010 }] }),
      ],
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
  const repoOf = (sessions: SessionData[]): RepoData => repo("r", [{ ...folder({}), sessions }]);

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

describe("promptWithImages", () => {
  it("returns the bare goal when nothing was pasted", () => {
    expect(promptWithImages("  fix the header  ", [])).toBe("fix the header");
  });

  it("names the image and tells Claude to read it first", () => {
    const prompt = promptWithImages("match this design", [
      "/task/.claude/pasted-images/paste-1.png",
    ]);
    // The path alone isn't enough — a bare path in a prompt is something
    // Claude may or may not act on, so the instruction is what makes the
    // attachment reliable.
    expect(prompt).toBe(
      "match this design — Attached image — read it first, before anything else: " +
        "/task/.claude/pasted-images/paste-1.png",
    );
  });

  it("pluralizes for several images and lists them in paste order", () => {
    const prompt = promptWithImages("compare these", ["/a/paste-1.png", "/a/paste-2.png"]);
    expect(prompt).toContain("Attached images — read them first");
    expect(prompt).toContain("/a/paste-1.png /a/paste-2.png");
  });

  it("an image with no typed goal is still a prompt", () => {
    // Pasting a screenshot and hitting create is a complete ask on its own —
    // this must not collapse to an empty prompt, which would skip the launch.
    const prompt = promptWithImages("", ["/a/paste-1.png"]);
    expect(prompt).toBe("Attached image — read it first, before anything else: /a/paste-1.png");
  });

  it("never emits a newline — the prompt is typed into a PTY inside a quoted arg", () => {
    // A literal newline mid-quote drops zsh's line editor to a PS2
    // continuation prompt; the goal itself may contain one, but nothing this
    // function adds should.
    const prompt = promptWithImages("goal", ["/a/paste-1.png", "/a/paste-2.png"]);
    expect(prompt).not.toContain("\n");
  });
});

describe("claudeCommand", () => {
  it("passes --permission-mode before the prompt when asked", () => {
    expect(claudeCommand("do it", { permissionMode: "plan" })).toBe(
      "claude --permission-mode 'plan' 'do it'\r",
    );
  });

  it("omits the flag by default", () => {
    expect(claudeCommand("do it")).toBe("claude 'do it'\r");
  });
});

describe("dynamicFlowPrompt", () => {
  it("leads with the goal and carries the whole post-approval pipeline in order", () => {
    const prompt = dynamicFlowPrompt("add dark mode", "main");
    expect(prompt.startsWith("add dark mode — ")).toBe(true);
    // The steps must appear in delivery order — implement, review, simplify,
    // rebase, PR, merge — since the session executes them as written.
    const order = [
      "implement",
      "/code-review low --fix",
      "/simplify",
      "rebase",
      "gh pr create",
      "gh pr merge",
    ];
    const positions = order.map((s) => prompt.indexOf(s));
    expect(positions.every((p) => p >= 0)).toBe(true);
    expect(positions.toSorted((a, b) => a - b)).toEqual(positions);
  });

  it("names the actual base branch, not a hardcoded main", () => {
    const prompt = dynamicFlowPrompt("fix", "release/2.0");
    expect(prompt).toContain("release/2.0");
    expect(prompt).not.toContain("main");
  });

  it("carries an effective origin/ base ref through verbatim as the rebase target", () => {
    // Callers pass `TaskCreated.baseLabel` (`origin/main`, not `main`) —
    // inside the task's worktree a fetch never advances local `main`, so the
    // prompt must name the remote-tracking ref.
    const prompt = dynamicFlowPrompt("fix", "origin/main");
    expect(prompt).toContain("rebase this branch onto the latest origin/main");
  });

  it("stands alone when the goal is empty (image-only ask)", () => {
    const prompt = dynamicFlowPrompt("  ", "main");
    expect(prompt.startsWith("This is a dynamic task")).toBe(true);
  });

  it("never emits a newline — typed into a PTY inside a quoted arg", () => {
    expect(dynamicFlowPrompt("goal", "main")).not.toContain("\n");
  });
});

describe("isPasteableImage", () => {
  it("accepts the types tt_tasks::pasted writes an extension for", () => {
    for (const m of ["image/png", "image/jpeg", "image/gif", "image/webp"]) {
      expect(isPasteableImage(m)).toBe(true);
    }
  });

  it("ignores mime parameters and casing", () => {
    expect(isPasteableImage("image/PNG;charset=utf-8")).toBe(true);
  });

  it("rejects types the Rust side would error on", () => {
    // Filtering at paste time means an unsupported paste is silently ignored
    // where the user can see it didn't take, instead of failing mid-create.
    expect(isPasteableImage("image/svg+xml")).toBe(false);
    expect(isPasteableImage("text/plain")).toBe(false);
  });
});

describe("ownerRepoFromOrigin", () => {
  it("parses https, ssh, and scp-like origin urls to owner/name", () => {
    expect(ownerRepoFromOrigin("https://github.com/ChrisTowles/towles-tool-rs")).toBe(
      "ChrisTowles/towles-tool-rs",
    );
    expect(ownerRepoFromOrigin("https://github.com/octo/widgets.git")).toBe("octo/widgets");
    expect(ownerRepoFromOrigin("git@github.com:octo/widgets.git")).toBe("octo/widgets");
    expect(ownerRepoFromOrigin("ssh://git@github.com/octo/widgets")).toBe("octo/widgets");
    expect(ownerRepoFromOrigin("https://github.com/octo/widgets/")).toBe("octo/widgets");
  });

  it("returns undefined for missing or unparseable origins", () => {
    expect(ownerRepoFromOrigin(undefined)).toBeUndefined();
    expect(ownerRepoFromOrigin(null)).toBeUndefined();
    expect(ownerRepoFromOrigin("")).toBeUndefined();
    expect(ownerRepoFromOrigin("not a url")).toBeUndefined();
  });
});
