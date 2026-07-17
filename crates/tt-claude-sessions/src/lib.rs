//! Token-accounting and treemap-rendering library backing the desktop app's
//! Claude Sessions screen.
//!
//! Parses Claude Code session JSONL files
//! (`~/.claude/projects/**/<sessionId>.jsonl`), aggregates token usage per
//! session/model/tool, computes waste metrics, builds a d3 treemap tree plus a
//! per-day per-project stacked bar chart, and renders them into an embedded
//! HTML template. The ledger path ([`ledger`]) additionally powers the
//! screen's Overview/Sessions tabs (totals, day stacks, ranked bars, search).
//!
//! This crate is deliberately Tauri-free. All filesystem-touching functions take
//! explicit paths (e.g. `projects_dir: &Path`), so tests never read the real
//! `~/.claude`. The JSONL schema is modelled tolerantly (see [`types`]): a
//! malformed line is skipped, not fatal.
//!
//! Module map:
//! - [`types`] — input/output data types.
//! - [`parser`] — day-cutoff computation.
//! - [`tools`] — tool-call extraction from content blocks.
//! - [`labels`] — session-label extraction and cleanup.
//! - [`analyzer`] — per-session token analysis, model/project name helpers.
//! - [`sessions`] — session discovery on disk, bar-chart aggregation.
//! - [`treemap`] — treemap tree construction.
//! - [`render`] — HTML rendering from the embedded template.
//! - [`ledger`] — single-parse session scan + summary aggregates + search.

use thiserror::Error;

pub mod analyzer;
pub mod labels;
pub mod ledger;
pub mod parser;
pub mod render;
pub mod sessions;
pub mod tools;
pub mod treemap;
pub mod types;

/// Errors surfaced by the library. JSONL parse failures are intentionally
/// *not* errors — malformed lines are skipped — so the only fallible surface is
/// filesystem access.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// Re-export the public API surface.
pub use analyzer::{
    SessionAnalysis, aggregate_session_tools, analyze_session, extract_project_name,
    get_model_name, get_primary_model,
};
pub use labels::extract_session_label;
pub use ledger::{
    LedgerTotals, SearchHit, SessionDetail, build_ledger_days, build_ledger_model_totals,
    build_ledger_project_totals, ledger_totals, normalize_repo_name, scan_sessions_detailed,
    search_sessions,
};
pub use parser::calculate_cutoff_ms;
pub use render::generate_treemap_html;
pub use sessions::{
    build_bar_chart_data, find_recent_sessions, find_session_path, session_result_for_path,
};
pub use tools::{extract_tool_data, extract_tool_detail, sanitize_string, truncate_detail};
pub use treemap::{build_all_sessions_treemap, build_session_treemap, build_turn_nodes};
pub use types::{
    BarChartData, BarChartDay, ModelBar, ProjectBar, SessionResult, ToolData, TreemapNode,
};
// The Claude Code transcript schema + parse/title/usage projections live in
// the shared crate; re-export the pieces this crate's consumers use so they
// need not depend on tt-claude-code directly.
pub use tt_claude_code::{
    Content, Message, TranscriptEntry, Usage, parse_transcript, parse_transcript_file,
};
