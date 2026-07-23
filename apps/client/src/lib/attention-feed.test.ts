import { describe, expect, it } from "vitest";
import { buildAttentionFeed } from "./attention-feed";
import { EMPTY_SNAPSHOT, type DmItem, type PrItem, type StoreSnapshot } from "./data";
import type { RepoData, StatePayload } from "./agentboard";

function pr(over: Partial<PrItem>): PrItem {
  return {
    repo: "octo/widgets",
    number: 1,
    title: "a pr",
    branch: "feat/x",
    state: "open",
    checks: "passing",
    reviewState: "",
    url: "https://example.com/pr",
    updatedTs: 0,
    dismissedTs: 0,
    ...over,
  };
}

function dm(over: Partial<DmItem>): DmItem {
  return {
    channel: "C1",
    fromName: "Wife",
    text: "call me",
    ts: 1000,
    fromMe: false,
    fetchedAt: 0,
    dismissedTs: 0,
    ...over,
  };
}

function snapshot(over: Partial<StoreSnapshot>): StoreSnapshot {
  return { ...EMPTY_SNAPSHOT, ...over };
}

function repo(over: Partial<RepoData>): RepoData {
  return {
    key: "octo/widgets",
    dir: "/repos/widgets",
    name: "widgets",
    folders: [],
    needs: 0,
    ...over,
  };
}

const NO_AGENTS: StatePayload = {
  repos: [],
  preferredEditor: "",
  compactRecommendPercent: 30,
  windows: { windows: [], activeWindows: {} },
  collapsed: {},
  ts: 0,
};

function agents(repos: RepoData[]): StatePayload {
  return { ...NO_AGENTS, repos };
}

describe("buildAttentionFeed", () => {
  it("is empty when nothing needs attention", () => {
    expect(buildAttentionFeed(EMPTY_SNAPSHOT, NO_AGENTS)).toEqual([]);
  });

  it("ranks DMs and failing CI first, then review requests, then waiting agents", () => {
    const feed = buildAttentionFeed(
      snapshot({
        dms: [dm({ ts: 1000 })],
        prs: [
          pr({ number: 10, checks: "failing" }),
          pr({ number: 20, reviewState: "review_requested" }),
        ],
      }),
      agents([repo({ needs: 2 })]),
    );
    expect(feed.map((i) => i.kind)).toEqual(["dm", "pr-ci", "pr-review", "agent"]);
  });

  it("ignores a merged PR even with a stale failing/review-requested state", () => {
    const feed = buildAttentionFeed(
      snapshot({
        prs: [
          pr({ number: 9, state: "merged", checks: "failing", reviewState: "review_requested" }),
        ],
      }),
      NO_AGENTS,
    );
    expect(feed).toEqual([]);
  });

  it("surfaces a failing+review-requested PR once, in the urgent tier", () => {
    const feed = buildAttentionFeed(
      snapshot({ prs: [pr({ number: 7, checks: "failing", reviewState: "review_requested" })] }),
      NO_AGENTS,
    );
    expect(feed).toHaveLength(1);
    expect(feed[0].kind).toBe("pr-ci");
  });

  it("orders newest-first within a tier", () => {
    const feed = buildAttentionFeed(
      snapshot({
        prs: [
          pr({ number: 1, checks: "failing", updatedTs: 100 }),
          pr({ number: 2, checks: "failing", updatedTs: 900 }),
        ],
      }),
      NO_AGENTS,
    );
    expect(feed.map((i) => i.subtitle)).toEqual([
      "octo/widgets#2 · CI failing",
      "octo/widgets#1 · CI failing",
    ]);
  });

  it("skips passing PRs, answered DMs, and repos with no waiting sessions", () => {
    const feed = buildAttentionFeed(
      snapshot({
        prs: [pr({ checks: "passing" })],
        dms: [dm({ fromMe: true })],
      }),
      agents([repo({ needs: 0 })]),
    );
    expect(feed).toEqual([]);
  });

  it("carries a gh-prs focus target for PRs and an external url for DMs", () => {
    const feed = buildAttentionFeed(
      snapshot({
        prs: [pr({ number: 42, checks: "failing" })],
        dms: [dm({ url: "https://slack.example/dm" })],
      }),
      NO_AGENTS,
    );
    const prItem = feed.find((i) => i.kind === "pr-ci");
    const dmItem = feed.find((i) => i.kind === "dm");
    expect(prItem?.target).toEqual({ screen: "gh-prs", kind: "pr", id: "octo/widgets#42" });
    expect(dmItem?.url).toBe("https://slack.example/dm");
    expect(dmItem?.target).toBeUndefined();
  });

  it("points a waiting-agent row at the repo on agentboard", () => {
    const feed = buildAttentionFeed(
      EMPTY_SNAPSHOT,
      agents([repo({ key: "octo/gizmos", needs: 1 })]),
    );
    expect(feed[0].target).toEqual({ screen: "agentboard", kind: "repo", id: "octo/gizmos" });
    expect(feed[0].subtitle).toBe("1 session waiting");
  });
});
