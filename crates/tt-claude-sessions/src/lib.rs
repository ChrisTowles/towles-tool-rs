//! Token-accounting library backing the desktop app's Claude Sessions screen.
//!
//! Parses Claude Code session JSONL files
//! (`~/.claude/projects/**/<sessionId>.jsonl`) once per scan ([`ledger`]) and
//! derives everything the screen shows from that cached pass: stat totals,
//! day/repo/model aggregates, session search, and ranked waste findings
//! ([`insights`] — the answer-first replacement for the old treemap
//! explorer). A single session's turn/tool drill-down ([`breakdown`]) is the
//! only per-session re-parse, done on demand when a row is opened.
//!
//! This crate is deliberately Tauri-free. All filesystem-touching functions
//! take explicit paths (e.g. `projects_dir: &Path`), so tests never read the
//! real `~/.claude`. The JSONL schema is modelled tolerantly: a malformed line
//! is skipped, not fatal.
//!
//! Module map:
//! - [`types`] — shared output data types.
//! - [`parser`] — day-cutoff computation.
//! - [`tools`] — tool-call extraction from content blocks.
//! - [`analyzer`] — per-session token analysis, model/project name helpers.
//! - [`pricing`] — per-model token→dollar rates for cost estimation.
//! - [`ledger`] — single-parse session scan + summary aggregates + search.
//! - [`insights`] — ranked waste/habit findings over a scanned window.
//! - [`breakdown`] — one session's turn/tool drill-down.
//! - [`cadence`] — human-prompt cadence (time-of-day / per-day counts), not
//!   token/cost accounting.

use thiserror::Error;

pub mod analyzer;
pub mod breakdown;
pub mod cadence;
pub mod insights;
pub mod ledger;
pub mod parser;
pub mod pricing;
pub mod tools;
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
pub use breakdown::{SessionBreakdown, TurnBreakdown, build_session_breakdown, find_session_path};
pub use cadence::{CadenceSummary, DayBucket, DayHourCell, build_cadence};
pub use insights::{Insight, InsightKind, build_insights};
pub use ledger::{
    LedgerTotals, SearchHit, SessionDetail, build_ledger_days, build_ledger_model_totals,
    build_ledger_project_totals, ledger_totals, normalize_repo_name, scan_sessions_detailed,
    search_sessions,
};
pub use parser::calculate_cutoff_ms;
pub use pricing::{ModelPricing, pricing_for};
pub use tools::{extract_tool_data, extract_tool_detail, sanitize_string, truncate_detail};
pub use types::{BarChartDay, ModelBar, ProjectBar, ToolData};
// The Claude Code transcript schema + parse/title/usage projections live in
// the shared crate; re-export the pieces this crate's consumers use so they
// need not depend on tt-claude-code directly.
pub use tt_claude_code::{
    Content, Message, TranscriptEntry, Usage, parse_transcript, parse_transcript_file,
};
