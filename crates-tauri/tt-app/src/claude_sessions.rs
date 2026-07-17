//! Tauri bridge for the Claude Sessions screen: token spend by day/repo/model
//! plus session search, computed from `tt-claude-sessions` over Claude Code
//! session JSONL files. One scan parses each transcript once
//! (`scan_sessions_detailed`) and is cached in managed state so search
//! keystrokes never re-read the disk. The Treemap tab's HTML report is built
//! on demand (`claude_sessions_treemap_html`) and embedded in an iframe.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use tt_claude_sessions::{
    BarChartDay, LedgerTotals, ModelBar, ProjectBar, SessionDetail, build_all_sessions_treemap,
    build_bar_chart_data, build_ledger_days, build_ledger_model_totals,
    build_ledger_project_totals, find_recent_sessions, generate_treemap_html, ledger_totals,
    scan_sessions_detailed, search_sessions,
};

/// Max sessions scanned, matching the CLI's `SESSION_LIMIT`.
const SESSION_LIMIT: usize = 500;
/// Sessions returned in the summary's ranked list.
const TOP_SESSIONS: usize = 50;
/// Max search hits returned.
const SEARCH_LIMIT: usize = 100;

/// `~/.claude`, honoring `$HOME` so tests/multiple slots can redirect it.
fn claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude")
}

/// The last scan, kept so `claude_sessions_search` filters in memory. Keyed
/// by the `days` window it was scanned for.
struct CachedScan {
    days: f64,
    details: Vec<SessionDetail>,
}

/// Managed state: the cached scan behind an `Arc` so blocking-pool closures can
/// own a handle.
#[derive(Clone, Default)]
pub struct ClaudeSessionsCache(Arc<Mutex<Option<CachedScan>>>);

/// One session row for the frontend (ranked list and search results).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionRow {
    pub session_id: String,
    pub title: Option<String>,
    pub project: String,
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// The session's real launch directory, for "Open in Agentboard". `None`
    /// for transcripts predating the `cwd` field.
    pub cwd: Option<String>,
    /// Prompt-text context around the match; only set on search hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

impl ClaudeSessionRow {
    fn from_detail(d: &SessionDetail, snippet: Option<String>) -> Self {
        ClaudeSessionRow {
            session_id: d.session_id.clone(),
            title: d.title.clone(),
            project: d.project.clone(),
            date: d.date.clone(),
            input_tokens: d.usage.input_tokens,
            output_tokens: d.usage.output_tokens,
            cache_read_tokens: d.usage.cache_read_tokens,
            cache_creation_tokens: d.usage.cache_creation_tokens,
            cwd: d.cwd.clone(),
            snippet,
        }
    }
}

/// Everything the Claude Sessions screen renders from one scan.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionsSummary {
    pub totals: LedgerTotals,
    pub days: Vec<BarChartDay>,
    pub by_project: Vec<ProjectBar>,
    pub by_model: Vec<ModelBar>,
    /// Top sessions by input+output tokens — the outlier list.
    pub top_sessions: Vec<ClaudeSessionRow>,
}

fn scan(days: f64) -> Result<Vec<SessionDetail>, String> {
    let projects_dir = claude_dir().join("projects");
    if !projects_dir.exists() {
        return Err("No Claude projects directory found at ~/.claude/projects/".to_string());
    }
    let now_ms = chrono::Local::now().timestamp_millis();
    scan_sessions_detailed(&projects_dir, SESSION_LIMIT, days, now_ms).map_err(|e| e.to_string())
}

/// Scan (blocking pool — multi-MB parses would freeze the main thread), cache
/// the details for search, and return the aggregates.
#[tauri::command]
pub async fn claude_sessions_summary(
    days: f64,
    cache: tauri::State<'_, ClaudeSessionsCache>,
) -> Result<ClaudeSessionsSummary, String> {
    let handle = cache.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let details = scan(days)?;

        let mut ranked: Vec<&SessionDetail> = details.iter().collect();
        ranked.sort_by_key(|d| std::cmp::Reverse(d.billable()));
        let top_sessions = ranked
            .iter()
            .take(TOP_SESSIONS)
            .map(|d| ClaudeSessionRow::from_detail(d, None))
            .collect();

        let summary = ClaudeSessionsSummary {
            totals: ledger_totals(&details),
            days: build_ledger_days(&details),
            by_project: build_ledger_project_totals(&details),
            by_model: build_ledger_model_totals(&details),
            top_sessions,
        };
        *handle.lock().expect("claude sessions cache poisoned") =
            Some(CachedScan { days, details });
        Ok(summary)
    })
    .await
    .map_err(|e| format!("claude sessions scan task panicked: {e}"))?
}

/// Build the interactive treemap + bar-chart HTML report for the last `days`
/// (blocking pool — it re-parses every transcript). Returned as a full HTML
/// document the frontend embeds via `<iframe srcDoc>`; computed only when the
/// Treemap tab asks, so it stays uncached.
#[tauri::command]
pub async fn claude_sessions_treemap_html(days: f64) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let projects_dir = claude_dir().join("projects");
        if !projects_dir.exists() {
            return Err("No Claude projects directory found at ~/.claude/projects/".to_string());
        }
        let now_ms = chrono::Local::now().timestamp_millis();
        let sessions = find_recent_sessions(&projects_dir, SESSION_LIMIT, days, now_ms)
            .map_err(|e| format!("Failed to scan sessions: {e}"))?;
        if sessions.is_empty() {
            return Err("No sessions found in this range".to_string());
        }
        let bar_chart = build_bar_chart_data(&sessions);
        let treemap = build_all_sessions_treemap(&sessions)
            .map_err(|e| format!("Failed to build treemap: {e}"))?;
        Ok(generate_treemap_html(&treemap, &bar_chart))
    })
    .await
    .map_err(|e| format!("claude sessions treemap task panicked: {e}"))?
}

/// Search the cached scan's titles + prompt text; rescans only when the cache
/// is missing or was built for a different window. Hits come back newest-first.
#[tauri::command]
pub async fn claude_sessions_search(
    days: f64,
    query: String,
    cache: tauri::State<'_, ClaudeSessionsCache>,
) -> Result<Vec<ClaudeSessionRow>, String> {
    let handle = cache.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = handle.lock().expect("claude sessions cache poisoned");
        if !guard.as_ref().is_some_and(|c| c.days == days) {
            let details = scan(days)?;
            *guard = Some(CachedScan { days, details });
        }
        let details = &guard.as_ref().expect("cache populated above").details;
        let hits = search_sessions(details, &query);
        Ok(hits
            .into_iter()
            .take(SEARCH_LIMIT)
            .map(|h| ClaudeSessionRow::from_detail(&details[h.index], h.snippet))
            .collect())
    })
    .await
    .map_err(|e| format!("claude sessions search task panicked: {e}"))?
}
