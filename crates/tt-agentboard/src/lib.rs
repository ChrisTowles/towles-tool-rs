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
//! - [`ports`] — ps-tree + lsof port attribution.

use thiserror::Error;

pub mod git_info;
pub mod metadata;
pub mod ports;
pub mod session_order;
pub mod text;
pub mod tracker;
pub mod types;

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
pub use git_info::{GitInfo, GitInfoCache, compute_git_info};
pub use metadata::SessionMetadataStore;
pub use ports::PortScanner;
pub use session_order::{ReorderDelta, SessionOrder, default_session_order_path};
pub use tracker::{AgentTracker, instance_key};
pub use types::{
    AgentEvent, AgentEventDetails, AgentStatus, ClientCommand, LoopInfo, MetadataLogEntry,
    MetadataProgress, MetadataStatus, MetadataTone, ServerMessage, SessionData, SessionMetadata,
    SubagentInfo,
};
