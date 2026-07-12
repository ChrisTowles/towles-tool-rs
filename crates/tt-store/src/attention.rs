//! Edge detection for day-model attention notifications.
//!
//! Three watchers turn successive store reads into *edges* — the moment
//! something newly deserves your attention — so the host fires a desktop
//! notification exactly once per event instead of on every tick. This mirrors
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
//! - [`StaleCollectorWatch`] fires when a collector silently stops succeeding —
//!   its last healthy run ages past a per-collector threshold, or it fails for
//!   [`FAIL_STREAK`] consecutive runs (expired `gh` auth, a revoked Slack
//!   token). Edge-triggered per collector and cleared on recovery, so a
//!   long-broken collector alerts once, not every tick, and re-alerts if it
//!   recovers and breaks again. Only collectors the host passes as
//!   [`WatchedCollector`]s are considered, so disabled collectors never fire.
//!
//! All time is injected as `now_ms` (epoch ms); nothing here reads a clock.

use std::collections::{HashMap, HashSet};

use crate::{CalEvent, CollectRun, PrItem};

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

/// How many consecutive failing runs a collector must post before it counts as
/// stale on the failure path. One transient `gh`/network hiccup shouldn't fire;
/// a persistently broken collector (expired auth, revoked token) will.
pub const FAIL_STREAK: u32 = 3;

/// A collector whose freshness the host wants watched, and how long its last
/// healthy run may age before it counts as stale. The host builds one per
/// *enabled* collector (disabled ones are simply omitted, so they never fire)
/// and derives `stale_after_ms` from that collector's refresh cadence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedCollector {
    /// The `record_run` key (`prs`, `issues`, `claude:calendar`, `slack:dm`).
    pub key: String,
    /// Age of the last successful run beyond which the collector is stale (ms).
    pub stale_after_ms: i64,
}

/// One collector that just crossed from healthy to stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleCollectorEdge {
    /// The collector key that went stale.
    pub key: String,
    /// How long since its last healthy run (ms). For a collector that has only
    /// ever failed on our watch, this is the age of its latest run.
    pub stale_for_ms: i64,
    /// The failing run's message, when the stale state is a failure streak
    /// rather than pure staleness (e.g. the `gh`/Slack error).
    pub last_message: Option<String>,
}

/// Per-collector edge state carried across observations.
#[derive(Debug, Default)]
struct CollectorState {
    /// `ran_at` of the last run row we folded in, to detect a *new* run (rows
    /// are upserted, so `ran_at` changing is the only new-run signal).
    last_seen_ran_at: Option<i64>,
    /// `ran_at` of the most recent `ok == true` run we observed — the freshness
    /// baseline the age threshold is measured against.
    last_ok_at: Option<i64>,
    /// Consecutive failing runs observed (reset by any healthy run).
    fail_streak: u32,
    /// Whether the collector was stale as of the previous observation, for edge
    /// detection.
    prev_stale: bool,
    /// False until the first observation has primed the baseline, so a collector
    /// already stale at launch doesn't fire until it recovers and breaks again.
    primed: bool,
}

/// Tracks each watched collector's run history across snapshots and yields an
/// edge the tick a collector crosses healthy → stale. Stale means either its
/// last successful run has aged past `stale_after_ms`, or it has failed for
/// [`FAIL_STREAK`] consecutive runs. Edge-triggered per collector: a collector
/// that stays stale never repeats, and recovery (a fresh healthy run) clears the
/// state so a later break fires again. Collectors absent from the watched list
/// (disabled in settings) are dropped, so they never fire and re-enabling
/// re-primes from a clean baseline.
#[derive(Debug, Default)]
pub struct StaleCollectorWatch {
    by_key: HashMap<String, CollectorState>,
}

