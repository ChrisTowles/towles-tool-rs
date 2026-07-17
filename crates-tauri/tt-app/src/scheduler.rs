//! Collector scheduler: fills tt.db while the app is open.
//!
//! Cadence comes from `settings.collectors`, one tick per collector: PRs via
//! `gh` every `prs.refresh_seconds`; issues via `gh` every
//! `issues.refresh_minutes`; calendar via `claude -p` every
//! `calendar.refresh_minutes`, gated by `calendar.enabled` because those runs
//! cost tokens. Each batch runs in a blocking task on its **own** store
//! connection (tt-store opens WAL + busy-timeout), so a slow `claude -p` never
//! holds the UI's store mutex. After every batch the fresh snapshot is emitted
//! as `store://snapshot`.
//!
//! A `prs` or `issues` batch can also fire early, outside its normal cadence:
//! see the nudge-dir watch in [`spawn`], which reacts to `tt collect nudge
//! prs`/`tt collect nudge issues` by diffing each target's file mtime (see
//! [`changed_nudge_batches`]).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;
use tt_collect::CalendarProvider;
use tt_store::{
    ChecksFailedWatch, MeetingStartWatch, ReviewRequestedWatch, StaleCollectorWatch,
    WatchedCollector,
};

use crate::store::SNAPSHOT_EVENT;

/// How often the day-model attention watchers check for a meeting whose
/// countdown reached zero or a PR that newly entered the review-requested set.
/// Independent of collector cadence (those can be slow or off): this reads the
/// data already in tt.db, so the notification fires within one tick of the
/// event regardless of when the data was last refreshed.
const NOTIFY_TICK_SECS: u64 = 15;

/// A collector counts as stale once its last healthy run has aged past this
/// many times its own refresh cadence — enough missed cycles that it's clearly
/// stuck, not just one skipped tick.
const STALE_CADENCE_MULT: i64 = 4;

/// Floor for a collector's staleness threshold, so a fast collector (Slack every
/// minute) still gets a few minutes of grace before it alarms.
const STALE_FLOOR_MS: i64 = 5 * 60_000;

/// Build the [`WatchedCollector`] list for the stale-collector watch from the
/// current settings: one entry per *enabled* collector (disabled ones are
/// omitted, so they never fire), with a per-collector staleness threshold
/// derived from that collector's refresh cadence.
fn watched_collectors(collectors: &tt_config::CollectorsSettings) -> Vec<WatchedCollector> {
    fn threshold(cadence_ms: i64) -> i64 {
        (cadence_ms * STALE_CADENCE_MULT).max(STALE_FLOOR_MS)
    }
    let mut watched = Vec::new();
    if collectors.prs.enabled {
        watched.push(WatchedCollector {
            key: "prs".to_string(),
            stale_after_ms: threshold(collectors.prs.refresh_seconds.max(30) as i64 * 1000),
        });
    }
    if collectors.issues.enabled {
        watched.push(WatchedCollector {
            key: "issues".to_string(),
            stale_after_ms: threshold(collectors.issues.refresh_minutes.max(1) as i64 * 60_000),
        });
    }
    if collectors.calendar.enabled {
        watched.push(WatchedCollector {
            key: "claude:calendar".to_string(),
            stale_after_ms: threshold(collectors.calendar.refresh_minutes.max(1) as i64 * 60_000),
        });
    }
    if collectors.slack.enabled {
        watched.push(WatchedCollector {
            key: "slack:dm".to_string(),
            stale_after_ms: threshold(collectors.slack.refresh_seconds.max(30) as i64 * 1000),
        });
    }
    watched
}

#[derive(Clone)]
enum Batch {
    Prs,
    Issues,
    Calendar,
    SlackDm(tt_collect::SlackDmConfig),
}

