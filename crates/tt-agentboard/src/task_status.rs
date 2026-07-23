//! Auto-driving a worktree-backed task's board column from Agentboard state.
//!
//! The Board used to let a card be dragged between `backlog`/`doing`/`done`
//! by hand; that manual path is gone (2026-07-23). A task with a bound
//! worktree now reports where it *is*: `doing` while a live agent is running
//! in it, `backlog` the moment nothing is. `done` is untouched here — it's a
//! user/GitHub decision (see `tt-collect`'s `rollup_task_statuses` and
//! `Store::close_task`), never an agent-liveness fact.
//!
//! This mirrors `rollup_task_statuses`'s guard on purpose: skip anything
//! already closed (`outcome`/`archived_at` set) or sitting outside
//! `backlog`/`doing`, so this sync and the gh-driven rollup can never fight
//! over the same row.

use tt_store::Store;

use crate::bridge::StatePayload;
use crate::types::AgentStatus;

/// Sync every open, worktree-backed task's status to whether its folder
/// currently has a live, running agent. Returns how many rows changed.
pub fn sync_worktree_task_statuses(
    store: &Store,
    payload: &StatePayload,
    now_ms: i64,
) -> tt_store::Result<usize> {
    let mut changed = 0;
    for task in store.all_tasks()? {
        if task.outcome.is_some() || task.archived_at.is_some() {
            continue;
        }
        if task.status != "backlog" && task.status != "doing" {
            continue;
        }
        let Some(dir) = task.worktree.as_ref().and_then(|w| w.dir.as_deref()) else {
            continue;
        };
        let target = if folder_has_running_agent(payload, dir) { "doing" } else { "backlog" };
        if task.status != target {
            store.set_task_status(task.id, target, now_ms)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// A folder counts as "running" when one of its PTYs is both open (`live`)
/// and has an attributed agent whose latest status is [`AgentStatus::Busy`]
/// — idle/waiting/terminal states are not "running work", just a live shell.
fn folder_has_running_agent(payload: &StatePayload, dir: &str) -> bool {
    payload.repos.iter().flat_map(|r| &r.folders).any(|f| {
        f.dir == dir
            && f.sessions.iter().any(|s| {
                s.live
                    && matches!(s.agent_state.as_ref().map(|e| e.status), Some(AgentStatus::Busy))
            })
    })
}

#[cfg(test)]
mod tests {
    use tt_store::Store;

    use super::*;
    use crate::types::{AgentEvent, FolderData, RepoData, SessionData};

    /// A `FolderData` with just the fields this module reads set; the rest
    /// are inert.
    fn folder(dir: &str, sessions: Vec<SessionData>) -> FolderData {
        FolderData {
            name: dir.to_string(),
            dir: dir.to_string(),
            dir_missing: false,
            branch: "main".to_string(),
            is_worktree: false,
            files_changed: 0,
            lines_added: 0,
            lines_removed: 0,
            commits_ahead: 0,
            commits_behind: 0,
            dirty: false,
            commits_unlanded: 0,
            landed: None,
            sessions,
            needs: 0,
            base_branch: None,
            task_base_branch: None,
            compared_base: String::new(),
            metadata: None,
            has_port_drift: false,
            has_launch_config: false,
            quiet: false,
        }
    }

    fn session(live: bool, status: Option<AgentStatus>) -> SessionData {
        SessionData {
            id: "s1".to_string(),
            name: "shell 1".to_string(),
            created_at: 0,
            live,
            shell_kind: None,
            unseen: false,
            needs_since_ms: None,
            agent_state: status.map(|status| AgentEvent {
                agent: "claude".to_string(),
                session: "s1".to_string(),
                status,
                ts: 0,
                thread_id: None,
                thread_name: None,
                unseen: None,
                pane_id: None,
                details: None,
            }),
            agents: vec![],
            purpose: None,
            port_drift: vec![],
        }
    }

    fn payload(repos: Vec<RepoData>) -> StatePayload {
        StatePayload {
            repos,
            theme: None,
            preferred_editor: "vscode".to_string(),
            compact_recommend_percent: 30,
            windows: crate::windows::WindowsPayload::default(),
            collapsed: Default::default(),
            ts: 0,
        }
    }

    fn repo(folders: Vec<FolderData>) -> RepoData {
        RepoData {
            key: "k".to_string(),
            dir: folders.first().map(|f| f.dir.clone()).unwrap_or_default(),
            name: "repo".to_string(),
            origin_url: None,
            folders,
            needs: 0,
            meta: None,
        }
    }

    #[test]
    fn moves_backlog_to_doing_when_a_busy_agent_is_live_in_its_worktree() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship it", "backlog", None, 1).unwrap();
        s.set_task_worktree(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/repos/x/wt"))
            .unwrap();

        let p = payload(vec![repo(vec![folder(
            "/repos/x/wt",
            vec![session(true, Some(AgentStatus::Busy))],
        )])]);
        assert_eq!(sync_worktree_task_statuses(&s, &p, 10).unwrap(), 1);
        assert_eq!(s.task_by_id(t.id).unwrap().status, "doing");

        // Idempotent: running it again with the same payload changes nothing.
        assert_eq!(sync_worktree_task_statuses(&s, &p, 11).unwrap(), 0);
    }

    #[test]
    fn moves_doing_back_to_backlog_once_the_agent_stops() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship it", "doing", None, 1).unwrap();
        s.set_task_worktree(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/repos/x/wt"))
            .unwrap();

        // Live shell, but idle — not "running" work.
        let p = payload(vec![repo(vec![folder(
            "/repos/x/wt",
            vec![session(true, Some(AgentStatus::Idle))],
        )])]);
        assert_eq!(sync_worktree_task_statuses(&s, &p, 10).unwrap(), 1);
        assert_eq!(s.task_by_id(t.id).unwrap().status, "backlog");
    }

    #[test]
    fn never_touches_a_task_with_no_worktree_binding() {
        let s = Store::open_in_memory().unwrap();
        s.add_task("no worktree", "backlog", None, 1).unwrap();
        let p = payload(vec![]);
        assert_eq!(sync_worktree_task_statuses(&s, &p, 10).unwrap(), 0);
    }

    #[test]
    fn never_touches_a_closed_or_archived_task() {
        let s = Store::open_in_memory().unwrap();
        let closed = s.add_task("done and closed", "doing", None, 1).unwrap();
        s.set_task_worktree(
            closed.id,
            "/repos/x",
            Some("o/x"),
            Some("feat/y"),
            Some("/repos/x/wt"),
        )
        .unwrap();
        s.close_task(closed.id, tt_store::TaskOutcome::Abandoned, 5).unwrap();

        // No agent running at all — would otherwise flip a live "doing" back
        // to "backlog", but a closed task's frozen status must never move.
        let p = payload(vec![]);
        assert_eq!(sync_worktree_task_statuses(&s, &p, 10).unwrap(), 0);
        assert_eq!(s.task_by_id(closed.id).unwrap().status, "doing");
    }
}
