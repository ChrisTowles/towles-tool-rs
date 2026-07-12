//! The ranked "needs you" attention feed: a pure fold over PR and DM store rows
//! into stable, id-addressable [`AttnItem`]s ordered by urgency.
//!
//! Ordering is two-tier: failing-CI PRs and unanswered DMs come first, then
//! review-requested PRs. Within a tier, input order is preserved (failing PRs,
//! then DMs), so the feed is deterministic for a given store snapshot. Ids are
//! stable (`pr:{repo}#{number}`, `dm:{channel}`) so callers can dedupe or track
//! an item across calls.

use serde::Serialize;
use tt_store::{DmItem, PrItem};

/// PR `checks` value meaning CI is red.
const CHECKS_FAILING: &str = "failing";
/// PR `review_state` value meaning your review is requested.
const REVIEW_REQUESTED: &str = "review_requested";

/// One entry in the attention feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttnItem {
    /// Stable id: `pr:{repo}#{number}` for PRs, `dm:{channel}` for DMs.
    pub id: String,
    /// The item's category: `failing_ci`, `review_requested`, or `dm`.
    pub kind: &'static str,
    /// Short human-readable summary.
    pub label: String,
    /// Where to act on the item, when the source row carries a URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Whether a watched DM's newest message still needs your reply: it is not your
/// own and is newer than the last dismissal. This is the single source of truth
/// for the `needsReply` predicate shared by `dm_status`, `day_brief`, and the
/// feed below.
pub fn dm_needs_reply(dm: &DmItem) -> bool {
    !dm.from_me && dm.dismissed_ts < dm.ts
}

/// Rank tier for a `kind`: 0 for the most urgent (failing CI, DMs), 1 for
/// review requests. A stable sort on this preserves within-tier input order.
fn rank(kind: &str) -> u8 {
    match kind {
        "failing_ci" | "dm" => 0,
        _ => 1,
    }
}

/// Fold PR and DM rows into the ranked attention feed. A PR that is both failing
/// and review-requested appears once, under the more urgent `failing_ci` kind,
/// so every id is unique.
pub fn attention_feed(prs: &[PrItem], dms: &[DmItem]) -> Vec<AttnItem> {
    let mut items = Vec::new();

    // Tier 0a: failing CI.
    for pr in prs {
        if pr.checks == CHECKS_FAILING {
            items.push(AttnItem {
                id: format!("pr:{}#{}", pr.repo, pr.number),
                kind: "failing_ci",
                label: format!("{}#{} {}", pr.repo, pr.number, pr.title),
                url: Some(pr.url.clone()),
            });
        }
    }

    // Tier 0b: unanswered DMs.
    for dm in dms {
        if dm_needs_reply(dm) {
            items.push(AttnItem {
                id: format!("dm:{}", dm.channel),
                kind: "dm",
                label: format!("{}: {}", dm.from_name, dm.text),
                url: dm.url.clone(),
            });
        }
    }

    // Tier 1: review requested (excluding the failing ones surfaced above).
    for pr in prs {
        if pr.review_state == REVIEW_REQUESTED && pr.checks != CHECKS_FAILING {
            items.push(AttnItem {
                id: format!("pr:{}#{}", pr.repo, pr.number),
                kind: "review_requested",
                label: format!("{}#{} {}", pr.repo, pr.number, pr.title),
                url: Some(pr.url.clone()),
            });
        }
    }

    // Stable sort keeps within-tier input order (Vec::sort_by_key is stable).
    items.sort_by_key(|item| rank(item.kind));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(repo: &str, number: i64, checks: &str, review_state: &str) -> PrItem {
        PrItem {
            repo: repo.to_string(),
            number,
            title: format!("PR {number}"),
            branch: "feat".to_string(),
            state: "open".to_string(),
            checks: checks.to_string(),
            review_state: review_state.to_string(),
            url: format!("https://example.com/{repo}/pull/{number}"),
            updated_ts: 0,
        }
    }

    fn dm(channel: &str, from_me: bool, ts: i64, dismissed_ts: i64) -> DmItem {
        DmItem {
            channel: channel.to_string(),
            from_name: "Ada".to_string(),
            text: "ping?".to_string(),
            ts,
            from_me,
            url: Some(format!("https://slack.example/{channel}")),
            fetched_at: ts,
            dismissed_ts,
        }
    }

    #[test]
    fn dm_needs_reply_matches_banner_predicate() {
        assert!(dm_needs_reply(&dm("D1", false, 100, 50)), "their newer message needs a reply");
        assert!(!dm_needs_reply(&dm("D2", true, 100, 50)), "your own message does not");
        assert!(!dm_needs_reply(&dm("D3", false, 100, 100)), "dismissed at ts does not");
    }

    #[test]
    fn failing_ci_and_dms_rank_above_review_requests() {
        let prs = vec![
            pr("o/r", 1, "passing", "review_requested"),
            pr("o/r", 2, "failing", "none"),
        ];
        let dms = vec![dm("D1", false, 100, 0)];
        let feed = attention_feed(&prs, &dms);
        let kinds: Vec<&str> = feed.iter().map(|i| i.kind).collect();
        // Tier 0 (failing_ci, dm) before tier 1 (review_requested).
        assert_eq!(kinds, vec!["failing_ci", "dm", "review_requested"]);
    }

    #[test]
    fn ids_are_stable_and_shaped() {
        let prs = vec![pr("o/r", 7, "failing", "none")];
        let dms = vec![dm("C123", false, 100, 0)];
        let feed = attention_feed(&prs, &dms);
        assert_eq!(feed[0].id, "pr:o/r#7");
        assert_eq!(feed[1].id, "dm:C123");
        // Recomputing over identical input yields identical ids/order.
        let again = attention_feed(&prs, &dms);
        assert_eq!(feed, again);
    }

    #[test]
    fn failing_and_review_requested_pr_appears_once_as_failing() {
        let prs = vec![pr("o/r", 9, "failing", "review_requested")];
        let feed = attention_feed(&prs, &[]);
        assert_eq!(feed.len(), 1, "the PR must not appear under both kinds");
        assert_eq!(feed[0].kind, "failing_ci");
        assert_eq!(feed[0].id, "pr:o/r#9");
    }

    #[test]
    fn only_unanswered_dms_enter_the_feed() {
        let dms = vec![
            dm("D_UNANSWERED", false, 100, 0),
            dm("D_ANSWERED", true, 100, 0),
            dm("D_DISMISSED", false, 100, 100),
        ];
        let feed = attention_feed(&[], &dms);
        let ids: Vec<&str> = feed.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["dm:D_UNANSWERED"]);
    }

    #[test]
    fn empty_input_yields_empty_feed() {
        assert!(attention_feed(&[], &[]).is_empty());
    }

    #[test]
    fn review_requests_preserve_input_order() {
        let prs = vec![
            pr("o/r", 3, "passing", "review_requested"),
            pr("o/r", 1, "passing", "review_requested"),
            pr("o/r", 2, "passing", "review_requested"),
        ];
        let feed = attention_feed(&prs, &[]);
        let ids: Vec<&str> = feed.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["pr:o/r#3", "pr:o/r#1", "pr:o/r#2"]);
    }
}
