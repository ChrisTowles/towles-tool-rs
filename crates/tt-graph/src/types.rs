//! Data types for parsing Claude Code session JSONL files and building the
//! treemap / bar-chart output. Ports `src/commands/graph/types.ts`.
//!
//! The JSONL schema evolves, so input types ([`JournalEntry`], [`Message`],
//! [`Usage`]) are modelled tolerantly: every field is optional, unknown fields
//! are ignored, and content blocks are kept as raw [`serde_json::Value`] (the TS
//! consumes them untyped). A malformed line is skipped rather than fatal.
//!
//! Output types ([`TreemapNode`], [`BarChartData`], etc.) serialize with
//! camelCase keys and omit absent optional keys, matching the byte-shape the
//! HTML template's JavaScript consumes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single line from a session JSONL file. All fields tolerate absence.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JournalEntry {
    #[serde(rename = "type", default)]
    pub entry_type: String,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(rename = "gitBranch", default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

/// The `message` object on a [`JournalEntry`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub content: Option<Content>,
}

/// Message content: either a bare string or an array of content blocks. Blocks
/// are kept untyped (`serde_json::Value`) to tolerate the evolving schema.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Value>),
}

impl Content {
    /// The content blocks, or `None` when the content is a bare string.
    pub fn blocks(&self) -> Option<&[Value]> {
        match self {
            Content::Blocks(blocks) => Some(blocks),
            Content::Text(_) => None,
        }
    }
}

/// Token-usage accounting attached to an assistant message.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<i64>,
}

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
