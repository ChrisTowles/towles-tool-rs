//! The agentboard engine: tracker + metadata + session-order + git cache +
//! watchers behind one struct, host-agnostic. Extracted from
//! `crates-tauri/tt-app/src/agentboard.rs` (phase T3 of
//! docs/AGENTBOARD-TMUX-SPEC.md) so both hosts share it:
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
use crate::types::{AgentEvent, MetadataTone};
use crate::{
    AgentTracker, AgentWatcher, AmpAgentWatcher, ClaudeCodeAgentWatcher, CodexAgentWatcher,
    GitInfoCache, MetadataMutation, OpenCodeAgentWatcher, RepoEntry, SessionMetadataStore,
    SessionOrder, StatePayload, WatcherContext, add_repo, assemble_state, default_repos_path,
    instance_key, load_repos, remove_repo_by_name, repo_entries, resolve_session_name, save_repos,
};

// Prune schedule constants (BRIDGE-SPEC §4).
const STUCK_MS: i64 = 3 * 60 * 1000;
const STALE_MS: i64 = 12 * 60 * 60 * 1000;
const IDLE_MS: i64 = 30 * 1000;

/// Wall-clock epoch milliseconds (the hosts' `now`).
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Host/port for the agentboard HTTP listener, honoring `TT_AGENTBOARD_HOST`/
/// `TT_AGENTBOARD_PORT` (defaults `127.0.0.1:4201`), matching the TS server.
pub fn ingest_addr() -> (String, u16) {
    let host = std::env::var("TT_AGENTBOARD_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port =
        std::env::var("TT_AGENTBOARD_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(4201);
    (host, port)
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

/// Apply a validated metadata mutation to the engine.
pub fn apply_mutation(engine: &mut Engine, mutation: MetadataMutation, now: i64) {
    match mutation {
        MetadataMutation::SetStatus { session, text, tone } => {
            engine.set_status(&session, text.map(|t| StatusInput { text: t, tone }), now);
        }
        MetadataMutation::SetProgress { session, progress } => {
            engine.set_progress(&session, progress, now);
        }
        MetadataMutation::AppendLog { session, log } => {
            engine.append_log(&session, log, now);
        }
        MetadataMutation::ClearLogs { session } => {
            engine.clear_logs(&session);
        }
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
    git_cache: GitInfoCache,
    watchers: Vec<Box<dyn AgentWatcher + Send>>,
    theme: Option<String>,
    preferred_editor: String,
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

        Self {
            projects_dir: projects_dir.clone(),
            repo_paths: load_repos(&repos_path),
            repos_path,
            tracker: AgentTracker::new(),
            metadata: SessionMetadataStore::new(),
            order: SessionOrder::new(Some(order_path)),
            git_cache: GitInfoCache::new(),
            watchers: vec![
                Box::new(ClaudeCodeAgentWatcher::with_defaults()),
                Box::new(AmpAgentWatcher::with_defaults()),
                Box::new(CodexAgentWatcher::with_defaults()),
                Box::new(OpenCodeAgentWatcher::with_defaults()),
            ],
            theme,
            preferred_editor,
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

    /// One scan of every watcher with the repos.json-derived resolver
    /// (desktop mode).
    pub fn scan_once(&mut self, now: i64) {
        self.reload_repos();
        let entries = repo_entries(&self.repo_paths);
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
        repo_entries(&self.repo_paths).into_iter().find(|e| e.name == name).map(|e| e.dir)
    }

    /// The configured preferred editor command.
    pub fn preferred_editor(&self) -> String {
        self.preferred_editor.clone()
    }

    /// Refresh git info for each watched repo (runs git subprocesses).
    pub fn refresh_git(&mut self, now: i64) {
        for entry in repo_entries(&self.repo_paths) {
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
        let mut entries = repo_entries(&self.repo_paths);
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let payload = self.compute_payload_for_entries(&entries, now);
        // Drop metadata for repos no longer configured (§1 pruneSessions).
        let names: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
        self.metadata.prune_sessions(&names);
        payload
    }

    /// Full recompute for the given entries: pid-liveness pin → prune
    /// schedule → assemble snapshot. The tmux server passes entries derived
    /// from live tmux sessions.
    pub fn compute_payload_for_entries(&mut self, entries: &[RepoEntry], now: i64) -> StatePayload {
        // CLI-derived liveness drives the pruning pins (§4; T7 replaced the
        // ~/.claude/sessions pid files; the waiting synthesis is gone — the
        // claude watcher emits CLI-authoritative statuses directly).
        let live_threads: HashSet<String> = crate::claude_cli::fetch_agents_cached(
            std::time::Duration::from_millis(crate::watchers::claude_code::CLI_CACHE_TTL_MS),
        )
        .into_iter()
        .map(|a| a.session_id)
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
        let payload = assemble_state(
            entries,
            &git_infos,
            &self.tracker,
            &self.metadata,
            &mut self.order,
            theme,
            &editor,
            now,
        );

        self.last_payload = Some(payload.clone());
        payload
    }

    /// Drop metadata for sessions not in `names` (tmux-session mode calls
    /// this with the live session set).
    pub fn prune_metadata_sessions(&mut self, names: &HashSet<String>) {
        self.metadata.prune_sessions(names);
    }

    /// Mark-seen fast-path: patch `unseen` on the cached snapshot without a full
    /// rebuild (BRIDGE-SPEC §2). Returns the patched payload only if something changed.
    pub fn mark_seen_patch(&mut self, name: &str) -> Option<StatePayload> {
        if !self.tracker.mark_seen(name) {
            return None;
        }
        if let Some(payload) = &mut self.last_payload {
            for session in &mut payload.sessions {
                if session.name == name {
                    session.unseen = false;
                }
            }
            return Some(payload.clone());
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
