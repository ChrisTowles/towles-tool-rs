//! `ttr agentboard server` — the tmux-mode agentboard server (phase T3 of
//! docs/AGENTBOARD-TMUX-SPEC.md). Ports the transport/orchestration half of
//! slot-1 `runtime/server/index.ts` around the shared
//! [`tt_agentboard::engine::Engine`]:
//!
//! - sessions come from **live tmux sessions** (not repos.json — faithful to
//!   the TS server: any tmux session shows up, its dir is the active pane's
//!   cwd);
//! - transport is SSE + POST instead of WebSocket: `GET /events` streams
//!   `ServerMessage` JSON; TUI commands arrive as `POST /command` with an
//!   optional `fromSession` for client routing (per-request identity replaces
//!   the TS per-socket `identify-pane`);
//! - tmux hooks POST the same routes with the same bodies as the TS.
//!
//! Deviations: the 300ms sidebar-pane list cache is dropped (ensure/resize are
//! already debounced); `focus-agent-pane`/`kill-agent-pane` are no-ops until
//! phase T6; the server runs fine outside tmux (engine + metadata only — the
//! sidebar orchestration just finds no sessions).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, broadcast};

use tt_agentboard::engine::{Engine, apply_mutation, ingest_addr, now_ms};
use tt_agentboard::hook_http::{HookContext, parse_context, parse_resize_context};
use tt_agentboard::pane_agents::{
    PaneAgentMap, ancestor_pane_session, list_all_panes, merge_agents_with_pane_presence,
    override_terminal_if_pane_alive, pane_agent_sets_differ, ps_tree, resolve_agent_pane_id,
    scan_all_tmux_pane_agents,
};
use tt_agentboard::session_resolve::resolve_session_by_dir;
use tt_agentboard::sidebar_width_sync::{
    SidebarResizeContext, SidebarResizeSuppression, SidebarWindowSnapshot,
    resolve_sidebar_width_from_resize_context, snapshot_sidebar_windows,
};
use tt_agentboard::text::format_uptime;
use tt_agentboard::tmux::{MuxSessionInfo, SidebarPosition, SwitchTarget, TmuxProvider};
use tt_agentboard::types::{ClientCommand, SelectedAgent, ServerMessage, SessionViewedSelect};
use tt_agentboard::{PortScanner, RepoEntry, handle_request, parse_request_head, response_bytes};

use crate::ui;

pub const PID_FILE: &str = "/tmp/agentboard.pid";
const SERVER_IDLE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_SIDEBAR_WIDTH: u32 = 35;

/// Everything the route handlers and tasks share.
struct Shared {
    engine: Mutex<Engine>,
    sidebar: Mutex<SidebarState>,
    provider: TmuxProvider,
    /// Serialized `ServerMessage` JSON fanned out to every SSE client.
    tx: broadcast::Sender<String>,
    /// Debounced rebuild+broadcast trigger.
    emit: Notify,
    /// Eager watcher-scan trigger.
    scan: Notify,
    /// Connected SSE clients (idle shutdown when 0 for 30s).
    client_count: AtomicUsize,
    idle_since_ms: Mutex<Option<i64>>,
    shutting_down: AtomicBool,
    /// Pane-detected agents per session (T6), refreshed by the 3s scan.
    pane_agents: Mutex<PaneAgentMap>,
    /// Listening-port attribution per session (10s poll).
    ports: Mutex<PortScanner>,
    /// pid → owning pane's session (T7), last-good snapshot. A failed tmux
    /// scan keeps this instead of going empty — an empty map would silently
    /// fall back to dir-based resolution for every agent until the next scan.
    session_by_pane_pid: Mutex<HashMap<i32, String>>,
}

/// Sidebar orchestration state (ports the closure-captured locals of the TS
/// `startServer`).
struct SidebarState {
    visible: bool,
    width: u32,
    position: SidebarPosition,
    snapshots: indexmap::IndexMap<String, SidebarWindowSnapshot>,
    suppressed: indexmap::IndexMap<String, SidebarResizeSuppression>,
    cooldown: indexmap::IndexMap<String, i64>,
    pending_spawns: HashSet<String>,
    /// Debounced ensure-sidebar contexts, keyed by window id.
    pending_ensure: HashMap<String, HookContext>,
    pending_ensure_no_ctx: bool,
}

