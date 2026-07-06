//! Tauri bridge for the Graph screen: token spend by project and by model,
//! computed from `tt-graph` over Claude Code session JSONL files. Mirrors the
//! `ttr graph` CLI boundary in `crates-cli/tt-cli/src/commands/graph.rs`, minus
//! the treemap/HTML-report path — the app screen answers "where have I spent
//! my tokens" with two aggregate views instead of porting the full
//! session/turn drill-down.

use std::path::PathBuf;

use serde::Serialize;

use tt_graph::{
    ModelBar, ProjectBar, build_model_totals, build_project_totals, find_recent_sessions,
};

/// Max sessions scanned, matching the CLI's `SESSION_LIMIT`.
const SESSION_LIMIT: usize = 500;

/// `~/.claude`, honoring `$HOME` so tests/multiple slots can redirect it.
fn claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude")
}

/// Token spend broken down by project and by model over the selected window.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendSummary {
    pub by_project: Vec<ProjectBar>,
    pub by_model: Vec<ModelBar>,
}

/// Filter to sessions from the last `days` (0 = no limit), matching the CLI's
/// `--days` semantics.
///
/// `build_model_totals` re-parses every matched session's transcript (up to
/// `SESSION_LIMIT` files, some multi-MB), so this runs on a blocking-pool
/// thread rather than the main thread — a plain sync `#[tauri::command]` here
/// would otherwise freeze the whole window for the duration of the scan (see
/// `scheduler.rs`'s `run_batch` for the same pattern).
#[tauri::command]
pub async fn graph_spend_summary(days: f64) -> Result<SpendSummary, String> {
    tauri::async_runtime::spawn_blocking(move || graph_spend_summary_blocking(days))
        .await
        .map_err(|e| format!("graph scan task panicked: {e}"))?
}

fn graph_spend_summary_blocking(days: f64) -> Result<SpendSummary, String> {
    let projects_dir = claude_dir().join("projects");
    if !projects_dir.exists() {
        return Err("No Claude projects directory found at ~/.claude/projects/".to_string());
    }

    let now_ms = chrono::Local::now().timestamp_millis();
    let sessions = find_recent_sessions(&projects_dir, SESSION_LIMIT, days, now_ms)
        .map_err(|e| e.to_string())?;

    Ok(SpendSummary {
        by_project: build_project_totals(&sessions),
        by_model: build_model_totals(&sessions),
    })
}
