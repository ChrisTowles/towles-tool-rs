//! Embedded terminals: shells in PTYs (portable-pty), terminal state in
//! tt-vt (libghostty-vt), rendered by the app's canvas terminal view. Many
//! terminals live at once, keyed by a frontend-supplied `term_id` (the
//! agentboard screen spawns one or more per session, each rooted in the
//! session's folder). PTY bytes feed a per-terminal tt-vt engine thread;
//! the frontend receives `terminal://frame` events (dirty-row style runs,
//! cursor, title, mode hints) tagged with `termId`; input/resize/scroll come
//! back as commands.
//!
//! Shells are owned directly by the app process — closing the app kills them,
//! nothing persists across a restart.
//!
//! Concurrency contract: the [`TermState`] map lock is only ever held for map
//! surgery — never across a PTY write, a subprocess, or a kill/wait. Input
//! goes through a per-terminal channel + writer thread so a shell that stops
//! reading (Ctrl+S, stopped job) can only back up its own terminal, and every
//! reader/exit path is generation-checked so a replaced PTY's exit event can
//! never close its successor. The tt-vt engine thread is owned by the PTY
//! reader thread (dropped — and joined — at EOF, after the map entry is
//! resolved); the map only holds a cloneable input sender for resize/scroll.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use sysinfo::{Pid as SysPid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter, Manager, State};
use tt_vt::{
    EngineOptions, Event as VtEvent, Frame, Input as VtInput, KeyAction, KeyEvent, MouseAction,
    MouseButton, MouseInput, SearchMatch, Select as VtSelect, Sender as VtSender,
};

pub const FRAME_EVENT: &str = "terminal://frame";
pub const EXIT_EVENT: &str = "terminal://exit";
pub const NOTIFY_EVENT: &str = "terminal://notify";
const MAIN_WINDOW_LABEL: &str = "main";

/// Scrollback kept per terminal, in rows. Lives in the Rust engine, not the
/// webview (xterm.js used to hold this in the JS heap).
const MAX_SCROLLBACK: usize = 10_000;

/// Cap on scrollback search results per query — enough for "n/N matches"
/// navigation without shipping a megabyte of positions for `query = "e"`.
const SEARCH_MATCH_LIMIT: usize = 1000;

/// Queued-keystroke cap per terminal. When the shell stops draining its PTY
/// (flow-stopped, stopped job) further input errors instead of blocking or
/// growing without bound.
const INPUT_QUEUE_CAP: usize = 1024;

/// Monotonic id for PTY instances. `term_start` on an existing `term_id`
/// replaces the session; the generation lets the OLD reader thread recognize
/// it has been superseded and swallow its exit event instead of killing the
/// replacement (a webview reload restarts every terminal this way).
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// One live PTY session (one shell shown in one terminal view).
struct Session {
    master: Box<dyn MasterPty + Send>,
    /// Input queue consumed by this session's writer thread.
    input: SyncSender<Vec<u8>>,
    /// Feed for this terminal's tt-vt engine thread (resize/scroll from
    /// commands; the PTY reader holds its own clone for output bytes). Control
    /// inputs sent here never block behind queued output.
    vt: VtSender,
    child: Box<dyn Child + Send + Sync>,
    generation: u64,
    /// The shell's display name, resolved once at spawn time — e.g. "zsh",
    /// "bash". Best-effort: a user running a different shell inside this one
    /// (e.g. `bash` inside `zsh`) won't change it.
    shell_kind: String,
    /// This terminal's Claude Code IDE server (see `ide.rs`): dropping the
    /// session (kill/replace/teardown) shuts it down and removes its lockfile.
    /// `None` when the server failed to start — the shell still works, it just
    /// gets no IDE pairing.
    ide: Option<crate::ide::IdeServer>,
    /// The resolved working directory this shell was rooted in (`start_dir`'s
    /// result), if any. `None` for a shell that fell back to portable-pty
    /// inheriting the app's own cwd (no `cwd` requested and no home dir
    /// resolvable) — port-drift has nothing to check against then.
    dir: Option<std::path::PathBuf>,
    /// Ports `dir`'s `.env` claimed at spawn time (see
    /// `tt_agentboard::env_drift`) — the baseline a later drift check diffs
    /// against the file's current claims. Empty when `dir` is `None` or the
    /// file didn't exist yet at spawn.
    env_ports_at_spawn: BTreeMap<String, u16>,
}

/// All live terminals, keyed by the frontend's `term_id`, plus which one
/// currently holds keyboard focus.
#[derive(Default)]
pub struct TermState {
    sessions: Mutex<HashMap<String, Session>>,
    /// `term_id` of the focused terminal, if any. Set by the frontend via
    /// [`term_focus`]. Gates OSC 52 clipboard writes so a background pane can't
    /// hijack the system clipboard.
    focused: Mutex<Option<String>>,
}

impl TermState {
    /// Whether `term_id` is the currently focused terminal.
    fn is_focused(&self, term_id: &str) -> bool {
        self.focused.lock().unwrap().as_deref() == Some(term_id)
    }

    /// Record focus gained/lost for `term_id`. A blur only clears focus when
    /// this terminal still owns it, so a focus handoff (blur A, focus B)
    /// delivered out of order can't wipe B's focus.
    fn set_focus(&self, term_id: String, focused: bool) {
        let mut current = self.focused.lock().unwrap();
        if focused {
            *current = Some(term_id);
        } else if current.as_deref() == Some(term_id.as_str()) {
            *current = None;
        }
    }

