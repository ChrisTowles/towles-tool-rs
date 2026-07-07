//! Embedded terminals: shells in PTYs (portable-pty), rendered by ghostty-web
//! in the app. Many terminals live at once, keyed by a frontend-supplied
//! `term_id` (the agentboard screen spawns one or more per session, each rooted
//! in the session's folder). Output streams to the frontend as base64
//! `terminal://output` events tagged with `termId` (raw bytes — the frontend
//! terminal owns UTF-8 decoding across chunk boundaries); input/resize come
//! back as commands.
//!
//! When shpool is installed, the PTY runs `shpool attach` instead of the shell
//! directly, so the shell lives in a service-managed daemon and survives the
//! app: killing the PTY client only disconnects the session, and starting the
//! same `term_id` again resumes it (see [`crate::shpool`]). Explicit closes
//! (`term_kill`) kill the daemon-side session too.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub const OUTPUT_EVENT: &str = "terminal://output";
pub const EXIT_EVENT: &str = "terminal://exit";
const MAIN_WINDOW_LABEL: &str = "main";

/// One live PTY session (one shell shown in one xterm.js instance).
struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
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

    /// Kill and drop the session with `term_id`, if any.
    fn kill(&self, term_id: &str) {
        if let Some(mut session) = self.0.lock().unwrap().remove(term_id) {
            let _ = session.child.kill();
        }
    }

    /// Kill and drop every session (window teardown).
    fn kill_all(&self) {
        for (_, mut session) in self.0.lock().unwrap().drain() {
            let _ = session.child.kill();
        }
    }
}

/// Output chunk streamed to the frontend; `termId` routes it to the right xterm.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermChunk {
    term_id: String,
    /// Base64 of the raw PTY bytes.
    data: String,
}

/// Emitted once when a shell exits so the frontend can close that terminal.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TermExit {
    term_id: String,
}

/// Spawn a shell in a fresh PTY sized to the xterm.js grid, rooted at `cwd`
/// (falls back to `$HOME` when `cwd` is missing or not an existing dir).
/// Replaces any existing terminal with the same `term_id`.
#[tauri::command]
pub fn term_start(
    app: AppHandle,
    state: State<TermState>,
    term_id: String,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<(), String> {
    state.kill(&term_id);

    let pty = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("failed to open pty: {e}"))?;

    let shell = default_shell(std::env::var(SHELL_ENV_VAR).ok());
    let shell_kind = shell_kind_from_path(&shell);
    let dir = start_dir(cwd);
    let mut cmd = if crate::shpool::available() {
        // Persistent path: the PTY hosts an attach client; the shell itself
        // lives in the shpool daemon and survives this app process.
        crate::shpool::ensure_daemon();
        let mut c = CommandBuilder::new("shpool");
        c.args(crate::shpool::attach_args(&term_id, dir.as_deref()));
        c
    } else {
        CommandBuilder::new(shell)
    };
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
    // this session (see tt_agentboard::procenv). `term_id` == the session id. On
    // the shpool path this rides `forward_env` into the daemon-side shell.
    cmd.env("TT_SESSION_ID", &term_id);
    // Never nest: if the app itself was launched from inside a shpool session,
    // the inherited name would make the attach client think it's recursing.
    cmd.env_remove("SHPOOL_SESSION_NAME");
    if let Some(dir) = &dir {
        cmd.cwd(dir);
    }
    let child = pty.slave.spawn_command(cmd).map_err(|e| format!("failed to spawn shell: {e}"))?;

    let mut reader =
        pty.master.try_clone_reader().map_err(|e| format!("failed to clone pty reader: {e}"))?;
    let writer = pty.master.take_writer().map_err(|e| format!("failed to take pty writer: {e}"))?;

    state
        .0
        .lock()
        .unwrap()
        .insert(term_id.clone(), Session { master: pty.master, writer, child, shell_kind });

    // Liveness changed (a PTY appeared) — refresh the agentboard snapshot.
    notify_agentboard(&app);

    // Reader thread: pump PTY output to the frontend until EOF (shell exited).
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk =
                        TermChunk { term_id: term_id.clone(), data: BASE64.encode(&buf[..n]) };
                    let _ = app.emit_to(MAIN_WINDOW_LABEL, OUTPUT_EVENT, chunk);
                }
            }
        }
        let _ = app.emit_to(MAIN_WINDOW_LABEL, EXIT_EVENT, TermExit { term_id });
        notify_agentboard(&app); // shell exited — session no longer live
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

/// Forward keyboard input (xterm.js `onData` UTF-8 text) to the shell.
#[tauri::command]
pub fn term_write(state: State<TermState>, term_id: String, data: String) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    let session = guard.get_mut(&term_id).ok_or("no shell running")?;
    session.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())
}

/// Keep the PTY size in sync with the xterm.js grid.
#[tauri::command]
pub fn term_resize(
    state: State<TermState>,
    term_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    let session = guard.get_mut(&term_id).ok_or("no shell running")?;
    session
        .master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

/// Kill one shell (the frontend calls this when a terminal unmounts — an
/// explicit close). Also kills the daemon-side shpool session, if any, so a
/// deliberately closed pane doesn't linger detached forever.
#[tauri::command]
pub fn term_kill(app: AppHandle, state: State<TermState>, term_id: String) {
    state.kill(&term_id);
    crate::shpool::kill_session(&term_id);
    notify_agentboard(&app);
}

/// Drop every PTY when the main window goes away (wired to the window
/// Destroyed event in lib.rs). On the shpool path this only *disconnects* the
/// sessions — the daemon keeps the shells alive and the next launch resumes
/// them; without shpool it kills the shells outright (nothing to resume).
pub fn on_window_destroyed(app: &AppHandle, label: &str) {
    if label == MAIN_WINDOW_LABEL {
        app.state::<TermState>().kill_all();
    }
}

/// Emitted (with the live-shell count) when a window close is intercepted so
/// the frontend can ask: keep the shells running detached, or kill them?
pub const CLOSE_ASK_EVENT: &str = "app://close-requested";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloseAsk {
    live: usize,
}

/// Whether closing the main window needs the keep-or-kill dialog first: only
/// when shpool can actually keep shells alive (without it closing kills them
/// like it always did — nothing to ask) and at least one PTY is live. Emits
/// [`CLOSE_ASK_EVENT`] when returning true; the caller prevents the close and
/// the frontend answers via [`app_close`].
pub fn ask_before_close(app: &AppHandle, label: &str) -> bool {
    if label != MAIN_WINDOW_LABEL || !crate::shpool::available() {
        return false;
    }
    let live = app.state::<TermState>().live_ids().len();
    if live == 0 {
        return false;
    }
    let _ = app.emit_to(MAIN_WINDOW_LABEL, CLOSE_ASK_EVENT, CloseAsk { live });
    true
}

/// The keep-or-kill dialog's answer. `kill_sessions` kills every one of this
/// slot's daemon-side sessions (live *and* previously detached — "quit and
/// kill" means nothing left running); keeping just tears the window down,
/// which detaches. `destroy()` bypasses CloseRequested, so no re-prompt.
#[tauri::command]
pub fn app_close(app: AppHandle, kill_sessions: bool) {
    if kill_sessions {
        app.state::<TermState>().kill_all();
        crate::shpool::kill_slot_sessions();
    }
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = win.destroy();
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
