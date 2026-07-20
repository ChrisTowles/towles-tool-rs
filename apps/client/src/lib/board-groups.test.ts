import { describe, expect, it } from "vitest";
import {
  bucketByStatus,
  groupTasksByRepo,
  NO_REPO_GROUP,
  railRepoKeyForTask,
  repoGroupLabel,
  taskRepoKey,
  type RailRepoRow,
} from "@/lib/board-groups";
import type { TaskItem } from "@/lib/data";

function task(over: Partial<TaskItem> = {}): TaskItem {
  return {
    id: 1,
    text: "t",
    status: "backlog",
    position: 0,
    createdAt: 0,
    issues: [],
    prs: [],
    ...over,
  };
}

describe("taskRepoKey", () => {
  it("prefers the slot's owner/name over every other source", () => {
    const t = task({
      slot: { repoRoot: "/code/other", repo: "o/slot", branch: "b" },
      issues: [{ repo: "o/issue", number: 1, url: "u", state: "open" }],
      prs: [{ repo: "o/pr", number: 2, url: "u", state: "open", checks: "" }],
    });
    expect(taskRepoKey(t)).toBe("o/slot");
  });

  it("falls back to the repo root's basename when nothing else identifies a repo", () => {
    const t = task({ slot: { repoRoot: "/home/c/code/p/towles-tool-rs", branch: "b" } });
    expect(taskRepoKey(t)).toBe("towles-tool-rs");
  });

  it("prefers a linked repo over the repo root's basename", () => {
    // The `+` on a folder header binds that folder's dir, which for a worktree
    // slot is the branch slug — grouping on it would split one repo per slot.
    const t = task({
      slot: { repoRoot: "/code/p/blog/.claude/worktrees/feat-thing", branch: "feat/thing" },
      issues: [{ repo: "o/blog", number: 1, url: "u", state: "open" }],
    });
    expect(taskRepoKey(t)).toBe("o/blog");
  });

  it("ignores a trailing separator on the repo root", () => {
    const t = task({ slot: { repoRoot: "/code/p/blog/", branch: "b" } });
    expect(taskRepoKey(t)).toBe("blog");
  });

  it("keeps a slot-bound task in its lane after the worktree is removed", () => {
    // `dir` cleared, `repoRoot`/`repo` kept — a detached task must not jump lanes.
    const t = task({ slot: { repoRoot: "/code/x", repo: "o/x", branch: "feat/y" } });
    expect(taskRepoKey(t)).toBe("o/x");
  });

  it("uses the first issue link when there is no slot", () => {
    const t = task({ issues: [{ repo: "o/issue", number: 1, url: "u", state: "open" }] });
    expect(taskRepoKey(t)).toBe("o/issue");
  });

  it("uses the first PR link when there is no slot and no issue", () => {
    const t = task({ prs: [{ repo: "o/pr", number: 2, url: "u", state: "open", checks: "" }] });
    expect(taskRepoKey(t)).toBe("o/pr");
  });

  it("returns the no-repo bucket when nothing identifies a repo", () => {
    expect(taskRepoKey(task())).toBe(NO_REPO_GROUP);
  });
});

describe("repoGroupLabel", () => {
  it("strips the owner", () => {
    expect(repoGroupLabel("chris/towles-tool-rs")).toBe("towles-tool-rs");
  });

  it("passes a bare name through", () => {
    expect(repoGroupLabel("blog")).toBe("blog");
  });

  it("names the no-repo bucket", () => {
    expect(repoGroupLabel(NO_REPO_GROUP)).toBe("No repo");
  });
});

