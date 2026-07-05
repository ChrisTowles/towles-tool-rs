//! The agentboard engine: tracker + metadata + session-order + git cache +
//! watchers behind one struct, host-agnostic. Extracted from
//! `crates-tauri/tt-app/src/agentboard.rs` (phase T3 of
//! docs/AGENTBOARD-PORT.md) so both hosts share it:
//!
//! - the Tauri app drives it repos.json-first ([`Engine::scan_once`],
//!   [`Engine::compute_payload`]);
//! - the tmux-mode server derives entries from live tmux sessions and uses the
//!   `*_for_entries` / `*_with_resolver` variants.
//!
//! The engine is synchronous; hosts own scheduling (tokio tasks, debounces)
//! and transport (Tauri events, SSE).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::metadata::{LogInput, ProgressInput, StatusInput};
use crate::session_order::ReorderDelta;
use crate::types::{AgentEvent, AgentStatus, MetadataTone};
use crate::{
    AgentTracker, AgentWatcher, AmpAgentWatcher, ClaudeCodeAgentWatcher, CodexAgentWatcher,
    GitInfoCache, OpenCodeAgentWatcher, RepoEntry, SessionMetadataStore, SessionOrder,
    SessionRecord, SessionStore, StatePayload, WatcherContext, add_repo, assemble_state,
    default_repos_path, default_sessions_path, expand_slot_siblings, instance_key, load_repos,
    load_scan_roots, remove_repo_by_name, repo_entries, resolve_session_name, save_repos,
    save_scan_roots,
};

// Prune schedule constants (BRIDGE-SPEC §4).
const STUCK_MS: i64 = 3 * 60 * 1000;
const STALE_MS: i64 = 12 * 60 * 60 * 1000;
const IDLE_MS: i64 = 30 * 1000;

/// Wall-clock epoch milliseconds (the hosts' `now`).
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn parse_tone(tone: Option<String>) -> Option<MetadataTone> {
    match tone.as_deref() {
        Some("neutral") => Some(MetadataTone::Neutral),
        Some("info") => Some(MetadataTone::Info),
        Some("success") => Some(MetadataTone::Success),
        Some("warn") => Some(MetadataTone::Warn),
        Some("error") => Some(MetadataTone::Error),
        _ => None,
    }
}

/// Collects watcher emits during a scan, resolving project dirs (and, in
/// tmux mode, agent pids) to session names through injected resolvers.
struct CollectCtx<'a> {
    resolve: &'a dyn Fn(&str) -> Option<String>,
    resolve_pid: &'a dyn Fn(i32) -> Option<String>,
    events: Vec<AgentEvent>,
}

impl WatcherContext for CollectCtx<'_> {
    fn resolve_session(&self, project_dir: &str) -> Option<String> {
        (self.resolve)(project_dir)
    }
    fn resolve_session_by_pid(&self, pid: i32) -> Option<String> {
        (self.resolve_pid)(pid)
    }
    fn emit(&mut self, event: AgentEvent) {
        self.events.push(event);
    }
}

/// Owns all agentboard engine state. Hosts guard it with a `Mutex`.
pub struct Engine {
    projects_dir: PathBuf,
    repos_path: PathBuf,
    repo_paths: Vec<String>,
    tracker: AgentTracker,
    metadata: SessionMetadataStore,
    order: SessionOrder,
    folder_meta: crate::folder_meta::FolderMetaStore,
    windows: crate::windows::WindowsStore,
    sessions: SessionStore,
    git_cache: GitInfoCache,
    watchers: Vec<Box<dyn AgentWatcher + Send>>,
    theme: Option<String>,
    preferred_editor: String,
    compact_recommend_percent: u8,
    seeded_once: bool,
    last_payload: Option<StatePayload>,
}

