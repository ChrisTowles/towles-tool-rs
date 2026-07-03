//! `ttr agentboard tui` — the ratatui sidebar (phase T4 of
//! docs/AGENTBOARD-TMUX-SPEC.md). Ports slot-1 `tui/index.tsx` +
//! `tui/components/*` from OpenTUI/SolidJS to ratatui/crossterm.
//!
//! Transport: SSE subscription on a reader thread feeding an mpsc channel;
//! commands go out as `POST /command` with a `fromSession` envelope (replaces
//! the TS WebSocket + per-socket `identify-pane`).
//!
//! Deviations: no terminal-capability wait before the main-pane refocus
//! (crossterm issues no capability queries, so the leak the TS guarded
//! against cannot happen — refocus runs immediately). Mouse: click a card to
//! switch, click an agent row to focus its pane, wheel to move focus (the
//! hover/✕-click affordances are keyboard-only: `d` dismisses).

mod view;

use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use serde_json::json;

use tt_agentboard::themes::{Theme, resolve_theme};
use tt_agentboard::tmux::{SIDEBAR_PANE_TITLE, TmuxClient};
use tt_agentboard::types::{ClientCommand, ServerMessage, SessionData};

use crate::commands::agentboard_client::{ensure_server, http_post, sse_subscribe};
use crate::ui;

pub const SPINNERS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TOAST_MS: u64 = 4000;
const SESSIONIZER_SCRIPT: &str = include_str!("sessionizer.sh");

/// What a rendered list line resolves to for mouse hit-testing.
#[derive(Clone, PartialEq)]
pub enum LineTarget {
    None,
    Session(String),
    Agent { session: String, idx: usize },
}

