//! Edge detection for needs-you desktop notifications.
//!
//! [`NeedsYouWatch`] observes successive [`StatePayload`] snapshots (post
//! liveness-stamping, so [`crate::bridge::session_needs`] is truthful) and
//! reports only the sessions that *flipped into* needing you since the last
//! observation — an edge, not a level, so a session that stays blocked never
//! repeats. What the host does with an edge (fire a desktop notification,
//! suppress while focused, …) is its business; this module is pure state
//! diffing so it stays Tauri-free and unit-testable.
//!
//! The first observation only primes the baseline: pre-existing needs-you
//! states at app launch are levels, not flips, and must not spam.

use std::collections::HashMap;

use crate::StatePayload;
use crate::bridge::needs_reason;
use crate::types::NeedsYouReason;

/// One session that just flipped into needing you, with the display names the
/// notification shows and *why* it needs you. Status-report only — acting on it
/// happens in the real PTY, so no action metadata is carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsYouEdge {
    /// Stable session id (the PTY `term_id`).
    pub session_id: String,
    /// Repo display name (e.g. `towles-tool-rs`).
    pub repo: String,
    /// Session display name (e.g. `shell 1`).
    pub session: String,
    /// Why the session needs you, for the notification body wording.
    pub reason: NeedsYouReason,
    /// Epoch ms when the session entered needs-you (from
    /// `SessionData::needs_since_ms`), or `None` if the snapshot didn't stamp
    /// it. Edges within one `observe` are ordered oldest-first by this.
    pub needs_since_ms: Option<i64>,
}

/// Tracks each session's previous needs-you state across snapshots and yields
/// the false→true edges. Sessions that vanish are forgotten, so a session
/// that disappears and later reappears still blocked fires again (it re-entered
/// needing you from the watcher's point of view).
#[derive(Debug, Default)]
pub struct NeedsYouWatch {
    /// session id → whether it needed you in the previous snapshot.
    prev: HashMap<String, bool>,
    /// False until the first observation has primed the baseline.
    primed: bool,
}

