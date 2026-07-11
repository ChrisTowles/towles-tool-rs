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

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;
use tt_collect::CalendarProvider;

use crate::store::SNAPSHOT_EVENT;

#[derive(Clone)]
enum Batch {
    Prs,
    Issues,
    Calendar,
    SlackDm(tt_collect::SlackDmConfig),
}

/// Epoch milliseconds from the local wall clock (scheduler boundary clock).
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Spawn the scheduler loop. Collector cadence/enable/provider are re-read from
/// settings whenever `reload` is signalled (the `settings_set` command fires it),
/// so edits in the Settings window take effect live — no relaunch needed.
pub fn spawn(app: AppHandle, reload: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        loop {
            // (Re)load config and rebuild the tick intervals for this cycle.
            let collectors = tt_config::load().map(|s| s.collectors).unwrap_or_default();
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
                        run_batch(&app, Batch::Prs, provider, calendar_period_ms).await;
                    }
                    _ = issue_tick.tick(), if collectors.issues.enabled => {
                        run_batch(&app, Batch::Issues, provider, calendar_period_ms).await;
                    }
                    _ = calendar_tick.tick(), if collectors.calendar.enabled => {
                        run_batch(&app, Batch::Calendar, provider, calendar_period_ms).await;
                    }
                    _ = slack_tick.tick(), if slack_on => {
                        run_batch(
                            &app,
                            Batch::SlackDm(slack_config.clone()),
                            provider,
                            calendar_period_ms,
                        )
                        .await;
                    }
                }
            }
        }
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

/// Whether the main window currently reports itself minimized. Unknown states
/// (no window, backend error) count as visible so collection never silently
/// starves.
fn main_window_minimized(app: &AppHandle) -> bool {
    use tauri::Manager;
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
}
