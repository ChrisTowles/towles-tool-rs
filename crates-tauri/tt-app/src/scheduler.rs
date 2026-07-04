//! Collector scheduler: fills tt.db while the app is open.
//!
//! Cadence comes from `settings.assistant`: PRs via `gh` every
//! `pr_refresh_seconds`; calendar/email/tasks via `claude -p` every
//! `claude_refresh_minutes`, gated by `assistant.enabled` because those runs
//! cost tokens. Each batch runs in a blocking task on its **own** store
//! connection (tt-store opens WAL + busy-timeout), so a slow `claude -p` never
//! holds the UI's store mutex. After every batch the fresh snapshot is emitted
//! as `store://snapshot`.

use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio::time::MissedTickBehavior;

use crate::store::SNAPSHOT_EVENT;

#[derive(Clone, Copy)]
enum Batch {
    Prs,
    Claude,
}

/// Epoch milliseconds from the local wall clock (scheduler boundary clock).
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Spawn the scheduler loop. Settings are read once at startup; a cadence or
/// enable/disable change takes effect on the next app launch.
pub fn spawn(app: AppHandle) {
    let assistant = tt_config::load().map(|s| s.assistant).unwrap_or_default();
    tauri::async_runtime::spawn(async move {
        let mut pr_tick =
            tokio::time::interval(Duration::from_secs(assistant.pr_refresh_seconds.max(30)));
        pr_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let claude_period_ms = assistant.claude_refresh_minutes.max(1) as i64 * 60_000;
        let mut claude_tick = tokio::time::interval(Duration::from_millis(claude_period_ms as u64));
        claude_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = pr_tick.tick() => {
                    run_batch(&app, Batch::Prs, claude_period_ms).await;
                }
                _ = claude_tick.tick(), if assistant.enabled => {
                    run_batch(&app, Batch::Claude, claude_period_ms).await;
                }
            }
        }
    });
}

async fn run_batch(app: &AppHandle, batch: Batch, claude_period_ms: i64) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        run_batch_blocking(&app, batch, claude_period_ms)
    })
    .await;
}

fn run_batch_blocking(app: &AppHandle, batch: Batch, claude_period_ms: i64) {
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
            let summary = tt_collect::collect_prs(&store, &repos, now);
            if !summary.ok {
                eprintln!(
                    "scheduler: prs collect failed: {}",
                    summary.message.as_deref().unwrap_or("unknown")
                );
            }
        }
        Batch::Claude => {
            // Token-cost guard: an interval's first tick fires at startup, so a
            // relaunch inside half a refresh period would re-bill `claude -p`
            // for data we already have. Skip when the newest successful claude
            // run is that fresh.
            if claude_fresh_within(&store, now, claude_period_ms / 2) {
                return;
            }
            let summary = tt_collect::collect_calendar(&store, now);
            if !summary.ok {
                eprintln!(
                    "scheduler: calendar collect failed: {}",
                    summary.message.as_deref().unwrap_or("unknown")
                );
            }
            for summary in tt_collect::collect_email_and_tasks(&store, now) {
                if !summary.ok {
                    eprintln!(
                        "scheduler: {} collect failed: {}",
                        summary.collector,
                        summary.message.as_deref().unwrap_or("unknown")
                    );
                }
            }
        }
    }

    if let Ok(snapshot) = store.snapshot() {
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
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