impl Engine {
    /// Build from the real config locations (`~/.claude`, `~/.config/towles-tool`).
    pub fn new() -> Self {
        let projects_dir =
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("projects");
        let repos_path = default_repos_path();
        let order_path = crate::default_session_order_path();

        let settings = tt_config::load().unwrap_or_default();
        let theme = settings.agentboard.theme.and_then(|v| v.as_str().map(str::to_string));
        let preferred_editor = settings.preferred_editor;
        let compact_recommend_percent = settings
            .agentboard
            .compact_recommend_percent
            .unwrap_or(tt_config::DEFAULT_COMPACT_RECOMMEND_PERCENT);

        Self {
            projects_dir: projects_dir.clone(),
            repo_paths: load_repos(&repos_path),
            repos_path,
            tracker: AgentTracker::new(),
            metadata: SessionMetadataStore::new(),
            order: SessionOrder::new(Some(order_path)),
            sessions: SessionStore::new(Some(default_sessions_path())),
            folder_meta: crate::folder_meta::FolderMetaStore::new(Some(
                crate::folder_meta::default_folder_meta_path(),
            )),
            windows: crate::windows::WindowsStore::new(Some(crate::default_windows_path())),
            git_cache: GitInfoCache::new(),
            watchers: vec![
                Box::new(ClaudeCodeAgentWatcher::with_defaults()),
                Box::new(AmpAgentWatcher::with_defaults()),
                Box::new(CodexAgentWatcher::with_defaults()),
                Box::new(OpenCodeAgentWatcher::with_defaults()),
            ],
            theme,
            preferred_editor,
            compact_recommend_percent,
            seeded_once: false,
            last_payload: None,
        }
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.projects_dir.clone()
    }

    /// Re-read `repos.json` so changes made by the `ttr agentboard` CLI (which
    /// writes the same file) are picked up without restarting the host.
    fn reload_repos(&mut self) {
        self.repo_paths = load_repos(&self.repos_path);
    }

    /// The repo paths actually scanned/displayed: the persisted config plus
    /// any auto-discovered slot siblings (`tt:parallel-slots` convention).
    /// Computed fresh — `self.repo_paths` itself stays the raw, persisted
    /// list so add/remove keep operating on what the user actually configured.
    fn active_repo_paths(&self) -> Vec<String> {
        expand_slot_siblings(&self.repo_paths)
    }

    /// One scan of every watcher with the repos.json-derived resolver
    /// (desktop mode).
    pub fn scan_once(&mut self, now: i64) {
        self.reload_repos();
        let entries = repo_entries(&self.active_repo_paths());
        self.scan_once_with_resolvers(&|dir| resolve_session_name(dir, &entries), &|_| None, now);
    }

    /// One scan of every watcher: collect emits through the resolvers and feed
    /// them to the tracker (first scan's emits are seeded → marked unseen).
    pub fn scan_once_with_resolvers(
        &mut self,
        resolve: &dyn Fn(&str) -> Option<String>,
        resolve_pid: &dyn Fn(i32) -> Option<String>,
        now: i64,
    ) {
        let mut ctx = CollectCtx { resolve, resolve_pid, events: Vec::new() };
        for watcher in &mut self.watchers {
            watcher.scan(&mut ctx, now);
        }
        let seed = !self.seeded_once;
        for event in ctx.events {
            self.tracker.apply_event(event, seed);
        }
        self.seeded_once = true;
    }

    /// The absolute dir for a session name, if configured (for open-in-editor).
    pub fn repo_dir_for(&mut self, name: &str) -> Option<String> {
        self.reload_repos();
        repo_entries(&self.active_repo_paths()).into_iter().find(|e| e.name == name).map(|e| e.dir)
    }

    /// The configured preferred editor command.
    pub fn preferred_editor(&self) -> String {
        self.preferred_editor.clone()
    }

    /// Refresh git info for each watched repo (runs git subprocesses).
    pub fn refresh_git(&mut self, now: i64) {
        for entry in repo_entries(&self.active_repo_paths()) {
            self.git_cache.refresh(&entry.dir, now);
        }
    }

