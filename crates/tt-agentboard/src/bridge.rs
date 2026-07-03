//! Pure state-snapshot assembly for the Tauri bridge (agentboard phase 3). Ports
//! the data-composition half of slot-1 `server/index.ts` `computeState`, per
//! docs/AGENTBOARD-BRIDGE-SPEC.md §1/§6.
//!
//! Everything here is pure (no tmux, no tauri, no I/O): the host gathers the
//! inputs (repos, git infos, tracker, metadata, order) and wires its runtime
//! around [`assemble_state`]. The TS "waiting synthesis" (terminal status +
//! live process → waiting) is gone: since T7 the claude-code watcher emits
//! CLI-authoritative statuses, so no post-hoc rewrite is needed.

use std::collections::HashMap;

use serde::Serialize;

use crate::git_info::GitInfo;
use crate::metadata::SessionMetadataStore;
use crate::repos::RepoEntry;
use crate::session_order::SessionOrder;
use crate::tracker::AgentTracker;
use crate::types::SessionData;

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

/// Assemble the trimmed [`StatePayload`] from the current inputs. Ports
/// `computeState` (§1) minus the dropped fields (§6). Pure: reads the tracker /
/// metadata, syncs+applies the custom order, and maps each repo to a
/// [`SessionData`]. Dropped fields (`createdAt`, `panes`, `windows`, `uptime`,
/// `isWorktree`, `ports`, `eventTimestamps`) carry default/zero values.
///
#[allow(clippy::too_many_arguments)]
pub fn assemble_state(
    entries: &[RepoEntry],
    git_infos: &HashMap<String, GitInfo>,
    tracker: &AgentTracker,
    metadata: &SessionMetadataStore,
    order: &mut SessionOrder,
    theme: Option<String>,
    preferred_editor: &str,
    ts: i64,
) -> StatePayload {
    // §1 ordering: the caller owns the base order (the desktop engine passes
    // name-sorted repo entries; the tmux server passes created-at-sorted live
    // sessions, matching the TS). The persisted custom order is synced with and
    // applied over that base.
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    order.sync(&names);
    let ordered = order.apply(&names);

    let mut sessions = Vec::with_capacity(ordered.len());
    for name in &ordered {
        let Some(entry) = entries.iter().find(|e| &e.name == name) else {
            continue;
        };
        let git = git_infos.get(&entry.dir).cloned().unwrap_or_default();
        let agent_state = tracker.get_state(name);
        let agents = tracker.get_agents(name);

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
    use crate::types::{AgentEvent, AgentStatus};

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
    fn assemble_orders_and_maps_fields() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Busy, "ta"), false);
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
        let payload = assemble_state(
            &entries(),
            &git,
            &tracker,
            &metadata,
            &mut order,
            Some("mocha".into()),
            "code",
            999,
        );
        assert_eq!(payload.ts, 999);
        assert_eq!(payload.theme.as_deref(), Some("mocha"));
        assert_eq!(payload.preferred_editor, "code");
        // Alphabetical by name: alpha, beta.
        assert_eq!(payload.sessions[0].name, "alpha");
        assert_eq!(payload.sessions[0].branch, "main");
        assert_eq!(payload.sessions[0].files_changed, 3);
        assert_eq!(payload.sessions[0].agent_state.as_ref().unwrap().status, AgentStatus::Busy);
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
            0,
        );
        assert_eq!(payload.sessions[0].name, "beta");
        assert_eq!(payload.sessions[1].name, "alpha");
    }
}