    /// Ids of every session with a live PTY right now. The agentboard bridge
    /// stamps these onto the emitted snapshot as `SessionData.live`.
    pub fn live_ids(&self) -> std::collections::HashSet<String> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }

    /// Each live session's shell kind. The agentboard bridge stamps these onto
    /// the emitted snapshot as `SessionData.shellKind`.
    pub fn shell_kinds(&self) -> HashMap<String, String> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| (id.clone(), s.shell_kind.clone()))
            .collect()
    }

    /// Each live session's port-claim drift: what its folder's `.env` claimed
    /// at spawn time vs what it claims right now. The agentboard bridge
    /// stamps these onto `SessionData.portDrift` before every emit (same
    /// pattern as `live`/`shell_kinds`). Re-reads each session's `.env`
    /// fresh — a small file, and only run on the poll/emit path, never per
    /// keystroke. Sessions with nothing to compare (`dir` unresolved) or no
    /// drift are simply absent from the map.
    pub fn port_drift(&self) -> HashMap<String, Vec<tt_agentboard::env_drift::PortDrift>> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, s)| {
                let dir = s.dir.as_deref()?;
                let current = tt_agentboard::env_drift::read_current_ports(dir);
                let drift = tt_agentboard::env_drift::diff(&s.env_ports_at_spawn, &current);
                (!drift.is_empty()).then(|| (id.clone(), drift))
            })
            .collect()
    }

    /// Kill, reap, and drop the session with `term_id`, if any. The kill/wait
    /// runs after the map lock is released. `pub(crate)` so slot removal
    /// (`slots.rs`) can tear down a folder's live PTYs before its worktree is
    /// deleted. Also sweeps every other process sharing the shell's POSIX
    /// session (see [`kill_session_stragglers`]) — SIGHUP to the shell alone
    /// only reaches jobs the shell still tracks, so this is what actually
    /// catches a backgrounded subshell (`(cmd &)`) or anything else disowned
    /// from the job table.
    pub(crate) fn kill(&self, term_id: &str) {
        let session = self.sessions.lock().unwrap().remove(term_id);
        if let Some(mut session) = session {
            let shell_pid = session.child.process_id();
            let _ = session.child.kill();
            if let Some(pid) = shell_pid {
                kill_session_stragglers(pid);
            }
            let _ = session.child.wait();
        }
    }

    /// Kill, reap, and drop every session (window teardown).
    fn kill_all(&self) {
        let sessions: Vec<Session> =
            self.sessions.lock().unwrap().drain().map(|(_, s)| s).collect();
        for mut session in sessions {
            let shell_pid = session.child.process_id();
            let _ = session.child.kill();
            if let Some(pid) = shell_pid {
                kill_session_stragglers(pid);
            }
            let _ = session.child.wait();
        }
    }

    /// Run `f` over every live session's IDE server rooted at `dir` (the
    /// diff pane routes highlights by folder). The callback only does cheap
    /// in-memory work (cache write + channel send), so holding the map lock
    /// across it stays within the lock contract above.
    pub(crate) fn for_ide_servers(&self, dir: &Path, mut f: impl FnMut(&crate::ide::IdeServer)) {
        let guard = self.sessions.lock().unwrap();
        for session in guard.values() {
            if let Some(ide) = &session.ide
                && same_dir(ide.cwd(), dir)
            {
                f(ide);
            }
        }
    }

    /// Every live session's IDE pairing state, for the frontend's initial
    /// snapshot (`ide_status` command).
    pub(crate) fn ide_statuses(&self) -> Vec<crate::ide::IdeStatus> {
        let guard = self.sessions.lock().unwrap();
        guard.values().filter_map(|s| s.ide.as_ref().map(|ide| ide.status())).collect()
    }

    /// Remove `term_id` only if it still holds `generation`, returning the
    /// session for reaping. A newer generation means this id was replaced —
    /// leave the replacement alone.
    fn take_if_current(&self, term_id: &str, generation: u64) -> Option<Session> {
        let mut guard = self.sessions.lock().unwrap();
        if guard.get(term_id).is_some_and(|s| s.generation == generation) {
            return guard.remove(term_id);
        }
        None
    }
}

/// SIGKILL every live process that shares `shell_pid`'s POSIX session,
/// except the shell itself (the caller already handles that one). A
/// backgrounded subshell (`(cmd &)`) never calls `setsid`, so it keeps the
/// shell's original session id for its whole life even after its immediate
/// parent (the subshell) exits and it gets reparented to init — invisible to
/// any parent-child process-tree walk, but still found here. This
/// deliberately does NOT reach a process that called `setsid` itself (a
/// genuinely daemonized `nohup`/`setsid` command): that's a real, deliberate
/// detach from the controlling terminal, the same boundary every terminal
/// emulator respects.
///
/// Unix-only: on Windows, sysinfo's `session_id` means the login/RDP
/// session, not a POSIX job/session group, so applying this logic there
/// would kill unrelated processes sharing the user's desktop session.
/// Windows has no equivalent fix here yet — it relies solely on
/// `ProcessSignaller`'s direct-child `TerminateProcess`.
#[cfg(unix)]
fn kill_session_stragglers(shell_pid: u32) {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let sid = SysPid::from_u32(shell_pid);
    for (pid, process) in sys.processes() {
        if *pid != sid && process.session_id() == Some(sid) {
            process.kill();
        }
    }
}

#[cfg(not(unix))]
fn kill_session_stragglers(_shell_pid: u32) {}

/// Render frame streamed to the frontend; `termId` routes it to the right
/// terminal view.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermFrame {
    term_id: String,
    frame: Frame,
}

/// Emitted once when a shell exits so the frontend can report how it died —
/// a clean logout (code 0) versus a crash or signal. `signal` is portable-pty's
/// resolved name ("Killed", "Terminated", …); the raw signal number isn't
/// exposed, and a signal death leaves `code` at portable-pty's placeholder (1),
/// so the frontend prefers `signal` when present.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermExit {
    term_id: String,
    code: i32,
    signal: Option<String>,
}

/// UI theme for a terminal engine (mirrors `tt_vt::Theme`): default colors
/// packed 0xRRGGBB, the ANSI 0–15 palette, and whether the theme is dark.
/// Sent at spawn (so color queries answer correctly from the first byte) and
/// again on every app theme change via [`term_theme`].
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermTheme {
    fg: u32,
    bg: u32,
    #[serde(default)]
    cursor: Option<u32>,
    palette16: [u32; 16],
    dark: bool,
}

impl From<TermTheme> for tt_vt::Theme {
    fn from(t: TermTheme) -> Self {
        tt_vt::Theme { fg: t.fg, bg: t.bg, cursor: t.cursor, palette16: t.palette16, dark: t.dark }
    }
}

/// Payload of a `terminal://notify` event: the program in a terminal raised
/// attention — a BEL, or a desktop notification (OSC 9/777, e.g. Claude Code
/// asking for input). The agentboard badges the session and shows the body.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermNotify {
    term_id: String,
    /// "bell" or "notify".
    kind: &'static str,
    /// Notification body; absent for a bell.
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