    /// Refresh git info for an arbitrary dir list (tmux-session mode).
    pub fn refresh_git_dirs(&mut self, dirs: &[String], now: i64) {
        for dir in dirs {
            self.git_cache.refresh(dir, now);
        }
    }

    /// Full recompute from repos.json (desktop mode). Base order is by name
    /// (createdAt is meaningless for configured repos).
    pub fn compute_payload(&mut self, now: i64) -> StatePayload {
        self.reload_repos();
        let mut entries = repo_entries(&self.active_repo_paths());
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        // New folders get a default `shell 1` seeded once; a folder whose
        // sessions were all closed stays empty (rendered as "no sessions").
        let mut seeded = false;
        for entry in &entries {
            if self.sessions.ensure_default(&entry.dir, now) {
                seeded = true;
            }
        }
        if seeded {
            let _ = self.sessions.save();
        }
        let payload = self.compute_payload_for_entries(&entries, now);
        // Drop metadata + session records for repos no longer configured.
        let names: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
        let dirs: HashSet<String> = entries.iter().map(|e| e.dir.clone()).collect();
        self.metadata.prune_sessions(&names);
        self.sessions.prune(&dirs);
        self.folder_meta.prune(&dirs);
        payload
    }

    /// Set (or clear) a folder's user-authored purpose. Persists on change.
    pub fn set_folder_purpose(&mut self, dir: &str, purpose: Option<&str>) -> bool {
        let changed = self.folder_meta.set_purpose(dir, purpose);
        if changed {
            let _ = self.folder_meta.save();
        }
        changed
    }

    /// Replace the persisted window layout (frontend-owned blob). Persists on
    /// change; returns whether it changed.
    pub fn set_windows(&mut self, payload: crate::windows::WindowsPayload) -> bool {
        let changed = self.windows.set(payload);
        if changed {
            let _ = self.windows.save();
        }
        changed
    }

    /// The current compact-nudge threshold (context-%).
    pub fn compact_recommend_percent(&self) -> u8 {
        self.compact_recommend_percent
    }

    /// Set the compact-nudge threshold and persist it to the shared settings
    /// file (`agentboard.compactRecommendPercent`). Clamped to 1..=100.
    pub fn set_compact_recommend_percent(&mut self, percent: u8) -> bool {
        let percent = percent.clamp(1, 100);
        if percent == self.compact_recommend_percent {
            return false;
        }
        self.compact_recommend_percent = percent;
        if let Ok(mut settings) = tt_config::load() {
            settings.agentboard.compact_recommend_percent = Some(percent);
            let _ = tt_config::save(&settings);
        }
        true
    }

