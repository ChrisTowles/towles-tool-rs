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

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tt_vt::{EngineOptions, Event as VtEvent, Frame, Input as VtInput, Select as VtSelect};

pub const FRAME_EVENT: &str = "terminal://frame";
pub const EXIT_EVENT: &str = "terminal://exit";
const MAIN_WINDOW_LABEL: &str = "main";

/// Scrollback kept per terminal, in rows. Lives in the Rust engine, not the
/// webview (xterm.js used to hold this in the JS heap).
const MAX_SCROLLBACK: usize = 10_000;

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
    /// commands; the PTY reader holds its own clone for output bytes).
    vt: std::sync::mpsc::Sender<VtInput>,
    child: Box<dyn Child + Send + Sync>,
    generation: u64,
    /// The shell's display name, resolved once at spawn time — e.g. "zsh",
    /// "bash". Best-effort: a user running a different shell inside this one
    /// (e.g. `bash` inside `zsh`) won't change it.
    shell_kind: String,
}

/// All live terminals, keyed by the frontend's `term_id`.
#[derive(Default)]
pub struct TermState(Mutex<HashMap<String, Session>>);

impl TermState {
    /// Ids of every session with a live PTY right now. The agentboard bridge
    /// stamps these onto the emitted snapshot as `SessionData.live`.
    pub fn live_ids(&self) -> std::collections::HashSet<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }

    /// Each live session's shell kind. The agentboard bridge stamps these onto
    /// the emitted snapshot as `SessionData.shellKind`.
    pub fn shell_kinds(&self) -> HashMap<String, String> {
        self.0.lock().unwrap().iter().map(|(id, s)| (id.clone(), s.shell_kind.clone())).collect()
    }

    /// Kill, reap, and drop the session with `term_id`, if any. The kill/wait
    /// runs after the map lock is released.
    fn kill(&self, term_id: &str) {
        let session = self.0.lock().unwrap().remove(term_id);
        if let Some(mut session) = session {
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
    }

    /// Kill, reap, and drop every session (window teardown).
    fn kill_all(&self) {
        let sessions: Vec<Session> = self.0.lock().unwrap().drain().map(|(_, s)| s).collect();
        for mut session in sessions {
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
    }

    /// Remove `term_id` only if it still holds `generation`, returning the
    /// session for reaping. A newer generation means this id was replaced —
    /// leave the replacement alone.
    fn take_if_current(&self, term_id: &str, generation: u64) -> Option<Session> {
        let mut guard = self.0.lock().unwrap();
        if guard.get(term_id).is_some_and(|s| s.generation == generation) {
            return guard.remove(term_id);
        }
        None
    }
}

/// Render frame streamed to the frontend; `termId` routes it to the right
/// terminal view.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermFrame {
    term_id: String,
    frame: Frame,
}

/// Emitted once when a shell exits so the frontend can close that terminal.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermExit {
    term_id: String,
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
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || term_start_blocking(app, term_id, cols, rows, cwd))
        .await
        .map_err(|e| format!("terminal spawn task failed: {e}"))?
}

fn term_start_blocking(
    app: AppHandle,
    term_id: String,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<(), String> {
    let state = app.state::<TermState>();
    state.kill(&term_id);

    let pty = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("failed to open pty: {e}"))?;

    let shell = default_shell(std::env::var(SHELL_ENV_VAR).ok());
    let shell_kind = shell_kind_from_path(&shell);
    let dir = start_dir(cwd);
    let mut cmd = CommandBuilder::new(shell);
    // Drop any TT_* var inherited from the app process itself (e.g. TT_DEV_PORT,
    // set by scripts/dev-port.mjs for *this* slot's own dev server) so a shell
    // command run inside this terminal — like `npm run dev` for a different
    // repo/slot — resolves its own port/session instead of colliding with the
    // outer one.
    let inherited_tt_vars: Vec<String> = cmd
        .iter_full_env_as_str()
        .filter(|(k, _)| k.starts_with("TT_"))
        .map(|(k, _)| k.to_string())
        .collect();
    for key in inherited_tt_vars {
        cmd.env_remove(key);
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
        }
    })
    .map_err(|e| format!("failed to start terminal engine: {e}"))?;
    let vt_tx = vt.sender();

    state.0.lock().unwrap().insert(
        term_id.clone(),
        Session { master: pty.master, input: input_tx, vt: vt_tx, child, generation, shell_kind },
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
    // shell exited or this PTY was replaced.
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
            let _ = session.child.wait();
            let _ = app.emit_to(MAIN_WINDOW_LABEL, EXIT_EVENT, TermExit { term_id });
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

/// Forward keyboard input (UTF-8 text / escape sequences the terminal view
/// encoded) to the shell. Queues onto the session's writer thread — never
/// blocks, even against a shell that has stopped reading its PTY.
#[tauri::command]
pub fn term_write(state: State<TermState>, term_id: String, data: String) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
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
    let guard = state.0.lock().unwrap();
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
    let guard = state.0.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Scroll(delta));
    Ok(())
}

/// Ask the engine to emit one full frame regardless of dirty state. The view
/// calls this when a pane transitions from hidden (`display:none`) back to
/// visible: dirty-only frames never resend rows the engine considers clean,
/// so a stale canvas would otherwise stay stale until a scroll (#47).
#[tauri::command]
pub fn term_request_full(state: State<TermState>, term_id: String) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::RequestFull);
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
    let guard = state.0.lock().unwrap();
    let session = guard.get(&term_id).ok_or("no shell running")?;
    let _ = session.vt.send(VtInput::Select(op));
    Ok(())
}

/// Plain text of the terminal's active selection (empty string when there is
/// none). The engine thread answers over a bounded channel; a dead engine
/// yields an error rather than a hang.
#[tauri::command]
pub async fn term_copy(app: AppHandle, term_id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (reply_tx, reply_rx) = sync_channel::<Option<String>>(1);
        {
            let state = app.state::<TermState>();
            let guard = state.0.lock().unwrap();
            let session = guard.get(&term_id).ok_or("no shell running")?;
            session.vt.send(VtInput::Copy(reply_tx)).map_err(|_| "terminal engine gone")?;
        }
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map(|text| text.unwrap_or_default())
            .map_err(|_| "terminal engine did not answer".to_string())
    })
    .await
    .map_err(|e| format!("copy task failed: {e}"))?
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
    use super::{default_shell, shell_kind_from_path, start_dir};

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
}
