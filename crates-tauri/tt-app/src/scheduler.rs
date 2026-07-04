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

use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio::time::MissedTickBehavior;
use tt_collect::CalendarProvider;

use crate::store::SNAPSHOT_EVENT;

#[derive(Clone, Copy)]
enum Batch {
    Prs,
    Issues,
    Calendar,
}

/// Epoch milliseconds from the local wall clock (scheduler boundary clock).
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Spawn the scheduler loop. Settings are read once at startup; a cadence or
/// enable/disable change takes effect on the next app launch.
pub fn spawn(app: AppHandle) {
    let collectors = tt_config::load().map(|s| s.collectors).unwrap_or_default();
    let provider = CalendarProvider::from_str_lenient(&collectors.calendar.provider);
    tauri::async_runtime::spawn(async move {
        let mut pr_tick =
            tokio::time::interval(Duration::from_secs(collectors.prs.refresh_seconds.max(30)));
        pr_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut issue_tick = tokio::time::interval(Duration::from_secs(
            collectors.issues.refresh_minutes.max(1) * 60,
        ));
        issue_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let calendar_period_ms = collectors.calendar.refresh_minutes.max(1) as i64 * 60_000;
        let mut calendar_tick =
            tokio::time::interval(Duration::from_millis(calendar_period_ms as u64));
        calendar_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = pr_tick.tick(), if collectors.prs.enabled => {
                    run_batch(&app, Batch::Prs, provider, calendar_period_ms).await;
                }
                _ = issue_tick.tick(), if collectors.issues.enabled => {
                    run_batch(&app, Batch::Issues, provider, calendar_period_ms).await;
                }
                _ = calendar_tick.tick(), if collectors.calendar.enabled => {
                    run_batch(&app, Batch::Calendar, provider, calendar_period_ms).await;
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
            // for data we already have. Skip when the newest successful claude
            // run is that fresh.
            if claude_fresh_within(&store, now, calendar_period_ms / 2) {
                return;
            }
            log_failure(tt_collect::collect_calendar(&store, provider, now));
        }
    }

    if let Ok(snapshot) = store.snapshot() {
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
    }
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

/// Whether any successful `claude:*` collector run is younger than `max_age_ms`.
fn claude_fresh_within(store: &tt_store::Store, now: i64, max_age_ms: i64) -> bool {
    match store.runs() {
        Ok(runs) => runs
            .iter()
            .filter(|r| r.ok && r.collector.starts_with("claude:"))
            .any(|r| now - r.ran_at < max_age_ms),
        Err(_) => false,
    }
}