    /// Full recompute for the given entries: pid-liveness pin → prune
    /// schedule → assemble snapshot. The tmux server passes entries derived
    /// from live tmux sessions.
    pub fn compute_payload_for_entries(&mut self, entries: &[RepoEntry], now: i64) -> StatePayload {
        // CLI-derived liveness drives the pruning pins (§4; T7 replaced the
        // ~/.claude/sessions pid files; the waiting synthesis is gone — the
        // claude watcher emits CLI-authoritative statuses directly).
        let cli_agents = crate::claude_cli::fetch_agents_cached(std::time::Duration::from_millis(
            crate::watchers::claude_code::CLI_CACHE_TTL_MS,
        ));
        let live_threads: HashSet<String> =
            cli_agents.iter().map(|a| a.session_id.clone()).collect();
        // Link each live agent to the PTY session it runs in: read `TT_SESSION_ID`
        // from the agent's process env (/proc), keyed by its thread id (==
        // sessionId). Agents with no injected id (e.g. started in an external
        // terminal, or non-Claude kinds without a pid source) stay unmapped and
        // fall back to their folder's default session in `assemble_state`.
        let tt_session_by_thread: HashMap<String, String> = cli_agents
            .iter()
            .filter_map(|a| {
                crate::procenv::read_session_id(a.pid).map(|sid| (a.session_id.clone(), sid))
            })
            .collect();
        let mut pinned: HashMap<String, Vec<String>> = HashMap::new();
        for entry in entries {
            for agent in self.tracker.get_agents(&entry.name) {
                let Some(tid) = agent.thread_id.clone() else {
                    continue;
                };
                if live_threads.contains(&tid) {
                    pinned
                        .entry(entry.name.clone())
                        .or_default()
                        .push(instance_key(&agent.agent, Some(&tid)));
                }
            }
        }
        self.tracker.set_pinned_instances_multi(&pinned);

        // Prune schedule — every broadcast (§4).
        self.tracker.prune_stuck(STUCK_MS, now);
        self.tracker.prune_terminal(now);
        self.tracker.prune_stale(STALE_MS, now);
        self.tracker.prune_idle(IDLE_MS, now);
        self.tracker.prune_superseded_by_pane();

        let mut git_infos = HashMap::new();
        for entry in entries {
            git_infos.insert(entry.dir.clone(), self.git_cache.get(&entry.dir));
        }

        let theme = self.theme.clone();
        let editor = self.preferred_editor.clone();
        // Attribute each agent event to the PTY session it ran in, joining the
        // event's thread id (== the CLI sessionId) to the `TT_SESSION_ID` read
        // from that agent's process. Unmatched → folder's default session.
        let attribute = |event: &AgentEvent| {
            event.thread_id.as_ref().and_then(|tid| tt_session_by_thread.get(tid).cloned())
        };
        // Supplement CLI detection: app-spawned Claude sessions the CLI snapshot
        // never enumerated, found by scanning /proc for our injected
        // TT_SESSION_ID and enriched with task name + status from the
        // transcript the process has open. Keyed by session id; consumed only
        // for sessions the tracker left idle. First live process per id wins.
        let mut session_agents: HashMap<String, AgentEvent> = HashMap::new();
        for proc in crate::procenv::scan_session_agents() {
            if session_agents.contains_key(&proc.session_id) {
                continue;
            }
            let (thread_name, status) = match &proc.transcript {
                Some(p) => crate::watchers::claude_code::enrich_from_transcript(p),
                None => (None, AgentStatus::Idle),
            };
            session_agents.insert(
                proc.session_id.clone(),
                AgentEvent {
                    agent: "claude-code".to_string(),
                    session: String::new(),
                    status,
                    ts: now,
                    thread_id: None,
                    thread_name,
                    unseen: None,
                    pane_id: None,
                    details: None,
                },
            );
        }
        let mut payload = assemble_state(
            entries,
            &git_infos,
            &self.tracker,
            &self.metadata,
            &self.sessions,
            &self.folder_meta,
            &attribute,
            &session_agents,
            theme,
            &editor,
            self.compact_recommend_percent,
            now,
        );

        payload.windows = self.windows.payload().clone();
        self.last_payload = Some(payload.clone());
        payload
    }

    /// Drop metadata for sessions not in `names` (tmux-session mode calls
    /// this with the live session set).
    pub fn prune_metadata_sessions(&mut self, names: &HashSet<String>) {
        self.metadata.prune_sessions(names);
    }

    /// Mark a folder's agents seen. Returns a fresh payload only if something
    /// changed. `unseen` now lives on individual sessions (derived from their
    /// agent state), so we recompute rather than patch the cached snapshot.
    pub fn mark_seen_patch(&mut self, name: &str) -> Option<StatePayload> {
        if !self.tracker.mark_seen(name) {
            return None;
        }
        Some(self.compute_payload(now_ms()))
    }

    /// Clear unseen flags for a session a client just focused. Returns whether
    /// anything was cleared (ports `tracker.handleFocus`).
    pub fn handle_focus(&mut self, name: &str) -> bool {
        self.tracker.handle_focus(name)
    }

    pub fn last_payload(&self) -> Option<StatePayload> {
        self.last_payload.clone()
    }

