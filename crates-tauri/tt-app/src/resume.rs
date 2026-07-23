//! App-side session-resume lifecycle: own the run marker, and serve the
//! resume picker its candidates for the previous run — however it ended.
//!
//! The decisions live in `tt_agentboard::resume` (Tauri-free, unit-tested);
//! this is the shell supplying the real clock, pid, paths and window events.

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tt_agentboard::resume::{self, PriorRun, ResumeCandidate, RunMarker};

use crate::agentboard::{Ab, now_ms};

/// Owns this run's marker file plus the verdict on the *previous* run.
pub struct ResumeState {
    path: PathBuf,
    pid: u32,
    started_at_ms: i64,
    /// The previous run's end time, taken exactly once by
    /// [`ab_resume_candidates`] — a webview reload re-runs the frontend's
    /// startup effect, and the picker must not reappear.
    ended_at_ms: Mutex<Option<i64>>,
}

impl ResumeState {
    /// Classify the last run and claim the marker for this one.
    pub fn begin() -> Self {
        let path = resume::default_runtime_path();
        let pid = std::process::id();
        let now = now_ms();
        let ended_at_ms =
            match resume::begin_run(&path, pid, now, crate::instance_lock::pid_is_alive) {
                PriorRun::Ended { at_ms } => {
                    eprintln!("resume: previous run ended (last heartbeat {at_ms})");
                    Some(at_ms)
                }
                PriorRun::Clean => None,
            };
        Self { path, pid, started_at_ms: now, ended_at_ms: Mutex::new(ended_at_ms) }
    }

    /// Rewrite the marker. `started_at_ms` is carried in memory rather than
    /// re-read each time, so a heartbeat is one write instead of a read+write.
    fn touch(&self) {
        let _ = resume::write_marker(
            &self.path,
            &RunMarker { pid: self.pid, started_at_ms: self.started_at_ms, heartbeat_ms: now_ms() },
        );
    }
}

/// Keep the marker fresh, bounding how far the estimated end time can lag the
/// real one (see `resume::PRIOR_RUN_TIME_SLACK_MS`).
pub fn spawn_heartbeat(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let interval = std::time::Duration::from_millis(resume::HEARTBEAT_INTERVAL_MS as u64);
        loop {
            tokio::time::sleep(interval).await;
            app.state::<ResumeState>().touch();
        }
    });
}

/// Record an orderly shutdown's exact moment, rather than leaving the next
/// launch to estimate it from the last heartbeat (up to one interval stale).
pub fn on_window_destroyed(app: &AppHandle) {
    if let Some(state) = app.try_state::<ResumeState>() {
        state.touch();
    }
}

/// Panes that were running Claude when the app last went down — empty unless
/// a previous run (of any kind: crash, kill, or an ordinary quit) was found,
/// and empty on every call after the first.
#[tauri::command]
pub async fn ab_resume_candidates(app: AppHandle) -> Result<Vec<ResumeCandidate>, String> {
    let Some(ended_at_ms) = app.state::<ResumeState>().ended_at_ms.lock().unwrap().take() else {
        return Ok(Vec::new());
    };

    tauri::async_runtime::spawn_blocking(move || {
        // Snapshot under the lock, scan without it: locating and parsing
        // transcripts is disk work, and every `ab_*` command plus the state
        // poll would queue behind the engine lock for its duration.
        let ab = app.state::<Ab>();
        let (records, projects_dir) = {
            let engine = ab.engine.lock().unwrap();
            (engine.session_records(), engine.projects_dir())
        };
        resume::select_candidates(
            records.iter().map(|(dir, rec)| (dir.as_str(), rec)),
            ended_at_ms,
            resume::DEFAULT_RESUME_WINDOW_MS,
            |dir, sid| resume::locate_transcript(&projects_dir, dir, sid),
            resume::transcript_title,
        )
    })
    .await
    .map_err(|e| format!("resume scan failed: {e}"))
}