impl SidebarState {
    fn from_config() -> Self {
        let settings = tt_config::load().unwrap_or_default();
        let width = settings
            .agentboard
            .sidebar_width
            .map(|w| w.max(1.0) as u32)
            .unwrap_or(DEFAULT_SIDEBAR_WIDTH);
        let position = match settings.agentboard.sidebar_position {
            Some(tt_config::SidebarPosition::Right) => SidebarPosition::Right,
            _ => SidebarPosition::Left,
        };
        Self {
            // Visible by default so a fresh server (including restart) spawns
            // sidebars on the first ensure-sidebar without needing a toggle.
            visible: true,
            width,
            position,
            snapshots: indexmap::IndexMap::new(),
            suppressed: indexmap::IndexMap::new(),
            cooldown: indexmap::IndexMap::new(),
            pending_spawns: HashSet::new(),
            pending_ensure: HashMap::new(),
            pending_ensure_no_ctx: false,
        }
    }
}

/// The command the sidebar pane runs. Built from `current_exe()` so the
/// binary rename (`ttr` → `tt`) never breaks respawns (the TS hardcoded
/// `tt`, which broke during the first cutover attempt).
fn tui_command(window_id: &str) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "ttr".to_string());
    format!("REFOCUS_WINDOW={window_id} exec {exe} agentboard tui")
}

pub fn run() -> i32 {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            ui::error(&format!("Failed to start tokio runtime: {e}"));
            return 1;
        }
    };
    runtime.block_on(serve())
}

async fn serve() -> i32 {
    let (host, port) = ingest_addr();
    let listener = match TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            ui::error(&format!("agentboard server: cannot bind {host}:{port} ({e})"));
            return 1;
        }
    };

    let (tx, _) = broadcast::channel(64);
    let shared = Arc::new(Shared {
        engine: Mutex::new(Engine::new()),
        sidebar: Mutex::new(SidebarState::from_config()),
        provider: TmuxProvider::new(),
        tx,
        emit: Notify::new(),
        scan: Notify::new(),
        client_count: AtomicUsize::new(0),
        idle_since_ms: Mutex::new(None),
        shutting_down: AtomicBool::new(false),
        pane_agents: Mutex::new(PaneAgentMap::new()),
        ports: Mutex::new(PortScanner::new()),
        session_by_pane_pid: Mutex::new(HashMap::new()),
    });

    // PID file.
    let _ = std::fs::File::create(PID_FILE)
        .and_then(|mut f| writeln!(f, "{}", std::process::id()).map(|_| f));

    // Bootstrap: hooks, boot-active sessions, initial scan.
    shared.provider.setup_hooks(&host, port);
    {
        let boot: Vec<String> = shared
            .provider
            .client()
            .list_clients()
            .into_iter()
            .map(|c| c.session_name)
            .filter(|s| !s.is_empty())
            .collect();
        let mut boot = boot;
        if boot.is_empty()
            && let Some(current) = shared.provider.current_session()
        {
            boot.push(current);
        }
        if !boot.is_empty() {
            shared.engine.lock().unwrap().set_active_sessions(&boot);
        }
    }

    // Debounced emitter: coalesce triggers into one rebuild + broadcast.
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            loop {
                shared.emit.notified().await;
                tokio::time::sleep(Duration::from_millis(200)).await;
                broadcast_state(&shared);
            }
        });
    }

    // Watcher scan: every 2s, or eagerly on signal.
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(2000));
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shared.scan.notified() => {}
                }
                scan_once(&shared);
                shared.emit.notify_one();
            }
        });
    }

    // Git-stat poll: refresh the live sessions' dirs every 1.5s.
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1500));
            loop {
                interval.tick().await;
                let dirs: Vec<String> =
                    shared.provider.list_sessions().into_iter().map(|s| s.dir).collect();
                {
                    let mut engine = shared.engine.lock().unwrap();
                    engine.refresh_git_dirs(&dirs, now_ms());
                }
                shared.emit.notify_one();
            }
        });
    }

    // Ensure-sidebar debounce drain (150ms after the last enqueue).
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(150));
            loop {
                interval.tick().await;
                drain_pending_ensures(&shared);
            }
        });
    }

    // Pane-agent scan: every 3s while clients are connected (T6), plus the
    // eager rescan /focus triggers.
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(3000));
            loop {
                interval.tick().await;
                if shared.client_count.load(Ordering::SeqCst) == 0 {
                    continue;
                }
                let shared = shared.clone();
                // The scan shells out (tmux/ps/claude/sqlite) — keep it off
                // the async workers.
                tokio::task::spawn_blocking(move || refresh_pane_agents(&shared)).await.ok();
            }
        });
    }

    // Port poll: every 10s while clients are connected.
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10_000));
            loop {
                interval.tick().await;
                if shared.client_count.load(Ordering::SeqCst) == 0 {
                    continue;
                }
                let shared = shared.clone();
                tokio::task::spawn_blocking(move || {
                    let names: Vec<String> =
                        shared.provider.list_sessions().into_iter().map(|s| s.name).collect();
                    let changed = shared.ports.lock().unwrap().refresh(&names);
                    if changed {
                        shared.emit.notify_one();
                    }
                })
                .await
                .ok();
            }
        });
    }

    // Idle shutdown: no SSE clients for 30s → quit (matches the TS).
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1000));
            loop {
                interval.tick().await;
                let idle_since = *shared.idle_since_ms.lock().unwrap();
                if let Some(since) = idle_since
                    && now_ms() - since > SERVER_IDLE_TIMEOUT_MS as i64
                    && shared.client_count.load(Ordering::SeqCst) == 0
                {
                    quit_all(&shared);
                }
            }
        });
    }

    // Signals: unset hooks + drop the PID file, keep sidebars.
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {}
                _ = term.recv() => {}
            }
            cleanup(&shared);
            std::process::exit(0);
        });
    }

    // Start idle countdown immediately (no clients yet), kick the first scan.
    *shared.idle_since_ms.lock().unwrap() = Some(now_ms());
    shared.scan.notify_one();
    ui::info(&format!("agentboard server listening on {host}:{port}"));

    loop {
        let Ok((socket, _)) = listener.accept().await else {
            continue;
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            handle_conn(socket, shared).await;
        });
    }
}

