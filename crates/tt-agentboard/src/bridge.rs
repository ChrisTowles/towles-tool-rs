//! Pure state-snapshot assembly for the Tauri bridge (agentboard phase 3). Ports
//! the data-composition half of slot-1 `server/index.ts` `computeState`, per
//! docs/AGENTBOARD-BRIDGE-SPEC.md §1/§6.
//!
//! Everything here is pure (no tmux, no tauri, no I/O): the tt-app layer gathers
//! the inputs (repos, git infos, tracker, metadata, order, pid-liveness) and
//! wires tokio/tauri around [`assemble_state`]. The pane-presence "waiting"
//! synthesis is replaced by pid-liveness (§6): a terminal journal status with a
//! live process becomes `waiting`.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::git_info::GitInfo;
use crate::metadata::SessionMetadataStore;
use crate::repos::RepoEntry;
use crate::session_order::SessionOrder;
use crate::tracker::AgentTracker;
use crate::types::{AgentEvent, AgentStatus, SessionData};

/// The state snapshot emitted to the client. Trimmed from the TS `ServerState`
/// per §6: no `sidebarWidth` (the app window owns layout).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePayload {
    pub sessions: Vec<SessionData>,
    pub theme: Option<String>,
    pub preferred_editor: String,
    pub ts: i64,
}

/// If `state` is a terminal status backed by a live process, rewrite it to
/// `waiting`. Ports `overrideTerminalIfPaneAlive`, pid-liveness edition (§6).
pub fn synthesize_waiting(
    state: Option<AgentEvent>,
    is_live: &dyn Fn(&AgentEvent) -> bool,
) -> Option<AgentEvent> {
    state.map(|mut ev| {
        if ev.status.is_terminal() && is_live(&ev) {
            ev.status = AgentStatus::Waiting;
        }
        ev
    })
}

/// Per-agent waiting synthesis. Ports the per-agent half of
/// `mergeAgentsWithPanePresence`; the pane orphan-drop / synthetic-add branches
/// are dropped (§6 — no panes in the desktop app).
pub fn merge_agents_waiting(
    agents: Vec<AgentEvent>,
    is_live: &dyn Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    agents
        .into_iter()
        .map(|mut ev| {
            if ev.status.is_terminal() && is_live(&ev) {
                ev.status = AgentStatus::Waiting;
            }
            ev
        })
        .collect()
}