#[derive(Clone, Copy, PartialEq)]
pub enum PanelFocus {
    Sessions,
    Agents,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Modal {
    None,
    ConfirmKill,
    Help,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToastTone {
    Error,
    /// Reserved for parity with the TS toast tones; nothing emits it yet.
    #[allow(dead_code)]
    Info,
    Success,
}

pub struct App {
    pub sessions: Vec<SessionData>,
    pub theme: &'static Theme,
    pub preferred_editor: String,
    pub connected: bool,
    pub focused_session: Option<String>,
    pub panel_focus: PanelFocus,
    pub focused_agent_idx: usize,
    pub modal: Modal,
    pub kill_target: Option<String>,
    pub toast: Option<(String, ToastTone, Instant)>,
    /// Optimistic switch-away marker (cleared when a focus broadcast says this
    /// session is viewed again).
    pub pending_switch: Option<String>,
    /// The tmux session this sidebar pane lives in (the ● you-are-here marker).
    pub startup_session: Option<String>,
    pub spin_idx: usize,
    pub scroll: u16,
    pub now_ms: i64,
    /// Per-line click targets for the last rendered session list.
    pub hit_map: Vec<LineTarget>,
    /// Screen area of the last rendered session list.
    pub list_area: ratatui::layout::Rect,
    exit: bool,
}

impl App {
    fn new(startup_session: Option<String>) -> Self {
        Self {
            sessions: Vec::new(),
            theme: resolve_theme(None),
            preferred_editor: "code".into(),
            connected: false,
            focused_session: None,
            panel_focus: PanelFocus::Sessions,
            focused_agent_idx: 0,
            modal: Modal::None,
            kill_target: None,
            toast: None,
            pending_switch: None,
            startup_session,
            spin_idx: 0,
            scroll: 0,
            now_ms: 0,
            hit_map: Vec::new(),
            list_area: ratatui::layout::Rect::default(),
            exit: false,
        }
    }

    pub fn current_session(&self) -> Option<&str> {
        self.pending_switch.as_deref().or(self.startup_session.as_deref())
    }

    pub fn focused_data(&self) -> Option<&SessionData> {
        let name = self.focused_session.as_deref()?;
        self.sessions.iter().find(|s| s.name == name)
    }

    fn toast(&mut self, message: impl Into<String>, tone: ToastTone) {
        self.toast = Some((message.into(), tone, Instant::now()));
    }

    fn send(&mut self, command: ClientCommand, success_msg: Option<&str>) {
        if !self.connected {
            self.toast("not connected to agentboard server", ToastTone::Error);
            return;
        }
        let envelope = json!({ "fromSession": self.startup_session, "command": command });
        match http_post("/command", &envelope.to_string()) {
            Ok(status) if status < 400 => {
                if let Some(msg) = success_msg {
                    self.toast(msg.to_string(), ToastTone::Success);
                }
            }
            _ => self.toast("not connected to agentboard server", ToastTone::Error),
        }
    }

    fn switch_to_session(&mut self, name: String) {
        // Optimistic local update — the server's broadcasts reconcile later.
        self.pending_switch = Some(name.clone());
        self.focused_session = Some(name.clone());
        self.panel_focus = PanelFocus::Sessions;
        self.focused_agent_idx = 0;
        self.send(ClientCommand::SwitchSession { name }, None);
    }

    fn move_local_focus(&mut self, delta: i64) {
        if self.sessions.is_empty() {
            return;
        }
        let current_idx = self
            .focused_session
            .as_deref()
            .and_then(|name| self.sessions.iter().position(|s| s.name == name))
            .unwrap_or(0);
        let next = (current_idx as i64 + delta).clamp(0, self.sessions.len() as i64 - 1) as usize;
        // Selection is local to this TUI — never sent to the server.
        self.focused_session = Some(self.sessions[next].name.clone());
    }

    fn move_agent_focus(&mut self, delta: i64) {
        let count = self.focused_data().map(|d| d.agents.len()).unwrap_or(0);
        if count == 0 {
            return;
        }
        self.focused_agent_idx =
            (self.focused_agent_idx as i64 + delta).clamp(0, count as i64 - 1) as usize;
    }

    fn activate_focused_agent(&mut self) {
        let Some(data) = self.focused_data() else {
            return;
        };
        let Some(agent) = data.agents.get(self.focused_agent_idx) else {
            return;
        };
        let (session, agent_name) = (data.name.clone(), agent.agent.clone());
        let (thread_id, thread_name) = (agent.thread_id.clone(), agent.thread_name.clone());
        self.pending_switch = Some(session.clone());
        let msg = format!("focusing {agent_name}");
        self.send(
            ClientCommand::FocusAgentPane { session, agent: agent_name, thread_id, thread_name },
            Some(&msg),
        );
    }

    fn dismiss_focused_agent(&mut self) {
        let Some(data) = self.focused_data() else {
            return;
        };
        let agents_len = data.agents.len();
        let Some(agent) = data.agents.get(self.focused_agent_idx) else {
            return;
        };
        let cmd = ClientCommand::DismissAgent {
            session: data.name.clone(),
            agent: agent.agent.clone(),
            thread_id: agent.thread_id.clone(),
        };
        let msg = format!("dismissed {}", agent.agent);
        self.send(cmd, Some(&msg));
        if self.focused_agent_idx >= agents_len.saturating_sub(1) && agents_len > 1 {
            self.focused_agent_idx = agents_len - 2;
        }
        if agents_len <= 1 {
            self.panel_focus = PanelFocus::Sessions;
        }
    }

    fn kill_focused_agent_pane(&mut self) {
        let Some(data) = self.focused_data() else {
            return;
        };
        let Some(agent) = data.agents.get(self.focused_agent_idx) else {
            return;
        };
        let cmd = ClientCommand::KillAgentPane {
            session: data.name.clone(),
            agent: agent.agent.clone(),
            thread_id: agent.thread_id.clone(),
            thread_name: agent.thread_name.clone(),
        };
        let msg = format!("killed {} pane", agent.agent);
        self.send(cmd, Some(&msg));
    }

    fn create_new_session(&mut self) {
        if std::env::var("TMUX").is_err() {
            self.send(ClientCommand::NewSession, None);
            return;
        }
        // Write the embedded sessionizer to a temp path and pop it up.
        let path = std::env::temp_dir().join("agentboard-sessionizer.sh");
        if std::fs::write(&path, SESSIONIZER_SCRIPT).is_err() {
            self.toast("failed to write sessionizer script", ToastTone::Error);
            return;
        }
        let tmux = TmuxClient::new();
        tmux.display_popup(&tt_agentboard::tmux::client::PopupOptions {
            title: Some(" new session "),
            width: Some("60%"),
            height: Some("60%"),
            ..tt_agentboard::tmux::client::PopupOptions::command(&format!(
                "bash \"{}\"",
                path.display()
            ))
        });
    }

    fn open_in_editor(&mut self) {
        let Some(data) = self.focused_data() else {
            return;
        };
        if data.dir.is_empty() {
            return;
        }
        let (dir, editor) = (data.dir.clone(), self.preferred_editor.clone());
        // Strip tmux env vars so the editor isn't locked into our session.
        let result = std::process::Command::new(&editor)
            .arg(&dir)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("TMUX_PLUGIN_MANAGER_PATH")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match result {
            Ok(_) => self.toast(format!("opening {dir} in {editor}"), ToastTone::Success),
            Err(e) => {
                self.toast(format!("failed to spawn editor \"{editor}\": {e}"), ToastTone::Error)
            }
        }
    }

    fn apply_message(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::State { sessions, theme, preferred_editor, .. } => {
                self.sessions = sessions;
                // Selection is local — initialize to this sidebar's own
                // session, and repair it if the selected session disappeared.
                let selected_ok = self
                    .focused_session
                    .as_deref()
                    .is_some_and(|sel| self.sessions.iter().any(|s| s.name == sel));
                if !selected_ok {
                    let startup = self.startup_session.as_deref();
                    self.focused_session = self
                        .sessions
                        .iter()
                        .find(|s| Some(s.name.as_str()) == startup)
                        .or_else(|| self.sessions.first())
                        .map(|s| s.name.clone());
                }
                if let Some(pending) = self.pending_switch.as_deref()
                    && !self.sessions.iter().any(|s| s.name == pending)
                {
                    self.pending_switch = None;
                }
                self.theme = resolve_theme(theme.as_deref());
                if !preferred_editor.is_empty() {
                    self.preferred_editor = preferred_editor;
                }
            }
            ServerMessage::SessionViewed { name, select } => {
                if Some(name.as_str()) == self.startup_session.as_deref() {
                    // A client is viewing this session again — any optimistic
                    // switch-away marker is stale.
                    self.pending_switch = None;
                    if let Some(sel) = select
                        && self.sessions.iter().any(|s| s.name == sel.session)
                    {
                        self.focused_session = Some(sel.session.clone());
                        match sel.agent {
                            Some(agent_sel) => {
                                let idx = self
                                    .sessions
                                    .iter()
                                    .find(|s| s.name == sel.session)
                                    .map(|d| &d.agents)
                                    .and_then(|agents| {
                                        agents.iter().position(|a| {
                                            a.agent == agent_sel.agent
                                                && a.thread_id == agent_sel.thread_id
                                        })
                                    });
                                if let Some(idx) = idx {
                                    self.panel_focus = PanelFocus::Agents;
                                    self.focused_agent_idx = idx;
                                }
                            }
                            None => self.panel_focus = PanelFocus::Sessions,
                        }
                    }
                }
            }
            ServerMessage::ReIdentify => {
                // Per-request fromSession replaces socket identity — no-op.
            }
            ServerMessage::Quit => self.exit = true,
            ServerMessage::Resize { .. } => {}
        }
    }

    fn handle_key(&mut self, key: event::KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        // --- Help modal: any key closes ---
        if self.modal == Modal::Help {
            self.modal = Modal::None;
            return;
        }

        // --- Confirm-kill modal ---
        if self.modal == Modal::ConfirmKill {
            if key.code == KeyCode::Char('y')
                && let Some(target) = self.kill_target.take()
            {
                let msg = format!("killed {target}");
                self.send(ClientCommand::KillSession { name: target }, Some(&msg));
            }
            self.kill_target = None;
            self.modal = Modal::None;
            return;
        }

        // Alt+Up/Down → reorder ±1; Alt+Shift+Up/Down → top/bottom.
        if key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down)
        {
            if let Some(name) = self.focused_session.clone() {
                use tt_agentboard::session_order::ReorderDelta;
                let up = key.code == KeyCode::Up;
                let delta = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    if up { ReorderDelta::Top } else { ReorderDelta::Bottom }
                } else if up {
                    ReorderDelta::Up
                } else {
                    ReorderDelta::Down
                };
                self.send(ClientCommand::ReorderSession { name, delta }, None);
            }
            return;
        }