/// Spawn a shell in a fresh PTY sized to the xterm.js grid, rooted at `cwd`
/// (falls back to `$HOME` when `cwd` is missing or not an existing dir).
/// Replaces any existing terminal with the same `term_id`. Async: runs on a
/// blocking task so PTY setup never blocks the main thread.
#[tauri::command]
pub async fn term_start(
    app: AppHandle,
    term_id: String,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    theme: Option<TermTheme>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        term_start_blocking(app, term_id, cols, rows, cwd, theme)
    })
    .await
    .map_err(|e| format!("terminal spawn task failed: {e}"))?
}

fn term_start_blocking(
    app: AppHandle,
    term_id: String,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    theme: Option<TermTheme>,
) -> Result<(), String> {
    let state = app.state::<TermState>();
    state.kill(&term_id);

    let pty = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("failed to open pty: {e}"))?;

    let shell = default_shell(std::env::var(SHELL_ENV_VAR).ok());
    let shell_kind = shell_kind_from_path(&shell);
    let dir = start_dir(cwd);
    // Snapshot the folder's current port claims now — the baseline a later
    // drift check compares against `.env`'s live claims (see
    // `tt_agentboard::env_drift`). The shell itself never reads this; it's
    // purely bookkeeping for the agentboard bridge.
    let env_ports_at_spawn =
        dir.as_deref().map(tt_agentboard::env_drift::read_current_ports).unwrap_or_default();

    // Claude Code IDE pairing: a per-terminal WebSocket MCP server + lockfile
    // (see ide.rs / docs/CLAUDE-CODE-IDE.md). Best-effort — a bind failure
    // costs the pairing, never the shell.
    let diag_hub = app.state::<std::sync::Arc<crate::diagnostics::DiagHub>>().inner().clone();
    let ide = dir.as_ref().and_then(|d| {
        match crate::ide::IdeServer::start(app.clone(), term_id.clone(), d.clone(), diag_hub) {
            Ok(server) => Some(server),
            Err(error) => {
                eprintln!("warning: IDE server for terminal {term_id} unavailable: {error}");
                None
            }
        }
    });

    // Every terminal and agent session starts here. A PTY outlives this call
    // and has no exit code to wait for, so it can't use tt_exec's span shape —
    // but it's the app's most consequential spawn, so it still gets recorded.
    tt_exec::record_detached_spawn(&shell, &[], "pty");
    let mut cmd = CommandBuilder::new(shell);
    // Scrub the app instance's own env out of the shell's inherited environment
    // (dev-server port + session/instance stamps, Tauri build config, the npm
    // process that launched us) so a nested `npm run dev` / `tt-app` started
    // inside this terminal re-derives its own port and session identity instead
    // of colliding with the outer instance (issue #39). Everything else — PATH,
    // HOME, SHELL, … — survives. We then re-stamp TERM and a fresh session id
    // below.
    let inherited: Vec<(String, String)> =
        cmd.iter_full_env_as_str().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    cmd.env_clear();
    for (key, value) in tt_exec::scrub_app_instance_env(inherited) {
        cmd.env(key, value);
    }
    cmd.env("TERM", "xterm-256color");
    // Stamp the PTY with its session id so a Claude agent launched inside inherits
    // it; the agentboard engine reads it back from /proc to attribute the agent to
    // this session (see tt_agentboard::procenv). `term_id` == the session id.
    // The instance stamp disambiguates two running app instances hosting the
    // same shared session record (sessions.json is cross-instance): each window
    // only reports agents whose stamp matches its own.
    cmd.env(tt_agentboard::procenv::TT_SESSION_ENV, &term_id);
    cmd.env(tt_agentboard::procenv::TT_INSTANCE_ENV, tt_agentboard::procenv::instance_id());
    // Pair a `claude` started in this pane with this pane's IDE server. The
    // scrub above already dropped any *inherited* CLAUDE_CODE_SSE_PORT (that
    // one stamps nested-session identity, issue #39); this is our own. An env
    // port match short-circuits Claude Code's lockfile pid/cwd checks, so the
    // pairing is deterministic even with several slots' panes open at once.
    if let Some(ide) = &ide {
        cmd.env("CLAUDE_CODE_SSE_PORT", ide.port().to_string());
    }
    if let Some(dir) = &dir {
        cmd.cwd(dir);
    }
    let child = pty.slave.spawn_command(cmd).map_err(|e| format!("failed to spawn shell: {e}"))?;

    let mut reader =
        pty.master.try_clone_reader().map_err(|e| format!("failed to clone pty reader: {e}"))?;
    let mut writer =
        pty.master.take_writer().map_err(|e| format!("failed to take pty writer: {e}"))?;

    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let (input_tx, input_rx): (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) =
        sync_channel(INPUT_QUEUE_CAP);

    // Terminal state engine: consumes PTY bytes, produces render frames for
    // the frontend and reply bytes (DA1 etc.) for the shell.
    let vt = tt_vt::Session::spawn(EngineOptions { cols, rows, max_scrollback: MAX_SCROLLBACK }, {
        let app = app.clone();
        let term_id = term_id.clone();
        let pty_input = input_tx.clone();
        move |event| match event {
            VtEvent::Frame(frame) => {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    FRAME_EVENT,
                    TermFrame { term_id: term_id.clone(), frame },
                );
            }
            // Best-effort: a full input queue drops the reply; the
            // querying program times out like it would on a slow tty.
            VtEvent::PtyReply(bytes) => {
                let _ = pty_input.try_send(bytes);
            }
            // A program in this shell copied text via OSC 52. Write it to the
            // system clipboard, but ONLY when this terminal is the focused one:
            // a background pane (another agent's shell) must not be able to
            // silently overwrite the clipboard. Read-side OSC 52 is not handled.
            // Attention signals (BEL / OSC 9 notifications) go to the
            // agentboard, which badges the session and feeds needs-you.
            VtEvent::Bell => {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    NOTIFY_EVENT,
                    TermNotify { term_id: term_id.clone(), kind: "bell", body: None },
                );
            }
            VtEvent::Notify(body) => {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    NOTIFY_EVENT,
                    TermNotify { term_id: term_id.clone(), kind: "notify", body: Some(body) },
                );
            }
            VtEvent::Clipboard(text) => {
                use tauri_plugin_clipboard_manager::ClipboardExt;
                if app.state::<TermState>().is_focused(&term_id) {
                    let _ = app.clipboard().write_text(text);
                }
            }
        }
    })
    .map_err(|e| format!("failed to start terminal engine: {e}"))?;
    let vt_tx = vt.sender();
    // Push the UI theme before the reader thread pumps the shell's first
    // output, so an early OSC 10/11 probe (how Claude Code detects dark vs
    // light) already answers the app's real colors — control inputs drain
    // ahead of bytes in the engine.
    if let Some(theme) = theme {
        let _ = vt_tx.send(VtInput::Theme(theme.into()));
    }

    state.sessions.lock().unwrap().insert(
        term_id.clone(),
        Session {
            master: pty.master,
            input: input_tx,
            vt: vt_tx,
            child,
            generation,
            shell_kind,
            ide,
            dir,
            env_ports_at_spawn,
        },
    );

    // Liveness changed (a PTY appeared) — refresh the agentboard snapshot.
    notify_agentboard(&app);

    // Writer thread: drain the input queue into the PTY in arrival order. A
    // shell that stops reading blocks only this thread; the channel cap bounds
    // the backlog. Ends when the session is dropped (sender closes) or the
    // PTY write fails.
    std::thread::spawn(move || {
        while let Ok(bytes) = input_rx.recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
        }
    });

    // Reader thread: pump PTY output into the terminal engine until EOF
    // (shell exited). Owns the engine handle: dropping `vt` after the map
    // entry is resolved joins the engine thread exactly once, whether the
    // shell exited or this PTY was replaced. Feeding bytes blocks when the
    // engine is behind (bounded byte queue); the read then stops, the kernel
    // PTY buffer fills, and the shell is flow-controlled — output can't balloon
    // engine memory.
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if !vt.send(VtInput::Bytes(buf[..n].to_vec())) {
                        break;
                    }
                }
            }
        }
        // EOF can mean (a) the shell exited, or (b) this PTY was replaced /
        // explicitly killed. Only (a) — where this generation still owns the
        // id — may emit the exit event; a stale exit after a replacement
        // would make the frontend close the NEW session.
        let state = app.state::<TermState>();
        if let Some(mut session) = state.take_if_current(&term_id, generation) {
            let status = session.child.wait().ok();
            let code = status.as_ref().map(|s| s.exit_code() as i32).unwrap_or(0);
            let signal = status.as_ref().and_then(|s| s.signal().map(str::to_string));
            let _ = app.emit_to(MAIN_WINDOW_LABEL, EXIT_EVENT, TermExit { term_id, code, signal });
            notify_agentboard(&app); // shell exited — session no longer live
        }
        drop(vt);
    });

    Ok(())
}

