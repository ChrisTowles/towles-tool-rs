import { describe, expect, it } from "vitest";
import {
  applyRepoOrder,
  orderSettled,
  reorderDirs,
  sameOrder,
  showAddPath,
  untrackedCandidates,
} from "./repo-manager";

const dirs = ["/a", "/b", "/c"];

describe("reorderDirs", () => {
  it("moves a row before another", () => {
    expect(reorderDirs(dirs, "/c", "/a")).toEqual(["/c", "/a", "/b"]);
    expect(reorderDirs(dirs, "/a", "/c")).toEqual(["/b", "/a", "/c"]);
  });

  it("moves a row to the end", () => {
    expect(reorderDirs(dirs, "/a", "end")).toEqual(["/b", "/c", "/a"]);
  });

  it("is a no-op when dropped on itself", () => {
    expect(reorderDirs(dirs, "/b", "/b")).toEqual(dirs);
  });

  it("ignores an unknown dragged dir", () => {
    expect(reorderDirs(dirs, "/nope", "/a")).toEqual(dirs);
  });

  it("appends when the target is gone", () => {
    expect(reorderDirs(dirs, "/a", "/gone")).toEqual(["/b", "/c", "/a"]);
  });
});

describe("applyRepoOrder", () => {
  const repos = [{ dir: "/a" }, { dir: "/b" }, { dir: "/c" }];

  it("passes the snapshot through when there is no overlay", () => {
    expect(applyRepoOrder(repos, null)).toEqual(repos);
  });

  it("sorts by the optimistic order", () => {
    expect(applyRepoOrder(repos, ["/c", "/b", "/a"]).map((r) => r.dir)).toEqual(["/c", "/b", "/a"]);
  });

  it("keeps a repo the order never mentioned, at the end", () => {
    const extra = [...repos, { dir: "/new" }];
    expect(applyRepoOrder(extra, ["/c", "/a", "/b"]).map((r) => r.dir)).toEqual([
      "/c",
      "/a",
      "/b",
      "/new",
    ]);
  });

  it("ignores an ordered dir that is no longer tracked", () => {
    expect(applyRepoOrder(repos, ["/gone", "/b"]).map((r) => r.dir)).toEqual(["/b", "/a", "/c"]);
  });
});

describe("sameOrder", () => {
  it("compares sequences", () => {
    expect(sameOrder(dirs, ["/a", "/b", "/c"])).toBe(true);
    expect(sameOrder(dirs, ["/a", "/c", "/b"])).toBe(false);
    expect(sameOrder(dirs, ["/a", "/b"])).toBe(false);
  });
});

describe("untrackedCandidates", () => {
  const candidates = [
    { name: "a", dir: "/a", active: true },
    { name: "b", dir: "/b", active: false },
    { name: "c", dir: "/c", active: false },
  ];

  it("drops active repos", () => {
    expect(untrackedCandidates(candidates, new Set()).map((c) => c.dir)).toEqual(["/b", "/c"]);
  });

  it("drops a just-tracked repo whose active flag hasn't refreshed yet", () => {
    expect(untrackedCandidates(candidates, new Set(["/b"])).map((c) => c.dir)).toEqual(["/c"]);
  });
});

describe("showAddPath", () => {
  const candidates = [{ name: "a", dir: "/a", active: true }];

  it("needs an absolute path", () => {
    expect(showAddPath("web", candidates, new Set())).toBe(false);
    expect(showAddPath("/srv/web", candidates, new Set())).toBe(true);
  });

  it("stays hidden when the path is already listed", () => {
    expect(showAddPath("/a", candidates, new Set())).toBe(false);
    expect(showAddPath("/b", candidates, new Set(["/b"]))).toBe(false);
  });
});

describe("orderSettled", () => {
  it("is false with no optimistic order in flight", () => {
    expect(orderSettled(null, ["/a", "/b"])).toBe(false);
  });

  it("is false while the snapshot still shows the pre-drag order", () => {
    expect(orderSettled(["/b", "/a"], ["/a", "/b"])).toBe(false);
  });

  it("is true once the snapshot matches the drag", () => {
    expect(orderSettled(["/b", "/a"], ["/b", "/a"])).toBe(true);
  });

  it("settles even when another window tracked a repo mid-drag", () => {
    // The regression this exists for: demanding an exact list match would
    // never settle here, pinning the overlay and masking the backend order
    // forever. /c is new and legitimately absent from `order`.
    expect(orderSettled(["/b", "/a"], ["/b", "/a", "/c"])).toBe(true);
  });

  it("settles even when another window untracked a repo mid-drag", () => {
    expect(orderSettled(["/b", "/a", "/c"], ["/b", "/c"])).toBe(true);
  });

  it("stays unsettled when the surviving repos are still in the old order", () => {
    expect(orderSettled(["/b", "/a"], ["/a", "/b", "/c"])).toBe(false);
  });
});