impl StaleCollectorWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold in the current run rows and return collectors that newly went stale.
    /// `watched` names the collectors to consider (one per enabled collector)
    /// and their per-collector thresholds; `runs` is the full `collect_runs`
    /// table. Updates the baseline as a side effect.
    pub fn observe(
        &mut self,
        now_ms: i64,
        watched: &[WatchedCollector],
        runs: &[CollectRun],
    ) -> Vec<StaleCollectorEdge> {
        // Drop state for collectors no longer watched (disabled since last tick)
        // so a re-enable re-primes rather than replaying a stale edge.
        let live: HashSet<&str> = watched.iter().map(|w| w.key.as_str()).collect();
        self.by_key.retain(|k, _| live.contains(k.as_str()));

        let mut edges = Vec::new();
        for w in watched {
            // No run row yet: nothing has executed, so there's nothing to call
            // stale (and no baseline to age against). Leave it unprimed.
            let Some(run) = runs.iter().find(|r| r.collector == w.key) else {
                continue;
            };
            let state = self.by_key.entry(w.key.clone()).or_default();

            // Fold a *new* run (upserts mean a changed `ran_at` is the signal)
            // into the freshness baseline and failure streak.
            if state.last_seen_ran_at != Some(run.ran_at) {
                state.last_seen_ran_at = Some(run.ran_at);
                if run.ok {
                    state.last_ok_at = Some(run.ran_at);
                    state.fail_streak = 0;
                } else {
                    state.fail_streak += 1;
                }
            }

            let by_failure = state.fail_streak >= FAIL_STREAK;
            // Age the last healthy run; with no observed success, only the
            // failure path can fire (we have no freshness baseline).
            let by_age = state.last_ok_at.is_some_and(|ok_at| now_ms - ok_at > w.stale_after_ms);
            let stale = by_failure || by_age;

            if state.primed && !state.prev_stale && stale {
                let stale_for_ms = match state.last_ok_at {
                    Some(ok_at) => now_ms - ok_at,
                    None => now_ms - run.ran_at,
                };
                let last_message = if run.ok { None } else { run.message.clone() };
                edges.push(StaleCollectorEdge { key: w.key.clone(), stale_for_ms, last_message });
            }
            state.prev_stale = stale;
            state.primed = true;
        }
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

    // --- StaleCollectorWatch ---------------------------------------------

    fn run(collector: &str, ok: bool, ran_at: i64, message: Option<&str>) -> CollectRun {
        CollectRun {
            collector: collector.to_string(),
            ran_at,
            ok,
            message: message.map(str::to_string),
        }
    }

    /// One minute of epoch ms, for readable thresholds/ages in the tests.
    const MIN: i64 = 60_000;

    fn watched(key: &str, stale_after_ms: i64) -> Vec<WatchedCollector> {
        vec![WatchedCollector { key: key.to_string(), stale_after_ms }]
    }

    #[test]
    fn stale_by_age_fires_once_then_recovers_and_refires() {
        let mut w = StaleCollectorWatch::new();
        let cfg = watched("issues", 30 * MIN);

        // Healthy at launch: primes the baseline, no edge.
        let healthy = [run("issues", true, 0, None)];
        assert!(w.observe(0, &cfg, &healthy).is_empty());
        // Still fresh a bit later: quiet.
        assert!(w.observe(20 * MIN, &cfg, &healthy).is_empty());

        // Last healthy run has now aged past 30m with no refresh: one edge.
        let edges = w.observe(32 * MIN, &cfg, &healthy);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].key, "issues");
        assert_eq!(edges[0].stale_for_ms, 32 * MIN);
        assert!(edges[0].last_message.is_none());

        // Stays stale on later ticks: no repeat.
        assert!(w.observe(40 * MIN, &cfg, &healthy).is_empty());
        assert!(w.observe(90 * MIN, &cfg, &healthy).is_empty());

        // A fresh healthy run recovers it (clears stale) without firing.
        let recovered = [run("issues", true, 95 * MIN, None)];
        assert!(w.observe(95 * MIN, &cfg, &recovered).is_empty());

        // Going stale again fires a second edge.
        let edges = w.observe(130 * MIN, &cfg, &recovered);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].stale_for_ms, 35 * MIN);
    }

    #[test]
    fn stale_by_failure_streak_fires_after_n_consecutive_failures() {
        let mut w = StaleCollectorWatch::new();
        let cfg = watched("prs", 30 * MIN);

        // Prime healthy.
        assert!(w.observe(0, &cfg, &[run("prs", true, 0, None)]).is_empty());

        // Distinct failing runs (each a new `ran_at`) build the streak. The
        // threshold is FAIL_STREAK = 3; the first two stay quiet.
        assert!(
            w.observe(MIN, &cfg, &[run("prs", false, MIN, Some("gh auth expired"))]).is_empty()
        );
        assert!(
            w.observe(2 * MIN, &cfg, &[run("prs", false, 2 * MIN, Some("gh auth expired"))])
                .is_empty()
        );
        let edges =
            w.observe(3 * MIN, &cfg, &[run("prs", false, 3 * MIN, Some("gh auth expired"))]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].key, "prs");
        assert_eq!(edges[0].last_message.as_deref(), Some("gh auth expired"));

        // Still failing: no repeat.
        assert!(
            w.observe(4 * MIN, &cfg, &[run("prs", false, 4 * MIN, Some("gh auth expired"))])
                .is_empty()
        );

        // Recovery clears the streak.
        assert!(w.observe(5 * MIN, &cfg, &[run("prs", true, 5 * MIN, None)]).is_empty());
    }

    #[test]
    fn repeated_ticks_on_the_same_failing_run_do_not_advance_the_streak() {
        // The notify loop ticks faster than the collector runs, so the same
        // failing row is observed many times. Only *distinct* runs count.
        let mut w = StaleCollectorWatch::new();
        let cfg = watched("slack:dm", 30 * MIN);
        assert!(w.observe(0, &cfg, &[run("slack:dm", true, 0, None)]).is_empty());

        let failing = [run("slack:dm", false, MIN, Some("invalid_auth"))];
        for t in 1..10 {
            assert!(
                w.observe(t * MIN, &cfg, &failing).is_empty(),
                "one failing run must never reach the streak threshold"
            );
        }
    }

    #[test]
    fn already_stale_at_launch_does_not_fire_until_it_recovers_and_breaks_again() {
        // First observation only primes: a collector broken before we started
        // watching shouldn't spam on launch.
        let mut w = StaleCollectorWatch::new();
        let cfg = watched("issues", 30 * MIN);
        // Last healthy run is 2h old at first sight — already stale, but primed
        // silently.
        let stale_row = [run("issues", true, 0, None)];
        assert!(w.observe(120 * MIN, &cfg, &stale_row).is_empty());
        // Stays quiet while stale.
        assert!(w.observe(130 * MIN, &cfg, &stale_row).is_empty());
        // Recovers, then breaks again → now it fires.
        let recovered = [run("issues", true, 130 * MIN, None)];
        assert!(w.observe(130 * MIN, &cfg, &recovered).is_empty());
        assert_eq!(w.observe(165 * MIN, &cfg, &recovered).len(), 1);
    }

    #[test]
    fn disabled_collector_never_fires_and_re_enabling_reprimes() {
        let mut w = StaleCollectorWatch::new();
        let runs = [run("issues", true, 0, None)];

        // `issues` is not in the watched list (disabled): even a long-stale run
        // yields no edge.
        let none: Vec<WatchedCollector> = vec![];
        assert!(w.observe(200 * MIN, &none, &runs).is_empty());
        assert!(w.observe(400 * MIN, &none, &runs).is_empty());

        // Enable it now: the first observation re-primes (no launch spam) even
        // though the run is already ancient.
        let cfg = watched("issues", 30 * MIN);
        assert!(w.observe(400 * MIN, &cfg, &runs).is_empty());
    }

    #[test]
    fn watches_each_collector_independently() {
        let mut w = StaleCollectorWatch::new();
        let cfg = vec![
            WatchedCollector { key: "prs".to_string(), stale_after_ms: 8 * MIN },
            WatchedCollector { key: "issues".to_string(), stale_after_ms: 30 * MIN },
        ];
        let runs = [run("prs", true, 0, None), run("issues", true, 0, None)];
        assert!(w.observe(0, &cfg, &runs).is_empty());

        // At 10m only `prs` (8m threshold) is stale; `issues` (30m) is fine.
        let edges = w.observe(10 * MIN, &cfg, &runs);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].key, "prs");
    }

    #[test]
    fn no_run_row_yet_stays_quiet() {
        // A watched collector that has never posted a run must not fire (nothing
        // to age, no failure to count).
        let mut w = StaleCollectorWatch::new();
        let cfg = watched("issues", 30 * MIN);
        assert!(w.observe(0, &cfg, &[]).is_empty());
        assert!(w.observe(500 * MIN, &cfg, &[]).is_empty());
    }
}