// --- State computation & broadcast ---

/// Live tmux sessions sorted by creation time then name (the TS base order).
fn sorted_sessions(provider: &TmuxProvider) -> Vec<MuxSessionInfo> {
    let mut sessions = provider.list_sessions();
    sessions.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.name.cmp(&b.name)));
    sessions
}

fn scan_once(shared: &Shared) {
    let dir_map: Vec<(String, String)> =
        shared.provider.list_sessions().into_iter().map(|s| (s.dir, s.name)).collect();
    // pid → owning pane's session (T7): agents are attributed to the tmux
    // session whose pane runs them, so shared dirs (slot clones) and odd
    // cwds resolve correctly; the dir map remains the fallback. A failed tmux
    // scan keeps the last-good snapshot — an empty map would silently
    // misattribute every agent to the dir-map fallback until the next scan.
    if let Some(panes) = list_all_panes() {
        let fresh: HashMap<i32, String> = panes
            .into_iter()
            .filter(|p| p.session != tt_agentboard::tmux::STASH_SESSION)
            .map(|p| (p.pid, p.session))
            .collect();
        *shared.session_by_pane_pid.lock().unwrap() = fresh;
    }
    let session_by_pane_pid = shared.session_by_pane_pid.lock().unwrap().clone();
    let tree = ps_tree();
    let mut engine = shared.engine.lock().unwrap();
    engine.scan_once_with_resolvers(
        &|project_dir| resolve_session_by_dir(project_dir, &dir_map),
        &|pid| ancestor_pane_session(pid, &session_by_pane_pid, &tree),
        now_ms(),
    );
}