        match key.code {
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('q') => {
                self.send(ClientCommand::Quit, None);
                self.exit = true;
            }
            KeyCode::Esc => {
                if self.panel_focus == PanelFocus::Agents {
                    self.panel_focus = PanelFocus::Sessions;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.panel_focus == PanelFocus::Agents {
                    self.move_agent_focus(-1);
                } else {
                    self.move_local_focus(-1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.panel_focus == PanelFocus::Agents {
                    self.move_agent_focus(1);
                } else {
                    self.move_local_focus(1);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.panel_focus == PanelFocus::Agents {
                    self.panel_focus = PanelFocus::Sessions;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let agents = self.focused_data().map(|d| d.agents.len()).unwrap_or(0);
                if self.panel_focus == PanelFocus::Sessions && agents > 0 {
                    self.panel_focus = PanelFocus::Agents;
                    self.focused_agent_idx = self.focused_agent_idx.min(agents - 1);
                }
            }
            KeyCode::Enter => {
                if self.panel_focus == PanelFocus::Agents {
                    self.activate_focused_agent();
                } else if let Some(name) = self.focused_session.clone() {
                    self.switch_to_session(name);
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if self.sessions.is_empty() {
                    return;
                }
                let len = self.sessions.len();
                let cur_idx = self
                    .current_session()
                    .and_then(|cur| self.sessions.iter().position(|s| s.name == cur))
                    .unwrap_or(0);
                let step = if key.code == KeyCode::BackTab { len - 1 } else { 1 };
                let next = self.sessions[(cur_idx + step) % len].name.clone();
                self.switch_to_session(next);
            }
            KeyCode::Char('r') => self.send(ClientCommand::Refresh, Some("refreshing sessions")),
            KeyCode::Char('d') => {
                if self.panel_focus == PanelFocus::Agents {
                    self.dismiss_focused_agent();
                }
            }
            KeyCode::Char('x') => {
                if self.panel_focus == PanelFocus::Agents {
                    self.kill_focused_agent_pane();
                } else if let Some(name) = self.focused_session.clone() {
                    self.kill_target = Some(name);
                    self.modal = Modal::ConfirmKill;
                }
            }
            KeyCode::Char('e') => self.open_in_editor(),
            KeyCode::Char('n') => self.create_new_session(),
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if let Some(target) = self.sessions.get(idx) {
                    let name = target.name.clone();
                    self.switch_to_session(name);
                }
            }
            _ => {}
        }
    }
}

impl App {
    fn handle_mouse(&mut self, mouse: event::MouseEvent) {
        // Any click closes the help overlay; confirm-kill stays keyboard-only.
        if self.modal == Modal::Help {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.modal = Modal::None;
            }
            return;
        }
        if self.modal != Modal::None {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_local_focus(-1),
            MouseEventKind::ScrollDown => self.move_local_focus(1),
            MouseEventKind::Down(MouseButton::Left) => {
                let area = self.list_area;
                if mouse.row < area.y
                    || mouse.row >= area.y + area.height
                    || mouse.column < area.x
                    || mouse.column >= area.x + area.width
                {
                    return;
                }
                let idx = (mouse.row - area.y) as usize + self.scroll as usize;
                match self.hit_map.get(idx).cloned() {
                    Some(LineTarget::Session(name)) => {
                        // Ports SessionCard onSelect: select + switch.
                        self.focused_session = Some(name.clone());
                        self.switch_to_session(name);
                    }
                    Some(LineTarget::Agent { session, idx }) => {
                        // Ports AgentRow onFocusPane: move selection with the
                        // click and focus the agent's pane.
                        self.focused_session = Some(session.clone());
                        self.panel_focus = PanelFocus::Agents;
                        self.focused_agent_idx = idx;
                        self.activate_focused_agent();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Refocus the main (non-sidebar) pane. Ports `refocusMainPane`: run from the
/// TUI process right after startup so the sidebar spawn doesn't steal focus.
fn refocus_main_pane() {
    let Ok(pane) = std::env::var("TMUX_PANE") else {
        return;
    };
    let tmux = TmuxClient::new();
    let window_id = std::env::var("REFOCUS_WINDOW")
        .ok()
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| tmux.display("#{window_id}", Some(&pane)));
    if window_id.is_empty() {
        return;
    }
    let main = tmux
        .list_panes(tt_agentboard::tmux::client::PaneScope::Window(&window_id))
        .into_iter()
        .find(|p| p.title != SIDEBAR_PANE_TITLE);
    if let Some(main) = main {
        tmux.select_pane(&main.id);
    }
}

fn local_session_name() -> Option<String> {
    let pane = std::env::var("TMUX_PANE").ok()?;
    let name = TmuxClient::new().display("#{session_name}", Some(&pane));
    if name.is_empty() { None } else { Some(name) }
}

fn wall_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Disables mouse capture on drop — including during a panic unwind.
/// ratatui's panic hook restores raw mode and the alternate screen, but mouse
/// capture is our own opt-in on top of `ratatui::init()`, so without this
/// guard a panic left the terminal spewing escape sequences on every click.
struct MouseCaptureGuard;

impl MouseCaptureGuard {
    fn enable() -> Self {
        let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
        Self
    }
}

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    }
}

pub fn run() -> i32 {
    if let Err(e) = ensure_server() {
        ui::error(&e);
        return 1;
    }

    let (tx, rx) = mpsc::channel::<ServerMessage>();
    let (conn_tx, conn_rx) = mpsc::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        // Signals both "failed to connect" and "connection dropped".
        let _ = conn_tx.send(sse_subscribe(tx));
    });

    let mut app = App::new(local_session_name());
    app.connected = true; // ensure_server succeeded; first failure downgrades it
    app.now_ms = wall_ms();

    let mut terminal = ratatui::init();
    let _mouse = MouseCaptureGuard::enable();
    refocus_main_pane();

    let mut last_spin = Instant::now();
    let result = loop {
        // Drain server messages.
        while let Ok(msg) = rx.try_recv() {
            app.apply_message(msg);
        }
        // SSE thread ended → server is gone; exit like the TS onclose,
        // surfacing the reason when the subscription itself failed.
        match conn_rx.try_recv() {
            Ok(Ok(())) => break Ok(()),
            Ok(Err(e)) => break Err(e),
            Err(_) => {}
        }
        if app.exit {
            break Ok(());
        }

        app.now_ms = wall_ms();
        let has_running = app
            .sessions
            .iter()
            .any(|s| s.agents.iter().any(|a| a.status == tt_agentboard::types::AgentStatus::Busy));
        if has_running && last_spin.elapsed() >= Duration::from_millis(120) {
            app.spin_idx = (app.spin_idx + 1) % SPINNERS.len();
            last_spin = Instant::now();
        }
        if let Some((_, _, at)) = &app.toast
            && at.elapsed() >= Duration::from_millis(TOAST_MS)
        {
            app.toast = None;
        }

        if let Err(e) = terminal.draw(|frame| view::draw(frame, &mut app)) {
            break Err(e.to_string());
        }

        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) => app.handle_key(key),
                Ok(Event::Mouse(mouse)) => app.handle_mouse(mouse),
                Ok(_) => {}
                Err(e) => break Err(e.to_string()),
            },
            Ok(false) => {}
            Err(e) => break Err(e.to_string()),
        }
    };

    drop(_mouse);
    ratatui::restore();
    match result {
        Ok(()) => 0,
        Err(e) => {
            ui::error(&format!("agentboard tui: {e}"));
            1
        }
    }
}
