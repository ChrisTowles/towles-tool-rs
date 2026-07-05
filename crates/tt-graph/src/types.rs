//! Output data types for building the treemap / bar-chart. Ports the output
//! half of `src/commands/graph/types.ts`.
//!
//! The Claude Code transcript **input** schema (`TranscriptEntry`, `Message`,
//! `Usage`, `Content`) now lives in the shared [`tt_claude_code`] crate — the
//! single quarantine for that internal, version-volatile format. This module
//! keeps only tt-graph's own output types ([`TreemapNode`], [`BarChartData`],
//! etc.), which serialize with camelCase keys and omit absent optional keys to
//! match the byte-shape the HTML template's JavaScript consumes.

use serde::Serialize;

/// An individual tool call with token attribution, used in tooltips and
/// treemap children. Ports `ToolData` from `types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolData {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// One project's aggregated tokens within a [`BarChartDay`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBar {
    pub project: String,
    pub total_tokens: i64,
}

/// One day of the stacked bar chart: per-project token totals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarChartDay {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub projects: Vec<ProjectBar>,
}

/// The full stacked bar chart payload.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarChartData {
    pub days: Vec<BarChartDay>,
}

/// A session discovered on disk, before its JSONL is fully parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResult {
    pub session_id: String,
    pub path: std::path::PathBuf,
    /// `YYYY-MM-DD` in the local timezone.
    pub date: String,
    pub tokens: i64,
    /// The encoded project directory name (e.g. `-home-ctowles-code-p-tool`).
    pub project: String,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime: i64,
    /// The session's human title, if any: last `custom-title`, else last
    /// `ai-title` (see [`crate::parser::parse_session_title`]). `None` when the
    /// transcript carries neither.
    pub title: Option<String>,
}

/// A node in the d3 treemap tree. Ports `TreemapNode` from `types.ts`.
///
/// Field order and camelCase keys mirror the TS; every optional key is omitted
/// when absent so the shape matches what the template's JS expects.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapNode {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<TreemapNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeated_reads: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_efficiency: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}