/// Nudge the agentboard's debounced emitter so `SessionData.live` flips promptly
/// when a PTY starts or exits (instead of waiting for the next 2s scan tick).
fn notify_agentboard(app: &AppHandle) {
    if let Some(ab) = app.try_state::<crate::agentboard::Ab>() {
        ab.emit.notify_one();
    }
}

/// Forward raw text to the shell (IME-composed input, the image-paste
/// signal byte). Keystrokes ride [`term_key`] instead, where the engine
/// encodes them against live terminal state — this command is the plain-text
/// escape hatch, not an escape-sequence path. Queues onto the session's
/// writer thread — never blocks, even against a shell that has stopped
/// reading its PTY.
#[tauri::command]
pub fn term_write(state: State<TermState>, term_id: String, data: String) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    match session.input.try_send(data.into_bytes()) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err("terminal input backed up (shell not reading)".into()),
        Err(TrySendError::Disconnected(_)) => Err("no shell running".into()),
    }
}

/// Keep the PTY and the terminal engine in sync with the rendered grid.
/// `cell_width`/`cell_height` are the renderer's cell size in px (used for
/// pixel size reports; 0 when unknown).
#[tauri::command]
pub fn term_resize(
    state: State<TermState>,
    term_id: String,
    cols: u16,
    rows: u16,
    cell_width: Option<u16>,
    cell_height: Option<u16>,
) -> Result<(), String> {
    let (cw, ch) = (cell_width.unwrap_or(0), cell_height.unwrap_or(0));
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    session
        .master
        .resize(PtySize { rows, cols, pixel_width: cols * cw, pixel_height: rows * ch })
        .map_err(|e| e.to_string())?;
    let _ = session.vt.send(VtInput::Resize {
        cols,
        rows,
        cell_width_px: u32::from(cw),
        cell_height_px: u32::from(ch),
    });
    Ok(())
}

/// Scroll the terminal viewport into scrollback (`delta` rows, up is
/// negative); `None` jumps back to the live bottom.
#[tauri::command]
pub fn term_scroll(
    state: State<TermState>,
    term_id: String,
    delta: Option<isize>,
) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Scroll(delta));
    Ok(())
}

/// A mouse-wheel gesture at viewport cell (`x`, `y`), `lines` rows (up is
/// negative). The view always forwards the wheel; the engine owns the whole
/// policy — scrollback paging when scrolled back, wheel reports when the
/// program tracks the mouse, alternate-scroll arrow keys on the alt screen
/// (mode 1007), viewport scrolling otherwise.
#[tauri::command]
pub fn term_wheel(
    state: State<TermState>,
    term_id: String,
    x: u16,
    y: u16,
    lines: i32,
) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Wheel { x, y, lines });
    Ok(())
}

/// Ask the engine to emit one full frame regardless of dirty state. The view
/// calls this when a pane transitions from hidden (`display:none`) back to
/// visible: dirty-only frames never resend rows the engine considers clean,
/// so a stale canvas would otherwise stay stale until a scroll (#47).
/// A pointer event from the terminal view (mirrors `tt_vt::MouseInput`),
/// forwarded to the program when it enabled mouse tracking. The view only
/// sends these while the frame's mode hints say the mouse is tracked, but
/// the engine re-checks, so a stale hint can't inject input.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermMouse {
    action: TermMouseAction,
    #[serde(default)]
    button: Option<TermMouseButton>,
    x: u16,
    y: u16,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    any_button: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TermMouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TermMouseButton {
    Left,
    Middle,
    Right,
}

