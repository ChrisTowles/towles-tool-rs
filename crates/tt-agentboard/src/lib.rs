//! Tauri-free core engine for agentboard. Ports the *data engine* under slot-1
//! `packages/agentboard/src` — the in-memory agent state machine, per-session
//! metadata, custom session ordering, git-info, and port attribution.
//!
//! This crate is deliberately transport-free (phase 1 of the agentboard port, see
//! [docs/AGENTBOARD-PORT.md](../../../docs/AGENTBOARD-PORT.md)): **no tmux, no
//! WebSocket/HTTP broadcast, no fs watchers, no poll loops, no UI**. Where the TS
//! entangles state with transport (WS broadcasts, tmux calls, `setInterval`
//! polls, `fs.watch`), only the pure logic is ported and the transport is left to
//! the future Tauri layer.
//!
//! Time is injected: functions that read the clock in TS take an explicit
//! `now_ms` parameter here (the same pattern as `tt-graph`), so tests stay
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
pub mod collapse;
pub mod engine;
pub mod folder_meta;
pub mod fs_notify;
pub mod git_info;
pub mod metadata;
pub mod procenv;
pub mod repos;
pub mod session_order;
pub mod sessions;
pub mod text;
pub mod themes;
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

// Re-export the public surface.
pub use bridge::{StatePayload, assemble_state};
pub use collapse::{CollapsePayload, CollapseStore, default_collapse_path};
pub use folder_meta::{FolderMeta, FolderMetaStore, default_folder_meta_path};
pub use git_info::{GitInfo, GitInfoCache, compute_git_info};
pub use metadata::SessionMetadataStore;
pub use repos::{
    RepoEntry, add_repo, add_repo_persisted, default_repos_path, load_repos, load_scan_roots,
    remove_repo_by_dir, remove_repo_persisted, repo_entries, resolve_session_name, save_repos,
    save_scan_roots,
};
pub use session_order::{ReorderDelta, SessionOrder, default_session_order_path};
pub use sessions::{SessionRecord, SessionStore, default_sessions_path};
pub use tracker::{AgentTracker, instance_key};
pub use types::{
    AgentEvent, AgentEventDetails, AgentStatus, ClientCommand, FolderData, LoopInfo,
    MetadataLogEntry, MetadataProgress, MetadataStatus, MetadataTone, RepoData, ServerMessage,
    SessionData, SessionMetadata, SubagentInfo, TmuxSessionData,
};
pub use watcher::{AgentWatcher, WatcherContext};
pub use watchers::amp::AmpAgentWatcher;
pub use watchers::claude_code::ClaudeCodeAgentWatcher;
pub use watchers::codex::CodexAgentWatcher;
pub use watchers::opencode::OpenCodeAgentWatcher;
pub use windows::{AgWindow, WindowsPayload, WindowsStore, default_windows_path};
