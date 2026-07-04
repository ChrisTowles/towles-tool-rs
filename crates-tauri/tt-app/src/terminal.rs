//! Embedded terminals: shells in PTYs (portable-pty), rendered by xterm.js in
//! the app. Unlike the tmux-mode agentboard, the desktop app owns the PTYs
//! directly. Many terminals live at once, keyed by a frontend-supplied
//! `term_id` (the agentboard screen spawns one or more per session, each rooted
//! in the session's folder). Output streams to the frontend as base64
//! `terminal://output` events tagged with `termId` (raw bytes — xterm.js owns
//! UTF-8 decoding across chunk boundaries); input/resize come back as commands.

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
}

/// All live terminals, keyed by the frontend's `term_id`.
#[derive(Default)]
pub struct TermState(Mutex<HashMap<String, Session>>);

impl TermState {
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

    let mut cmd = CommandBuilder::new(default_shell(std::env::var("SHELL").ok()));
    cmd.env("TERM", "xterm-256color");
    // Stamp the PTY with its session id so a Claude agent launched inside inherits
    // it; the agentboard engine reads it back from /proc to attribute the agent to
    // this session (see tt_agentboard::procenv). `term_id` == the session id.
    cmd.env("TT_SESSION_ID", &term_id);
    if let Some(dir) = start_dir(cwd) {
        cmd.cwd(dir);
    }
    let child = pty.slave.spawn_command(cmd).map_err(|e| format!("failed to spawn shell: {e}"))?;

    let mut reader =
        pty.master.try_clone_reader().map_err(|e| format!("failed to clone pty reader: {e}"))?;
    let writer = pty.master.take_writer().map_err(|e| format!("failed to take pty writer: {e}"))?;

    state.0.lock().unwrap().insert(term_id.clone(), Session { master: pty.master, writer, child });

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
    });

    Ok(())
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

/// Kill one shell (the frontend calls this when a terminal unmounts).
#[tauri::command]
pub fn term_kill(state: State<TermState>, term_id: String) {
    state.kill(&term_id);
}

/// Kill every shell when the main window goes away, so no orphan shells
/// accumulate (wired to the window Destroyed event in lib.rs).
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

fn default_shell(shell_env: Option<String>) -> String {
    shell_env.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "/bin/bash".to_string())
}

#[cfg(test)]
mod tests {
    use super::{default_shell, start_dir};

    #[test]
    fn prefers_shell_env() {
        assert_eq!(default_shell(Some("/usr/bin/zsh".into())), "/usr/bin/zsh");
    }

    #[test]
    fn falls_back_to_bash() {
        assert_eq!(default_shell(None), "/bin/bash");
        assert_eq!(default_shell(Some("  ".into())), "/bin/bash");
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
