//! Tauri bridge for the Claude Sessions screen: Claude Code session history
//! across every repo, computed from `tt-graph` over Claude Code session JSONL
//! files. Mirrors the `ttr claude-sessions` CLI boundary in
//! `crates-cli/tt-cli/src/commands/claude_sessions.rs`, minus the treemap/HTML-
//! report path — the app screen answers "where have I spent my tokens" and
//! "what have I been working on" with aggregate bars plus a recent-sessions
//! list, instead of porting the full session/turn drill-down.

use std::path::PathBuf;

use serde::Serialize;

use tt_graph::{
    ModelBar, ProjectBar, build_model_totals, build_project_totals, extract_project_name,
    find_recent_sessions, parse_transcript_file,
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

/// One Claude Code session, for the recent-sessions list.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    pub session_id: String,
    pub title: Option<String>,
    /// Human-readable project label decoded from the `~/.claude/projects`
    /// directory name (e.g. `towles-tool-rs-slot-0`).
    pub project: String,
    /// `YYYY-MM-DD` in the local timezone.
    pub date: String,
    pub tokens: i64,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime: i64,
    /// The real absolute working directory this session ran in, read back
    /// from the transcript's own `cwd` field. `None` for older transcripts
    /// logged before Claude Code recorded it — the fork actions need a real
    /// path, so the frontend hides them when this is absent.
    pub cwd: Option<String>,
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
pub async fn claude_sessions_summary(days: f64) -> Result<SpendSummary, String> {
    tauri::async_runtime::spawn_blocking(move || claude_sessions_summary_blocking(days))
        .await
        .map_err(|e| format!("claude sessions scan task panicked: {e}"))?
}

fn claude_sessions_summary_blocking(days: f64) -> Result<SpendSummary, String> {
    let sessions = scan_sessions(days)?;

    Ok(SpendSummary {
        by_project: build_project_totals(&sessions),
        by_model: build_model_totals(&sessions),
    })
}

/// Recent Claude Code sessions across every repo, most-recent first. Cheap:
/// reuses the lightweight discovery pass, no per-session re-parse.
#[tauri::command]
pub async fn claude_sessions_list(days: f64) -> Result<Vec<SessionListItem>, String> {
    tauri::async_runtime::spawn_blocking(move || claude_sessions_list_blocking(days))
        .await
        .map_err(|e| format!("claude sessions scan task panicked: {e}"))?
}

fn claude_sessions_list_blocking(days: f64) -> Result<Vec<SessionListItem>, String> {
    let sessions = scan_sessions(days)?;

    Ok(sessions
        .into_iter()
        .map(|s| {
            let cwd = session_cwd(&s.path);
            SessionListItem {
                session_id: s.session_id,
                title: s.title,
                project: extract_project_name(&s.project),
                date: s.date,
                tokens: s.tokens,
                mtime: s.mtime,
                cwd,
            }
        })
        .collect())
}

/// The real working directory a session ran in, read back from the first
/// transcript entry that recorded one.
fn session_cwd(path: &std::path::Path) -> Option<String> {
    parse_transcript_file(path).into_iter().find_map(|e| e.cwd)
}

fn scan_sessions(days: f64) -> Result<Vec<tt_graph::SessionResult>, String> {
    let projects_dir = claude_dir().join("projects");
    if !projects_dir.exists() {
        return Err("No Claude projects directory found at ~/.claude/projects/".to_string());
    }

    let now_ms = chrono::Local::now().timestamp_millis();
    find_recent_sessions(&projects_dir, SESSION_LIMIT, days, now_ms).map_err(|e| e.to_string())
}