impl From<TermMouse> for MouseInput {
    fn from(m: TermMouse) -> Self {
        MouseInput {
            action: match m.action {
                TermMouseAction::Press => MouseAction::Press,
                TermMouseAction::Release => MouseAction::Release,
                TermMouseAction::Motion => MouseAction::Motion,
            },
            button: m.button.map(|b| match b {
                TermMouseButton::Left => MouseButton::Left,
                TermMouseButton::Middle => MouseButton::Middle,
                TermMouseButton::Right => MouseButton::Right,
            }),
            x: m.x,
            y: m.y,
            shift: m.shift,
            alt: m.alt,
            ctrl: m.ctrl,
            any_button: m.any_button,
        }
    }
}

/// Forward a pointer event to the program in its negotiated mouse protocol.
#[tauri::command]
pub fn term_mouse(
    state: State<TermState>,
    term_id: String,
    event: TermMouse,
) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Mouse(event.into()));
    Ok(())
}

#[tauri::command]
pub fn term_request_full(state: State<TermState>, term_id: String) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::RequestFull);
    Ok(())
}

/// Report whether the pane is on-screen. Frontend panes never unmount (a
/// backgrounded tab sits behind another one at `display:none`), so without
/// this a session streaming output keeps rendering at the interactive frame
/// cap for a canvas nothing is painting. `term_request_full` — already
/// called when a pane comes back — catches the canvas up in full once
/// visible again.
#[tauri::command]
pub fn term_visibility(
    state: State<TermState>,
    term_id: String,
    visible: bool,
) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Visibility(visible));
    Ok(())
}

/// Drop the terminal's scrollback history, keeping the visible screen
/// (right-click "Clear scrollback"). The engine forces a full frame so the
/// view learns the scrollback depth collapsed.
#[tauri::command]
pub fn term_clear(state: State<TermState>, term_id: String) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::ClearScrollback);
    Ok(())
}

/// Apply a selection gesture from the terminal view, in viewport cell
/// coordinates. `kind`: drag (anchor→head range), word (double-click),
/// line (triple-click), all, clear.
#[tauri::command]
pub fn term_select(
    state: State<TermState>,
    term_id: String,
    kind: String,
    ax: Option<u16>,
    ay: Option<u16>,
    bx: Option<u16>,
    by: Option<u16>,
) -> Result<(), String> {
    let op = match kind.as_str() {
        "drag" => VtSelect::Range {
            ax: ax.unwrap_or(0),
            ay: ay.unwrap_or(0),
            bx: bx.unwrap_or(0),
            by: by.unwrap_or(0),
        },
        "word" => VtSelect::Word { x: ax.unwrap_or(0), y: ay.unwrap_or(0) },
        "line" => VtSelect::Line { x: ax.unwrap_or(0), y: ay.unwrap_or(0) },
        "all" => VtSelect::All,
        "clear" => VtSelect::Clear,
        other => return Err(format!("unknown selection kind: {other}")),
    };
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Select(op));
    Ok(())
}

/// Copy the terminal's active selection to the system clipboard, entirely in
/// Rust. The webview's `navigator.clipboard` is unreliable in WebKitGTK (and
/// undefined on insecure dev origins), which silently broke copy-on-select —
/// so the clipboard write happens here, through the same plugin the OSC 52
/// path uses. User-initiated (chord, context menu, copy-on-select), so unlike
/// OSC 52 it is not focus-gated. Returns the copied text (empty when there
/// was no selection); the engine thread answers over a bounded channel, and a
/// dead engine yields an error rather than a hang.
#[tauri::command]
pub async fn term_copy(app: AppHandle, term_id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (reply_tx, reply_rx) = sync_channel::<Option<String>>(1);
        {
            let state = app.state::<TermState>();
            let guard = state.sessions.lock().unwrap();
            let session = guard.get(&term_id).ok_or("no shell running")?;
            if !session.vt.send(VtInput::Copy(reply_tx)) {
                return Err("terminal engine gone".to_string());
            }
        }
        let text = reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map(|text| text.unwrap_or_default())
            .map_err(|_| "terminal engine did not answer".to_string())?;
        if !text.is_empty() {
            use tauri_plugin_clipboard_manager::ClipboardExt;
            app.clipboard()
                .write_text(text.clone())
                .map_err(|e| format!("clipboard write failed: {e}"))?;
        }
        Ok(text)
    })
    .await
    .map_err(|e| format!("copy task failed: {e}"))?
}

/// Case-insensitive substring search over the terminal's full scrollback +
/// active area. Returns match positions (absolute row, column, width) top to
/// bottom, capped at [`SEARCH_MATCH_LIMIT`]. The engine thread answers over
/// a bounded channel; a dead engine yields an error rather than a hang.
#[tauri::command]
pub async fn term_search(
    app: AppHandle,
    term_id: String,
    query: String,
) -> Result<Vec<SearchMatch>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (reply_tx, reply_rx) = sync_channel::<Vec<SearchMatch>>(1);
        {
            let state = app.state::<TermState>();
            let guard = state.sessions.lock().unwrap();
            let session = guard.get(&term_id).ok_or("no shell running")?;
            if !session.vt.send(VtInput::Search {
                query,
                limit: SEARCH_MATCH_LIMIT,
                reply: reply_tx,
            }) {
                return Err("terminal engine gone".to_string());
            }
        }
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "terminal engine did not answer".to_string())
    })
    .await
    .map_err(|e| format!("search task failed: {e}"))?
}

/// A keystroke from the terminal view, in DOM `KeyboardEvent` terms
/// (mirrors `tt_vt::KeyEvent`). The engine encodes it against live terminal
/// state — kitty keyboard protocol, DECCKM, keypad mode — so the frontend
/// never builds escape sequences.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermKey {
    code: String,
    key: String,
    action: TermKeyAction,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    meta: bool,
    #[serde(default)]
    caps_lock: bool,
    #[serde(default)]
    num_lock: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TermKeyAction {
    Press,
    Repeat,
    Release,
}

impl From<TermKey> for KeyEvent {
    fn from(k: TermKey) -> Self {
        KeyEvent {
            code: k.code,
            key: k.key,
            action: match k.action {
                TermKeyAction::Press => KeyAction::Press,
                TermKeyAction::Repeat => KeyAction::Repeat,
                TermKeyAction::Release => KeyAction::Release,
            },
            shift: k.shift,
            alt: k.alt,
            ctrl: k.ctrl,
            meta: k.meta,
            caps_lock: k.caps_lock,
            num_lock: k.num_lock,
        }
    }
}