    /// Sessions currently being viewed at boot — they never count as unseen
    /// (ports `tracker.setActiveSessions`).
    pub fn set_active_sessions(&mut self, sessions: &[String]) {
        self.tracker.set_active_sessions(sessions);
    }

    /// Persist a pane assignment onto a tracked instance (pane merge, T6).
    pub fn assign_pane_id(
        &mut self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
        pane_id: &str,
    ) {
        self.tracker.assign_pane_id(session, agent, thread_id, pane_id);
    }

    pub fn dismiss(&mut self, session: &str, agent: &str, thread_id: Option<&str>) -> bool {
        self.tracker.dismiss(session, agent, thread_id)
    }

    pub fn reorder(&mut self, name: &str, delta: ReorderDelta) {
        self.order.reorder(name, delta);
    }

    /// Set the theme and persist it to the shared settings' `agentboard.theme`
    /// (interop-safe — that key exists in the TS schema).
    pub fn set_theme(&mut self, theme: String) {
        self.theme = Some(theme.clone());
        let mut settings = tt_config::load().unwrap_or_default();
        settings.agentboard.theme = Some(serde_json::Value::String(theme));
        let _ = tt_config::save(&settings);
    }

    /// Absolute dirs currently on the rail (freshly reloaded, including
    /// auto-discovered slot siblings), so the add-repo picker can exclude
    /// repos that are already shown.
    pub fn repo_dirs(&mut self) -> Vec<String> {
        self.reload_repos();
        self.active_repo_paths()
    }

    /// Configured scan roots for the add-repo picker (`scanRoots` in repos.json).
    /// Empty when unset — the caller substitutes its own default (`~/code`).
    pub fn scan_roots(&self) -> Vec<String> {
        load_scan_roots(&self.repos_path)
    }

    /// Persist the add-repo picker's scan roots, preserving the repo list.
    pub fn set_scan_roots(&mut self, roots: Vec<String>) {
        let _ = save_scan_roots(&self.repos_path, &roots);
    }

    pub fn add_repo(&mut self, path: &str) -> bool {
        let added = add_repo(&mut self.repo_paths, path);
        if added {
            let _ = save_repos(&self.repos_path, &self.repo_paths);
        }
        added
    }

    pub fn remove_repo(&mut self, name: &str) -> bool {
        let removed = remove_repo_by_name(&mut self.repo_paths, name);
        if removed {
            let _ = save_repos(&self.repos_path, &self.repo_paths);
        }
        removed
    }

    /// Add a PTY session to a folder and persist. Returns the created record.
    pub fn add_session(&mut self, dir: &str, name: Option<&str>, now: i64) -> SessionRecord {
        let record = self.sessions.add(dir, name, now);
        let _ = self.sessions.save();
        record
    }

    /// Rename a PTY session by id. Returns whether it changed.
    pub fn rename_session(&mut self, id: &str, name: &str) -> bool {
        let changed = self.sessions.rename(id, name);
        if changed {
            let _ = self.sessions.save();
        }
        changed
    }

    /// Remove a PTY session by id. Returns whether it was removed. (A folder left
    /// empty is re-seeded with a default shell on the next `compute_payload`.)
    pub fn close_session(&mut self, id: &str) -> bool {
        let removed = self.sessions.remove(id);
        if removed {
            let _ = self.sessions.save();
        }
        removed
    }

    pub fn set_status(&mut self, session: &str, input: Option<StatusInput>, now: i64) {
        self.metadata.set_status(session, input, now);
    }
    pub fn set_progress(&mut self, session: &str, input: Option<ProgressInput>, now: i64) {
        self.metadata.set_progress(session, input, now);
    }
    pub fn append_log(&mut self, session: &str, input: LogInput, now: i64) {
        self.metadata.append_log(session, input, now);
    }
    pub fn clear_logs(&mut self, session: &str) {
        self.metadata.clear_logs(session);
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}
