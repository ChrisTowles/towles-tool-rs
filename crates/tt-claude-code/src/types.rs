//! The Claude Code session-transcript line schema — the ONE canonical model of
//! `~/.claude/projects/**/<sessionId>.jsonl` lines.
//!
//! **This schema is internal to Claude Code and version-volatile** — Anthropic
//! documents the entry format as internal. It is modelled tolerantly here (every
//! field optional, unknown fields ignored, content blocks kept as raw
//! [`serde_json::Value`]) and this crate is the single place the shape is
//! assumed, so a re-validation on a Claude Code upgrade touches one crate. Both
//! consumers ([`tt-graph`] batch analysis and `tt-agentboard`'s live engine)
//! read through these types.
//!
//! We evaluated the Agent SDK's `listSessions`/`getSessionMessages` as an
//! alternative and rejected it: Node-only (no Rust binding), it strips
//! `requestId` (half the dedup key), and it conflates custom/ai titles. Direct
//! JSONL parsing stays the source of truth.

use serde::Deserialize;
use serde_json::Value;

/// A single line from a session JSONL transcript. All fields tolerate absence.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscriptEntry {
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
    /// `true` on lines belonging to a subagent's side conversation (Task tool
    /// runs); those user lines are agent-authored, not the human's.
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: Option<bool>,
    #[serde(default)]
    pub slug: Option<String>,
    /// The user-set session title (from a `custom-title` line). Authoritative.
    #[serde(rename = "customTitle", default)]
    pub custom_title: Option<String>,
    /// Claude Code's auto-generated title (from an `ai-title` line). Fallback.
    #[serde(rename = "aiTitle", default)]
    pub ai_title: Option<String>,
    /// The real absolute working directory Claude Code was launched from.
    /// Constant for the life of a session (not affected by `cd`s a subprocess
    /// runs) — the precise filesystem path a "resume this session elsewhere"
    /// action needs, unlike the lossy, hyphen-encoded project directory name.
    #[serde(default)]
    pub cwd: Option<String>,
}

impl TranscriptEntry {
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

/// The `message` object on a [`TranscriptEntry`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    /// The API message id (e.g. `msg_…`), half of [`TranscriptEntry::dedup_key`].
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

/// Token-usage accounting attached to an assistant message. A superset of what
/// each consumer needs: input/output + cache-read/creation totals, plus the
/// nested per-TTL cache-creation split the live engine uses for cache countdown.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<i64>,
    /// Tokens written into the prompt cache. The majority of tokens in practice.
    #[serde(default)]
    pub cache_creation_input_tokens: Option<i64>,
    /// Per-TTL breakdown of cache-creation tokens (ephemeral 5m / 1h).
    #[serde(default)]
    pub cache_creation: Option<CacheCreation>,
}

/// The `cache_creation` sub-object: cache-write tokens split by cache TTL.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheCreation {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: Option<i64>,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: Option<i64>,
}

/// Message content: either a bare string or an array of content blocks. Blocks
/// are kept untyped ([`serde_json::Value`]) to tolerate the evolving schema;
/// use the typed accessors below rather than re-parsing blocks in each consumer.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Value>),
}

impl Content {
    /// The raw content blocks, or `None` when the content is a bare string.
    pub fn blocks(&self) -> Option<&[Value]> {
        match self {
            Content::Blocks(blocks) => Some(blocks),
            Content::Text(_) => None,
        }
    }

    /// The bare-string content, or `None` when it is a block array.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text(s) => Some(s.as_str()),
            Content::Blocks(_) => None,
        }
    }

    /// The first meaningful text: the bare string, else the first non-empty
    /// `text` block. Used to derive thread names / session labels.
    pub fn first_text(&self) -> Option<&str> {
        match self {
            Content::Text(s) if !s.is_empty() => Some(s.as_str()),
            Content::Text(_) => None,
            Content::Blocks(_) => self.text_blocks().next(),
        }
    }

    /// Iterator over the text of each non-empty `text` block (empty for a bare
    /// string).
    pub fn text_blocks(&self) -> impl Iterator<Item = &str> {
        self.blocks().unwrap_or(&[]).iter().filter_map(|b| {
            if b.get("type").and_then(Value::as_str) == Some("text") {
                b.get("text").and_then(Value::as_str).filter(|t| !t.is_empty())
            } else {
                None
            }
        })
    }

    /// Iterator over the `tool_use` blocks as lightweight typed views (empty for
    /// a bare string).
    pub fn tool_uses(&self) -> impl Iterator<Item = ToolUse<'_>> {
        self.blocks().unwrap_or(&[]).iter().filter_map(|b| {
            (b.get("type").and_then(Value::as_str) == Some("tool_use"))
                .then_some(ToolUse { block: b })
        })
    }
}