/// Encode a keystroke in the terminal engine and write the bytes to the
/// shell. Control-channel send: never blocked behind queued output.
#[tauri::command]
pub fn term_key(state: State<TermState>, term_id: String, event: TermKey) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Key(event.into()));
    Ok(())
}

/// Reply of [`term_paste`]: whether the engine wants user confirmation before
/// writing anything (bracketed paste off + a newline in the text — the paste
/// would execute in the shell immediately). The caller shows a confirm and
/// retries with `force: true`.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteReply {
    needs_confirm: bool,
}

/// Paste text into the shell through the engine's paste encoder, which strips
/// dangerous control bytes (an embedded `ESC[201~` can no longer escape the
/// paste bracket and inject commands) and honors the negotiated bracketed
/// paste mode — the frontend no longer encodes paste itself. The engine
/// thread answers over a bounded channel; a dead engine yields an error
/// rather than a hang.
#[tauri::command]
pub async fn term_paste(
    app: AppHandle,
    term_id: String,
    text: String,
    force: Option<bool>,
) -> Result<PasteReply, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (reply_tx, reply_rx) = sync_channel::<tt_vt::PasteOutcome>(1);
        {
            let state = app.state::<TermState>();
            let guard = state.sessions.lock().unwrap();
            let session = guard.get(&term_id).ok_or("no shell running")?;
            if !session.vt.send(VtInput::Paste {
                text,
                force: force.unwrap_or(false),
                reply: reply_tx,
            }) {
                return Err("terminal engine gone".to_string());
            }
        }
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map(|outcome| PasteReply {
                needs_confirm: outcome == tt_vt::PasteOutcome::NeedsConfirm,
            })
            .map_err(|_| "terminal engine did not answer".to_string())
    })
    .await
    .map_err(|e| format!("paste task failed: {e}"))?
}

/// Scroll the viewport so the given absolute row (0 = oldest scrollback row)
/// is visible — search prev/next navigation jumps the viewport to a match.
#[tauri::command]
pub fn term_scroll_to(state: State<TermState>, term_id: String, row: usize) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::ScrollTo(row));
    Ok(())
}

/// Push the UI theme (default colors, ANSI palette, dark/light) into a
/// running terminal's engine — called on app theme changes so OSC 10/11 and
/// color-scheme queries keep answering the truth. The engine forces a full
/// frame, so the canvas repaints in the new colors without a separate nudge.
#[tauri::command]
pub fn term_theme(
    state: State<TermState>,
    term_id: String,
    theme: TermTheme,
) -> Result<(), String> {
    let guard = state.sessions.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Theme(theme.into()));
    Ok(())
}

/// Record which terminal holds keyboard focus. The terminal view calls this
/// with `focused: true` when its hidden input gains focus and `false` when it
/// loses it. Focus gates OSC 52 clipboard writes: only the focused terminal may
/// set the system clipboard, so a background pane can't hijack it. The blur is
/// no-op'd unless this terminal is still the focused one, so a focus handoff
/// (blur A then focus B) can't clear B's focus if the events arrive reordered.
#[tauri::command]
pub fn term_focus(state: State<TermState>, term_id: String, focused: bool) {
    // Also tell the engine: a program that asked for focus events (mode
    // 1004) gets CSI I / CSI O; the engine is silent otherwise.
    if let Some(session) = state.sessions.lock().unwrap().get(&term_id) {
        let _ = session.vt.send(VtInput::Focus(focused));
    }
    state.set_focus(term_id, focused);
}

/// Kill one shell (the frontend calls this when a terminal unmounts — an
/// explicit close).
#[tauri::command]
pub fn term_kill(app: AppHandle, term_id: String) {
    app.state::<TermState>().kill(&term_id);
    notify_agentboard(&app);
}

/// Drop every PTY when the main window goes away (wired to the window
/// Destroyed event in lib.rs) — shells don't survive the app closing.
pub fn on_window_destroyed(app: &AppHandle, label: &str) {
    if label == MAIN_WINDOW_LABEL {
        app.state::<TermState>().kill_all();
    }
}

