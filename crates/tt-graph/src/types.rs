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
///
/// The `custom-title` / `ai-title` line types and the `requestId` field are
/// internal Claude Code transcript details and are version-volatile — Anthropic
/// documents the entry format as internal. We parse them tolerantly (all fields
/// optional, unknown fields ignored) and quarantine every schema assumption in
/// this module; re-validate on a Claude Code upgrade whose changelog mentions
/// session-title or transcript changes. (We evaluated the Agent SDK's
/// `listSessions`/`getSessionMessages` instead: it's Node-only with no Rust
/// binding, strips `requestId`, and conflates custom/ai titles — so direct
/// JSONL parsing stays the source of truth. See issues #17/#18.)
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
    /// Correlates streaming re-logs of the same assistant response; paired with
    /// `message.id` it forms the dedup key ccusage/Claude-Code-Usage-Monitor use.
    #[serde(rename = "requestId", default)]
    pub request_id: Option<String>,
    #[serde(rename = "gitBranch", default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    /// The user-set session title (from a `custom-title` line). Authoritative.
    #[serde(rename = "customTitle", default)]
    pub custom_title: Option<String>,
    /// Claude Code's auto-generated title (from an `ai-title` line). Fallback.
    #[serde(rename = "aiTitle", default)]
    pub ai_title: Option<String>,
}

impl JournalEntry {
    /// Dedup key `message.id:requestId`, present only when **both** ids exist.
    ///
    /// Claude Code re-logs the same assistant message's `usage` more than once
    /// (streaming re-logs, compaction, sidechain copies); counting each line
    /// inflates totals. Entries missing either id return `None` and are always
    /// counted (never collapsed with other id-less entries).
    pub fn dedup_key(&self) -> Option<String> {
        let message_id = self.message.as_ref()?.id.as_deref()?;
        let request_id = self.request_id.as_deref()?;
        Some(format!("{message_id}:{request_id}"))
    }
}

/// The `message` object on a [`JournalEntry`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    /// The API message id (e.g. `msg_…`), half of the [`JournalEntry::dedup_key`].
    #[serde(default)]
    pub id: Option<String>,
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
    /// Tokens written into the prompt cache. The majority of tokens in practice,
    /// so omitting it understates cache volume.
    #[serde(default)]
    pub cache_creation_input_tokens: Option<i64>,
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
