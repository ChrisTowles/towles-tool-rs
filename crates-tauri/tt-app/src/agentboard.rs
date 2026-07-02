//! Tauri bridge for agentboard (phases 3+4). Wires tokio/tauri around the pure
//! engine pieces in `tt-agentboard`: assembles the [`StatePayload`] snapshot,
//! emits it as the `agentboard://state` event, and exposes client commands as
//! Tauri commands.
//!
//! Everything data-shaped is pure in the crate (`bridge::assemble_state`,
//! waiting-synthesis, `repos`); this module only owns the engine state (behind a
//! `Mutex`), the pid-liveness pinning, and the scan/git tokio tasks. Ports the
//! composition/broadcast half of slot-1 `server/index.ts` per
//! docs/AGENTBOARD-BRIDGE-SPEC.md.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, State};
use tokio::sync::Notify;

use tt_agentboard::fs_notify::DirNotifier;
use tt_agentboard::metadata::{LogInput, ProgressInput, StatusInput};
use tt_agentboard::session_order::ReorderDelta;
use tt_agentboard::types::{AgentEvent, MetadataTone};
use tt_agentboard::{
    AgentTracker, AgentWatcher, ClaudeCodeAgentWatcher, ClaudePidLookup, GitInfoCache, RepoEntry,
    SessionMetadataStore, SessionOrder, StatePayload, WatcherContext, add_repo, assemble_state,
    default_repos_path, instance_key, load_repos, remove_repo_by_name, repo_entries,
    resolve_session_name, save_repos,
};

/// Tauri event carrying the state snapshot.
pub const STATE_EVENT: &str = "agentboard://state";

// Prune schedule constants (BRIDGE-SPEC §4).
const STUCK_MS: i64 = 3 * 60 * 1000;
const STALE_MS: i64 = 12 * 60 * 60 * 1000;
const IDLE_MS: i64 = 30 * 1000;

/// Wall-clock epoch milliseconds (the bridge's `now`).
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Collects watcher emits during a scan, resolving sessions against the current
/// repo entries (adopted fix #3: encoded↔encoded match).
struct CollectCtx {
    entries: Vec<RepoEntry>,
    events: Vec<AgentEvent>,
}

impl WatcherContext for CollectCtx {
    fn resolve_session(&self, project_dir: &str) -> Option<String> {
        resolve_session_name(project_dir, &self.entries)
    }
    fn emit(&mut self, event: AgentEvent) {
        self.events.push(event);
    }
}

