import { dmsNeedingAttention, type StoreSnapshot } from "./data";
import type { StatePayload } from "./agentboard";
import type { FocusTarget } from "./focus-target";

/**
 * The day bar's "needs you" feed: every item currently demanding the owner's
 * attention, ranked, as one flat list the popover renders and navigates from.
 * Pure over the two live snapshots so ordering is unit-tested; the day bar owns
 * the presentation and the actual navigation.
 *
 * Ordering (by `tier`, then newest first): unanswered DMs and failing-CI PRs
 * are the top tier, then review-requested PRs, then repos with agents blocked
 * on you. Agent status is only *reported* here — rows navigate to the row, they
 * never act on the agent.
 */

export type AttentionKind = "dm" | "pr-ci" | "pr-review" | "agent";

export type AttentionItem = {
  /** Stable React key / de-dupe id. */
  id: string;
  kind: AttentionKind;
  /** Ordering bucket (lower = more urgent). */
  tier: number;
  title: string;
  subtitle: string;
  /** In-app deep link — navigate via `openTabWithFocus`. */
  target?: FocusTarget;
  /** External URL (a Slack DM) — opened instead of navigating in-app. */
  url?: string;
  /** Secondary sort key within a tier (newer first). */
  sortTs: number;
};

const TIER_URGENT = 0; // DMs + failing CI
const TIER_REVIEW = 1; // review requested
const TIER_AGENT = 2; // agents waiting on you

export function buildAttentionFeed(
  snapshot: StoreSnapshot,
  agentState: StatePayload,
): AttentionItem[] {
  const items: AttentionItem[] = [];

  for (const dm of dmsNeedingAttention(snapshot)) {
    items.push({
      id: `dm:${dm.channel}:${dm.ts}`,
      kind: "dm",
      tier: TIER_URGENT,
      title: dm.fromName,
      subtitle: dm.text,
      url: dm.url,
      sortTs: dm.ts,
    });
  }

  // Merged PRs live in the snapshot too (briefly, so a folder's rail chip can
  // turn purple), but a merged PR never needs attention.
  for (const pr of snapshot.prs.filter((p) => p.state === "open")) {
    const prId = `${pr.repo}#${pr.number}`;
    const target: FocusTarget = { screen: "gh-prs", kind: "pr", id: prId };
    // Failing CI outranks review-requested; a PR that is both surfaces once, in
    // the more-urgent bucket (mirrors `prRank`).
    if (pr.checks === "failing") {
      items.push({
        id: `pr-ci:${prId}`,
        kind: "pr-ci",
        tier: TIER_URGENT,
        title: pr.title,
        subtitle: `${prId} · CI failing`,
        target,
        sortTs: pr.updatedTs,
      });
    } else if (pr.reviewState === "review_requested") {
      items.push({
        id: `pr-review:${prId}`,
        kind: "pr-review",
        tier: TIER_REVIEW,
        title: pr.title,
        subtitle: `${prId} · review requested`,
        target,
        sortTs: pr.updatedTs,
      });
    }
  }

  for (const repo of agentState.repos) {
    if (repo.needs <= 0) continue;
    items.push({
      id: `agent:${repo.key}`,
      kind: "agent",
      tier: TIER_AGENT,
      title: repo.name,
      subtitle: `${repo.needs} session${repo.needs === 1 ? "" : "s"} waiting`,
      target: { screen: "agentboard", kind: "repo", id: repo.key },
      sortTs: 0,
    });
  }

  return items.toSorted(
    (a, b) => a.tier - b.tier || b.sortTs - a.sortTs || a.id.localeCompare(b.id),
  );
}