/// One in-flight flag per collector. A batch is fire-and-forget spawned so the
/// select! loop never parks on it (a calendar run can take the full
/// `CLAUDE_TIMEOUT`); the flag makes sure a slow run doesn't stack a second run
/// of the *same* collector on the next tick. Persisted across settings reloads
/// (like the attention watchers) so an in-flight batch is still tracked after a
/// cadence rebuild.
#[derive(Default)]
struct BatchGuards {
    prs: Arc<AtomicBool>,
    issues: Arc<AtomicBool>,
    calendar: Arc<AtomicBool>,
    slack: Arc<AtomicBool>,
}

/// Try to claim a collector's in-flight slot for this tick. Returns `true` when
/// the slot was free (now marked in-flight, caller runs the batch and must
/// release it when done); `false` when a previous run is still ongoing and this
/// tick should be skipped. A pure CAS on the guard — the only decision the
/// fire-and-forget path makes, unit-tested below.
fn claim_in_flight(guard: &AtomicBool) -> bool {
    guard.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// Epoch milliseconds from the local wall clock (scheduler boundary clock).
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Last-observed mtime of each nudge-dir file, so [`changed_nudge_batches`]
/// can tell *which* target actually changed instead of eagerly refreshing
/// `prs` on any touch to the directory (the dir is watched non-recursively
/// per-file-unaware — see `tt_agentboard::fs_notify::DirNotifier`). Persists
/// across settings-reload rebuilds, like the batch guards.
#[derive(Default)]
struct NudgeSeen {
    prs: Option<SystemTime>,
    issues: Option<SystemTime>,
}

/// Given a debounced "something in the nudge dir changed" wakeup, diff each
/// target's file mtime against what was last seen and return the batches
/// whose file actually advanced. Pure and file-mtime-only (no store access),
/// so it's cheap to call on every nudge and easy to unit test with a tempdir.
fn changed_nudge_batches(dir: &Path, seen: &mut NudgeSeen) -> Vec<Batch> {
    let mut changed = Vec::new();
    for (file_name, batch, last) in [
        ("prs", Batch::Prs, &mut seen.prs),
        ("issues", Batch::Issues, &mut seen.issues),
    ] {
        let mtime = std::fs::metadata(dir.join(file_name)).and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime != *last {
            changed.push(batch);
        }
        *last = mtime;
    }
    changed
}

/// Spawn the scheduler loop. Collector cadence/enable/provider are re-read from
/// settings whenever `reload` is signalled (the `settings_set` command fires it),
/// so edits in the Settings screen take effect live — no relaunch needed.
pub fn spawn(app: AppHandle, reload: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        // Attention watchers persist across settings reloads: their edge state
        // (which meeting is being tracked, which PRs are already requested) must
        // survive a cadence rebuild so a reload never re-fires a stale edge.
        let mut meeting_watch = MeetingStartWatch::new();
        let mut review_watch = ReviewRequestedWatch::new();
        let mut checks_watch = ChecksFailedWatch::new();
        let mut stale_watch = StaleCollectorWatch::new();
        // In-flight guards also persist across reloads: a batch spawned under the
        // old cadence must still block a duplicate under the new one.
        let guards = BatchGuards::default();
        // Eager-refresh accelerant: an external process (the `towles-tool-app`
        // Claude Code plugin's PostToolUse hook, via `tt collect nudge prs`/
        // `tt collect nudge issues`) can touch a file in the nudge dir right
        // after a `gh pr`/`gh issue` mutation to make that view update well
        // before its next scheduled tick. Same debounced fs-notify pattern as
        // the agentboard journal watch in `lib.rs` — `.ok()` so a watch
        // failure (e.g. inotify limits) just falls back to the normal poll
        // cadence instead of breaking startup. The watch itself only signals
        // "something in the dir changed"; `changed_nudge_batches` below
        // resolves that into which target(s) actually advanced.
        let nudge_dir = tt_config::nudge_dir_path().ok();
        let nudge_notify = Arc::new(Notify::new());
        let _nudge_watcher = nudge_dir.as_ref().and_then(|dir| {
            std::fs::create_dir_all(dir).ok()?;
            let n = nudge_notify.clone();
            tt_agentboard::fs_notify::DirNotifier::watch(dir, move || n.notify_one()).ok()
        });
        let mut nudge_seen = NudgeSeen::default();
        loop {
            // (Re)load config and rebuild the tick intervals for this cycle.
            let collectors = tt_config::load().map(|s| s.collectors).unwrap_or_default();
            // Rebuilt each cycle so enable/cadence edits change what's watched;
            // the watch's edge state persists across the rebuild.
            let watched = watched_collectors(&collectors);
            let provider = CalendarProvider::from_str_lenient(&collectors.calendar.provider);
            let calendar_period_ms = collectors.calendar.refresh_minutes.max(1) as i64 * 60_000;

            let mut pr_tick =
                tokio::time::interval(Duration::from_secs(collectors.prs.refresh_seconds.max(30)));
            pr_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut issue_tick = tokio::time::interval(Duration::from_secs(
                collectors.issues.refresh_minutes.max(1) * 60,
            ));
            issue_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut calendar_tick =
                tokio::time::interval(Duration::from_millis(calendar_period_ms as u64));
            calendar_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut slack_tick = tokio::time::interval(Duration::from_secs(
                collectors.slack.refresh_seconds.max(30),
            ));
            slack_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Fixed cadence, independent of any collector's enable/refresh: the
            // attention watchers only read existing tt.db rows.
            let mut notify_tick = tokio::time::interval(Duration::from_secs(NOTIFY_TICK_SECS));
            notify_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Enabled but unconfigured stays quiet: without a token every tick
            // would just record the same failure.
            let slack_on = collectors.slack.enabled && !collectors.slack.token.trim().is_empty();
            let slack_config = tt_collect::SlackDmConfig {
                token: collectors.slack.token.clone(),
                watch_user_id: collectors.slack.watch_user_id.clone(),
                watch_name: collectors.slack.watch_name.clone(),
            };

            // Inner loop runs the current cadence until settings change, then
            // breaks to rebuild from the top with the new values.
            loop {
                tokio::select! {
                    _ = reload.notified() => break,
                    _ = pr_tick.tick(), if collectors.prs.enabled => {
                        spawn_batch(&app, Batch::Prs, provider, calendar_period_ms, &guards.prs);
                    }
                    _ = issue_tick.tick(), if collectors.issues.enabled => {
                        spawn_batch(&app, Batch::Issues, provider, calendar_period_ms, &guards.issues);
                    }
                    _ = calendar_tick.tick(), if collectors.calendar.enabled => {
                        // Quiet-hours gate: outside the configured working-hours
                        // window (nights/weekends) skip the token-costing
                        // `claude -p` run entirely. Evaluated per tick against the
                        // local wall clock; disabling quiet hours restores 24/7.
                        if tt_collect::should_run_calendar(
                            now_ms(),
                            &collectors.calendar.quiet_hours,
                        ) {
                            spawn_batch(
                                &app,
                                Batch::Calendar,
                                provider,
                                calendar_period_ms,
                                &guards.calendar,
                            );
                        }
                    }
                    _ = slack_tick.tick(), if slack_on => {
                        spawn_batch(
                            &app,
                            Batch::SlackDm(slack_config.clone()),
                            provider,
                            calendar_period_ms,
                            &guards.slack,
                        );
                    }
                    _ = nudge_notify.notified() => {
                        if let Some(dir) = &nudge_dir {
                            for batch in changed_nudge_batches(dir, &mut nudge_seen) {
                                let guard = match batch {
                                    Batch::Prs if collectors.prs.enabled => &guards.prs,
                                    Batch::Issues if collectors.issues.enabled => &guards.issues,
                                    _ => continue,
                                };
                                spawn_batch(&app, batch, provider, calendar_period_ms, guard);
                            }
                        }
                    }
                    _ = notify_tick.tick() => {
                        run_notify_check(
                            &app,
                            &mut meeting_watch,
                            &mut review_watch,
                            &mut checks_watch,
                            &mut stale_watch,
                            &watched,
                        )
                        .await;
                    }
                }
            }
        }
    });
}

