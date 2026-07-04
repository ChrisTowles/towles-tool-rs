//! Pure state-snapshot assembly for the Tauri bridge (Folder Rail). Builds the
//! three-level **Repo → Folder → Session** snapshot the React client renders.
//!
//! Everything here is pure (no tmux, no tauri, no I/O): the host gathers the
//! inputs (repos, git infos, tracker, metadata, persisted sessions, and the
//! agent→session attribution closure) and wires its runtime around
//! [`assemble_state`].
//!
//! - A [`FolderData`] is one checkout on disk (a `RepoEntry`), carrying its git
//!   stats and its 1..N PTY [`SessionData`]s.
//! - Folders group into a [`RepoData`] by `git remote get-url origin` (a
//!   remoteless folder stands alone under `key = "path:<dir>"`).
//! - Each folder's agent events (from the tracker, keyed by folder name) are
//!   distributed across its sessions by the `attribute` closure — which maps an
//!   event to the PTY `TT_SESSION_ID` it ran in. Unattributed events fall back
//!   to the folder's default (first) session.

use std::collections::HashMap;

use serde::Serialize;

use crate::git_info::GitInfo;
use crate::metadata::SessionMetadataStore;
use crate::repos::RepoEntry;
use crate::sessions::SessionStore;
use crate::tracker::AgentTracker;
use crate::types::{AgentEvent, AgentStatus, FolderData, RepoData, SessionData};

/// The state snapshot emitted to the client: repos, each grouping its folders,
/// each holding its PTY sessions.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePayload {
    pub repos: Vec<RepoData>,
    pub theme: Option<String>,
    pub preferred_editor: String,
    pub ts: i64,
}

/// Assemble the [`StatePayload`] from the current inputs. Pure. Maps each repo
/// entry to a [`FolderData`] (git stats + persisted sessions + attributed
/// agents + `needs`), then groups folders into [`RepoData`] by origin URL.
///
/// `attribute` maps an agent event to the PTY session id it was detected in
/// (via `TT_SESSION_ID`); return `None` to fall back to the folder's default
/// session. Entries are assumed pre-sorted by the caller (the engine sorts by
/// name); repo grouping preserves first-seen order.
#[allow(clippy::too_many_arguments)]
pub fn assemble_state(
    entries: &[RepoEntry],
    git_infos: &HashMap<String, GitInfo>,
    tracker: &AgentTracker,
    metadata: &SessionMetadataStore,
    sessions: &SessionStore,
    attribute: &dyn Fn(&AgentEvent) -> Option<String>,
    theme: Option<String>,
    preferred_editor: &str,
    ts: i64,
) -> StatePayload {
    let mut repos: Vec<RepoData> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for entry in entries {
        let git = git_infos.get(&entry.dir).cloned().unwrap_or_default();
        let folder = build_folder(entry, &git, tracker, metadata, sessions, attribute);

        let origin = git.origin_url.clone();
        let key = origin.clone().unwrap_or_else(|| format!("path:{}", entry.dir));
        let repo_name =
            origin.as_deref().and_then(repo_name_from_origin).unwrap_or_else(|| entry.name.clone());

        let needs = folder.needs;
        match index.get(&key) {
            Some(&i) => {
                repos[i].folders.push(folder);
                repos[i].needs += needs;
            }
            None => {
                index.insert(key.clone(), repos.len());
                repos.push(RepoData {
                    key,
                    name: repo_name,
                    origin_url: origin,
                    folders: vec![folder],
                    needs,
                });
            }
        }
    }

    StatePayload { repos, theme, preferred_editor: preferred_editor.to_string(), ts }
}

