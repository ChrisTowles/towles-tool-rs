//! `tt-claude-code` — the single, Tauri-free home for reading Claude Code
//! session transcripts (`~/.claude/projects/**/<sessionId>.jsonl`).
//!
//! This crate owns the one canonical model of the **internal, version-volatile**
//! transcript schema ([`TranscriptEntry`] and friends) plus the pure projections
//! both consumers need:
//! - [`parse_transcript`] / [`parse_transcript_file`] — tolerant JSONL parsing.
//! - [`session_title`] — human session name (custom-title > ai-title).
//! - [`usage_totals`] — deduplicated token accounting (by `message.id` +
//!   `requestId`), including cache-read/creation volume.
//! - Typed content accessors ([`Content::text_blocks`], [`Content::tool_uses`])
//!   so status / thread-name / tool logic reads blocks through one place.
//!
//! Consumers:
//! - `tt-claude-sessions` — batch/historical treemap + token analysis.
//! - `tt-agentboard` — the live agent engine (CLI liveness + `/proc` PID +
//!   fs-notify + tail enrichment). Those live-gathering concerns stay in
//!   `tt-agentboard`; only the schema/parse/projection knowledge lives here.
//!
//! Everything is tolerant (all fields optional, unknown fields ignored, blank /
//! malformed lines skipped, unreadable files → empty) and deterministic (no
//! clock, no `$HOME` reads — callers pass paths in).

pub mod cwd;
pub mod models;
pub mod parse;
pub mod prompts;
pub mod title;
pub mod types;
pub mod usage;

pub use cwd::{session_cwd, session_cwd_file, session_cwd_str};
pub use models::{
    CONTEXT_1M, CONTEXT_200K, ResolvedWindow, WindowSource, context_window, model_known,
    resolve_window,
};
pub use parse::{parse_transcript, parse_transcript_file};
pub use prompts::{user_prompt_blob, user_prompts};
pub use title::{session_title, session_title_file, session_title_str};
pub use types::{CacheCreation, Content, Message, ToolUse, TranscriptEntry, Usage};
pub use usage::{UsageTotals, usage_totals, usage_totals_file, usage_totals_str};