describe("groupTasksByRepo", () => {
  it("sorts lanes by label with the no-repo bucket last", () => {
    const groups = groupTasksByRepo([
      task({ id: 1 }),
      task({ id: 2, slot: { repoRoot: "/r", repo: "o/zebra", branch: "b" } }),
      task({ id: 3, slot: { repoRoot: "/r", repo: "o/apple", branch: "b" } }),
    ]);
    expect(groups.map((g) => g.key)).toEqual(["o/apple", "o/zebra", NO_REPO_GROUP]);
  });

  it("sorts by bare name, not by owner", () => {
    const groups = groupTasksByRepo([
      task({ id: 1, slot: { repoRoot: "/r", repo: "zzz/apple", branch: "b" } }),
      task({ id: 2, slot: { repoRoot: "/r", repo: "aaa/zebra", branch: "b" } }),
    ]);
    expect(groups.map((g) => g.label)).toEqual(["apple", "zebra"]);
  });

  it("collects every task of a repo into one lane, preserving input order", () => {
    const slot = { repoRoot: "/r", repo: "o/x", branch: "b" };
    const groups = groupTasksByRepo([
      task({ id: 1, slot }),
      task({ id: 2, slot: { repoRoot: "/r", repo: "o/y", branch: "b" } }),
      task({ id: 3, slot }),
    ]);
    expect(groups.find((g) => g.key === "o/x")?.tasks.map((t) => t.id)).toEqual([1, 3]);
  });

  it("groups a slot-bound and an issue-linked task together when the repo matches", () => {
    const groups = groupTasksByRepo([
      task({ id: 1, slot: { repoRoot: "/r", repo: "o/x", branch: "b" } }),
      task({ id: 2, issues: [{ repo: "o/x", number: 9, url: "u", state: "open" }] }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].tasks.map((t) => t.id)).toEqual([1, 2]);
  });

  it("produces no lanes for no tasks", () => {
    expect(groupTasksByRepo([])).toEqual([]);
  });
});

describe("railRepoKeyForTask", () => {
  const rail: RailRepoRow[] = [
    {
      key: "path:/code/p/tt-rs",
      dir: "/code/p/tt-rs",
      originUrl: "git@github.com:ChrisTowles/towles-tool-rs.git",
      folders: [{ dir: "/code/p/tt-rs" }, { dir: "/code/p/tt-rs/.claude/worktrees/feat-x" }],
    },
    {
      key: "path:/code/p/dawn",
      dir: "/code/p/dawn",
      originUrl: "https://github.com/ChrisTowles/dawncaster-re",
      folders: [{ dir: "/code/p/dawn" }],
    },
  ];

  it("matches the slot's worktree dir to a rail folder", () => {
    const t = task({
      slot: {
        repoRoot: "/elsewhere",
        dir: "/code/p/tt-rs/.claude/worktrees/feat-x",
        branch: "feat/x",
      },
    });
    expect(railRepoKeyForTask(rail, t)).toBe("path:/code/p/tt-rs");
  });

  it("matches a detached task's repo root after the worktree dir is gone", () => {
    const t = task({ slot: { repoRoot: "/code/p/dawn", repo: "other/fork", branch: "b" } });
    expect(railRepoKeyForTask(rail, t)).toBe("path:/code/p/dawn");
  });

  it("falls back to GitHub owner/name from an issue link when no path matches", () => {
    const t = task({
      issues: [{ repo: "ChrisTowles/dawncaster-re", number: 3, url: "u", state: "open" }],
    });
    expect(railRepoKeyForTask(rail, t)).toBe("path:/code/p/dawn");
  });

  it("returns null for a task whose repo isn't on the rail", () => {
    const t = task({ slot: { repoRoot: "/code/p/unrelated", repo: "o/unrelated" } });
    expect(railRepoKeyForTask(rail, t)).toBeNull();
  });

  it("returns null for a no-repo task", () => {
    expect(railRepoKeyForTask(rail, task())).toBeNull();
  });
});

describe("bucketByStatus", () => {
  it("buckets every status and sorts by position, created-at as tiebreak", () => {
    const cols = bucketByStatus([
      task({ id: 1, status: "doing", position: 2 }),
      task({ id: 2, status: "doing", position: 1 }),
      task({ id: 3, status: "doing", position: 1, createdAt: -1 }),
      task({ id: 4, status: "done" }),
    ]);
    expect(cols.doing.map((t) => t.id)).toEqual([3, 2, 1]);
    expect(cols.done.map((t) => t.id)).toEqual([4]);
    expect(cols.backlog).toEqual([]);
  });

  it("drops (not crashes on) a card whose status the wire invented", () => {
    const rogue = { ...task({ id: 9 }), status: "someday" as never };
    const cols = bucketByStatus([rogue, task({ id: 1 })]);
    expect(
      Object.values(cols)
        .flat()
        .map((t) => t.id),
    ).toEqual([1]);
  });
});
