//! Shared serde types. Ports slot-1 `runtime/shared.ts` +
//! `runtime/contracts/agent.ts`.
//!
//! All wire types use camelCase field names and the same `type` discriminants as
//! the TS, so snapshots serialize identically to what the React client consumes.
//! Input types are tolerant (no `deny_unknown_fields`).

use serde::{Deserialize, Serialize};

// --- Constants (ports the `shared.ts` module constants) ---

/// Default localhost port for the (future) agentboard server.
pub const DEFAULT_SERVER_PORT: u16 = 4201;
/// Default localhost host for the (future) agentboard server.
pub const DEFAULT_SERVER_HOST: &str = "127.0.0.1";
/// PID file path used by the (future) server.
pub const PID_FILE: &str = "/tmp/agentboard.pid";
pub const SERVER_IDLE_TIMEOUT_MS: i64 = 30_000;
pub const STUCK_RUNNING_TIMEOUT_MS: i64 = 3 * 60 * 1000;
pub const STALE_AGENT_TIMEOUT_MS: i64 = 12 * 60 * 60 * 1000;
/// An unpinned idle instance is a dead session; prune it shortly after.
pub const IDLE_PRUNE_MS: i64 = 30_000;
pub const JOURNAL_IDLE_TIMEOUT_MS: i64 = 120_000;

// --- Agent contract (ports `contracts/agent.ts`) ---

/// Lifecycle status of an agent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Running,
    Done,
    Error,
    Waiting,
    Question,
    Interrupted,
}

impl AgentStatus {
    /// The "terminal" statuses (`TERMINAL_STATUSES` in TS): the agent finished a
    /// turn and is awaiting user acknowledgement.
    pub fn is_terminal(self) -> bool {
        matches!(self, AgentStatus::Done | AgentStatus::Error | AgentStatus::Interrupted)
    }

    /// Catppuccin status color (`STATUS_COLORS`).
    pub fn color(self) -> &'static str {
        match self {
            AgentStatus::Idle => palette::SURFACE2,
            AgentStatus::Running => palette::YELLOW,
            AgentStatus::Done => palette::GREEN,
            AgentStatus::Error => palette::RED,
            AgentStatus::Waiting => palette::BLUE,
            AgentStatus::Question => palette::SKY,
            AgentStatus::Interrupted => palette::PEACH,
        }
    }

    /// Status glyph (`STATUS_ICONS`).
    pub fn icon(self) -> &'static str {
        match self {
            AgentStatus::Idle => "○",
            AgentStatus::Running => "●",
            AgentStatus::Done => "✓",
            AgentStatus::Error => "✗",
            AgentStatus::Waiting => "◉",
            AgentStatus::Question => "?",
            AgentStatus::Interrupted => "⚠",
        }
    }
}

/// State of a self-paced `/loop`. Ports `LoopInfo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopInfo {
    /// Epoch ms when the loop is scheduled to fire next.
    pub next_wake_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A sub-agent spawned by the parent session. Ports `SubagentInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Optional per-agent live details. Ports `AgentEventDetails`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_max: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_ttl_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<SubagentInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#loop: Option<LoopInfo>,
}

/// A single agent-status event. Ports `AgentEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub agent: String,
    pub session: String,
    pub status: AgentStatus,
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_name: Option<String>,
    /// Set by the tracker when serializing — true if the user hasn't seen this terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unseen: Option<bool>,
    /// Set by the pane scanner — the tmux pane ID where this agent was detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<AgentEventDetails>,
}

// --- Session snapshot (ports `SessionData`) ---

/// A single session in the state snapshot broadcast to clients. Ports `SessionData`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionData {
    pub name: String,
    pub created_at: i64,
    pub dir: String,
    pub branch: String,
    pub is_worktree: bool,
    pub files_changed: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub commits_delta: i64,
    pub unseen: bool,
    pub panes: i64,
    pub ports: Vec<u32>,
    pub windows: i64,
    pub uptime: String,
    pub agent_state: Option<AgentEvent>,
    pub agents: Vec<AgentEvent>,
    pub event_timestamps: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadata>,
}

// --- Programmatic metadata (ports the metadata section of `shared.ts`) ---

/// Tone hint for status/log lines. Ports `MetadataTone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataTone {
    Neutral,
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataStatus {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<MetadataTone>,
    pub ts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub ts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataLogEntry {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<MetadataTone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub ts: i64,
}

/// Per-session agent-pushed metadata. Ports `SessionMetadata`. `status`/`progress`
/// are always present (serialized as `null` when empty), matching the TS shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub status: Option<MetadataStatus>,
    pub progress: Option<MetadataProgress>,
    #[serde(default)]
    pub logs: Vec<MetadataLogEntry>,
}

// --- Server → client messages (ports `ServerMessage`) ---

/// Optional selection carried by a `session-viewed` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionViewedSelect {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<SelectedAgent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedAgent {
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Messages the server pushes to clients. Ports the `ServerMessage` union
/// (tagged on `type`, kebab-case discriminants, camelCase fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ServerMessage {
    State {
        sessions: Vec<SessionData>,
        theme: Option<String>,
        sidebar_width: f64,
        preferred_editor: String,
        ts: i64,
    },
    SessionViewed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<SessionViewedSelect>,
    },
    Resize {
        width: f64,
    },
    Quit,
    ReIdentify,
}

// --- Client → server commands (ports `ClientCommand`) ---

