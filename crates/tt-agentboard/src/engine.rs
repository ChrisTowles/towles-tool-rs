//! The agentboard engine: tracker + metadata + session-order + git cache +
//! watchers behind one struct, host-agnostic. Extracted from
//! `crates-tauri/tt-app/src/agentboard.rs` (phase T3 of the agentboard port)
//! so every host shares it.
//!
//! The engine is synchronous; hosts own scheduling (tokio tasks, debounces)
//! and transport (Tauri events, MCP responses). Hosts guard it with a `Mutex`,
//! so everything expensive that does NOT need engine state is deliberately
//! outside `impl Engine`: [`collect_agent_snapshot`] (claude CLI + `/proc` +
//! transcript reads) and [`crate::git_info::compute_git_info`] (git
//! subprocesses) run unlocked, and their results are handed to cheap locked
//! methods ([`Engine::compute_payload_with`], [`Engine::store_git_info`]).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::bridge::{StatePayload, assemble_state};
use crate::git_info::GitInfoCache;
use crate::metadata::{LogInput, ProgressInput, SessionMetadataStore, StatusInput};
use crate::procenv::InstanceScope;
use crate::repos::{
    RepoEntry, add_repo_persisted, default_repos_path, load_repos, load_scan_roots,
    remove_repo_persisted, repo_entries, resolve_session_name, save_scan_roots,
    untrack_missing_persisted,
};
use crate::session_order::{ReorderDelta, SessionOrder};
use crate::sessions::{SessionRecord, SessionStore, default_sessions_path};
use crate::tracker::{AgentTracker, instance_key};
use crate::types::{AgentEvent, AgentStatus, MetadataTone};
use crate::watcher::{AgentWatcher, WatcherContext};
use crate::watchers::amp::AmpAgentWatcher;
use crate::watchers::claude_code::ClaudeCodeAgentWatcher;
use crate::watchers::codex::CodexAgentWatcher;
use crate::watchers::opencode::OpenCodeAgentWatcher;

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
    repo_meta: crate::repo_meta::RepoMetaStore,
    windows: crate::windows::WindowsStore,
    collapse: crate::collapse::CollapseStore,
    sessions: SessionStore,
    git_cache: GitInfoCache,
    watchers: Vec<Box<dyn AgentWatcher + Send>>,
    theme: Option<String>,
    preferred_editor: String,
    compact_recommend_percent: u8,
    seeded_once: bool,
    /// Sticky agent→PTY attribution: thread id (CLI sessionId) → the
    /// `TT_SESSION_ID` read from that agent's process env. Refreshed from live
    /// processes each compute and kept while the tracker still holds the
    /// thread, so an agent that exits stays on the pane it ran in instead of
    /// drifting to its folder's default session.
    thread_sessions: HashMap<String, String>,
    /// Which app instances' agents this host reports (see
    /// [`crate::procenv::InstanceScope`]).
    scope: InstanceScope,
}

/// Everything [`Engine::compute_payload_with`] needs that is derived from the
/// system rather than engine state: the claude CLI's live-agent snapshot, the
/// `TT_SESSION_ID` read from each agent's process env, and the app-spawned
/// agents found by scanning `/proc`. Collect it with [`collect_agent_snapshot`]
/// — outside the engine lock, since it spawns a process and reads transcripts.
pub struct AgentSnapshot {
    live_threads: HashSet<String>,
    tt_session_by_thread: HashMap<String, String>,
    session_agents: HashMap<String, AgentEvent>,
}

