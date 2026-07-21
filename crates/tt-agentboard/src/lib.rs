//! Tauri-free core engine for agentboard. Ports the *data engine* under §task§-1
//! `packages/agentboard/src` — the in-memory agent state machine, per-session
//! metadata, custom session ordering, git-info, and port attribution.
//!
//! This crate is deliberately transport-free (phase 1 of the agentboard port):
//! **no tmux, no
//! WebSocket/HTTP broadcast, no fs watchers, no poll loops, no UI**. Where the TS
//! entangles state with transport (WS broadcasts, tmux calls, `setInterval`
//! polls, `fs.watch`), only the pure logic is ported and the transport is left to
//! the future Tauri layer.
//!
//! Time is injected: functions that read the clock in TS take an explicit
//! `now_ms` parameter here (the same pattern as `tt-claude-sessions`), so tests stay
//! deterministic and never touch a real clock.
//!
//! Module map (mirrors the TS split):
//! - [`types`] — shared serde types (SessionData, AgentEvent, ServerMessage,
//!   ClientCommand, metadata, constants, palette). camelCase so snapshots match
//!   the shapes the React client consumes.
//! - [`tracker`] — [`tracker::AgentTracker`], the agent-instance state machine.
//! - [`metadata`] — [`metadata::SessionMetadataStore`], agent-pushed status/progress/log.
//! - [`session_order`] — [`session_order::SessionOrder`], persisted custom ordering.
//! - [`git_info`] — branch/worktree/diff-stat computation with a 5s cache.

use thiserror::Error;

pub mod bridge;
pub mod claude_cli;
pub mod cleanup;
pub mod collapse;
pub mod engine;
pub mod env_drift;
pub mod folder_meta;
pub mod fs_notify;
pub mod git_info;
pub mod launch;
pub mod metadata;
pub mod notify;
pub mod persist;
pub mod procenv;
pub mod repo_meta;
pub mod repos;
pub mod resume;
pub mod session_order;
pub mod sessions;
pub mod text;
pub mod tracker;
pub mod types;
pub mod watcher;
pub mod watchers;
pub mod windows;

/// Errors surfaced by the agentboard core. Filesystem access (session-order
/// persistence) is the only fallible surface; parse/subprocess failures are
/// intentionally swallowed to empty/false, matching the TS behavior.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// Re-export the externally-consumed surface (the app, the remaining CLI
// commands, and the collector/MCP/doctor crates). Everything else stays
// reachable through its module path; the 2026-07-19 CLI trim removed the
// last importer of the wider re-export list.
pub use bridge::StatePayload;
pub use env_drift::PortDrift;
pub use git_info::{
    CommitStat, DiffFile, DiffMode, base_file_content, commit_stats, compute_git_info, diff_files,
    diff_patch,
};
pub use launch::{LaunchConfig, port_listening, read_launch_file};
pub use notify::{NeedsYouEdge, NeedsYouWatch};
pub use repo_meta::{HexColor, RepoAccentStyle, RepoMeta};
pub use repos::{RepoEntry, default_repos_path, load_repos, remove_repo_persisted, repo_entries};
pub use session_order::ReorderDelta;
pub use sessions::SessionRecord;
pub use types::{
    AgentEvent, AgentEventDetails, AgentStatus, ClientCommand, FolderData, LoopInfo,
    MetadataLogEntry, MetadataProgress, MetadataStatus, MetadataTone, NeedsYouReason, RepoData,
    ServerMessage, SessionData, SessionMetadata, SubagentInfo, TmuxSessionData,
};
pub use windows::WindowsPayload;
