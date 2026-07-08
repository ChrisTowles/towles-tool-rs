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
//!   event to the PTY `TT_SESSION_ID` it ran in. An attributed event renders
//!   only on that exact session (an id foreign to this folder's records is
//!   dropped, never guessed); only events with no attribution at all fall back
//!   to the folder's default (first) session.

use std::collections::HashMap;

use serde::Serialize;

use crate::folder_meta::FolderMetaStore;
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
    /// Context-% at/above which a cold session shows the "compact" nudge
    /// (settings `agentboard.compactRecommendPercent`, default 30).
    pub compact_recommend_percent: u8,
    /// Persisted window layout (frontend-owned; attached by the engine).
    pub windows: crate::windows::WindowsPayload,
    /// Persisted folder-rail collapse/expand state, keyed by row key (attached
    /// by the engine). Absent key ⇒ expanded.
    pub collapsed: std::collections::BTreeMap<String, bool>,
    pub ts: i64,
}

/// Assemble the [`StatePayload`] from the current inputs. Pure. Maps each repo
/// entry to a [`FolderData`] (git stats + persisted sessions + attributed
/// agents + `needs`), then groups folders into [`RepoData`] by origin URL.
///
/// `attribute` maps an agent event to the PTY session id it was detected in
/// (via `TT_SESSION_ID`); an id that matches none of the folder's records drops
/// the event (it lives in another instance's session). Return `None` — no
/// attribution machinery for this event — to fall back to the folder's default
/// session. `session_agents` (keyed by session id) supplements the tracker with
/// app-spawned agents the CLI snapshot missed — used only for sessions that end
/// up with no tracker-attributed state. Entries are assumed pre-sorted by the
/// caller (the engine sorts by name); repo grouping preserves first-seen order.
#[allow(clippy::too_many_arguments)]
pub fn assemble_state(
    entries: &[RepoEntry],
    git_infos: &HashMap<String, GitInfo>,
    tracker: &AgentTracker,
    metadata: &SessionMetadataStore,
    sessions: &SessionStore,
    folder_meta: &FolderMetaStore,
    attribute: &dyn Fn(&AgentEvent) -> Option<String>,
    session_agents: &HashMap<String, AgentEvent>,
    theme: Option<String>,
    preferred_editor: &str,
    compact_recommend_percent: u8,
    ts: i64,
) -> StatePayload {
    let mut repos: Vec<RepoData> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for entry in entries {
        let git = git_infos.get(&entry.dir).cloned().unwrap_or_default();
        let folder = build_folder(
            entry,
            &git,
            tracker,
            metadata,
            sessions,
            folder_meta,
            attribute,
            session_agents,
        );

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

    StatePayload {
        repos,
        theme,
        preferred_editor: preferred_editor.to_string(),
        compact_recommend_percent,
        windows: crate::windows::WindowsPayload::default(), // engine attaches
        collapsed: std::collections::BTreeMap::new(),       // engine attaches
        ts,
    }
}

/// Build one folder: git stats + its persisted sessions with agents distributed
/// by `attribute` (attributed → that exact session or dropped; no attribution →
/// default session), plus a placeholder `needs` count (always 0 here — see
/// [`session_needs`] — the app recomputes it after stamping shell liveness via
/// [`recompute_needs`]).
#[allow(clippy::too_many_arguments)]
fn build_folder(
    entry: &RepoEntry,
    git: &GitInfo,
    tracker: &AgentTracker,
    metadata: &SessionMetadataStore,
    sessions: &SessionStore,
    folder_meta: &FolderMetaStore,
    attribute: &dyn Fn(&AgentEvent) -> Option<String>,
    session_agents: &HashMap<String, AgentEvent>,
) -> FolderData {
    let records = sessions.sessions_for(&entry.dir);
    let folder_agents = tracker.get_agents(&entry.name);
    let default_id = records.first().map(|r| r.id.clone());

    // Bucket each agent onto the session it ran in. A positively attributed
    // agent renders only on that exact record: an id that isn't one of this
    // folder's records means the agent runs in some *other* app instance's
    // session (sessions.json is shared across windows/slots), and dropping it
    // beats pinning someone else's agent — name, cache chip and all — onto an
    // unrelated pane. Only agents with no attribution machinery at all (kinds
    // without a pid→TT_SESSION_ID path, non-Linux hosts) fall back to the
    // folder's default (first) session.
    let mut by_session: HashMap<String, Vec<AgentEvent>> = HashMap::new();
    for agent in folder_agents {
        let sid = match attribute(&agent) {
            Some(id) => records.iter().any(|r| r.id == id).then_some(id),
            None => default_id.clone(),
        };
        if let Some(sid) = sid {
            by_session.entry(sid).or_default().push(agent);
        }
    }

    let session_data: Vec<SessionData> = records
        .iter()
        .map(|r| {
            let agents = by_session.remove(&r.id).unwrap_or_default();
            let agent_state = pick_state(&agents);
            // Supplement: an app-spawned Claude we found by scanning /proc for
            // this session's TT_SESSION_ID, when the CLI snapshot never reported
            // it (so the tracker has nothing). Only fills an otherwise-idle row.
            let agent_state = agent_state.or_else(|| session_agents.get(&r.id).cloned());
            let unseen = agent_state.as_ref().and_then(|e| e.unseen).unwrap_or(false);
            SessionData {
                id: r.id.clone(),
                name: r.name.clone(),
                created_at: r.created_at,
                live: false,      // stamped by the app from its PTY registry
                shell_kind: None, // stamped by the app from its PTY registry
                unseen,
                agent_state,
                agents,
                purpose: r.purpose.clone(),
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
        purpose: folder_meta.purpose_for(&entry.dir).map(str::to_string),
        metadata: metadata.get(&entry.name).cloned(),
    }
}

/// Whether a session "needs you". A session only counts if a shell actually
/// exists for it (`live`) — otherwise a stale agent status would make the
/// day-bar cry wolf about a shell that's gone. Given a real shell, it needs
/// you when its agent is blocked (`Waiting`) or broke (`Error`), or when its
/// turn just ended (`Complete` / `Interrupted`) and the user hasn't looked
/// yet (`unseen`, cleared by `ab_mark_seen` when the row is selected).
///
/// Note: `live` is stamped app-side (see `recompute_needs`), so at engine
/// assemble time (always false) this is always `false` — assemble-time
/// `needs` is a placeholder the app overwrites.
pub fn session_needs(s: &SessionData) -> bool {
    if !s.live {
        return false;
    }
    match s.agent_state.as_ref().map(|e| e.status) {
        Some(AgentStatus::Waiting) | Some(AgentStatus::Error) => true,
        Some(AgentStatus::Complete) | Some(AgentStatus::Interrupted) => s.unseen,
        _ => false,
    }
}

/// Recompute every folder's and repo's `needs` from its sessions with
/// [`session_needs`]. The engine assembles `needs` before the app has stamped
/// `live` (so every count is a 0 placeholder); the app calls this after
/// stamping so every payload it emits carries truthful counts.
pub fn recompute_needs(payload: &mut StatePayload) {
    for repo in &mut payload.repos {
        let mut repo_needs = 0;
        for folder in &mut repo.folders {
            folder.needs = folder.sessions.iter().filter(|s| session_needs(s)).count() as i64;
            repo_needs += folder.needs;
        }
        repo.needs = repo_needs;
    }
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
            &FolderMetaStore::default(),
            &no_attr,
            &HashMap::new(),
            Some("mocha".into()),
            "code",
            30,
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
        let payload = assemble_state(
            &entries,
            &git,
            &tracker,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &no_attr,
            &HashMap::new(),
            None,
            "code",
            30,
            0,
        );
        // One repo, two folders (the checkouts).
        assert_eq!(payload.repos.len(), 1);
        assert_eq!(payload.repos[0].name, "proj");
        assert_eq!(payload.repos[0].folders.len(), 2);
    }

    /// A `SessionData` with just the fields `session_needs` reads set; the rest
    /// are inert defaults.
    fn session(live: bool, status: Option<AgentStatus>, unseen: bool) -> SessionData {
        SessionData {
            id: "s".into(),
            name: "shell 1".into(),
            created_at: 0,
            live,
            shell_kind: None,
            unseen,
            agent_state: status.map(|status| AgentEvent {
                agent: "claude-code".into(),
                session: "s".into(),
                status,
                ts: 1,
                thread_id: None,
                thread_name: None,
                unseen: Some(unseen),
                pane_id: None,
                details: None,
            }),
            agents: Vec::new(),
            purpose: None,
        }
    }

    #[test]
    fn session_needs_requires_a_shell_and_attention_state() {
        // Live + waiting/error counts.
        assert!(session_needs(&session(true, Some(AgentStatus::Waiting), false)));
        assert!(session_needs(&session(true, Some(AgentStatus::Error), false)));
        // Not live: a stale status must NOT count.
        assert!(!session_needs(&session(false, Some(AgentStatus::Waiting), false)));
        assert!(!session_needs(&session(false, Some(AgentStatus::Error), false)));
        // Live, ended turn, unseen → your turn, counts. Seen → doesn't.
        assert!(session_needs(&session(true, Some(AgentStatus::Complete), true)));
        assert!(!session_needs(&session(true, Some(AgentStatus::Complete), false)));
        assert!(session_needs(&session(true, Some(AgentStatus::Interrupted), true)));
        // Live but busy/idle/no-agent never needs you.
        assert!(!session_needs(&session(true, Some(AgentStatus::Busy), false)));
        assert!(!session_needs(&session(true, Some(AgentStatus::Idle), false)));
        assert!(!session_needs(&session(true, None, false)));
    }

    #[test]
    fn assemble_time_needs_is_zero_before_stamping() {
        // The engine assembles live=false, so even a waiting agent yields
        // needs=0 until the app stamps shell liveness and recomputes.
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Waiting, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        store.ensure_default("/r/alpha", 1);
        let git = HashMap::new();
        let entries = vec![RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() }];
        let payload = assemble_state(
            &entries,
            &git,
            &tracker,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &no_attr,
            &HashMap::new(),
            None,
            "code",
            30,
            0,
        );
        assert_eq!(payload.repos[0].folders[0].needs, 0);
        assert_eq!(payload.repos[0].needs, 0);
    }

    #[test]
    fn recompute_needs_bubbles_folder_to_repo() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Waiting, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        store.ensure_default("/r/alpha", 1);
        let git = HashMap::new();
        let entries = vec![RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() }];
        let mut payload = assemble_state(
            &entries,
            &git,
            &tracker,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &no_attr,
            &HashMap::new(),
            None,
            "code",
            30,
            0,
        );
        // Simulate the app stamping the session's shell as live, then recompute.
        payload.repos[0].folders[0].sessions[0].live = true;
        recompute_needs(&mut payload);
        assert_eq!(payload.repos[0].folders[0].needs, 1);
        assert_eq!(payload.repos[0].needs, 1);

        // Stamp it back to no shell: needs falls to 0 at both levels.
        payload.repos[0].folders[0].sessions[0].live = false;
        recompute_needs(&mut payload);
        assert_eq!(payload.repos[0].folders[0].needs, 0);
        assert_eq!(payload.repos[0].needs, 0);
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
            &entries,
            &git,
            &tracker,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &attribute,
            &HashMap::new(),
            None,
            "code",
            30,
            0,
        );
        let folder = &payload.repos[0].folders[0];
        let one = folder.sessions.iter().find(|s| s.id == s1.id).unwrap();
        let two = folder.sessions.iter().find(|s| s.id == s2.id).unwrap();
        assert!(one.agent_state.is_none());
        assert_eq!(two.agent_state.as_ref().unwrap().status, AgentStatus::Busy);
    }

    #[test]
    fn foreign_attribution_is_dropped_not_defaulted() {
        // An agent positively attributed to a session id that matches none of
        // this folder's records runs in some other app instance's session
        // (sessions.json is shared) — it must not land on the default session,
        // even when the folder has only one (the old single-session mirror
        // leaked folder-level state here).
        let mut tracker = AgentTracker::new();
        tracker.apply_event(ev("alpha", AgentStatus::Busy, "ta"), false);
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        store.add("/r/alpha", Some("one"), 1);
        let git = HashMap::new();
        let entries = vec![RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() }];
        let attribute = |_: &AgentEvent| Some("someone-elses-session".to_string());
        let payload = assemble_state(
            &entries,
            &git,
            &tracker,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &attribute,
            &HashMap::new(),
            None,
            "code",
            30,
            0,
        );
        let folder = &payload.repos[0].folders[0];
        assert!(folder.sessions[0].agent_state.is_none());
        assert!(folder.sessions[0].agents.is_empty());
    }

    #[test]
    fn session_agents_supplement_idle_sessions_only() {
        // No tracker agent: the /proc-detected session agent fills the row.
        let tracker = AgentTracker::new();
        let metadata = SessionMetadataStore::new();
        let mut store = SessionStore::new(None);
        let rec = store.add("/r/alpha", Some("shell 1"), 1);
        let git = HashMap::new();
        let entries = vec![RepoEntry { name: "alpha".into(), dir: "/r/alpha".into() }];

        let mut supplemental = HashMap::new();
        supplemental.insert(
            rec.id.clone(),
            AgentEvent {
                agent: "claude-code".into(),
                session: String::new(),
                status: AgentStatus::Busy,
                ts: 5,
                thread_id: None,
                thread_name: Some("uninstall gitbutler".into()),
                unseen: None,
                pane_id: None,
                details: None,
            },
        );
        let payload = assemble_state(
            &entries,
            &git,
            &tracker,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &no_attr,
            &supplemental,
            None,
            "code",
            30,
            0,
        );
        let s = &payload.repos[0].folders[0].sessions[0];
        assert_eq!(s.agent_state.as_ref().unwrap().status, AgentStatus::Busy);
        assert_eq!(
            s.agent_state.as_ref().unwrap().thread_name.as_deref(),
            Some("uninstall gitbutler")
        );

        // With a real tracker agent, the CLI/tracker state wins over the supplement.
        let mut tracker2 = AgentTracker::new();
        tracker2.apply_event(ev("alpha", AgentStatus::Waiting, "ta"), false);
        let payload2 = assemble_state(
            &entries,
            &git,
            &tracker2,
            &metadata,
            &store,
            &FolderMetaStore::default(),
            &no_attr,
            &supplemental,
            None,
            "code",
            30,
            0,
        );
        let s2 = &payload2.repos[0].folders[0].sessions[0];
        assert_eq!(s2.agent_state.as_ref().unwrap().status, AgentStatus::Waiting);
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
