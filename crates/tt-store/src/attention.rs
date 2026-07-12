//! Edge detection for day-model attention notifications.
//!
//! Two watchers turn successive store reads into *edges* — the moment something
//! newly deserves your attention — so the host fires a desktop notification
//! exactly once per event instead of on every tick. This mirrors
//! `tt_agentboard::NeedsYouWatch`: pure state diffing, Tauri-free and
//! unit-testable, with the "fire a notification / suppress while focused" policy
//! left to the host.
//!
//! - [`MeetingStartWatch`] fires when the next meeting's countdown reaches zero
//!   (its start time arrives). It only ever fires for a meeting it first saw
//!   *before* it started, so a meeting already in progress at launch — or one
//!   that arrives from the calendar collector already underway — never fires a
//!   bogus "starting now". This is strictly the next-meeting countdown reaching
//!   zero; it does not widen the calendar surface.
//! - [`ReviewRequestedWatch`] fires when a PR newly enters the review-requested
//!   set (`review_state == "review_requested"`). Edge-triggered, so a PR that
//!   stays in the set never repeats; the first observation only primes the
//!   baseline so PRs already awaiting your review at launch don't spam.
//!
//! All time is injected as `now_ms` (epoch ms); nothing here reads a clock.

use std::collections::HashSet;

use crate::{CalEvent, PrItem};

/// The `review_state` value the PRs collector writes when your review has been
/// requested (see `tt_collect::prs`).
const REVIEW_REQUESTED: &str = "review_requested";

/// The next meeting just started — its countdown reached zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingStartEdge {
    /// Provider event id (stable across collector refreshes).
    pub external_id: String,
    /// Meeting title, for the notification body.
    pub title: String,
    /// The start instant that just arrived (epoch ms).
    pub start_ts: i64,
}

/// Watches the single current-or-next meeting and yields a [`MeetingStartEdge`]
/// the first tick its start time has arrived — but only for a meeting first
/// observed while still in the future. A meeting seen already-started (app
/// launched mid-meeting, or the collector delivered one already underway) is
/// never fired: its countdown never "reached zero" on our watch.
#[derive(Debug, Default)]
pub struct MeetingStartWatch {
    /// The meeting currently being tracked (its `external_id`), if any.
    watching: Option<String>,
    /// Whether `watching` was seen while still in the future (start_ts > now).
    seen_before_start: bool,
    /// Whether the start edge for `watching` has already fired.
    fired: bool,
}

impl MeetingStartWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe the current-or-next meeting (as resolved by
    /// [`crate::Store::current_or_next_event`]) at `now_ms`. Returns the start
    /// edge on the tick the meeting crosses into "started", at most once per
    /// meeting.
    pub fn observe(&mut self, now_ms: i64, next: Option<&CalEvent>) -> Option<MeetingStartEdge> {
        let ev = next?;

        // A different meeting than last tick (the previous one ended, or the
        // collector swapped in a new soonest): start tracking it fresh.
        if self.watching.as_deref() != Some(&ev.external_id) {
            self.watching = Some(ev.external_id.clone());
            self.seen_before_start = ev.start_ts > now_ms;
            self.fired = false;
        }

        let started = ev.start_ts <= now_ms;
        if started && self.seen_before_start && !self.fired {
            self.fired = true;
            return Some(MeetingStartEdge {
                external_id: ev.external_id.clone(),
                title: ev.title.clone(),
                start_ts: ev.start_ts,
            });
        }
        None
    }
}

/// One PR that just entered the review-requested set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequestedEdge {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
}

/// Tracks which PRs sit in the review-requested set across snapshots and yields
/// the ones that just entered it. A PR that stays requested never repeats; one
/// that leaves and re-enters fires again. The first observation only primes the
/// baseline, so PRs already awaiting your review at launch don't spam.
#[derive(Debug, Default)]
pub struct ReviewRequestedWatch {
    /// `repo#number` keys that were review-requested in the previous snapshot.
    prev: HashSet<String>,
    /// False until the first observation has primed the baseline.
    primed: bool,
}