impl NeedsYouWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diff `payload` against the previous snapshot and return the sessions
    /// that newly need you. Updates the baseline as a side effect.
    pub fn observe(&mut self, payload: &StatePayload) -> Vec<NeedsYouEdge> {
        let mut edges = Vec::new();
        let mut current: HashMap<String, bool> = HashMap::with_capacity(self.prev.len());

        for repo in &payload.repos {
            for folder in &repo.folders {
                for s in &folder.sessions {
                    let reason = needs_reason(s);
                    let needs = reason.is_some();
                    let was = self.prev.get(&s.id).copied().unwrap_or(false);
                    if self.primed
                        && !was
                        && let Some(reason) = reason
                    {
                        edges.push(NeedsYouEdge {
                            session_id: s.id.clone(),
                            repo: repo.name.clone(),
                            session: s.name.clone(),
                            reason,
                            needs_since_ms: s.needs_since_ms,
                        });
                    }
                    current.insert(s.id.clone(), needs);
                }
            }
        }

        // Oldest-first: when several sessions flip in the same snapshot, the
        // one that's been waiting longest surfaces first. A missing stamp sorts
        // last (treated as "just now"); ties keep render (repo→folder→session)
        // order since the sort is stable.
        edges.sort_by_key(|e| e.needs_since_ms.unwrap_or(i64::MAX));

        self.prev = current;
        self.primed = true;
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentEvent, AgentStatus, FolderData, RepoData, SessionData};

    fn session(id: &str, name: &str, live: bool, status: Option<AgentStatus>) -> SessionData {
        SessionData {
            id: id.to_string(),
            name: name.to_string(),
            created_at: 0,
            live,
            shell_kind: None,
            unseen: false,
            needs_since_ms: None,
            agent_state: status.map(|s| AgentEvent {
                agent: "claude".into(),
                session: name.to_string(),
                status: s,
                ts: 1,
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

    fn payload(repo: &str, sessions: Vec<SessionData>) -> StatePayload {
        StatePayload {
            repos: vec![RepoData {
                meta: None,
                key: format!("path:/{repo}"),
                dir: format!("/{repo}"),
                name: repo.to_string(),
                origin_url: None,
                folders: vec![FolderData {
                    name: format!("{repo}-task-0"),
                    dir: format!("/{repo}"),
                    dir_missing: false,
                    branch: "main".into(),
                    is_worktree: false,
                    landed: None,
                    files_changed: 0,
                    lines_added: 0,
                    lines_removed: 0,
                    commits_ahead: 0,
                    commits_behind: 0,
                    dirty: false,
                    commits_unlanded: 0,
                    sessions,
                    needs: 0,
                    base_branch: None,
                    task_base_branch: None,
                    compared_base: String::new(),
                    metadata: None,
                    has_port_drift: false,
                    has_launch_config: false,
                    quiet: false,
                }],
                needs: 0,
            }],
            theme: None,
            preferred_editor: String::new(),
            compact_recommend_percent: 30,
            windows: crate::windows::WindowsPayload::default(),
            collapsed: Default::default(),
            ts: 0,
        }
    }

    #[test]
    fn first_observation_primes_without_firing() {
        let mut w = NeedsYouWatch::new();
        // Already blocked at launch: a level, not a flip — no edge.
        let p = payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Waiting))]);
        assert!(w.observe(&p).is_empty());
        // And it stays quiet while the state holds.
        assert!(w.observe(&p).is_empty());
    }

    #[test]
    fn flip_into_needs_you_fires_once() {
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]));

        let blocked =
            payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Waiting))]);
        let edges = w.observe(&blocked);
        assert_eq!(
            edges,
            vec![NeedsYouEdge {
                session_id: "s1".into(),
                repo: "repo".into(),
                session: "shell 1".into(),
                reason: NeedsYouReason::WaitingForInput,
                needs_since_ms: None,
            }]
        );
        // Still blocked next snapshot: no repeat.
        assert!(w.observe(&blocked).is_empty());
    }

    /// The edge's reason mirrors the status that tripped `session_needs`.
    #[test]
    fn reason_matches_triggering_status() {
        // Waiting → waiting for input.
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]));
        let edges = w.observe(&payload(
            "repo",
            vec![session("s1", "shell 1", true, Some(AgentStatus::Waiting))],
        ));
        assert_eq!(edges[0].reason, NeedsYouReason::WaitingForInput);

        // Error → errored.
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]));
        let edges = w.observe(&payload(
            "repo",
            vec![session("s1", "shell 1", true, Some(AgentStatus::Error))],
        ));
        assert_eq!(edges[0].reason, NeedsYouReason::Errored);

        // Complete + unseen → finished.
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]));
        let mut done = session("s1", "shell 1", true, Some(AgentStatus::Complete));
        done.unseen = true;
        let edges = w.observe(&payload("repo", vec![done]));
        assert_eq!(edges[0].reason, NeedsYouReason::Finished);

        // Interrupted + unseen → finished.
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]));
        let mut interrupted = session("s1", "shell 1", true, Some(AgentStatus::Interrupted));
        interrupted.unseen = true;
        let edges = w.observe(&payload("repo", vec![interrupted]));
        assert_eq!(edges[0].reason, NeedsYouReason::Finished);
    }

    #[test]
    fn refires_after_leaving_and_reentering() {
        let mut w = NeedsYouWatch::new();
        let busy = payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]);
        let blocked =
            payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Waiting))]);
        w.observe(&busy);
        assert_eq!(w.observe(&blocked).len(), 1);
        assert!(w.observe(&busy).is_empty()); // back to work
        assert_eq!(w.observe(&blocked).len(), 1); // blocked again → new edge
    }

    #[test]
    fn new_session_appearing_blocked_fires() {
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![]));
        let edges = w.observe(&payload(
            "repo",
            vec![session("s2", "shell 2", true, Some(AgentStatus::Error))],
        ));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].session, "shell 2");
        assert_eq!(edges[0].reason, NeedsYouReason::Errored);
    }

    #[test]
    fn concurrent_flips_are_ordered_oldest_first() {
        let mut w = NeedsYouWatch::new();
        // Prime with both busy (nothing needing yet).
        w.observe(&payload(
            "repo",
            vec![
                session("fresh", "shell fresh", true, Some(AgentStatus::Busy)),
                session("old", "shell old", true, Some(AgentStatus::Busy)),
            ],
        ));
        // Both flip to waiting in the same snapshot, but `old` entered earlier
        // (smaller needs_since_ms). Render order puts `fresh` first.
        let mut fresh = session("fresh", "shell fresh", true, Some(AgentStatus::Waiting));
        fresh.needs_since_ms = Some(5_000);
        let mut old = session("old", "shell old", true, Some(AgentStatus::Waiting));
        old.needs_since_ms = Some(1_000);
        let edges = w.observe(&payload("repo", vec![fresh, old]));
        let ids: Vec<&str> = edges.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(ids, vec!["old", "fresh"]);
        assert_eq!(edges[0].needs_since_ms, Some(1_000));
    }

    #[test]
    fn dead_shell_never_fires() {
        let mut w = NeedsYouWatch::new();
        w.observe(&payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Busy))]));
        // Agent says waiting but the PTY is gone: session_needs is false.
        let stale =
            payload("repo", vec![session("s1", "shell 1", false, Some(AgentStatus::Waiting))]);
        assert!(w.observe(&stale).is_empty());
    }

    #[test]
    fn vanished_sessions_are_forgotten() {
        let mut w = NeedsYouWatch::new();
        let blocked =
            payload("repo", vec![session("s1", "shell 1", true, Some(AgentStatus::Waiting))]);
        w.observe(&blocked); // primes with s1 needing
        w.observe(&payload("repo", vec![])); // s1 vanished
        // Reappearing still blocked is a fresh entry into needs-you.
        assert_eq!(w.observe(&blocked).len(), 1);
    }
}