/// Open a file path Ctrl/⌘-clicked in a terminal in the preferred editor,
/// seeking to `line` (1-based, from the link's `:line` suffix) if one was
/// given and the editor's command is one we know a goto syntax for. Relative
/// paths resolve against `cwd` (the clicked pane's working dir) and a leading
/// `~` expands to home. Spawns without waiting — like `journal_open` /
/// `ab_open_in_editor`, so a non-forking editor (vim, `code --wait`) doesn't
/// freeze the app. Report-only: it opens an editor, never writing to the PTY.
#[tauri::command]
pub fn term_open_path(path: String, cwd: Option<String>, line: Option<u32>) -> Result<(), String> {
    let settings = tt_config::load().map_err(|e| format!("failed to load settings: {e}"))?;
    let editor = settings.preferred_editor.trim();
    if editor.is_empty() {
        return Err("No preferred editor configured".into());
    }
    let full = resolve_clicked_path(&path, cwd.as_deref());
    if !full.exists() {
        return Err(format!("No such file: {}", full.display()));
    }
    let editor_args = editor_open_args(editor, &full, line);
    let arg_strs: Vec<String> =
        editor_args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    tt_exec::record_detached_spawn(
        editor,
        &arg_strs.iter().map(String::as_str).collect::<Vec<_>>(),
        "editor",
    );
    std::process::Command::new(editor)
        .args(&editor_args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to launch {editor}: {e}"))
}

/// How a recognized editor's command wants "open this file at this line" on
/// its command line — there's no universal convention.
enum GotoStyle {
    /// `code --goto path:line`
    VsCodeGoto,
    /// `subl path:line` / `zed path:line`
    PathColonLine,
    /// `vim +line path`
    PlusLineFlag,
    /// `idea --line line path`
    LineFlag,
}

/// The goto syntax for `editor`'s command, matched by basename (case- and
/// extension-insensitive, so `Code.exe`/`/usr/bin/code` both match `code`).
/// `None` for anything we don't recognize.
fn goto_style(editor: &str) -> Option<GotoStyle> {
    let name = Path::new(editor)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(editor)
        .to_ascii_lowercase();
    match name.as_str() {
        "code" | "code-insiders" | "cursor" | "codium" | "vscodium" | "windsurf" => {
            Some(GotoStyle::VsCodeGoto)
        }
        "subl" | "sublime_text" | "zed" | "zeditor" | "hx" => Some(GotoStyle::PathColonLine),
        "vim" | "nvim" | "gvim" | "mvim" | "vi" | "nano" | "emacs" | "emacsclient" => {
            Some(GotoStyle::PlusLineFlag)
        }
        "idea" | "webstorm" | "pycharm" | "goland" | "rider" | "clion" | "rubymine"
        | "phpstorm" | "studio" => Some(GotoStyle::LineFlag),
        _ => None,
    }
}

/// Build the args to open `path` in `editor`. Without a `line`, or for an
/// editor command [`goto_style`] doesn't recognize, this is just the bare
/// path — guessing a goto syntax wrong would hand an unrecognized editor a
/// broken argument instead of the file it asked for.
fn editor_open_args(editor: &str, path: &Path, line: Option<u32>) -> Vec<OsString> {
    let Some(line) = line else {
        return vec![path.as_os_str().to_owned()];
    };
    match goto_style(editor) {
        Some(GotoStyle::VsCodeGoto) => {
            vec!["--goto".into(), format!("{}:{line}", path.display()).into()]
        }
        Some(GotoStyle::PathColonLine) => vec![format!("{}:{line}", path.display()).into()],
        Some(GotoStyle::PlusLineFlag) => {
            vec![format!("+{line}").into(), path.as_os_str().to_owned()]
        }
        Some(GotoStyle::LineFlag) => {
            vec![
                "--line".into(),
                line.to_string().into(),
                path.as_os_str().to_owned(),
            ]
        }
        None => vec![path.as_os_str().to_owned()],
    }
}

/// Resolve a clicked path against the pane's `cwd`: absolute paths as-is, a
/// leading `~/` to the home dir, everything else joined onto `cwd`.
fn resolve_clicked_path(path: &str, cwd: Option<&str>) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    match cwd.filter(|c| !c.trim().is_empty()) {
        Some(dir) => Path::new(dir).join(p),
        None => p.to_path_buf(),
    }
}

/// Whether two folder paths name the same directory. Canonicalizes both when
/// possible so symlinked checkouts and trailing-slash variants still match the
/// diff pane's routing key.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Resolve the shell's working directory: the requested `cwd` if it exists,
/// otherwise the user's home. `None` lets portable-pty inherit the app's cwd.
fn start_dir(cwd: Option<String>) -> Option<std::path::PathBuf> {
    if let Some(dir) = cwd.filter(|d| !d.trim().is_empty())
        && Path::new(&dir).is_dir()
    {
        return Some(dir.into());
    }
    dirs::home_dir()
}

/// Env var that names the user's preferred shell: `$SHELL` on Unix,
/// `%COMSPEC%` on Windows (there's no `$SHELL` equivalent there).
#[cfg(windows)]
const SHELL_ENV_VAR: &str = "COMSPEC";
#[cfg(not(windows))]
const SHELL_ENV_VAR: &str = "SHELL";

fn default_shell(shell_env: Option<String>) -> String {
    shell_env.filter(|s| !s.trim().is_empty()).unwrap_or_else(fallback_shell)
}

/// `powershell.exe` on Windows (resolved via PATH; ships on every supported
/// Windows version), `/bin/bash` elsewhere.
#[cfg(windows)]
fn fallback_shell() -> String {
    "powershell.exe".to_string()
}
#[cfg(not(windows))]
fn fallback_shell() -> String {
    "/bin/bash".to_string()
}