/// Assemble the trimmed [`StatePayload`] from the current inputs. Ports
/// `computeState` (§1) minus the dropped fields (§6). Pure: reads the tracker /
/// metadata, syncs+applies the custom order, and maps each repo to a
/// [`SessionData`]. Dropped fields (`createdAt`, `panes`, `windows`, `uptime`,
/// `isWorktree`, `ports`, `eventTimestamps`) carry default/zero values.
///
/// `live_threads` holds the thread ids whose OS process is alive (computed by the
/// bridge via claude-pid), used for the `waiting` synthesis.
#[allow(clippy::too_many_arguments)]
pub fn assemble_state(
    entries: &[RepoEntry],
    git_infos: &HashMap<String, GitInfo>,
    tracker: &AgentTracker,
    metadata: &SessionMetadataStore,
    order: &mut SessionOrder,
    theme: Option<String>,
    preferred_editor: &str,
    live_threads: &HashSet<String>,
    ts: i64,
) -> StatePayload {
    // §1 ordering: the caller owns the base order (the desktop engine passes
    // name-sorted repo entries; the tmux server passes created-at-sorted live
    // sessions, matching the TS). The persisted custom order is synced with and
    // applied over that base.
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    order.sync(&names);
    let ordered = order.apply(&names);

    let is_live = |ev: &AgentEvent| -> bool {
        ev.thread_id.as_deref().is_some_and(|t| live_threads.contains(t))
    };

    let mut sessions = Vec::with_capacity(ordered.len());
    for name in &ordered {
        let Some(entry) = entries.iter().find(|e| &e.name == name) else {
            continue;
        };
        let git = git_infos.get(&entry.dir).cloned().unwrap_or_default();
        let agent_state = synthesize_waiting(tracker.get_state(name), &is_live);
        let agents = merge_agents_waiting(tracker.get_agents(name), &is_live);

        sessions.push(SessionData {
            name: name.clone(),
            created_at: 0,
            dir: entry.dir.clone(),
            branch: git.branch,
            is_worktree: false,
            files_changed: git.files_changed,
            lines_added: git.lines_added,
            lines_removed: git.lines_removed,
            commits_delta: git.commits_delta,
            unseen: tracker.is_unseen(name),
            panes: 0,
            ports: Vec::new(),
            windows: 0,
            uptime: String::new(),
            agent_state,
            agents,
            event_timestamps: Vec::new(),
            metadata: metadata.get(name).cloned(),
        });
    }

    StatePayload { sessions, theme, preferred_editor: preferred_editor.to_string(), ts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentEvent;

    fn ev(session: &str, status: AgentStatus, thread: &str) -> AgentEvent {
        AgentEvent {
            agent: "claude-code".into(),
            session: session.into(),
            status,
            ts: 1,
            thread_id: Some(thread.into()),
            thread_name: None,
            unseen: None,
            pane_id: None,
            details: None,
        }
    }

    fn entries() -> Vec<RepoEntry> {
        vec![
            RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() },
            RepoEntry { name: "beta".into(), dir: "/r/beta".into() },
        ]
    }

    #[test]
    fn waiting_synthesis_only_for_live_terminal() {
        let live: HashSet<String> = ["t1".to_string()].into_iter().collect();
        let is_live = |e: &AgentEvent| e.thread_id.as_deref().is_some_and(|t| live.contains(t));

        // terminal + live → waiting
        let done_live = synthesize_waiting(Some(ev("s", AgentStatus::Done, "t1")), &is_live);
        assert_eq!(done_live.unwrap().status, AgentStatus::Waiting);
        // terminal + dead → unchanged
        let done_dead = synthesize_waiting(Some(ev("s", AgentStatus::Done, "t2")), &is_live);
        assert_eq!(done_dead.unwrap().status, AgentStatus::Done);
        // non-terminal + live → unchanged
        let run_live = synthesize_waiting(Some(ev("s", AgentStatus::Running, "t1")), &is_live);
        assert_eq!(run_live.unwrap().status, AgentStatus::Running);
    }

    #[test]
    fn assemble_orders_and_maps_fields() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Running, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut order = SessionOrder::new(None);
        let mut git = HashMap::new();
        git.insert(
            "/r/alpha".to_string(),
            GitInfo {
                branch: "main".into(),
                files_changed: 3,
                lines_added: 10,
                lines_removed: 2,
                commits_delta: 1,
                ..Default::default()
            },
        );
        let live = HashSet::new();

        let payload = assemble_state(
            &entries(),
            &git,
            &tracker,
            &metadata,
            &mut order,
            Some("mocha".into()),
            "code",
            &live,
            999,
        );
        assert_eq!(payload.ts, 999);
        assert_eq!(payload.theme.as_deref(), Some("mocha"));
        assert_eq!(payload.preferred_editor, "code");
        // Alphabetical by name: alpha, beta.
        assert_eq!(payload.sessions[0].name, "alpha");
        assert_eq!(payload.sessions[0].branch, "main");
        assert_eq!(payload.sessions[0].files_changed, 3);
        assert_eq!(payload.sessions[0].agent_state.as_ref().unwrap().status, AgentStatus::Running);
        assert_eq!(payload.sessions[1].name, "beta");
        // beta has no git info → defaults.
        assert_eq!(payload.sessions[1].branch, "");
    }

    #[test]
    fn assemble_respects_custom_order() {
        let tracker = AgentTracker::new();
        let metadata = SessionMetadataStore::new();
        let mut order = SessionOrder::new(None);
        order.sync(&["alpha".to_string(), "beta".to_string()]);
        order.reorder("beta", crate::session_order::ReorderDelta::Top);
        let payload = assemble_state(
            &entries(),
            &HashMap::new(),
            &tracker,
            &metadata,
            &mut order,
            None,
            "code",
            &HashSet::new(),
            0,
        );
        assert_eq!(payload.sessions[0].name, "beta");
        assert_eq!(payload.sessions[1].name, "alpha");
    }

    #[test]
    fn assemble_waiting_from_live_pid() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Done, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut order = SessionOrder::new(None);
        let live: HashSet<String> = ["ta".to_string()].into_iter().collect();
        let payload = assemble_state(
            &entries(),
            &HashMap::new(),
            &tracker,
            &metadata,
            &mut order,
            None,
            "code",
            &live,
            0,
        );
        // alpha's done agent is backed by a live pid → waiting.
        assert_eq!(payload.sessions[0].agent_state.as_ref().unwrap().status, AgentStatus::Waiting);
    }
}