/// Rebuild the snapshot from live tmux sessions and fan it out to SSE clients.
fn broadcast_state(shared: &Shared) {
    let sessions = sorted_sessions(&shared.provider);
    let entries: Vec<RepoEntry> =
        sessions.iter().map(|s| RepoEntry { name: s.name.clone(), dir: s.dir.clone() }).collect();
    let pane_counts = shared.provider.all_pane_counts();
    let sidebar_width = shared.sidebar.lock().unwrap().width;

    let now = now_ms();
    let payload = {
        let mut engine = shared.engine.lock().unwrap();
        let payload = engine.compute_payload_for_entries(&entries, now);
        let names: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
        engine.prune_metadata_sessions(&names);
        payload
    };

    // Patch the tmux-only fields the desktop snapshot leaves at defaults,
    // and fold in pane presence (T6) + port attribution.
    let mut data = payload.sessions;
    let by_name: HashMap<&str, &MuxSessionInfo> =
        sessions.iter().map(|s| (s.name.as_str(), s)).collect();
    let pane_agents = shared.pane_agents.lock().unwrap();
    for session in &mut data {
        if let Some(info) = by_name.get(session.name.as_str()) {
            session.created_at = info.created_at;
            session.windows = info.windows as i64;
            session.panes = pane_counts.get(&session.name).copied().unwrap_or(0) as i64;
            session.uptime = format_uptime(info.created_at, now / 1000);
        }
        session.ports = shared.ports.lock().unwrap().get(&session.name);

        let presences = pane_agents.get(&session.name);
        session.agent_state =
            override_terminal_if_pane_alive(session.agent_state.take(), presences);
        let watcher_agents = std::mem::take(&mut session.agents);
        session.agents = merge_agents_with_pane_presence(&session.name, watcher_agents, presences);
        // Persist pane attachments so the orphan-drop works on later merges
        // (the TS relied on mutating tracker objects by reference).
        let assignments: Vec<(String, Option<String>, String)> = session
            .agents
            .iter()
            .filter_map(|a| a.pane_id.clone().map(|p| (a.agent.clone(), a.thread_id.clone(), p)))
            .collect();
        if !assignments.is_empty() {
            let mut engine = shared.engine.lock().unwrap();
            for (agent, thread_id, pane_id) in assignments {
                engine.assign_pane_id(&session.name, &agent, thread_id.as_deref(), &pane_id);
            }
        }
    }
    drop(pane_agents);

    let msg = ServerMessage::State {
        sessions: data,
        theme: payload.theme,
        sidebar_width: sidebar_width as f64,
        preferred_editor: payload.preferred_editor,
        ts: now,
    };
    publish(shared, &msg);
}

fn publish(shared: &Shared, msg: &ServerMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = shared.tx.send(json);
    }
}

fn publish_session_viewed(shared: &Shared, name: &str, select: Option<SessionViewedSelect>) {
    publish(shared, &ServerMessage::SessionViewed { name: name.to_string(), select });
}

/// Rescan pane agents; on change, store + notify (ports `refreshPaneAgents`).
/// A failed tmux scan keeps the last-good snapshot — storing an empty map
/// would make every live agent vanish from the board until the next tick.
fn refresh_pane_agents(shared: &Shared) {
    let sidebar_ids: std::collections::HashSet<String> =
        shared.provider.list_sidebar_panes(None).into_iter().map(|p| p.pane_id).collect();
    let Some(next) = scan_all_tmux_pane_agents(&sidebar_ids, now_ms()) else {
        return;
    };
    let changed = {
        let mut current = shared.pane_agents.lock().unwrap();
        let changed = pane_agent_sets_differ(&current, &next);
        *current = next;
        changed
    };
    if changed {
        shared.emit.notify_one();
    }
}

/// Focus handling: rescan pane agents, clear unseen, notify TUIs.
fn handle_focus(shared: &Shared, name: &str) {
    refresh_pane_agents(shared);
    let had_unseen = shared.engine.lock().unwrap().handle_focus(name);
    if had_unseen {
        shared.emit.notify_one();
    }
    publish_session_viewed(shared, name, None);
}

fn switch_to_visible_index(shared: &Shared, index: i64, target: Option<&SwitchTarget>) {
    let payload = shared.engine.lock().unwrap().last_payload();
    let Some(payload) = payload else {
        shared.emit.notify_one();
        return;
    };
    let idx = index - 1;
    if idx < 0 || idx as usize >= payload.sessions.len() {
        return;
    }
    let name = &payload.sessions[idx as usize].name;
    shared.provider.switch_session(name, target);
}

// --- Sidebar orchestration ---

fn toggle_sidebar(shared: &Arc<Shared>) {
    let visible = {
        let mut sb = shared.sidebar.lock().unwrap();
        sb.visible = !sb.visible;
        sb.visible
    };
    if !visible {
        for pane in shared.provider.list_sidebar_panes(None) {
            shared.provider.hide_sidebar(&pane.pane_id);
        }
    } else {
        for w in shared.provider.list_active_windows() {
            ensure_sidebar_in_window(
                shared,
                Some(&HookContext {
                    client_tty: None,
                    session: w.session_name.clone(),
                    window_id: w.id.clone(),
                }),
            );
        }
        schedule_sidebar_resize(shared, None);
        publish(shared, &ServerMessage::ReIdentify);
    }
}