/// The shell's display name from its resolved program path — `/usr/bin/zsh`
/// -> "zsh", `powershell.exe` -> "powershell".
fn shell_kind_from_path(shell: &str) -> String {
    let base = Path::new(shell).file_name().and_then(|s| s.to_str()).unwrap_or(shell);
    base.strip_suffix(".exe").unwrap_or(base).to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        TermState, default_shell, editor_open_args, resolve_clicked_path, shell_kind_from_path,
        start_dir,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// Whether `pid` is still alive (`kill(pid, 0)` — no signal sent, just an
    /// existence/permission probe).
    #[cfg(unix)]
    fn pid_alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// The gap this closes: a shell's own `SIGHUP` only reaches jobs the
    /// shell still tracks in its job table. `(sleep 30 &)` backgrounds a
    /// subshell that exits immediately, reparenting `sleep` to init — it's
    /// invisible to a parent-child walk from the shell, but it never calls
    /// `setsid`, so it keeps the shell's session id for its whole life.
    /// `kill_session_stragglers` must still find and kill it, while leaving
    /// the "shell" (the session leader itself) alone — the caller kills that
    /// one separately.
    #[cfg(unix)]
    #[test]
    fn kill_session_stragglers_reaps_detached_background_jobs() {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let pid_file =
            std::env::temp_dir().join(format!("tt-term-test-{}-{}.pid", std::process::id(), 0));
        let script = format!("(sleep 30 & echo $! > {}); sleep 30", pid_file.to_string_lossy());

        // Stand in for the shell portable-pty spawns: made a session leader
        // via setsid in pre_exec, exactly like unix.rs does for every PTY
        // child.
        let mut leader = unsafe {
            Command::new("sh")
                .arg("-c")
                .arg(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
                .spawn()
                .expect("spawn session leader")
        };
        let leader_pid = leader.id() as i32;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut detached_pid = None;
        while std::time::Instant::now() < deadline {
            if let Ok(mut f) = std::fs::File::open(&pid_file) {
                let mut s = String::new();
                let _ = f.read_to_string(&mut s);
                if let Ok(pid) = s.trim().parse::<i32>() {
                    detached_pid = Some(pid);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_file(&pid_file);
        let detached_pid = detached_pid.expect("detached process wrote its pid in time");
        assert!(pid_alive(detached_pid), "detached process should have started");

        super::kill_session_stragglers(leader_pid as u32);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline && pid_alive(detached_pid) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!pid_alive(detached_pid), "detached process should have been killed");
        assert!(pid_alive(leader_pid), "the session leader itself must survive the sweep");

        let _ = leader.kill();
        let _ = leader.wait();
    }

    #[test]
    fn focus_gate_tracks_the_focused_terminal() {
        let state = TermState::default();
        assert!(!state.is_focused("a"), "nothing focused initially");

        state.set_focus("a".into(), true);
        assert!(state.is_focused("a"));
        assert!(!state.is_focused("b"));

        // Focus handoff a -> b: b becomes focused, a is not.
        state.set_focus("b".into(), true);
        assert!(state.is_focused("b"));
        assert!(!state.is_focused("a"));

        // A late/reordered blur from the previously-focused a must NOT clear
        // b's focus — only the current owner's blur clears it.
        state.set_focus("a".into(), false);
        assert!(state.is_focused("b"), "stale blur from a leaves b focused");

        // b's own blur clears focus.
        state.set_focus("b".into(), false);
        assert!(!state.is_focused("b"));
    }

    #[test]
    fn prefers_shell_env() {
        assert_eq!(default_shell(Some("/usr/bin/zsh".into())), "/usr/bin/zsh");
    }

    #[test]
    fn shell_kind_strips_dir_and_exe_suffix() {
        assert_eq!(shell_kind_from_path("/usr/bin/zsh"), "zsh");
        assert_eq!(shell_kind_from_path("/bin/bash"), "bash");
        assert_eq!(shell_kind_from_path("powershell.exe"), "powershell");
        assert_eq!(shell_kind_from_path("fish"), "fish");
    }

    #[test]
    fn falls_back_to_platform_default() {
        let expected = super::fallback_shell();
        assert_eq!(default_shell(None), expected);
        assert_eq!(default_shell(Some("  ".into())), expected);
    }

    #[test]
    fn start_dir_uses_existing_path() {
        let tmp = std::env::temp_dir();
        assert_eq!(start_dir(Some(tmp.to_string_lossy().into_owned())), Some(tmp));
    }

    #[test]
    fn start_dir_falls_back_to_home_for_missing_path() {
        // A path that does not exist must not be used; we fall back to home.
        assert_eq!(start_dir(Some("/no/such/dir/xyz".into())), dirs::home_dir());
        assert_eq!(start_dir(Some("   ".into())), dirs::home_dir());
        assert_eq!(start_dir(None), dirs::home_dir());
    }

    #[test]
    fn resolve_clicked_path_joins_relative_onto_cwd() {
        assert_eq!(
            resolve_clicked_path("crates/tt-vt/src/search.rs", Some("/repo")),
            PathBuf::from("/repo/crates/tt-vt/src/search.rs"),
        );
    }

    #[test]
    fn resolve_clicked_path_keeps_absolute_and_ignores_cwd() {
        assert_eq!(
            resolve_clicked_path("/home/ctowles/app.tsx", Some("/repo")),
            PathBuf::from("/home/ctowles/app.tsx"),
        );
    }

    #[test]
    fn resolve_clicked_path_expands_leading_tilde() {
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(resolve_clicked_path("~/src/a.rs", Some("/repo")), home.join("src/a.rs"));
    }

    #[test]
    fn resolve_clicked_path_relative_without_cwd_stays_relative() {
        assert_eq!(resolve_clicked_path("src/a.rs", None), PathBuf::from("src/a.rs"));
        assert_eq!(resolve_clicked_path("src/a.rs", Some("  ")), PathBuf::from("src/a.rs"));
    }

    #[test]
    fn editor_open_args_without_line_is_just_the_path() {
        let path = PathBuf::from("/repo/src/a.rs");
        assert_eq!(editor_open_args("code", &path, None), vec![OsString::from(&path)]);
    }

    #[test]
    fn editor_open_args_vscode_family_uses_goto() {
        let path = PathBuf::from("/repo/src/a.rs");
        for editor in [
            "code",
            "code-insiders",
            "cursor",
            "codium",
            "vscodium",
            "windsurf",
        ] {
            assert_eq!(
                editor_open_args(editor, &path, Some(42)),
                vec![
                    OsString::from("--goto"),
                    OsString::from("/repo/src/a.rs:42")
                ],
                "editor: {editor}",
            );
        }
    }

    #[test]
    fn editor_open_args_recognizes_editor_by_basename_and_extension() {
        let path = PathBuf::from("/repo/src/a.rs");
        assert_eq!(
            editor_open_args("/usr/bin/code", &path, Some(42)),
            vec![
                OsString::from("--goto"),
                OsString::from("/repo/src/a.rs:42")
            ],
        );
        assert_eq!(
            editor_open_args("Code.exe", &path, Some(42)),
            vec![
                OsString::from("--goto"),
                OsString::from("/repo/src/a.rs:42")
            ],
        );
    }

    #[test]
    fn editor_open_args_sublime_and_zed_use_path_colon_line() {
        let path = PathBuf::from("/repo/src/a.rs");
        for editor in ["subl", "sublime_text", "zed", "hx"] {
            assert_eq!(
                editor_open_args(editor, &path, Some(7)),
                vec![OsString::from("/repo/src/a.rs:7")],
                "editor: {editor}",
            );
        }
    }

    #[test]
    fn editor_open_args_vim_family_uses_plus_line_flag() {
        let path = PathBuf::from("/repo/src/a.rs");
        for editor in [
            "vim",
            "nvim",
            "gvim",
            "mvim",
            "vi",
            "nano",
            "emacs",
            "emacsclient",
        ] {
            assert_eq!(
                editor_open_args(editor, &path, Some(3)),
                vec![OsString::from("+3"), OsString::from(&path)],
                "editor: {editor}",
            );
        }
    }

    #[test]
    fn editor_open_args_jetbrains_family_uses_line_flag() {
        let path = PathBuf::from("/repo/src/a.rs");
        for editor in ["idea", "webstorm", "pycharm", "goland", "rider", "clion"] {
            assert_eq!(
                editor_open_args(editor, &path, Some(9)),
                vec![
                    OsString::from("--line"),
                    OsString::from("9"),
                    OsString::from(&path)
                ],
                "editor: {editor}",
            );
        }
    }

    #[test]
    fn editor_open_args_unrecognized_editor_falls_back_to_bare_path() {
        let path = PathBuf::from("/repo/src/a.rs");
        assert_eq!(
            editor_open_args("some-custom-editor", &path, Some(5)),
            vec![OsString::from(&path)]
        );
    }
}