/// Gather the live-agent inputs for a payload rebuild. Runs the (cached)
/// `claude agents` fetch and the `/proc` scans; call it WITHOUT holding the
/// engine lock so a slow claude CLI can't stall `ab_*` commands. `scope` must
/// match the engine's (see [`Engine::new`]) so snapshot attribution and the
/// watcher's admission agree on which agents are ours.
pub fn collect_agent_snapshot(now: i64, scope: &InstanceScope) -> AgentSnapshot {
    // CLI-derived liveness drives the pruning pins (§4; T7 replaced the
    // ~/.claude/sessions pid files; the waiting synthesis is gone — the
    // claude watcher emits CLI-authoritative statuses directly).
    let cli_agents = crate::claude_cli::fetch_agents_cached(std::time::Duration::from_millis(
        crate::watchers::claude_code::CLI_CACHE_TTL_MS,
    ));
    let live_threads: HashSet<String> = cli_agents.iter().map(|a| a.session_id.clone()).collect();
    // Link each live agent to the PTY session it runs in: read `TT_SESSION_ID`
    // from the agent's process env (/proc), keyed by its thread id (==
    // sessionId). Agents with no injected id (e.g. started in an external
    // terminal, or non-Claude kinds without a pid source) stay unmapped and
    // fall back to their folder's default session in `assemble_state`. Agents
    // out of `scope` (another app instance's PTYs) stay unmapped too — the
    // watcher drops their events entirely, so nothing falls back for them.
    let tt_session_by_thread: HashMap<String, String> = cli_agents
        .iter()
        .filter_map(|a| {
            crate::procenv::session_id_in_scope(a.pid, scope).map(|sid| (a.session_id.clone(), sid))
        })
        .collect();
    // Supplement CLI detection: app-spawned Claude sessions the CLI snapshot
    // never enumerated, found by scanning /proc for our injected
    // TT_SESSION_ID and enriched with task name + status from the
    // transcript the process has open. Keyed by session id; consumed only
    // for sessions the tracker left idle. First live process per id wins.
    let mut session_agents: HashMap<String, AgentEvent> = HashMap::new();
    for proc in crate::procenv::scan_session_agents(scope) {
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
    AgentSnapshot { live_threads, tt_session_by_thread, session_agents }
}

impl Engine {
    /// Build from the real config locations (`~/.claude`, `~/.config/towles-tool`).
    /// `scope` picks which app instances' agents this host reports: the app
    /// passes [`InstanceScope::this_app`] (its own PTYs only — sessions.json is
    /// shared, so another instance's PTY can carry the same session id); the
    /// MCP server passes [`InstanceScope::Any`].
    pub fn new(scope: InstanceScope) -> Self {
        let projects_dir =
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("projects");
        let repos_path = default_repos_path();
        let order_path = crate::session_order::default_session_order_path();

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
            repo_meta: crate::repo_meta::RepoMetaStore::new(Some(
                crate::repo_meta::default_repo_meta_path(),
            )),
            windows: crate::windows::WindowsStore::new(
                Some(crate::windows::default_windows_path()),
            ),
            collapse: crate::collapse::CollapseStore::new(Some(
                crate::collapse::default_collapse_path(),
            )),
            git_cache: GitInfoCache::new(),
            watchers: vec![
                Box::new(ClaudeCodeAgentWatcher::with_defaults(scope.clone())),
                Box::new(AmpAgentWatcher::with_defaults()),
                Box::new(CodexAgentWatcher::with_defaults()),
                Box::new(OpenCodeAgentWatcher::with_defaults()),
            ],
            theme,
            preferred_editor,
            compact_recommend_percent,
            seeded_once: false,
            thread_sessions: HashMap::new(),
            scope,
        }
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.projects_dir.clone()
    }

    /// Re-read `repos.json` so changes made by the `tt agentboard` CLI (which
    /// writes the same file) are picked up without restarting the host. A
    /// torn/corrupt read (the file exists but won't parse — most likely racing
    /// another instance's write, #75) keeps the last known-good list rather
    /// than degrading to empty, which would prune every folder's sessions.
    fn reload_repos(&mut self) {
        if let Some(paths) = crate::repos::try_load_repos(&self.repos_path) {
            self.repo_paths = paths;
        }
    }

    /// One scan of every watcher with the repos.json-derived resolver
    /// (desktop mode).
    pub fn scan_once(&mut self, now: i64) {
        self.reload_repos();
        let all_paths = self.expand_with_worktrees();
        let entries = repo_entries(&all_paths);
        self.scan_once_with_resolvers(&|dir| resolve_session_name(dir, &entries), &|_| None, now);
    }

    /// `self.repo_paths` plus any `git worktree` checkouts of those repos that
    /// aren't already tracked (via `git worktree list`, e.g. a checkout under
    /// `.claude/worktrees/`) — so a worktree shows up in the rail without the
    /// user adding it to repos.json. Derived live on every call, nothing
    /// persisted, so a `git worktree remove` just stops appearing next poll.
    /// Distinct from the "multiple clones" pattern (separate `repoPaths`
    /// entries, unrelated repos to git): those are unaffected here.
    ///
    /// This only decides which dirs show up as entries at all — nesting them
    /// into [`crate::types::RepoData`] rows is [`crate::bridge::assemble_state`]'s
    /// job, decided from each entry's [`crate::git_info::GitInfo::common_dir`],
    /// not from how a dir got onto this list.
    ///
    /// Cache-only: never shells to git — this only reads each tracked repo's
    /// already-cached `worktree_dirs` (from its last `compute_git_info`), so
    /// a freshly discovered worktree shows up the instant its parent's cache
    /// lists it, with no dependency on the *child's* own git info being
    /// warmed yet. This method runs under the engine lock (every `ab_*`
    /// command and the watcher-scan loop share it), and every other command
    /// is dispatched inline on the UI thread, so shelling out to git here (as
    /// this used to do via `get_or_refresh`) could hold the lock through
    /// git's full subprocess chain (`compute_git_info` is ~9 sequential
    /// spawns) and freeze the whole app, not just the caller. The host warms
    /// the cache out of band instead (see the watcher-scan block and
    /// `ab_add_repo` in `crates-tauri/tt-app/src/lib.rs` / `agentboard.rs`).
    fn expand_with_worktrees(&mut self) -> Vec<String> {
        let base = self.repo_paths.clone();
        let cache = &self.git_cache;
        merge_worktree_dirs(&base, |dir| cache.get(dir).worktree_dirs)
    }

    /// Dirs in the worktree-merged target set whose git-cache entry is
    /// missing or older than the TTL as of `now`, paired with that folder's
    /// base-branch override (if any) — for the host to compute with
    /// [`crate::git_info::compute_git_info`] *outside* the engine lock, then
    /// hand back via [`Self::warm_git_cache`]. Read-only; safe to call under
    /// the lock since it never shells out.
    /// Each entry also carries that folder's previously cached value, which the
    /// host passes back into `compute_git_info` so an unmoved repo revalidates
    /// cheaply instead of re-running the landing probe.
    pub fn stale_git_targets(
        &mut self,
        now: i64,
    ) -> Vec<(String, Option<String>, crate::git_info::GitInfo)> {
        self.reload_repos();
        let all_paths = self.expand_with_worktrees();
        all_paths
            .into_iter()
            .filter(|d| !self.git_cache.is_fresh(d, now))
            .map(|d| {
                let base = self.folder_meta.base_branch_for(&d).map(str::to_string);
                let previous = self.git_cache.get(&d);
                (d, base, previous)
            })
            .collect()
    }

    /// Store freshly computed git info for dirs the host warmed outside the
    /// lock (see [`Self::stale_git_targets`]). Returns whether any entry's
    /// value actually changed, so the host can skip a no-op re-emit.
    pub fn warm_git_cache(
        &mut self,
        results: Vec<(String, crate::git_info::GitInfo)>,
        now: i64,
    ) -> bool {
        let mut changed = false;
        for (dir, info) in results {
            changed |= self.store_git_info(&dir, info, now);
        }
        changed
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

    /// The dirs whose git info the host should recompute (all watched repos,
    /// freshly reloaded), paired with each folder's base-branch override (if
    /// any). Cheap; hold the lock only for this, then run
    /// [`crate::git_info::compute_git_info`] per dir unlocked and hand the
    /// results back via [`Engine::store_git_info`].
    /// Like [`Self::stale_git_targets`], each entry carries the folder's
    /// previously cached value so the host's recompute can revalidate cheaply
    /// instead of re-running the landing probe.
    pub fn git_targets(&mut self) -> Vec<(String, Option<String>, crate::git_info::GitInfo)> {
        self.reload_repos();
        repo_entries(&self.repo_paths)
            .into_iter()
            .map(|e| {
                let base = self.folder_meta.base_branch_for(&e.dir).map(str::to_string);
                let previous = self.git_cache.get(&e.dir);
                (e.dir, base, previous)
            })
            .collect()
    }

    /// Store one repo's freshly computed git info. Returns whether it differs
    /// from the cached value, so the host can skip re-emitting an unchanged
    /// snapshot.
    pub fn store_git_info(&mut self, dir: &str, info: crate::git_info::GitInfo, now: i64) -> bool {
        let changed = self.git_cache.get(dir) != info;
        self.git_cache.insert(dir, info, now);
        changed
    }

    /// Full recompute from repos.json (desktop mode). Base order is by name
    /// (createdAt is meaningless for configured repos).
    ///
    /// Collects the agent snapshot itself — convenient for hosts without a hot
    /// loop (the MCP server). Hot loops should call [`collect_agent_snapshot`]
    /// unlocked and pass it to [`Engine::compute_payload_with`].
    pub fn compute_payload(&mut self, now: i64) -> StatePayload {
        let snapshot = collect_agent_snapshot(now, &self.scope);
        self.compute_payload_with(&snapshot, now)
    }

    /// Full recompute from repos.json using a pre-collected agent snapshot.
    pub fn compute_payload_with(&mut self, snapshot: &AgentSnapshot, now: i64) -> StatePayload {
        self.reload_repos();
        let all_paths = self.expand_with_worktrees();
        // NOT name-sorted: `expand_with_worktrees` already returns repos in
        // `repoPaths` order (each followed by its worktrees), and that order is
        // the user's — what a drag in Settings → Agentboard → Repos persists.
        // Sorting by name here silently discarded that choice.
        let entries = repo_entries(&all_paths);
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
        let payload = self.compute_payload_for_entries(&entries, snapshot, now);
        // Drop metadata + session records for repos no longer configured.
        // Skipped when the resolved entry set is empty: every configured repo
        // vanishing in one poll is far more likely a transient glitch (torn
        // repos.json read, worktree-list hiccup) than a real config wipe, and
        // pruning on it deletes every folder's session records (#75). Stale
        // records left by a genuine remove-all are pruned on the next
        // non-empty poll.
        if !entries.is_empty() {
            let names: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
            let dirs: HashSet<String> = entries.iter().map(|e| e.dir.clone()).collect();
            self.metadata.prune_sessions(&names);
            self.sessions.prune(&dirs);
            self.folder_meta.prune(&dirs);
            // Repo identity is deliberately NOT pruned here. Everything else in
            // this block is derived state that's cheap to lose and regenerates
            // on the next poll; a hand-picked icon and color is user-authored,
            // has no undo, and this `dirs` set is known to churn spuriously
            // (the `!entries.is_empty()` guard above exists because of #75).
            // Sweeping identity on a transient stat failure or an
            // untrack/retrack would silently destroy a choice the user made.
            // The map is bounded by "repos the user has ever styled" — a few
            // kilobytes — so it's reaped on explicit removal, not on a timer.
            let gone = self.windows.prune(&dirs);
            if !gone.is_empty() {
                let _ = self.windows.save(&gone);
            }
        }
        payload
    }

    /// Replace a repo's icon/color identity; an all-empty `meta` resets it to
    /// the default. Persists on change.
    pub fn set_repo_meta(&mut self, dir: &str, meta: crate::repo_meta::RepoMeta) -> bool {
        let changed = self.repo_meta.set_meta(dir, meta);
        if changed {
            let _ = self.repo_meta.save();
        }
        changed
    }

    /// Set (or clear) a folder's base-branch override. Persists on change.
    pub fn set_folder_base_branch(&mut self, dir: &str, base_branch: Option<&str>) -> bool {
        let changed = self.folder_meta.set_base_branch(dir, base_branch);
        if changed {
            let _ = self.folder_meta.save();
            // The cached stats were computed against the old baseline —
            // invalidate so the next watcher-scan tick (2s, not the 10s stat
            // poll) recomputes them against the new override right away.
            self.git_cache.invalidate(Some(dir));
        }
        changed
    }

    /// Replace the persisted window layout (frontend-owned blob). Persists on
    /// change; returns whether it changed.
    /// `touched` is the set of folder dirs whose windows/active-window the
    /// frontend actually mutated since its last save (see
    /// [`crate::windows::WindowsStore::save`]) — required so a whole-blob save
    /// from one Agentboard window can't clobber another window's folders.
    pub fn set_windows(
        &mut self,
        payload: crate::windows::WindowsPayload,
        touched: &[String],
    ) -> bool {
        let changed = self.windows.set(payload);
        if changed {
            let _ = self.windows.save(touched);
        }
        changed
    }

    /// Set (or clear) one folder-rail row's collapsed state. Persists on change.
    pub fn set_collapsed(&mut self, key: &str, collapsed: bool) -> bool {
        let changed = self.collapse.set(key, collapsed);
        if changed {
            let _ = self.collapse.save();
        }
        changed
    }

    /// Set the compact-nudge threshold and persist it to the shared settings
    /// file (`agentboard.compactRecommendPercent`). Clamped to 1..=100.
    /// Persists via `save_merge` so keys the TypeScript CLI owns survive.
    pub fn set_compact_recommend_percent(&mut self, percent: u8) -> bool {
        let percent = percent.clamp(1, 100);
        if percent == self.compact_recommend_percent {
            return false;
        }
        self.compact_recommend_percent = percent;
        if let Ok(mut settings) = tt_config::load() {
            settings.agentboard.compact_recommend_percent = Some(percent);
            let _ = tt_config::save_merge(&settings);
        }
        true
    }

    /// Full recompute for the given entries: pid-liveness pin → prune
    /// schedule → assemble snapshot.
    fn compute_payload_for_entries(
        &mut self,
        entries: &[RepoEntry],
        snapshot: &AgentSnapshot,
        now: i64,
    ) -> StatePayload {
        // Link each live agent to the PTY session it runs in: read `TT_SESSION_ID`
        // from the agent's process env (/proc), keyed by its thread id (==
        // sessionId). Attributions are sticky: they live in `thread_sessions`
        // for as long as the tracker still holds the thread, so an agent whose
        // process exited (its final Complete/Interrupted state) stays on the
        // pane it ran in instead of drifting to its folder's default session.
        // Agents with no injected id (e.g. non-Claude kinds without a pid
        // source) stay unmapped and fall back to their folder's default
        // session in `assemble_state`.
        // The same join, persisted: `thread_sessions` is in-memory and dies
        // with the process, so a crash would take the pane→agent pairing with
        // it and the next launch would have nothing to offer resuming
        // (see [`crate::resume`]).
        let mut link_changed = false;
        for (tid, sid) in &snapshot.tt_session_by_thread {
            self.thread_sessions.insert(tid.clone(), sid.clone());
            link_changed |= self.sessions.note_agent(sid, tid);
        }
        if link_changed && let Err(e) = self.sessions.save() {
            log::warn!("failed to persist agent session links: {e}");
        }
        let mut pinned: HashMap<String, Vec<String>> = HashMap::new();
        let mut tracked_threads: HashSet<String> = HashSet::new();
        for entry in entries {
            for agent in self.tracker.get_agents(&entry.name) {
                let Some(tid) = agent.thread_id.clone() else {
                    continue;
                };
                if snapshot.live_threads.contains(&tid) {
                    pinned
                        .entry(entry.name.clone())
                        .or_default()
                        .push(instance_key(&agent.agent, Some(&tid)));
                }
                tracked_threads.insert(tid);
            }
        }
        self.tracker.set_pinned_instances_multi(&pinned);
        // Drop cached attributions for threads that are neither alive nor
        // still shown by the tracker — the cache stays bounded by the set of
        // agents actually on the board.
        self.thread_sessions
            .retain(|tid, _| snapshot.live_threads.contains(tid) || tracked_threads.contains(tid));

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
        // from that agent's process (sticky across process exit, see above).
        // No mapping → folder's default session; a mapping onto a session that
        // isn't one of the folder's records → dropped (see `assemble_state`).
        let attribute = |event: &AgentEvent| {
            event.thread_id.as_ref().and_then(|tid| self.thread_sessions.get(tid).cloned())
        };
        let mut payload = assemble_state(
            entries,
            &git_infos,
            &self.tracker,
            &self.metadata,
            &self.sessions,
            &self.folder_meta,
            &attribute,
            &snapshot.session_agents,
            theme,
            &editor,
            self.compact_recommend_percent,
            now,
        );

        // Repo identity is a pure lookup on the row's anchor dir, so it
        // decorates the assembled payload rather than adding another store to
        // that already-wide signature.
        for repo in &mut payload.repos {
            repo.meta = self.repo_meta.meta_for(&repo.dir).cloned();
        }

        payload.windows = self.windows.payload().clone();
        payload.collapsed = self.collapse.payload().collapsed.clone();
        payload
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

    pub fn dismiss(&mut self, session: &str, agent: &str, thread_id: Option<&str>) -> bool {
        self.tracker.dismiss(session, agent, thread_id)
    }

    pub fn reorder(&mut self, name: &str, delta: ReorderDelta) {
        self.order.reorder(name, delta);
    }

    /// Set the theme and persist it to the shared settings' `agentboard.theme`
    /// (interop-safe — that key exists in the TS schema). Persists via
    /// `save_merge` so keys the TypeScript CLI owns survive, and skips the
    /// write entirely when the settings file is unreadable — writing defaults
    /// over a momentarily unreadable file would wipe the user's config.
    pub fn set_theme(&mut self, theme: String) {
        self.theme = Some(theme.clone());
        match tt_config::load() {
            Ok(mut settings) => {
                settings.agentboard.theme = Some(serde_json::Value::String(theme));
                let _ = tt_config::save_merge(&settings);
            }
            Err(e) => log::warn!("theme not persisted: settings unreadable: {e}"),
        }
    }

    /// Absolute dirs currently on the rail (freshly reloaded), so the add-repo
    /// picker can exclude repos that are already added.
    pub fn repo_dirs(&mut self) -> Vec<String> {
        self.reload_repos();
        self.repo_paths.clone()
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

    /// Adds straight against `repos.json` (reread-fresh-then-write; see
    /// [`add_repo_persisted`]) rather than trusting `self.repo_paths`, which
    /// may be stale — another Agentboard window may have added/removed a
    /// different repo since our last reload.
    pub fn add_repo(&mut self, path: &str) -> bool {
        match add_repo_persisted(&self.repos_path, path) {
            Ok((merged, added)) => {
                self.repo_paths = merged;
                added
            }
            Err(_) => false,
        }
    }

    pub fn remove_repo(&mut self, dir: &str) -> bool {
        match remove_repo_persisted(&self.repos_path, dir) {
            Ok((merged, removed)) => {
                self.repo_paths = merged;
                // Explicit removal is the one place identity is reaped — see
                // `RepoMetaStore::forget`.
                if removed && self.repo_meta.forget(dir) {
                    let _ = self.repo_meta.save();
                }
                removed
            }
            Err(_) => false,
        }
    }

    /// Set the rail's repo order (the user's drag). Persists on change; see
    /// [`reorder_repos`] for why a stale `desired` can't drop a repo.
    pub fn set_repo_order(&mut self, desired: &[String]) -> std::io::Result<()> {
        let merged = crate::repos::reorder_repos_persisted(&self.repos_path, desired)?;
        self.repo_paths = merged;
        Ok(())
    }

    /// Untrack every repo whose directory is gone from disk (the rail's
    /// "missing" ghosts). Returns the dropped dirs; empty on IO failure.
    pub fn untrack_missing(&mut self) -> Vec<String> {
        match untrack_missing_persisted(&self.repos_path) {
            Ok((merged, removed)) => {
                self.repo_paths = merged;
                removed
            }
            Err(_) => Vec::new(),
        }
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

    /// Set (or clear) a session's user-authored purpose. Persists on change.
    pub fn set_session_purpose(&mut self, id: &str, purpose: Option<&str>) -> bool {
        let changed = self.sessions.set_purpose(id, purpose);
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

    /// Session ids scoped to `dir`, without removing anything — a task
    /// removal must kill live PTYs *before* attempting the (fallible)
    /// worktree removal, but must not drop the records until removal has
    /// actually succeeded (see [`Self::close_folder`]).
    pub fn session_ids_for(&self, dir: &str) -> Vec<String> {
        self.sessions.sessions_for(dir).iter().map(|r| r.id.clone()).collect()
    }

    /// Every persisted session record with its folder dir, cloned.
    ///
    /// Exists so a caller can take this snapshot under the engine lock and then
    /// do slow work (the resume picker's transcript scan) with the lock
    /// released — holding it across disk I/O would block every `ab_*` command
    /// and the state poll behind it.
    pub fn session_records(&self) -> Vec<(String, SessionRecord)> {
        self.sessions
            .iter()
            .flat_map(|(dir, list)| list.iter().map(move |rec| (dir.to_string(), rec.clone())))
            .collect()
    }

    /// Tear a folder's live rail state down immediately, ahead of its
    /// checkout disappearing (a task removal): drop every session record and
    /// every window/pane scoped to it, persisting both right away instead of
    /// waiting for the next poll's repo-keyed prune in
    /// [`Self::compute_payload_with`]. Returns the removed session ids so the
    /// caller can kill their live PTYs (a session id doubles as its `term_id`)
    /// — killing them first is what actually ends any Claude Code process
    /// running inside, since closing the PTY's controlling terminal signals
    /// its foreground job.
    pub fn close_folder(&mut self, dir: &str) -> Vec<String> {
        let ids: Vec<String> =
            self.sessions.sessions_for(dir).iter().map(|r| r.id.clone()).collect();
        if !ids.is_empty() {
            for id in &ids {
                self.sessions.remove(id);
            }
            let _ = self.sessions.save();
        }
        if self.windows.remove_folder(dir) {
            let _ = self.windows.save(&[dir.to_string()]);
        }
        ids
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

/// Pure merge behind [`Engine::expand_with_worktrees`]: `repo_paths` plus each
/// dir's worktrees (via `worktrees_of`), deduped, configured entries first.
/// Split out so the merge/dedup logic is unit-testable without a real
/// `GitInfoCache`/git subprocess.
///
/// Only decides which dirs appear as entries at all — a dir already in
/// `repo_paths` is never duplicated even when `worktrees_of` also reports it
/// for another entry. Whether two dirs in the result end up nesting into one
/// [`crate::types::RepoData`] row is decided later, structurally, by
/// [`crate::bridge::assemble_state`] matching each entry's
/// [`crate::git_info::GitInfo::common_dir`] — not by anything recorded here.
fn merge_worktree_dirs(
    repo_paths: &[String],
    mut worktrees_of: impl FnMut(&str) -> Vec<String>,
) -> Vec<String> {
    // Emits each tracked repo immediately followed by its own discovered
    // worktrees, so the result carries `repoPaths` order — which *is* the
    // user's chosen rail order (see `reorder_repos`). The caller must not
    // re-sort this by name or that choice is silently discarded.
    let mut seen: HashSet<String> = repo_paths.iter().cloned().collect();
    let mut all: Vec<String> = Vec::with_capacity(repo_paths.len());
    for dir in repo_paths {
        all.push(dir.clone());
        // Sorted so a repo's own worktrees keep a stable order between polls —
        // `git worktree list` order is not guaranteed, and only the *group's*
        // position is the user's to choose, not the order within it.
        let mut wts = worktrees_of(dir);
        wts.sort();
        for wt in wts {
            if seen.insert(wt.clone()) {
                all.push(wt);
            }
        }
    }
    all
}

#[cfg(test)]
mod merge_worktree_dirs_tests {
    use super::merge_worktree_dirs;

    #[test]
    fn appends_discovered_worktrees_after_configured_paths() {
        let repo_paths = vec!["/repo/main".to_string()];
        let all = merge_worktree_dirs(&repo_paths, |dir| {
            assert_eq!(dir, "/repo/main");
            vec!["/repo/.claude/worktrees/feat".to_string()]
        });
        assert_eq!(all, vec!["/repo/main", "/repo/.claude/worktrees/feat"]);
    }

    #[test]
    fn dedupes_a_worktree_already_configured_explicitly() {
        // e.g. towles-tool-rs-task-2 manually added even though it's also a
        // worktree of towles-tool-rs-task-1 — must not appear twice.
        let repo_paths = vec!["/repo/task-1".to_string(), "/repo/task-2".to_string()];
        let all = merge_worktree_dirs(&repo_paths, |dir| match dir {
            "/repo/task-1" => vec!["/repo/task-2".to_string()],
            _ => vec![],
        });
        assert_eq!(all, repo_paths);
    }

    #[test]
    fn no_worktrees_leaves_the_list_unchanged() {
        let repo_paths = vec!["/repo/plain-clone".to_string()];
        let all = merge_worktree_dirs(&repo_paths, |_| vec![]);
        assert_eq!(all, repo_paths);
    }
}