fn ensure_sidebar_in_window(shared: &Arc<Shared>, ctx: Option<&HookContext>) {
    {
        let sb = shared.sidebar.lock().unwrap();
        if !sb.visible {
            return;
        }
    }

    let cur_session = match ctx.map(|c| c.session.clone()) {
        Some(s) => Some(s),
        None => shared.provider.current_session(),
    };
    if cur_session.is_none() {
        return;
    }

    let window_id = match ctx.map(|c| c.window_id.clone()) {
        Some(w) => w,
        None => match shared.provider.current_window_id() {
            Some(w) => w,
            None => return,
        },
    };

    // Spawn-in-progress guard.
    {
        let mut sb = shared.sidebar.lock().unwrap();
        if !sb.pending_spawns.insert(window_id.clone()) {
            return;
        }
    }

    let existing = shared.provider.list_sidebar_panes(None);
    let has_in_window = existing.iter().any(|p| p.window_id == window_id);
    if !has_in_window {
        let (width, position) = {
            let sb = shared.sidebar.lock().unwrap();
            (sb.width, sb.position)
        };
        let command = tui_command(&window_id);
        let spawned = shared.provider.spawn_sidebar(&window_id, width, position, &command);
        log::debug!("ensure_sidebar_in_window: spawned {spawned:?} in {window_id}");
    }

    shared.sidebar.lock().unwrap().pending_spawns.remove(&window_id);

    if !has_in_window {
        // Layout changed — enforce widths (with the tmux settle follow-up).
        schedule_sidebar_resize(shared, None);
    }
}

/// Collapse rapid hook-fired ensure calls; drained by the 150ms tick task.
fn enqueue_ensure(shared: &Shared, ctx: Option<HookContext>) {
    let mut sb = shared.sidebar.lock().unwrap();
    match ctx {
        Some(c) => {
            sb.pending_ensure.insert(c.window_id.clone(), c);
        }
        None => sb.pending_ensure_no_ctx = true,
    }
}

fn drain_pending_ensures(shared: &Arc<Shared>) {
    let (ctxs, no_ctx) = {
        let mut sb = shared.sidebar.lock().unwrap();
        let ctxs: Vec<HookContext> = sb.pending_ensure.drain().map(|(_, c)| c).collect();
        let no_ctx = std::mem::take(&mut sb.pending_ensure_no_ctx);
        (ctxs, no_ctx)
    };
    if ctxs.is_empty() && no_ctx {
        ensure_sidebar_in_window(shared, None);
        return;
    }
    for c in &ctxs {
        ensure_sidebar_in_window(shared, Some(c));
    }
}

/// Resize now, then again after tmux settles its layout (~120ms).
fn schedule_sidebar_resize(shared: &Arc<Shared>, ctx: Option<SidebarResizeContext>) {
    resize_sidebars(shared, ctx);
    let shared = shared.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        resize_sidebars(&shared, None);
    });
}

fn resize_sidebars(shared: &Shared, ctx: Option<SidebarResizeContext>) {
    let panes = shared.provider.list_sidebar_panes(None);
    let mut sb = shared.sidebar.lock().unwrap();

    if panes.is_empty() {
        sb.snapshots = indexmap::IndexMap::new();
        return;
    }

    let now = now_ms();
    let SidebarState { snapshots, suppressed, cooldown, .. } = &mut *sb;
    let next_width = resolve_sidebar_width_from_resize_context(
        ctx.as_ref(),
        &panes,
        snapshots,
        suppressed,
        cooldown,
        now,
    );

    if let Some(next) = next_width
        && next != sb.width
    {
        // A deliberate divider drag — adopt and persist it.
        sb.width = next;
        let mut settings = tt_config::load().unwrap_or_default();
        settings.agentboard.sidebar_width = Some(next as f64);
        let _ = tt_config::save(&settings);
        shared.emit.notify_one();
    }

    let width = sb.width;
    for pane in &panes {
        if pane.width == width {
            continue;
        }
        sb.suppressed.insert(
            pane.pane_id.clone(),
            SidebarResizeSuppression { width, expires_at: now + 1_000 },
        );
        shared.provider.resize_sidebar_pane(&pane.pane_id, width);
    }

    sb.snapshots = snapshot_sidebar_windows(&panes);
}