/// Fire-and-forget a collect batch so the select! loop stays hot. If the
/// collector's previous run is still in flight the tick is skipped; otherwise the
/// guard is claimed, the batch runs on its own task (blocking work stays on the
/// blocking pool inside `run_batch`), and the guard is released when it finishes.
fn spawn_batch(
    app: &AppHandle,
    batch: Batch,
    provider: CalendarProvider,
    calendar_period_ms: i64,
    guard: &Arc<AtomicBool>,
) {
    if !claim_in_flight(guard) {
        return;
    }
    let app = app.clone();
    let guard = guard.clone();
    tauri::async_runtime::spawn(async move {
        run_batch(&app, batch, provider, calendar_period_ms).await;
        guard.store(false, Ordering::Release);
    });
}

async fn run_batch(
    app: &AppHandle,
    batch: Batch,
    provider: CalendarProvider,
    calendar_period_ms: i64,
) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        run_batch_blocking(&app, batch, provider, calendar_period_ms)
    })
    .await;
}

fn run_batch_blocking(
    app: &AppHandle,
    batch: Batch,
    provider: CalendarProvider,
    calendar_period_ms: i64,
) {
    // Data collected while the window is minimized has no audience; skip the
    // subprocess sweep (and, for calendar, the claude tokens). The next tick
    // after a restore refreshes everything.
    if main_window_minimized(app) {
        return;
    }

    let store = match tt_store::Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            eprintln!("scheduler: store unavailable ({e}); skipping collect batch");
            return;
        }
    };
    let now = now_ms();

    match batch {
        Batch::Prs => {
            let repos = tt_collect::tracked_repo_dirs();
            log_failure(tt_collect::collect_prs(&store, &repos, now));
        }
        Batch::Issues => {
            let repos = tt_collect::tracked_repo_dirs();
            log_failure(tt_collect::collect_issues(&store, &repos, now));
        }
        Batch::Calendar => {
            // Token-cost guard: an interval's first tick fires at startup, so a
            // relaunch inside half a refresh period would re-bill `claude -p`
            // for data we already have — and a run that just FAILED must back
            // off rather than re-bill immediately on the next relaunch, so any
            // recorded run (ok or not) counts as fresh here.
            if claude_ran_within(&store, now, calendar_period_ms / 2) {
                return;
            }
            log_failure(tt_collect::collect_calendar(&store, provider, now));
        }
        Batch::SlackDm(config) => {
            log_failure(tt_collect::collect_slack_dm(&store, &config, now));
        }
    }

    if let Ok(snapshot) = store.snapshot() {
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
    }
}