impl ReviewRequestedWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diff `prs` against the previous snapshot and return the PRs that newly
    /// need your review. Updates the baseline as a side effect.
    pub fn observe(&mut self, prs: &[PrItem]) -> Vec<ReviewRequestedEdge> {
        let mut edges = Vec::new();
        let mut current = HashSet::with_capacity(self.prev.len());

        for pr in prs {
            if pr.review_state != REVIEW_REQUESTED {
                continue;
            }
            let key = format!("{}#{}", pr.repo, pr.number);
            let is_new = !self.prev.contains(&key);
            current.insert(key);
            if self.primed && is_new {
                edges.push(ReviewRequestedEdge {
                    repo: pr.repo.clone(),
                    number: pr.number,
                    title: pr.title.clone(),
                    url: pr.url.clone(),
                });
            }
        }

        self.prev = current;
        self.primed = true;
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(external_id: &str, start_ts: i64) -> CalEvent {
        CalEvent {
            id: 1,
            external_id: external_id.to_string(),
            title: format!("Meeting {external_id}"),
            start_ts,
            end_ts: Some(start_ts + 1_800_000),
            attendees: vec![],
            location: None,
            join_url: None,
        }
    }

    fn pr(repo: &str, number: i64, review_state: &str) -> PrItem {
        PrItem {
            repo: repo.to_string(),
            number,
            title: format!("PR {number}"),
            branch: "feat/x".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: review_state.to_string(),
            url: format!("https://github.com/{repo}/pull/{number}"),
            updated_ts: 0,
        }
    }

    // --- MeetingStartWatch ------------------------------------------------

    #[test]
    fn meeting_fires_once_when_start_arrives() {
        let mut w = MeetingStartWatch::new();
        let ev = event("a", 1000);
        // Seen while still in the future: no fire yet.
        assert_eq!(w.observe(500, Some(&ev)), None);
        assert_eq!(w.observe(900, Some(&ev)), None);
        // Start arrives: one edge.
        let edge = w.observe(1000, Some(&ev)).expect("start edge");
        assert_eq!(edge.external_id, "a");
        assert_eq!(edge.start_ts, 1000);
        // Still in progress next ticks: no repeat.
        assert_eq!(w.observe(1200, Some(&ev)), None);
        assert_eq!(w.observe(1500, Some(&ev)), None);
    }

    #[test]
    fn meeting_in_progress_at_launch_never_fires() {
        let mut w = MeetingStartWatch::new();
        // First observation already started (launched mid-meeting): a level,
        // not a countdown reaching zero on our watch.
        let ev = event("a", 1000);
        assert_eq!(w.observe(1500, Some(&ev)), None);
        assert_eq!(w.observe(2000, Some(&ev)), None);
    }

    #[test]
    fn meeting_arriving_already_started_never_fires() {
        // Calendar collector delivers a meeting that is already underway (we
        // never saw it in the future) — no bogus "starting now".
        let mut w = MeetingStartWatch::new();
        assert_eq!(w.observe(500, None), None); // no data yet
        let ev = event("a", 100); // started at 100, now 500
        assert_eq!(w.observe(500, Some(&ev)), None);
    }

    #[test]
    fn meeting_advances_to_next_after_one_ends() {
        let mut w = MeetingStartWatch::new();
        let a = event("a", 1000);
        assert_eq!(w.observe(900, Some(&a)), None);
        assert!(w.observe(1000, Some(&a)).is_some()); // a starts

        // `a` ended; current-or-next is now `b`, seen in the future first.
        let b = event("b", 3000);
        assert_eq!(w.observe(2000, Some(&b)), None);
        let edge = w.observe(3000, Some(&b)).expect("b start edge");
        assert_eq!(edge.external_id, "b");
    }

    #[test]
    fn meeting_collector_refresh_does_not_refire() {
        // Same external_id, refreshed title/end after firing: no repeat.
        let mut w = MeetingStartWatch::new();
        let ev = event("a", 1000);
        assert_eq!(w.observe(900, Some(&ev)), None);
        assert!(w.observe(1000, Some(&ev)).is_some());

        let mut refreshed = event("a", 1000);
        refreshed.title = "Meeting a (updated)".to_string();
        refreshed.end_ts = Some(9_999_999);
        assert_eq!(w.observe(1100, Some(&refreshed)), None);
    }

    // --- ReviewRequestedWatch --------------------------------------------

    #[test]
    fn review_first_observation_primes_without_firing() {
        let mut w = ReviewRequestedWatch::new();
        let prs = vec![pr("me/repo", 1, REVIEW_REQUESTED)];
        // Already requested at launch: a level, not a flip.
        assert!(w.observe(&prs).is_empty());
        // Stays quiet while it holds.
        assert!(w.observe(&prs).is_empty());
    }

    #[test]
    fn review_flip_into_requested_fires_once() {
        let mut w = ReviewRequestedWatch::new();
        w.observe(&[pr("me/repo", 1, "approved")]);

        let edges = w.observe(&[pr("me/repo", 1, REVIEW_REQUESTED)]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].repo, "me/repo");
        assert_eq!(edges[0].number, 1);

        // Still requested next snapshot: no repeat.
        assert!(w.observe(&[pr("me/repo", 1, REVIEW_REQUESTED)]).is_empty());
    }

    #[test]
    fn review_refires_after_leaving_and_reentering() {
        let mut w = ReviewRequestedWatch::new();
        w.observe(&[pr("me/repo", 1, "approved")]);
        assert_eq!(w.observe(&[pr("me/repo", 1, REVIEW_REQUESTED)]).len(), 1);
        // Review submitted → leaves the set.
        assert!(w.observe(&[pr("me/repo", 1, "approved")]).is_empty());
        // Requested again → new edge.
        assert_eq!(w.observe(&[pr("me/repo", 1, REVIEW_REQUESTED)]).len(), 1);
    }

    #[test]
    fn review_only_fires_for_the_requested_state() {
        let mut w = ReviewRequestedWatch::new();
        w.observe(&[]); // prime empty
        let edges = w.observe(&[
            pr("me/repo", 1, "approved"),
            pr("me/repo", 2, ""),
            pr("me/repo", 3, REVIEW_REQUESTED),
        ]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].number, 3);
    }

    #[test]
    fn review_edge_survives_collector_refresh_without_refiring() {
        // The PRs collector replaces every row each tick; a still-requested PR
        // with an updated title must not re-fire.
        let mut w = ReviewRequestedWatch::new();
        w.observe(&[]);
        assert_eq!(w.observe(&[pr("me/repo", 7, REVIEW_REQUESTED)]).len(), 1);

        let mut refreshed = pr("me/repo", 7, REVIEW_REQUESTED);
        refreshed.title = "PR 7 (updated)".to_string();
        refreshed.updated_ts = 5000;
        assert!(w.observe(&[refreshed]).is_empty());
    }
}