/// A borrowed view over a `tool_use` content block. Fields are read lazily from
/// the underlying [`serde_json::Value`] so the volatile block/input shape stays
/// tolerant — callers `.get()` whatever input keys they need.
#[derive(Debug, Clone, Copy)]
pub struct ToolUse<'a> {
    block: &'a Value,
}

impl<'a> ToolUse<'a> {
    /// The tool name (e.g. `Read`, `Bash`, `ScheduleWakeup`).
    pub fn name(&self) -> Option<&'a str> {
        self.block.get("name").and_then(Value::as_str)
    }

    /// The tool-use id.
    pub fn id(&self) -> Option<&'a str> {
        self.block.get("id").and_then(Value::as_str)
    }

    /// The raw `input` object, if present. Callers read the keys they need
    /// (e.g. `file_path`, `delaySeconds`, `reason`) tolerantly.
    pub fn input(&self) -> Option<&'a Value> {
        self.block.get("input").filter(|v| !v.is_null())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_transcript;

    #[test]
    fn dedup_key_needs_both_ids() {
        let e: TranscriptEntry =
            serde_json::from_str(r#"{"requestId":"req_1","message":{"id":"msg_1"}}"#).unwrap();
        assert_eq!(e.dedup_key(), Some("msg_1:req_1".to_string()));

        let no_req: TranscriptEntry =
            serde_json::from_str(r#"{"message":{"id":"msg_1"}}"#).unwrap();
        assert_eq!(no_req.dedup_key(), None);

        let no_id: TranscriptEntry = serde_json::from_str(r#"{"requestId":"req_1"}"#).unwrap();
        assert_eq!(no_id.dedup_key(), None);
    }

    #[test]
    fn content_accessors_over_blocks() {
        // Single physical line — the transcript is JSONL (one object per line).
        let line = r#"{"message":{"content":[{"type":"text","text":"hello"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a.ts"}},{"type":"tool_use","id":"t2","name":"AskUserQuestion","input":{}}]}}"#;
        let entries = parse_transcript(line);
        let content = entries[0].message.as_ref().unwrap().content.as_ref().unwrap();

        assert_eq!(content.first_text(), Some("hello"));
        assert_eq!(content.text_blocks().collect::<Vec<_>>(), vec!["hello"]);

        let tools: Vec<_> = content.tool_uses().collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name(), Some("Read"));
        assert_eq!(tools[0].id(), Some("t1"));
        assert_eq!(
            tools[0].input().and_then(|i| i.get("file_path")).and_then(Value::as_str),
            Some("/a.ts")
        );
        assert_eq!(tools[1].name(), Some("AskUserQuestion"));
    }

    #[test]
    fn content_bare_string() {
        let entries = parse_transcript(r#"{"message":{"content":"just text"}}"#);
        let content = entries[0].message.as_ref().unwrap().content.as_ref().unwrap();
        assert_eq!(content.as_text(), Some("just text"));
        assert_eq!(content.first_text(), Some("just text"));
        assert_eq!(content.blocks(), None);
        assert_eq!(content.tool_uses().count(), 0);
    }

    #[test]
    fn tolerates_unknown_fields_and_nested_cache() {
        let line = r#"{"type":"assistant","weird":true,"message":{"id":"m","usage":{"input_tokens":10,"cache_creation_input_tokens":7,"cache_creation":{"ephemeral_1h_input_tokens":7,"ephemeral_5m_input_tokens":0}}}}"#;
        let entries = parse_transcript(line);
        let usage = entries[0].message.as_ref().unwrap().usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.cache_creation_input_tokens, Some(7));
        assert_eq!(usage.cache_creation.as_ref().unwrap().ephemeral_1h_input_tokens, Some(7));
    }
}