fn quit_all(shared: &Shared) {
    if shared.shutting_down.swap(true, Ordering::SeqCst) {
        return;
    }
    for pane in shared.provider.list_sidebar_panes(None) {
        shared.provider.kill_sidebar_pane(&pane.pane_id);
    }
    shared.provider.cleanup_sidebar();
    publish(shared, &ServerMessage::Quit);
    cleanup(shared);
    // Give the Quit message a beat to flush to SSE clients.
    std::thread::sleep(Duration::from_millis(50));
    std::process::exit(0);
}

fn cleanup(shared: &Shared) {
    let _ = std::fs::remove_file(PID_FILE);
    shared.provider.cleanup_hooks();
}

// --- Client command dispatch (POST /command) ---

/// Envelope for TUI commands. `fromSession` replaces the TS per-socket
/// `identify-pane` identity: each request carries the sender's session.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandEnvelope {
    #[serde(default)]
    from_session: Option<String>,
    command: ClientCommand,
}

fn handle_command(shared: &Arc<Shared>, envelope: CommandEnvelope) {
    let from_session = envelope.from_session;
    match envelope.command {
        ClientCommand::SwitchSession { name } => {
            let target = SwitchTarget { client_tty: None, from_session };
            shared.provider.switch_session(&name, Some(&target));
            // Optimistic: clear unseen and tell TUIs a client is arriving,
            // without waiting for the tmux hook round-trip.
            let had_unseen = shared.engine.lock().unwrap().handle_focus(&name);
            if had_unseen {
                shared.emit.notify_one();
            }
            let select = SessionViewedSelect { session: name.clone(), agent: None };
            publish_session_viewed(shared, &name, Some(select));
        }
        ClientCommand::SwitchIndex { index } => {
            let target = SwitchTarget { client_tty: None, from_session };
            switch_to_visible_index(shared, index, Some(&target));
        }
        ClientCommand::NewSession => {
            shared.provider.create_session(None, None);
            shared.emit.notify_one();
        }
        ClientCommand::KillSession { name } => {
            shared.provider.kill_session(&name);
            shared.emit.notify_one();
        }
        ClientCommand::ReorderSession { name, delta } => {
            shared.engine.lock().unwrap().reorder(&name, delta);
            shared.emit.notify_one();
        }
        ClientCommand::Refresh => shared.emit.notify_one(),
        ClientCommand::MarkSeen { name } => {
            let changed = shared.engine.lock().unwrap().mark_seen_patch(&name).is_some();
            if changed {
                shared.emit.notify_one();
            }
        }
        ClientCommand::DismissAgent { session, agent, thread_id } => {
            let changed =
                shared.engine.lock().unwrap().dismiss(&session, &agent, thread_id.as_deref());
            if changed {
                shared.emit.notify_one();
            }
        }
        ClientCommand::SetTheme { theme } => {
            shared.engine.lock().unwrap().set_theme(theme);
            shared.emit.notify_one();
        }
        ClientCommand::ReportWidth { .. } => {
            // No-op: sidebar width is config-only, not auto-saved from drag.
        }
        ClientCommand::Quit => quit_all(shared),
        ClientCommand::IdentifyPane { .. } => {
            // Per-socket identity replaced by fromSession on each request.
        }
        ClientCommand::FocusAgentPane { session, agent, thread_id, thread_name } => {
            let switched = focus_agent_pane(
                shared,
                &session,
                &agent,
                thread_id.as_deref(),
                thread_name.as_deref(),
                from_session.as_deref(),
            );
            // The viewer just landed on the agent session's sidebar — hand
            // the clicked-agent selection over so it highlights there.
            if switched {
                let select = SessionViewedSelect {
                    session: session.clone(),
                    agent: Some(SelectedAgent { agent, thread_id }),
                };
                publish_session_viewed(shared, &session, Some(select));
            }
        }
        ClientCommand::KillAgentPane { session, agent, thread_id, thread_name } => {
            if let Some(pane_id) =
                resolve_pane(shared, &session, &agent, thread_id.as_deref(), thread_name.as_deref())
            {
                shared.provider.client().kill_pane(&pane_id);
            }
        }
    }
}

