//! Sidebar/session orchestration on top of [`TmuxClient`]. Ports slot-1
//! `mux-tmux/provider.ts` with the capability-trait layer collapsed.
//!
//! Sidebar lifecycle: a pane titled [`SIDEBAR_PANE_TITLE`] is split off the
//! window's edge pane; "hiding" stashes it into the invisible
//! [`STASH_SESSION`] via `join-pane -d`, and the next spawn restores it with
//! `join-pane` instead of launching a fresh TUI process.
//!
//! Deviation from TS: `spawn_sidebar` takes the pane command from the caller
//! (the server layer builds it from `std::env::current_exe()`) instead of
//! hardcoding `tt agentboard tui` — the TS `spawn("tt", …)` PATH coupling is
//! the known cutover gotcha.

use super::client::{PaneInfo, PaneScope, SplitWindowOptions, TmuxClient};

/// Hidden session that stores stashed (hidden) sidebar panes.
pub const STASH_SESSION: &str = "_ab_stash";
/// Pane title marking a pane as an agentboard sidebar.
pub const SIDEBAR_PANE_TITLE: &str = "agentboard-sidebar";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarPosition {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxSessionInfo {
    pub name: String,
    /// Epoch seconds.
    pub created_at: i64,
    pub dir: String,
    pub windows: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveWindow {
    pub id: String,
    pub session_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPane {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: String,
    pub width: u32,
    pub window_width: Option<u32>,
}

/// Where to route a session switch.
#[derive(Debug, Clone, Default)]
pub struct SwitchTarget {
    /// Switch exactly this client.
    pub client_tty: Option<String>,
    /// Switch every client currently attached to this session.
    pub from_session: Option<String>,
}

/// TTYs of all clients attached to `from_session` (`runtime/client-routing.ts`).
pub fn resolve_switch_targets<'a>(
    clients: impl IntoIterator<Item = (&'a str, &'a str)>,
    from_session: Option<&str>,
) -> Vec<String> {
    let Some(from) = from_session else {
        return Vec::new();
    };
    clients
        .into_iter()
        .filter(|(_, session)| *session == from)
        .map(|(tty, _)| tty.to_string())
        .collect()
}

/// The window's edge pane to split a sidebar off of: leftmost pane for a
/// left sidebar, rightmost for a right sidebar.
pub fn pick_edge_pane(panes: &[PaneInfo], position: SidebarPosition) -> Option<&PaneInfo> {
    match position {
        SidebarPosition::Left => panes.iter().min_by_key(|p| p.left),
        SidebarPosition::Right => panes.iter().max_by_key(|p| p.right),
    }
}

/// Derive sidebar panes (+ their window's full width) from a pane listing.
/// Window width is the max `right + 1` across the window's panes, since tmux
/// has no direct per-pane "window width" format in list output.
pub fn sidebar_panes_from(panes: &[PaneInfo]) -> Vec<SidebarPane> {
    let mut window_widths: indexmap::IndexMap<&str, u32> = indexmap::IndexMap::new();
    for p in panes {
        let w = window_widths.entry(p.window_id.as_str()).or_insert(0);
        *w = (*w).max(p.right + 1);
    }
    panes
        .iter()
        .filter(|p| p.title == SIDEBAR_PANE_TITLE && p.session_name != STASH_SESSION)
        .map(|p| SidebarPane {
            pane_id: p.id.clone(),
            session_name: p.session_name.clone(),
            window_id: p.window_id.clone(),
            width: p.width,
            window_width: window_widths.get(p.window_id.as_str()).copied(),
        })
        .collect()
}

/// The 7 global hooks and their `run-shell` commands, exactly as the TS
/// `setupHooks`/`init` register them. `#{q:...}` shell-escapes each tmux
/// variable at hook-fire time to prevent injection from session names etc.
pub fn hook_definitions(host: &str, port: u16) -> Vec<(&'static str, String)> {
    let base = format!("http://{host}:{port}");
    let hook_post = |path: &str, data: Option<&str>| -> String {
        let body = match data {
            Some(d) => format!(" -d \\\"{d}\\\""),
            None => String::new(),
        };
        format!(
            "run-shell -b \"curl -s -o /dev/null -X POST {base}{path}{body} >/dev/null 2>&1 || true\""
        )
    };
    let focus_body = "#{q:client_tty}|#{q:session_name}|#{q:window_id}";
    let resize_body =
        "#{q:pane_id}|#{q:session_name}|#{q:window_id}|#{q:pane_width}|#{q:window_width}";

    let focus_cmd = hook_post("/focus", Some(focus_body));
    let refresh_cmd = hook_post("/refresh", None);
    let resize_cmd = hook_post("/resize-sidebars", None);
    let resize_pane_cmd = hook_post("/resize-sidebars", Some(resize_body));
    let ensure_cmd = hook_post("/ensure-sidebar", Some(focus_body));

    vec![
        // client-session-changed: update focus AND ensure sidebar in the new
        // session's window.
        ("client-session-changed", format!("{focus_cmd} ; {ensure_cmd}")),
        ("session-created", refresh_cmd.clone()),
        ("session-closed", refresh_cmd),
        ("client-resized", resize_cmd),
        ("after-select-window", ensure_cmd.clone()),
        ("after-new-window", ensure_cmd),
        ("after-resize-pane", resize_pane_cmd),
    ]
}

/// tmux orchestration for agentboard: session listing/switching plus the
/// sidebar pane lifecycle.
#[derive(Debug, Clone, Default)]
pub struct TmuxProvider {
    tmux: TmuxClient,
}

impl TmuxProvider {
    pub fn new() -> Self {
        Self { tmux: TmuxClient::new() }
    }

    pub fn client(&self) -> &TmuxClient {
        &self.tmux
    }

    // ─── Sessions ──────────────────────────────────────

    /// All sessions except the stash, with each session's dir corrected to
    /// its active pane's cwd where known.
    pub fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        let sessions = self.tmux.list_sessions();
        let active_dirs = self.tmux.active_session_dirs();
        sessions
            .into_iter()
            .filter(|s| s.name != STASH_SESSION)
            .map(|s| MuxSessionInfo {
                dir: active_dirs.get(&s.name).cloned().unwrap_or(s.dir),
                name: s.name,
                created_at: s.created_at,
                windows: s.window_count,
            })
            .collect()
    }

    pub fn switch_session(&self, name: &str, target: Option<&SwitchTarget>) {
        if let Some(tty) = target.and_then(|t| t.client_tty.as_deref()) {
            self.tmux.switch_client(name, Some(tty));
            return;
        }
        let ttys = match target.and_then(|t| t.from_session.as_deref()) {
            Some(from) => {
                let clients = self.tmux.list_clients();
                resolve_switch_targets(
                    clients.iter().map(|c| (c.tty.as_str(), c.session_name.as_str())),
                    Some(from),
                )
            }
            None => Vec::new(),
        };
        if ttys.is_empty() {
            // No known origin — let tmux pick its most-recently-active client.
            self.tmux.switch_client(name, None);
            return;
        }
        for tty in &ttys {
            self.tmux.switch_client(name, Some(tty));
        }
    }

    pub fn current_session(&self) -> Option<String> {
        self.tmux.current_session()
    }

    pub fn session_dir(&self, name: &str) -> String {
        self.tmux.session_dir(name)
    }

    pub fn pane_count(&self, name: &str) -> usize {
        self.tmux.pane_count(name)
    }

    pub fn create_session(&self, name: Option<&str>, dir: Option<&str>) {
        self.tmux.new_session(name, dir);
    }

    pub fn kill_session(&self, name: &str) {
        self.tmux.kill_session(name);
    }

    pub fn all_pane_counts(&self) -> indexmap::IndexMap<String, usize> {
        self.tmux.all_pane_counts()
    }

    // ─── Windows ───────────────────────────────────────

    /// Active windows outside the stash session.
    pub fn list_active_windows(&self) -> Vec<ActiveWindow> {
        self.tmux
            .list_windows(None)
            .into_iter()
            .filter(|w| w.active && w.session_name != STASH_SESSION)
            .map(|w| ActiveWindow { id: w.id, session_name: w.session_name })
            .collect()
    }

    pub fn current_window_id(&self) -> Option<String> {
        let id = self.tmux.current_window_id(None);
        if id.is_empty() { None } else { Some(id) }
    }

    // ─── Hooks ─────────────────────────────────────────

    pub fn setup_hooks(&self, server_host: &str, server_port: u16) {
        for (name, cmd) in hook_definitions(server_host, server_port) {
            self.tmux.set_global_hook(name, &cmd);
        }
    }

    pub fn cleanup_hooks(&self) {
        for (name, _) in hook_definitions("_", 0) {
            self.tmux.unset_global_hook(name);
        }
    }

    // ─── Sidebar lifecycle ─────────────────────────────

    /// Kill the stash session used for hiding sidebar panes.
    pub fn cleanup_sidebar(&self) {
        self.tmux.kill_session(STASH_SESSION);
    }

    /// Sidebar panes, optionally scoped to one session, with window widths.
    pub fn list_sidebar_panes(&self, session_name: Option<&str>) -> Vec<SidebarPane> {
        let panes = match session_name {
            Some(name) => self.tmux.list_panes(PaneScope::Session(name)),
            None => self.tmux.list_panes(PaneScope::All),
        };
        sidebar_panes_from(&panes)
    }

    /// Ensure the invisible stash session exists for hiding sidebar panes.
    fn ensure_stash(&self) {
        if !self.tmux.has_session(STASH_SESSION) {
            self.tmux.raw_run(&[
                "new-session",
                "-d",
                "-s",
                STASH_SESSION,
                "-x",
                "80",
                "-y",
                "24",
            ]);
        }
    }

    /// Spawn (or restore from stash) the sidebar pane in `window_id`, sized
    /// `width` at `position`. `command` is what runs in a fresh pane (the
    /// caller builds it from `current_exe()`). Returns the pane id.
    ///
    /// Neither path selects the new pane: the TUI's terminal-capability
    /// detection fires on focus-in, and refocusing the main pane immediately
    /// would leak capability query responses (DECRPM, DA1, Kitty graphics)
    /// into it as garbage escape sequences. The TUI refocuses via
    /// `REFOCUS_WINDOW` itself once detection finishes.
    pub fn spawn_sidebar(
        &self,
        window_id: &str,
        width: u32,
        position: SidebarPosition,
        command: &str,
    ) -> Option<String> {
        let panes = self.tmux.list_panes(PaneScope::Window(window_id));
        log::debug!("spawn_sidebar window={window_id} panes={}", panes.len());
        let target_pane = pick_edge_pane(&panes, position)?.id.clone();

        // Restore a stashed sidebar pane if one exists.
        let stash_panes = self.tmux.list_panes(PaneScope::Session(STASH_SESSION));
        if let Some(stashed) = stash_panes.iter().find(|p| p.title == SIDEBAR_PANE_TITLE) {
            log::debug!("spawn_sidebar: restoring {} onto {target_pane}", stashed.id);
            let join_flag = match position {
                SidebarPosition::Left => "-hb",
                SidebarPosition::Right => "-h",
            };
            self.tmux.raw_run(&[
                "join-pane",
                join_flag,
                "-f",
                "-l",
                &width.to_string(),
                "-s",
                &stashed.id,
                "-t",
                &target_pane,
            ]);
            self.tmux.set_pane_title(&stashed.id, SIDEBAR_PANE_TITLE);
            return Some(stashed.id.clone());
        }

        // No stashed pane — spawn fresh.
        log::debug!("spawn_sidebar: fresh split off {target_pane}");
        let new_pane = self.tmux.split_window(&SplitWindowOptions {
            target: &target_pane,
            vertical: false,
            before: position == SidebarPosition::Left,
            full_window: true,
            size: Some(width),
            command: Some(command),
        })?;
        self.tmux.set_pane_title(&new_pane.id, SIDEBAR_PANE_TITLE);
        Some(new_pane.id)
    }

    /// Stash (hide) a sidebar pane into the stash session.
    pub fn hide_sidebar(&self, pane_id: &str) {
        self.ensure_stash();
        // Ensure the stash window is large enough to accept another pane —
        // join-pane fails with "pane too small" when stash panes fill up.
        let stash_target = format!("{STASH_SESSION}:");
        self.tmux.raw_run(&[
            "resize-window",
            "-t",
            &stash_target,
            "-x",
            "200",
            "-y",
            "200",
        ]);
        log::debug!("hide_sidebar: stashing {pane_id}");
        self.tmux.raw_run(&["join-pane", "-d", "-s", pane_id, "-t", &stash_target]);
    }

    pub fn kill_sidebar_pane(&self, pane_id: &str) {
        self.tmux.kill_pane(pane_id);
    }

    pub fn resize_sidebar_pane(&self, pane_id: &str, width: u32) {
        self.tmux.resize_pane(pane_id, Some(width), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(
        id: &str,
        window_id: &str,
        title: &str,
        session: &str,
        left: u32,
        right: u32,
    ) -> PaneInfo {
        PaneInfo {
            id: id.into(),
            session_name: session.into(),
            window_id: window_id.into(),
            window_index: 0,
            index: 0,
            active: false,
            tty: String::new(),
            pid: 0,
            cwd: String::new(),
            command: String::new(),
            title: title.into(),
            width: right - left + 1,
            height: 50,
            left,
            right,
        }
    }

    #[test]
    fn resolve_switch_targets_filters_by_session() {
        let clients = [
            ("/dev/pts/1", "main"),
            ("/dev/pts/2", "side"),
            ("/dev/pts/3", "main"),
        ];
        let ttys = resolve_switch_targets(clients, Some("main"));
        assert_eq!(ttys, vec!["/dev/pts/1", "/dev/pts/3"]);
        assert!(resolve_switch_targets(clients, None).is_empty());
        assert!(resolve_switch_targets(clients, Some("nope")).is_empty());
    }

    #[test]
    fn edge_pane_is_leftmost_for_left_and_rightmost_for_right() {
        let panes = vec![
            pane("%1", "@1", "", "s", 0, 79),
            pane("%2", "@1", "", "s", 80, 159),
        ];
        assert_eq!(pick_edge_pane(&panes, SidebarPosition::Left).unwrap().id, "%1");
        assert_eq!(pick_edge_pane(&panes, SidebarPosition::Right).unwrap().id, "%2");
        assert!(pick_edge_pane(&[], SidebarPosition::Left).is_none());
    }

    #[test]
    fn sidebar_panes_computes_window_width_and_excludes_stash() {
        let panes = vec![
            pane("%1", "@1", "", "main", 0, 119),
            pane("%2", "@1", SIDEBAR_PANE_TITLE, "main", 120, 159),
            pane("%3", "@2", SIDEBAR_PANE_TITLE, STASH_SESSION, 0, 39),
        ];
        let sidebars = sidebar_panes_from(&panes);
        assert_eq!(sidebars.len(), 1);
        assert_eq!(sidebars[0].pane_id, "%2");
        assert_eq!(sidebars[0].width, 40);
        // Window width = max(right)+1 across the window's panes.
        assert_eq!(sidebars[0].window_width, Some(160));
    }

    #[test]
    fn hook_definitions_match_ts_setup_hooks() {
        let hooks = hook_definitions("127.0.0.1", 4201);
        let names: Vec<&str> = hooks.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "client-session-changed",
                "session-created",
                "session-closed",
                "client-resized",
                "after-select-window",
                "after-new-window",
                "after-resize-pane",
            ]
        );

        let by_name: std::collections::HashMap<&str, &str> =
            hooks.iter().map(|(n, c)| (*n, c.as_str())).collect();

        // Exact parity with the strings TS `setupHooks` registers.
        assert_eq!(
            by_name["session-created"],
            "run-shell -b \"curl -s -o /dev/null -X POST http://127.0.0.1:4201/refresh >/dev/null 2>&1 || true\""
        );
        assert_eq!(
            by_name["after-resize-pane"],
            "run-shell -b \"curl -s -o /dev/null -X POST http://127.0.0.1:4201/resize-sidebars -d \\\"#{q:pane_id}|#{q:session_name}|#{q:window_id}|#{q:pane_width}|#{q:window_width}\\\" >/dev/null 2>&1 || true\""
        );
        let ensure = "run-shell -b \"curl -s -o /dev/null -X POST http://127.0.0.1:4201/ensure-sidebar -d \\\"#{q:client_tty}|#{q:session_name}|#{q:window_id}\\\" >/dev/null 2>&1 || true\"";
        assert_eq!(by_name["after-select-window"], ensure);
        assert_eq!(by_name["after-new-window"], ensure);
        // client-session-changed chains focus + ensure.
        assert!(by_name["client-session-changed"].contains("/focus"));
        assert!(by_name["client-session-changed"].contains(" ; "));
        assert!(by_name["client-session-changed"].ends_with(ensure));
    }
}