/// Check the day-model attention watchers against the current tt.db state and
/// fire notifications for any new edges. The SQLite reads run on a blocking
/// worker (tt.db is shared, and a busy db can block); the watchers' edge state
/// lives on the async side so it survives across ticks. Unlike the collector
/// batches this runs even while the window is minimized — an unattended window
/// is exactly when a desktop notification matters.
async fn run_notify_check(
    app: &AppHandle,
    meeting_watch: &mut MeetingStartWatch,
    review_watch: &mut ReviewRequestedWatch,
    checks_watch: &mut ChecksFailedWatch,
    stale_watch: &mut StaleCollectorWatch,
    watched: &[WatchedCollector],
) {
    let read = tauri::async_runtime::spawn_blocking(|| {
        let store = tt_store::Store::open_default().ok()?;
        let now = now_ms();
        let next = store.current_or_next_event(now).ok().flatten();
        let prs = store.prs().unwrap_or_default();
        let runs = store.runs().unwrap_or_default();
        Some((now, next, prs, runs))
    })
    .await;

    let Ok(Some((now, next, prs, runs))) = read else {
        return;
    };

    if let Some(edge) = meeting_watch.observe(now, next.as_ref()) {
        notify_meeting_start(app, &edge);
    }
    for edge in review_watch.observe(&prs) {
        notify_review_requested(app, &edge);
    }
    for edge in checks_watch.observe(&prs) {
        notify_checks_failed(app, &edge);
    }
    for edge in stale_watch.observe(now, watched, &runs) {
        notify_stale_collector(app, &edge);
    }
}