// --- Focus/kill agent pane (T6, ports focusAgentPane/killAgentPane) ---

const PANE_HIGHLIGHT_BORDER: &str = "fg=#fab387,bold";
const PANE_HIGHLIGHT_BG: &str = "bg=#2a2a4a";
const PANE_HIGHLIGHT_MS: u64 = 300;

fn resolve_pane(
    shared: &Shared,
    session: &str,
    agent: &str,
    thread_id: Option<&str>,
    thread_name: Option<&str>,
) -> Option<String> {
    let sidebar_ids: std::collections::HashSet<String> =
        shared.provider.list_sidebar_panes(None).into_iter().map(|p| p.pane_id).collect();
    resolve_agent_pane_id(session, agent, thread_id, thread_name, &sidebar_ids)
}

/// Returns true when a pane was resolved and the viewer switched to it.
fn focus_agent_pane(
    shared: &Arc<Shared>,
    session: &str,
    agent: &str,
    thread_id: Option<&str>,
    thread_name: Option<&str>,
    from_session: Option<&str>,
) -> bool {
    let Some(pane_id) = resolve_pane(shared, session, agent, thread_id, thread_name) else {
        return false;
    };
    let tmux = shared.provider.client();

    // The agent's pane may live in another session/window. Switch the
    // client(s) attached to the sidebar's own session — resolved at action
    // time, so a multi-terminal setup moves the terminal that was used, not
    // whichever client happens to be most-recently-active.
    let ttys: Vec<String> = match from_session {
        Some(from) => {
            let clients = shared.provider.client().list_clients();
            tt_agentboard::tmux::resolve_switch_targets(
                clients.iter().map(|c| (c.tty.as_str(), c.session_name.as_str())),
                Some(from),
            )
        }
        None => Vec::new(),
    };
    if ttys.is_empty() {
        tmux.switch_client(session, None);
    } else {
        for tty in &ttys {
            tmux.switch_client(session, Some(tty));
        }
    }
    tmux.run(&["select-window", "-t", &pane_id]);
    tmux.select_pane(&pane_id);

    // Flash the pane so the eye lands on it, then reset.
    tmux.run(&[
        "set-option",
        "-p",
        "-t",
        &pane_id,
        "pane-active-border-style",
        PANE_HIGHLIGHT_BORDER,
    ]);
    tmux.set_pane_style(&pane_id, PANE_HIGHLIGHT_BG);
    let shared = shared.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(PANE_HIGHLIGHT_MS)).await;
        let tmux = shared.provider.client();
        tmux.run(&[
            "set-option",
            "-p",
            "-t",
            &pane_id,
            "-u",
            "pane-active-border-style",
        ]);
        tmux.set_pane_style(&pane_id, "");
    });
    true
}

// --- HTTP ---