/// Build one folder: git stats + its persisted sessions with agents distributed
/// by `attribute` (unattributed → default session), plus the `needs` count.
fn build_folder(
    entry: &RepoEntry,
    git: &GitInfo,
    tracker: &AgentTracker,
    metadata: &SessionMetadataStore,
    sessions: &SessionStore,
    attribute: &dyn Fn(&AgentEvent) -> Option<String>,
) -> FolderData {
    let records = sessions.sessions_for(&entry.dir);
    let folder_agents = tracker.get_agents(&entry.name);
    let folder_state = tracker.get_state(&entry.name);
    let default_id = records.first().map(|r| r.id.clone());

    // Bucket each agent onto the session it ran in (attributed id if it matches a
    // real record, else the folder's default session).
    let mut by_session: HashMap<String, Vec<AgentEvent>> = HashMap::new();
    for agent in folder_agents {
        let sid = attribute(&agent)
            .filter(|id| records.iter().any(|r| &r.id == id))
            .or_else(|| default_id.clone());
        if let Some(sid) = sid {
            by_session.entry(sid).or_default().push(agent);
        }
    }

    let single = records.len() == 1;
    let session_data: Vec<SessionData> = records
        .iter()
        .map(|r| {
            let agents = by_session.remove(&r.id).unwrap_or_default();
            // Single-session folder mirrors the tracker's folder-level priority
            // pick exactly; multi-session folders pick from the session's subset.
            let agent_state = if single { folder_state.clone() } else { pick_state(&agents) };
            let unseen = agent_state.as_ref().and_then(|e| e.unseen).unwrap_or(false);
            SessionData {
                id: r.id.clone(),
                name: r.name.clone(),
                created_at: r.created_at,
                unseen,
                agent_state,
                agents,
            }
        })
        .collect();

    let needs = session_data.iter().filter(|s| session_needs(s)).count() as i64;

    FolderData {
        name: entry.name.clone(),
        dir: entry.dir.clone(),
        branch: git.branch.clone(),
        is_worktree: git.is_worktree,
        files_changed: git.files_changed,
        lines_added: git.lines_added,
        lines_removed: git.lines_removed,
        commits_delta: git.commits_delta,
        sessions: session_data,
        needs,
        metadata: metadata.get(&entry.name).cloned(),
    }
}

/// A session "needs you" when its agent is blocked or broke.
fn session_needs(s: &SessionData) -> bool {
    matches!(
        s.agent_state.as_ref().map(|e| e.status),
        Some(AgentStatus::Waiting) | Some(AgentStatus::Error)
    )
}

/// Priority ordering for picking a session's headline agent state: attention
/// (waiting/error) first, then working, then terminal states, then idle;
/// ties broken by recency.
fn pick_state(agents: &[AgentEvent]) -> Option<AgentEvent> {
    agents.iter().max_by_key(|e| (status_rank(e.status), e.ts)).cloned()
}

fn status_rank(s: AgentStatus) -> u8 {
    match s {
        AgentStatus::Waiting => 5,
        AgentStatus::Error => 4,
        AgentStatus::Busy => 3,
        AgentStatus::Interrupted => 2,
        AgentStatus::Complete => 1,
        AgentStatus::Idle => 0,
    }
}