/// Fire a "meeting starting now" desktop notification. Suppressed when the main
/// window is focused (the header countdown already shows it) or when the
/// `agentboard.notifyMeetingStart` setting is off (default on).
fn notify_meeting_start(app: &AppHandle, edge: &tt_store::MeetingStartEdge) {
    use tauri_plugin_notification::NotificationExt;

    if window_focused(app) {
        return;
    }
    let enabled = tt_config::load()
        .map(|s| {
            s.agentboard.notify_meeting_start.unwrap_or(tt_config::DEFAULT_NOTIFY_MEETING_START)
        })
        .unwrap_or(tt_config::DEFAULT_NOTIFY_MEETING_START);
    if !enabled {
        return;
    }
    let _ = app.notification().builder().title("Meeting starting now").body(&edge.title).show();
}

/// Fire a "PR review requested" desktop notification. Suppressed when the main
/// window is focused (the day bar already shows review-requested PRs) or when
/// the `agentboard.notifyReviewRequested` setting is off (default on).
fn notify_review_requested(app: &AppHandle, edge: &tt_store::ReviewRequestedEdge) {
    use tauri_plugin_notification::NotificationExt;

    if window_focused(app) {
        return;
    }
    let enabled = tt_config::load()
        .map(|s| {
            s.agentboard
                .notify_review_requested
                .unwrap_or(tt_config::DEFAULT_NOTIFY_REVIEW_REQUESTED)
        })
        .unwrap_or(tt_config::DEFAULT_NOTIFY_REVIEW_REQUESTED);
    if !enabled {
        return;
    }
    let _ = app
        .notification()
        .builder()
        .title(format!("Review requested — {}#{}", edge.repo, edge.number))
        .body(&edge.title)
        .show();
}

/// Fire a "CI failing" desktop notification when one of your PRs' checks flip
/// into failing. Suppressed when the main window is focused (the day bar already
/// surfaces PR check state) or when the `agentboard.notifyChecksFailed` setting
/// is off (default on).
fn notify_checks_failed(app: &AppHandle, edge: &tt_store::ChecksFailedEdge) {
    use tauri_plugin_notification::NotificationExt;

    if window_focused(app) {
        return;
    }
    let enabled = tt_config::load()
        .map(|s| {
            s.agentboard.notify_checks_failed.unwrap_or(tt_config::DEFAULT_NOTIFY_CHECKS_FAILED)
        })
        .unwrap_or(tt_config::DEFAULT_NOTIFY_CHECKS_FAILED);
    if !enabled {
        return;
    }
    let _ = app
        .notification()
        .builder()
        .title(format!("CI failing on {}#{}", edge.repo, edge.number))
        .body(&edge.title)
        .show();
}

/// Fire a "collector went stale" desktop notification — a collector stopped
/// refreshing or is failing repeatedly (expired `gh` auth, revoked Slack token).
/// Unlike the meeting/review notifications this is *not* suppressed while the
/// window is focused: there's no always-on in-app surface for collector health,
/// so a focused user would otherwise never learn a collector died. Gated only by
/// the `agentboard.notifyStaleCollector` setting (default on).
fn notify_stale_collector(app: &AppHandle, edge: &tt_store::StaleCollectorEdge) {
    use tauri_plugin_notification::NotificationExt;

    let enabled = tt_config::load()
        .map(|s| {
            s.agentboard.notify_stale_collector.unwrap_or(tt_config::DEFAULT_NOTIFY_STALE_COLLECTOR)
        })
        .unwrap_or(tt_config::DEFAULT_NOTIFY_STALE_COLLECTOR);
    if !enabled {
        return;
    }

    let name = collector_label(&edge.key);
    let mut body =
        format!("{name} collector hasn't refreshed in {}", human_duration(edge.stale_for_ms));
    if let Some(msg) = &edge.last_message {
        body.push_str(&format!(" — {msg}"));
    }
    let _ = app.notification().builder().title("Collector went stale").body(body).show();
}