/// Commands a client sends to the server. Ports the `ClientCommand` union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ClientCommand {
    SwitchSession {
        name: String,
    },
    SwitchIndex {
        index: i64,
    },
    NewSession,
    KillSession {
        name: String,
    },
    ReorderSession {
        name: String,
        delta: crate::session_order::ReorderDelta,
    },
    Refresh,
    MarkSeen {
        name: String,
    },
    DismissAgent {
        session: String,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    SetTheme {
        theme: String,
    },
    ReportWidth {
        width: f64,
    },
    Quit,
    IdentifyPane {
        pane_id: String,
        session_name: String,
    },
    FocusAgentPane {
        session: String,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
    },
    KillAgentPane {
        session: String,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
    },
}

/// Catppuccin Mocha palette (ports the `C` constant object).
pub mod palette {
    pub const BLUE: &str = "#89b4fa";
    pub const LAVENDER: &str = "#b4befe";
    pub const PINK: &str = "#cba6f7";
    pub const MAUVE: &str = "#cba6f7";
    pub const YELLOW: &str = "#f9e2af";
    pub const GREEN: &str = "#a6e3a1";
    pub const RED: &str = "#f38ba8";
    pub const PEACH: &str = "#fab387";
    pub const TEAL: &str = "#94e2d5";
    pub const SKY: &str = "#89dceb";
    pub const TEXT: &str = "#cdd6f4";
    pub const SUBTEXT0: &str = "#a6adc8";
    pub const SUBTEXT1: &str = "#bac2de";
    pub const OVERLAY0: &str = "#6c7086";
    pub const OVERLAY1: &str = "#7f849c";
    pub const SURFACE0: &str = "#313244";
    pub const SURFACE1: &str = "#45475a";
    pub const SURFACE2: &str = "#585b70";
    pub const BASE: &str = "#1e1e2e";
    pub const MANTLE: &str = "#181825";
    pub const CRUST: &str = "#11111b";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_status_serializes_lowercase() {
        assert_eq!(serde_json::to_value(AgentStatus::Running).unwrap(), json!("running"));
        assert_eq!(
            serde_json::from_value::<AgentStatus>(json!("interrupted")).unwrap(),
            AgentStatus::Interrupted
        );
    }

    #[test]
    fn terminal_statuses_match_ts() {
        for s in [
            AgentStatus::Done,
            AgentStatus::Error,
            AgentStatus::Interrupted,
        ] {
            assert!(s.is_terminal());
        }
        for s in [
            AgentStatus::Idle,
            AgentStatus::Running,
            AgentStatus::Waiting,
            AgentStatus::Question,
        ] {
            assert!(!s.is_terminal());
        }
    }

    #[test]
    fn agent_event_omits_absent_optionals_and_camelcases() {
        let ev = AgentEvent {
            agent: "claude".into(),
            session: "proj".into(),
            status: AgentStatus::Running,
            ts: 1000,
            thread_id: None,
            thread_name: None,
            unseen: None,
            pane_id: None,
            details: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v, json!({"agent":"claude","session":"proj","status":"running","ts":1000}));
    }

    #[test]
    fn agent_details_loop_key_renamed() {
        let details = AgentEventDetails {
            r#loop: Some(LoopInfo { next_wake_at: 5, reason: Some("poll".into()) }),
            last_tool: Some("Bash".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&details).unwrap();
        assert_eq!(v["loop"]["nextWakeAt"], json!(5));
        assert_eq!(v["lastTool"], json!("Bash"));
        assert!(v.get("model").is_none());
    }

    #[test]
    fn server_message_state_shape() {
        let msg = ServerMessage::State {
            sessions: vec![],
            theme: Some("mocha".into()),
            sidebar_width: 40.0,
            preferred_editor: "code".into(),
            ts: 123,
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], json!("state"));
        assert_eq!(v["sidebarWidth"], json!(40.0));
        assert_eq!(v["preferredEditor"], json!("code"));
    }

    #[test]
    fn server_message_kebab_discriminants() {
        assert_eq!(
            serde_json::to_value(ServerMessage::ReIdentify).unwrap(),
            json!({"type":"re-identify"})
        );
        assert_eq!(serde_json::to_value(ServerMessage::Quit).unwrap(), json!({"type":"quit"}));
    }

    #[test]
    fn client_command_round_trips() {
        let cmd: ClientCommand =
            serde_json::from_value(json!({"type":"switch-session","name":"foo"})).unwrap();
        assert_eq!(cmd, ClientCommand::SwitchSession { name: "foo".into() });

        let cmd: ClientCommand = serde_json::from_value(
            json!({"type":"dismiss-agent","session":"s","agent":"claude","threadId":"t1"}),
        )
        .unwrap();
        assert_eq!(
            cmd,
            ClientCommand::DismissAgent {
                session: "s".into(),
                agent: "claude".into(),
                thread_id: Some("t1".into()),
            }
        );

        let cmd: ClientCommand =
            serde_json::from_value(json!({"type":"reorder-session","name":"s","delta":"top"}))
                .unwrap();
        assert_eq!(
            cmd,
            ClientCommand::ReorderSession {
                name: "s".into(),
                delta: crate::session_order::ReorderDelta::Top,
            }
        );
    }

    #[test]
    fn session_metadata_keeps_null_status_and_progress() {
        let meta = SessionMetadata::default();
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v["status"].is_null());
        assert!(v["progress"].is_null());
        assert_eq!(v["logs"], json!([]));
    }
}