/// The repo segment of an origin URL: strips a trailing `.git` / `/` and takes
/// the last path segment. Handles both `https://host/owner/repo.git` and
/// scp-style `git@host:owner/repo.git`.
fn repo_name_from_origin(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let seg = trimmed.rsplit(['/', ':']).next()?;
    (!seg.is_empty()).then(|| seg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentStatus;

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

    fn no_attr(_: &AgentEvent) -> Option<String> {
        None
    }

    #[test]
    fn folders_map_fields_and_seed_sessions() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Busy, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        store.ensure_default("/r/alpha", 1);
        store.ensure_default("/r/beta", 1);
        let mut git = HashMap::new();
        git.insert(
            "/r/alpha".to_string(),
            GitInfo {
                branch: "main".into(),
                files_changed: 3,
                lines_added: 10,
                lines_removed: 2,
                commits_delta: 1,
                origin_url: Some("git@github.com:me/alpha.git".into()),
                ..Default::default()
            },
        );
        let payload = assemble_state(
            &entries(),
            &git,
            &tracker,
            &metadata,
            &store,
            &no_attr,
            Some("mocha".into()),
            "code",
            999,
        );
        assert_eq!(payload.ts, 999);
        assert_eq!(payload.theme.as_deref(), Some("mocha"));
        // Two distinct origins (alpha has one; beta has none → path key) → two repos.
        assert_eq!(payload.repos.len(), 2);
        let alpha = &payload.repos[0];
        assert_eq!(alpha.name, "alpha"); // derived from origin repo segment
        assert_eq!(alpha.folders[0].branch, "main");
        assert_eq!(alpha.folders[0].files_changed, 3);
        assert_eq!(alpha.folders[0].sessions.len(), 1);
        // The folder's busy agent lands on its one session.
        assert_eq!(
            alpha.folders[0].sessions[0].agent_state.as_ref().unwrap().status,
            AgentStatus::Busy
        );
        // beta has no git info → standalone path-keyed repo, name = folder basename.
        assert!(payload.repos[1].key.starts_with("path:"));
        assert_eq!(payload.repos[1].name, "beta");
    }

    #[test]
    fn same_origin_folders_group_into_one_repo() {
        let tracker = AgentTracker::new();
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        store.ensure_default("/r/slot-0", 1);
        store.ensure_default("/r/slot-1", 1);
        let origin = "https://github.com/me/proj.git";
        let mut git = HashMap::new();
        for dir in ["/r/slot-0", "/r/slot-1"] {
            git.insert(
                dir.to_string(),
                GitInfo { origin_url: Some(origin.into()), ..Default::default() },
            );
        }
        let entries = vec![
            RepoEntry { name: "slot-0".into(), dir: "/r/slot-0".into() },
            RepoEntry { name: "slot-1".into(), dir: "/r/slot-1".into() },
        ];
        let payload =
            assemble_state(&entries, &git, &tracker, &metadata, &store, &no_attr, None, "code", 0);
        // One repo, two folders (the checkouts).
        assert_eq!(payload.repos.len(), 1);
        assert_eq!(payload.repos[0].name, "proj");
        assert_eq!(payload.repos[0].folders.len(), 2);
    }

    #[test]
    fn needs_bubbles_folder_to_repo() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Waiting, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        store.ensure_default("/r/alpha", 1);
        let git = HashMap::new();
        let entries = vec![RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() }];
        let payload =
            assemble_state(&entries, &git, &tracker, &metadata, &store, &no_attr, None, "code", 0);
        assert_eq!(payload.repos[0].folders[0].needs, 1);
        assert_eq!(payload.repos[0].needs, 1);
    }

    #[test]
    fn attribute_routes_agents_to_matching_session() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Busy, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        let s1 = store.add("/r/alpha", Some("one"), 1);
        let s2 = store.add("/r/alpha", Some("two"), 2);
        let git = HashMap::new();
        let entries = vec![RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() }];
        // Attribute the (only) busy agent to session two.
        let target = s2.id.clone();
        let attribute = move |_: &AgentEvent| Some(target.clone());
        let payload = assemble_state(
            &entries, &git, &tracker, &metadata, &store, &attribute, None, "code", 0,
        );
        let folder = &payload.repos[0].folders[0];
        let one = folder.sessions.iter().find(|s| s.id == s1.id).unwrap();
        let two = folder.sessions.iter().find(|s| s.id == s2.id).unwrap();
        assert!(one.agent_state.is_none());
        assert_eq!(two.agent_state.as_ref().unwrap().status, AgentStatus::Busy);
    }

    #[test]
    fn repo_name_from_origin_variants() {
        assert_eq!(repo_name_from_origin("git@github.com:me/proj.git").as_deref(), Some("proj"));
        assert_eq!(
            repo_name_from_origin("https://github.com/me/proj.git").as_deref(),
            Some("proj")
        );
        assert_eq!(repo_name_from_origin("https://github.com/me/proj/").as_deref(), Some("proj"));
    }
}