/// Owns all agentboard engine state. Guarded by a `Mutex` in [`Ab`].
pub struct Engine {
    projects_dir: PathBuf,
    repos_path: PathBuf,
    repo_paths: Vec<String>,
    tracker: AgentTracker,
    metadata: SessionMetadataStore,
    order: SessionOrder,
    git_cache: GitInfoCache,
    pid_lookup: ClaudePidLookup,
    watcher: ClaudeCodeAgentWatcher,
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
        let sessions_dir = ClaudePidLookup::default_dir();
        let repos_path = default_repos_path();
        let order_path = tt_agentboard::default_session_order_path();

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
            pid_lookup: ClaudePidLookup::new(sessions_dir.clone()),
            watcher: ClaudeCodeAgentWatcher::new(projects_dir, ClaudePidLookup::new(sessions_dir)),
            theme,
            preferred_editor,
            seeded_once: false,
            last_payload: None,
        }
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.projects_dir.clone()
    }

    /// One watcher scan: collect emits and feed them to the tracker (first scan's
    /// emits are seeded → marked unseen).
    pub fn scan_once(&mut self, now: i64) {
        let mut ctx = CollectCtx { entries: repo_entries(&self.repo_paths), events: Vec::new() };
        self.watcher.scan(&mut ctx, now);
        let seed = !self.seeded_once;
        for event in ctx.events {
            self.tracker.apply_event(event, seed);
        }
        self.seeded_once = true;
    }

    /// Refresh git info for each watched repo (runs git subprocesses).
    pub fn refresh_git(&mut self, now: i64) {
        for entry in repo_entries(&self.repo_paths) {
            self.git_cache.refresh(&entry.dir, now);
        }
    }

    /// Full recompute: pid-liveness pin → prune schedule → assemble snapshot.
    pub fn compute_payload(&mut self, now: i64) -> StatePayload {
        let entries = repo_entries(&self.repo_paths);

        // pid-liveness drives both pruning pins and the `waiting` synthesis (§4/§6).
        self.pid_lookup.invalidate();
        let mut pinned: HashMap<String, Vec<String>> = HashMap::new();
        let mut live_threads: HashSet<String> = HashSet::new();
        for entry in &entries {
            for agent in self.tracker.get_agents(&entry.name) {
                let Some(tid) = agent.thread_id.clone() else {
                    continue;
                };
                let alive = self
                    .pid_lookup
                    .pid_for_thread(&tid)
                    .is_some_and(|p| self.pid_lookup.is_alive(p));
                if alive {
                    live_threads.insert(tid.clone());
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
        for entry in &entries {
            git_infos.insert(entry.dir.clone(), self.git_cache.get(&entry.dir));
        }

        let theme = self.theme.clone();
        let editor = self.preferred_editor.clone();
        let payload = assemble_state(
            &entries,
            &git_infos,
            &self.tracker,
            &self.metadata,
            &mut self.order,
            theme,
            &editor,
            &live_threads,
            now,
        );

        // Drop metadata for repos no longer configured (§1 pruneSessions).
        let names: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
        self.metadata.prune_sessions(&names);

        self.last_payload = Some(payload.clone());
        payload
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

/// Managed Tauri state: the engine plus the task-signal handles.
pub struct Ab {
    pub engine: Arc<Mutex<Engine>>,
    /// Signals the debounced emitter to rebuild + emit.
    pub emit: Arc<Notify>,
    /// Signals the scan task to run an eager scan (fs-notify accelerant).
    pub scan: Arc<Notify>,
    /// Keeps the fs watcher alive.
    pub _notifier: Mutex<Option<DirNotifier>>,
}

fn parse_tone(tone: Option<String>) -> Option<MetadataTone> {
    match tone.as_deref() {
        Some("neutral") => Some(MetadataTone::Neutral),
        Some("info") => Some(MetadataTone::Info),
        Some("success") => Some(MetadataTone::Success),
        Some("warn") => Some(MetadataTone::Warn),
        Some("error") => Some(MetadataTone::Error),
        _ => None,
    }
}

// --- Tauri commands ---

/// Pull the current snapshot (initial mount).
#[tauri::command]
pub fn ab_get_state(state: State<Ab>) -> StatePayload {
    let mut engine = state.engine.lock().unwrap();
    engine.compute_payload(now_ms())
}

/// Clear unseen for a session (fast-path: patch + re-emit, no full rebuild).
#[tauri::command]
pub fn ab_mark_seen(state: State<Ab>, app: AppHandle, name: String) {
    let patched = {
        let mut engine = state.engine.lock().unwrap();
        engine.mark_seen_patch(&name)
    };
    if let Some(payload) = patched {
        let _ = app.emit(STATE_EVENT, payload);
    }
}

#[tauri::command]
pub fn ab_dismiss_agent(
    state: State<Ab>,
    session: String,
    agent: String,
    thread_id: Option<String>,
) {
    let changed = {
        let mut engine = state.engine.lock().unwrap();
        engine.dismiss(&session, &agent, thread_id.as_deref())
    };
    if changed {
        state.emit.notify_one();
    }
}

#[tauri::command]
pub fn ab_reorder_session(state: State<Ab>, name: String, delta: ReorderDelta) {
    state.engine.lock().unwrap().reorder(&name, delta);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_set_theme(state: State<Ab>, theme: String) {
    state.engine.lock().unwrap().set_theme(theme);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_add_repo(state: State<Ab>, path: String) {
    state.engine.lock().unwrap().add_repo(&path);
    state.scan.notify_one(); // discover the new repo's sessions
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_remove_repo(state: State<Ab>, name: String) {
    state.engine.lock().unwrap().remove_repo(&name);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_refresh(state: State<Ab>) {
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_set_status(
    state: State<Ab>,
    session: String,
    text: Option<String>,
    tone: Option<String>,
) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    let input = text.map(|t| StatusInput { text: t, tone: parse_tone(tone) });
    state.engine.lock().unwrap().set_status(&session, input, now_ms());
    state.emit.notify_one();
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn ab_set_progress(
    state: State<Ab>,
    session: String,
    current: Option<i64>,
    total: Option<i64>,
    percent: Option<f64>,
    label: Option<String>,
    clear: Option<bool>,
) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    let input = if clear == Some(true) {
        None
    } else {
        Some(ProgressInput { current, total, percent, label })
    };
    state.engine.lock().unwrap().set_progress(&session, input, now_ms());
    state.emit.notify_one();
    Ok(())
}

#[tauri::command]
pub fn ab_log(
    state: State<Ab>,
    session: String,
    message: String,
    tone: Option<String>,
    source: Option<String>,
) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    if message.is_empty() {
        return Err("message is required".into());
    }
    let input = LogInput { message, tone: parse_tone(tone), source };
    state.engine.lock().unwrap().append_log(&session, input, now_ms());
    state.emit.notify_one();
    Ok(())
}

#[tauri::command]
pub fn ab_clear_log(state: State<Ab>, session: String) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    state.engine.lock().unwrap().clear_logs(&session);
    state.emit.notify_one();
    Ok(())
}