/// Human collector name for a notification body, from its `record_run` key.
fn collector_label(key: &str) -> &str {
    match key {
        "prs" => "PRs",
        "issues" => "issues",
        "claude:calendar" => "calendar",
        "slack:dm" => "Slack",
        other => other,
    }
}

/// Render an elapsed duration (ms) as a compact `Nh`/`Nm`/`Ns` string for a
/// notification body, rounding down to the largest whole unit.
fn human_duration(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Whether the main window currently reports itself focused. Unknown states
/// (no window, backend error) count as not-focused so a notification still
/// fires rather than being silently swallowed.
fn window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false)
}

/// Whether the main window currently reports itself minimized. Unknown states
/// (no window, backend error) count as visible so collection never silently
/// starves.
fn main_window_minimized(app: &AppHandle) -> bool {
    app.get_webview_window("main").map(|w| w.is_minimized().unwrap_or(false)).unwrap_or(false)
}

fn log_failure(summary: tt_collect::CollectSummary) {
    if !summary.ok {
        eprintln!(
            "scheduler: {} collect failed: {}",
            summary.collector,
            summary.message.as_deref().unwrap_or("unknown")
        );
    }
}

/// Whether any `claude:*` collector run — successful or not — is younger than
/// `max_age_ms`. Failed runs count: a claude invocation that burned tokens and
/// then failed parsing must not be retried at relaunch speed.
fn claude_ran_within(store: &tt_store::Store, now: i64, max_age_ms: i64) -> bool {
    match store.runs() {
        Ok(runs) => runs
            .iter()
            .filter(|r| r.collector.starts_with("claude:"))
            .any(|r| now - r.ran_at < max_age_ms),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_in_flight_claims_a_free_slot_then_blocks_until_released() {
        let guard = AtomicBool::new(false);
        // First tick claims the free slot.
        assert!(claim_in_flight(&guard), "a free slot is claimable");
        // A second tick while the batch is still running is skipped.
        assert!(!claim_in_flight(&guard), "an in-flight slot is not re-claimable");
        assert!(!claim_in_flight(&guard), "repeated ticks keep skipping");
        // Once the batch releases the guard, the next tick claims it again.
        guard.store(false, Ordering::Release);
        assert!(claim_in_flight(&guard), "a released slot is claimable again");
    }

    #[test]
    fn claim_in_flight_is_independent_per_guard() {
        let prs = AtomicBool::new(false);
        let calendar = AtomicBool::new(false);
        // A slow calendar run holding its guard must not block the PR tick.
        assert!(claim_in_flight(&calendar), "calendar claims its own slot");
        assert!(claim_in_flight(&prs), "prs is unaffected by calendar in-flight");
        assert!(!claim_in_flight(&calendar), "calendar still in flight");
    }

    fn batch_names(batches: &[Batch]) -> Vec<&'static str> {
        batches
            .iter()
            .map(|b| match b {
                Batch::Prs => "prs",
                Batch::Issues => "issues",
                Batch::Calendar => "calendar",
                Batch::SlackDm(_) => "slack",
            })
            .collect()
    }

    #[test]
    fn changed_nudge_batches_detects_a_freshly_written_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = NudgeSeen::default();
        assert!(changed_nudge_batches(dir.path(), &mut seen).is_empty(), "nothing written yet");

        std::fs::write(dir.path().join("prs"), "1").unwrap();
        assert_eq!(batch_names(&changed_nudge_batches(dir.path(), &mut seen)), vec!["prs"]);
        // Already acknowledged: polling again with no further touch is quiet.
        assert!(changed_nudge_batches(dir.path(), &mut seen).is_empty());
    }

    #[test]
    fn changed_nudge_batches_tracks_each_target_independently() {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = NudgeSeen::default();
        std::fs::write(dir.path().join("issues"), "1").unwrap();
        assert_eq!(batch_names(&changed_nudge_batches(dir.path(), &mut seen)), vec!["issues"]);

        std::fs::write(dir.path().join("prs"), "1").unwrap();
        assert_eq!(
            batch_names(&changed_nudge_batches(dir.path(), &mut seen)),
            vec!["prs"],
            "issues was already acknowledged in the prior poll"
        );
    }

    #[test]
    fn changed_nudge_batches_fires_again_on_a_later_re_touch() {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = NudgeSeen::default();
        let path = dir.path().join("prs");
        std::fs::write(&path, "1").unwrap();
        changed_nudge_batches(dir.path(), &mut seen);

        // A second `gh pr merge` before the app's next poll re-touches the
        // same file; only the mtime is ever read (content is just a
        // debugging aid), so bump it explicitly rather than relying on
        // filesystem mtime resolution across two quick writes.
        let file = std::fs::File::open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(batch_names(&changed_nudge_batches(dir.path(), &mut seen)), vec!["prs"]);
    }

    #[test]
    fn claude_ran_within_counts_only_claude_collectors() {
        let store = tt_store::Store::open_in_memory().unwrap();
        store.record_run("prs", true, None, 90).unwrap();
        store.record_run("issues", true, None, 95).unwrap();
        assert!(!claude_ran_within(&store, 100, 50), "gh collectors never suppress calendar");

        store.record_run("claude:calendar", true, None, 90).unwrap();
        assert!(claude_ran_within(&store, 100, 50));
    }

    #[test]
    fn claude_ran_within_respects_the_age_window() {
        let store = tt_store::Store::open_in_memory().unwrap();
        store.record_run("claude:calendar", true, None, 10).unwrap();
        assert!(claude_ran_within(&store, 40, 50), "run 30ms old, window 50ms");
        assert!(!claude_ran_within(&store, 100, 50), "run 90ms old, window 50ms");
    }

    #[test]
    fn claude_ran_within_counts_failed_runs_as_backoff() {
        let store = tt_store::Store::open_in_memory().unwrap();
        store.record_run("claude:calendar", false, Some("no parseable JSON"), 90).unwrap();
        assert!(claude_ran_within(&store, 100, 50), "a failed paid run still suppresses retry");
    }

    #[test]
    fn claude_ran_within_is_not_fresh_on_query_error_or_empty() {
        let store = tt_store::Store::open_in_memory().unwrap();
        assert!(!claude_ran_within(&store, 100, 50));
    }

    #[test]
    fn watched_collectors_skips_disabled_and_derives_thresholds() {
        let mut c = tt_config::CollectorsSettings::default();
        // Defaults: prs+issues enabled, calendar+slack disabled.
        c.calendar.enabled = false;
        c.slack.enabled = false;
        let watched = watched_collectors(&c);
        let keys: Vec<&str> = watched.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["prs", "issues"]);
        // issues: 5m cadence * 4 = 20m.
        let issues = watched.iter().find(|w| w.key == "issues").unwrap();
        assert_eq!(issues.stale_after_ms, 20 * 60_000);
        // prs: 120s cadence * 4 = 8m.
        let prs = watched.iter().find(|w| w.key == "prs").unwrap();
        assert_eq!(prs.stale_after_ms, 8 * 60_000);
    }

    #[test]
    fn watched_collectors_thresholds_honor_the_floor() {
        let mut c = tt_config::CollectorsSettings::default();
        c.slack.enabled = true;
        c.slack.refresh_seconds = 30; // 30s * 4 = 2m, below the 5m floor.
        let watched = watched_collectors(&c);
        let slack = watched.iter().find(|w| w.key == "slack:dm").unwrap();
        assert_eq!(slack.stale_after_ms, STALE_FLOOR_MS);
    }

    #[test]
    fn human_duration_rounds_to_largest_unit() {
        assert_eq!(human_duration(45_000), "45s");
        assert_eq!(human_duration(32 * 60_000), "32m");
        assert_eq!(human_duration(2 * 3_600_000 + 60_000), "2h");
        assert_eq!(human_duration(-5), "0s");
    }
}
