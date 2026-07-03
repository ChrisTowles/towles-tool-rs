//! tmux integration for agentboard's tmux mode (phase T1 of
//! [docs/AGENTBOARD-TMUX-SPEC.md](../../../../docs/AGENTBOARD-TMUX-SPEC.md)).
//!
//! Ports slot-1 `packages/agentboard/src/mux-tmux/{client,provider}.ts`, with
//! the MuxProvider capability-trait layer collapsed (tmux was the only
//! provider ever implemented — see AGENTBOARD-PORT.md "What does NOT port").
//!
//! House rule: tmux subprocess calls live in thin, un-unit-tested methods;
//! everything parseable or decidable — `-F` format output parsing, edge-pane
//! selection, hook command construction, sidebar-pane derivation — is pure and
//! fixture-tested.

pub mod client;
pub mod provider;

pub use client::{ClientInfo, PaneInfo, SessionInfo, TmuxClient, TmuxRunResult, WindowInfo};
pub use provider::{
    ActiveWindow, MuxSessionInfo, SIDEBAR_PANE_TITLE, STASH_SESSION, SidebarPane, SidebarPosition,
    SwitchTarget, TmuxProvider, resolve_switch_targets,
};