async fn handle_conn(mut socket: TcpStream, shared: Arc<Shared>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];

    let head_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        match socket.read(&mut tmp).await {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    };

    let head_str = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let Some(head) = parse_request_head(&head_str) else {
        let _ = socket.write_all(response_bytes(400, "bad request").as_bytes()).await;
        return;
    };

    while buf.len() - head_end < head.content_length {
        match socket.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
        if buf.len() > 8 * 1024 * 1024 {
            break;
        }
    }
    let body_end = (head_end + head.content_length).min(buf.len());
    let body = String::from_utf8_lossy(&buf[head_end..body_end]).to_string();

    // Any HTTP request proves tmux hooks are still active.
    if shared.client_count.load(Ordering::SeqCst) > 0 {
        *shared.idle_since_ms.lock().unwrap() = None;
    } else {
        *shared.idle_since_ms.lock().unwrap() = Some(now_ms());
    }

    let (path, query) = match head.path.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (head.path.clone(), None),
    };

    match (head.method.as_str(), path.as_str()) {
        ("GET", "/events") => {
            serve_sse(socket, shared).await;
        }
        ("POST", "/refresh") => {
            shared.emit.notify_one();
            respond(socket, 200, "ok").await;
        }
        ("POST", "/focus") => {
            match parse_context(&body) {
                Some(ctx) => handle_focus(&shared, &ctx.session),
                None => {
                    // Legacy: body is just the session name.
                    let name = body.trim().trim_matches('"');
                    if !name.is_empty() {
                        handle_focus(&shared, name);
                    }
                }
            }
            respond(socket, 200, "ok").await;
        }
        ("POST", "/toggle") => {
            toggle_sidebar(&shared);
            shared.emit.notify_one();
            respond(socket, 200, "ok").await;
        }
        ("POST", "/ensure-sidebar") => {
            enqueue_ensure(&shared, parse_context(&body));
            respond(socket, 200, "ok").await;
        }
        ("POST", "/resize-sidebars") => {
            schedule_sidebar_resize(&shared, parse_resize_context(&body));
            respond(socket, 200, "ok").await;
        }
        ("POST", "/switch-index") => {
            let index = query
                .as_deref()
                .and_then(|q| {
                    q.split('&').find_map(|kv| kv.strip_prefix("index=")).map(str::to_string)
                })
                .and_then(|v| v.parse::<i64>().ok());
            match index {
                None => respond(socket, 400, "missing index").await,
                Some(index) => {
                    // ctx.clientTty comes from tmux expanding #{client_tty} at
                    // keypress time — fresh, so target that exact client.
                    let tty = parse_context(&body).and_then(|c| c.client_tty);
                    let target = SwitchTarget { client_tty: tty, from_session: None };
                    switch_to_visible_index(&shared, index, Some(&target));
                    respond(socket, 200, "ok").await;
                }
            }
        }
        ("POST", "/quit") => {
            respond(socket, 200, "ok").await;
            quit_all(&shared);
        }
        ("POST", "/shutdown") => {
            respond(socket, 200, "ok").await;
            // Deferred so the response flushes; lets restart terminate a
            // server whose PID file was lost.
            let shared = shared.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                cleanup(&shared);
                std::process::exit(0);
            });
        }
        ("POST", "/command") => match serde_json::from_str::<CommandEnvelope>(&body) {
            Ok(envelope) => {
                handle_command(&shared, envelope);
                respond(socket, 204, "").await;
            }
            Err(_) => respond(socket, 400, "invalid json").await,
        },
        ("POST", "/set-status" | "/set-progress" | "/log" | "/clear-log") => {
            let outcome = handle_request(&head.method, &path, &body);
            if let Some(mutation) = outcome.mutation {
                {
                    let mut engine = shared.engine.lock().unwrap();
                    apply_mutation(&mut engine, mutation, now_ms());
                }
                shared.emit.notify_one();
            }
            let _ =
                socket.write_all(response_bytes(outcome.status, &outcome.body).as_bytes()).await;
        }
        _ => {
            let routes = concat!(
                "{\"name\":\"agentboard server\",\"routes\":[",
                "\"GET /events\",\"POST /refresh\",\"POST /resize-sidebars\",",
                "\"POST /focus\",\"POST /toggle\",\"POST /quit\",\"POST /shutdown\",",
                "\"POST /switch-index?index=N\",\"POST /ensure-sidebar\",\"POST /command\",",
                "\"POST /set-status\",\"POST /set-progress\",\"POST /log\",\"POST /clear-log\"]}"
            );
            respond(socket, 200, routes).await;
        }
    }
}

async fn respond(mut socket: TcpStream, status: u16, body: &str) {
    let _ = socket.write_all(response_bytes(status, body).as_bytes()).await;
}

/// Stream `ServerMessage` JSON as SSE. The current state is sent first (a
/// fresh compute if none is cached), then every broadcast until the client
/// hangs up.
async fn serve_sse(mut socket: TcpStream, shared: Arc<Shared>) {
    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
    if socket.write_all(header.as_bytes()).await.is_err() {
        return;
    }

    shared.client_count.fetch_add(1, Ordering::SeqCst);
    *shared.idle_since_ms.lock().unwrap() = None;
    let mut rx = shared.tx.subscribe();

    // Seed with the current state. broadcast_state also fans out to other
    // clients; acceptable (idempotent snapshot).
    broadcast_state(&shared);

    loop {
        match rx.recv().await {
            Ok(json) => {
                let frame = format!("data: {json}\n\n");
                if socket.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    let remaining = shared.client_count.fetch_sub(1, Ordering::SeqCst) - 1;
    if remaining == 0 {
        *shared.idle_since_ms.lock().unwrap() = Some(now_ms());
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}
