//! Token-accounting and treemap-rendering library for the towles-tool CLI.
//!
//! Ports `src/commands/graph/` from the TypeScript CLI. The command parses
//! Claude Code session JSONL files (`~/.claude/projects/**/<sessionId>.jsonl`),
//! aggregates token usage per session/model/tool, computes waste metrics, builds
//! a d3 treemap tree plus a per-day per-project stacked bar chart, and renders
//! them into an embedded HTML template. It can also emit flat session rows as
//! JSON or CSV.
//!
//! This crate is deliberately Tauri-free. All filesystem-touching functions take
//! explicit paths (e.g. `projects_dir: &Path`), so tests never read the real
//! `~/.claude`. The JSONL schema is modelled tolerantly (see [`types`]): a
//! malformed line is skipped, not fatal.
//!
//! Module map (mirrors the TS split):
//! - [`types`] — input/output data types.
//! - [`parser`] — JSONL parsing, day filtering, quick token counts.
//! - [`tools`] — tool-call extraction from content blocks.
//! - [`labels`] — session-label extraction and cleanup.
//! - [`analyzer`] — per-session token analysis, model/project name helpers.
//! - [`sessions`] — session discovery on disk, bar-chart aggregation.
//! - [`treemap`] — treemap tree construction.
//! - [`format`] — flat session-row JSON/CSV output.
//! - [`render`] — HTML rendering from the embedded template.

use thiserror::Error;

pub mod analyzer;
pub mod format;
pub mod labels;
pub mod parser;
pub mod render;
pub mod sessions;
pub mod tools;
pub mod treemap;
pub mod types;

/// Errors surfaced by the graph library. JSONL parse failures are intentionally
/// *not* errors — malformed lines are skipped — so the only fallible surface is
/// filesystem access.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// Re-export the public API surface (mirrors the TS `index.ts` re-exports).
pub use analyzer::{
    SessionAnalysis, aggregate_session_tools, analyze_session, extract_project_name,
    get_model_name, get_primary_model,
};
pub use format::{SessionRow, build_session_rows, format_csv, format_json};
pub use labels::extract_session_label;
pub use parser::{HasMtime, calculate_cutoff_ms, filter_by_days};
pub use render::generate_treemap_html;
pub use sessions::{
    build_bar_chart_data, find_recent_sessions, find_session_path, session_result_for_path,
};
pub use tools::{extract_tool_data, extract_tool_detail, sanitize_string, truncate_detail};
pub use treemap::{build_all_sessions_treemap, build_session_treemap, build_turn_nodes};
pub use types::{BarChartData, BarChartDay, ProjectBar, SessionResult, ToolData, TreemapNode};
// The Claude Code transcript schema + parse/title/usage projections now live in
// the shared crate; re-export the pieces tt-graph's consumers (the CLI) use so
// they need not depend on tt-claude-code directly.
pub use tt_claude_code::{
    Content, Message, TranscriptEntry, Usage, parse_transcript, parse_transcript_file,
};

/// Valid output formats for the `graph` command. Ports `OutputFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Html,
    Json,
    Csv,
}

impl OutputFormat {
    /// Parse a `--format` value. Returns `None` for unrecognized formats.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "html" => Some(OutputFormat::Html),
            "json" => Some(OutputFormat::Json),
            "csv" => Some(OutputFormat::Csv),
            _ => None,
        }
    }
}
