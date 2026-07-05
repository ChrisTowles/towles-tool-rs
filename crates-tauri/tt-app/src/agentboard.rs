//! Tauri bridge for agentboard. The engine itself lives in
//! `tt_agentboard::engine`; this module owns the Tauri glue: the managed state,
//! the `agentboard://state` event, and the `ab_*` commands. Agent state is
//! derived by scanning `~/.claude` (see `lib.rs`), not pushed over HTTP.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use tt_agentboard::StatePayload;
use tt_agentboard::engine::parse_tone;
use tt_agentboard::fs_notify::DirNotifier;
use tt_agentboard::metadata::{LogInput, ProgressInput, StatusInput};
use tt_agentboard::session_order::ReorderDelta;

pub use tt_agentboard::engine::{Engine, now_ms};

/// Tauri event carrying the state snapshot.
pub const STATE_EVENT: &str = "agentboard://state";

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

/// Stamp `SessionData.live` from the app's PTY registry. The engine assembles
/// `live: false` (the Tauri-free crate can't see PTYs); every payload leaving
/// the app — command return or event — passes through here first.
pub fn stamp_live(payload: &mut StatePayload, live: &std::collections::HashSet<String>) {
    for repo in &mut payload.repos {
        for folder in &mut repo.folders {
            for session in &mut folder.sessions {
                session.live = live.contains(&session.id);
            }
        }
    }
}

/// The stamped payload, recomputed now. Shared by `ab_get_state` and emitters.
pub fn stamped_payload(app: &AppHandle) -> StatePayload {
    let ab = app.state::<Ab>();
    let mut payload = {
        let mut engine = ab.engine.lock().unwrap();
        engine.compute_payload(now_ms())
    };
    stamp_live(&mut payload, &app.state::<crate::terminal::TermState>().live_ids());
    payload
}

// --- Tauri commands ---

/// Pull the current snapshot (initial mount).
#[tauri::command]
pub fn ab_get_state(app: AppHandle) -> StatePayload {
    stamped_payload(&app)
}

/// Clear unseen for a session (fast-path: patch + re-emit, no full rebuild).
#[tauri::command]
pub fn ab_mark_seen(state: State<Ab>, app: AppHandle, name: String) {
    let patched = {
        let mut engine = state.engine.lock().unwrap();
        engine.mark_seen_patch(&name)
    };
    if let Some(mut payload) = patched {
        stamp_live(&mut payload, &app.state::<crate::terminal::TermState>().live_ids());
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

/// Read the add-repo picker's configured scan roots (`scanRoots` in repos.json).
/// Empty ⇒ the picker falls back to `~/code`.
#[tauri::command]
pub fn ab_get_scan_roots(state: State<Ab>) -> Vec<String> {
    state.engine.lock().unwrap().scan_roots()
}

/// Set the add-repo picker's scan roots. Blank entries are dropped; an empty
/// list clears the key so the picker falls back to `~/code`.
#[tauri::command]
pub fn ab_set_scan_roots(state: State<Ab>, roots: Vec<String>) {
    let cleaned: Vec<String> =
        roots.into_iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
    state.engine.lock().unwrap().set_scan_roots(cleaned);
}

/// A discovered git repo not yet on the rail, for the fuzzy add-repo picker.
#[derive(serde::Serialize)]
pub struct RepoCandidate {
    /// Friendly label, e.g. `p/towles-tool` (path relative to the scan root).
    pub name: String,
    /// Absolute path, passed back verbatim to `ab_add_repo`.
    pub dir: String,
}

/// Expand a leading `~`/`~/` in a configured scan root to the home dir.
fn expand_tilde(raw: &str, home: Option<&std::path::Path>) -> std::path::PathBuf {
    match (raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~")), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(raw),
    }
}

/// Discover git repos under the configured scan roots (`scanRoots` in
/// repos.json, defaulting to `~/code`) that aren't already on the rail, so the
/// add-repo picker can fuzzy-search them. Each candidate's `name` is its path
/// relative to whichever root it was found under.
#[tauri::command]
pub fn ab_discover_repos(state: State<Ab>) -> Vec<RepoCandidate> {
    use std::collections::HashSet;
    let (existing, configured): (HashSet<String>, Vec<String>) = {
        let mut engine = state.engine.lock().unwrap();
        (engine.repo_dirs().into_iter().collect(), engine.scan_roots())
    };
    let home = dirs::home_dir();
    let roots: Vec<std::path::PathBuf> = if configured.is_empty() {
        home.iter().map(|h| h.join("code")).collect()
    } else {
        configured.iter().map(|r| expand_tilde(r, home.as_deref())).collect()
    };
    tt_agentboard::repos::discover_git_repos(&roots, 4)
        .into_iter()
        .filter(|dir| !existing.contains(dir))
        .map(|dir| {
            let name = roots
                .iter()
                .find_map(|root| std::path::Path::new(&dir).strip_prefix(root).ok())
                .and_then(|p| p.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| dir.clone());
            RepoCandidate { name, dir }
        })
        .collect()
}

/// Add a PTY session to a folder. Returns the new record so the client can
/// select it immediately.
#[tauri::command]
pub fn ab_add_session(
    state: State<Ab>,
    dir: String,
    name: Option<String>,
) -> tt_agentboard::SessionRecord {
    let record = state.engine.lock().unwrap().add_session(&dir, name.as_deref(), now_ms());
    state.emit.notify_one();
    record
}

#[tauri::command]
pub fn ab_rename_session(state: State<Ab>, id: String, name: String) {
    state.engine.lock().unwrap().rename_session(&id, &name);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_close_session(state: State<Ab>, id: String) {
    state.engine.lock().unwrap().close_session(&id);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_refresh(state: State<Ab>) {
    state.emit.notify_one();
}

/// Set (or clear with `None`/blank) a folder's user-authored purpose.
#[tauri::command]
pub fn ab_set_folder_purpose(state: State<Ab>, dir: String, text: Option<String>) {
    let changed = state.engine.lock().unwrap().set_folder_purpose(&dir, text.as_deref());
    if changed {
        state.emit.notify_one();
    }
}

/// Set the compact-nudge threshold (context-%), persisting to shared settings.
#[tauri::command]
pub fn ab_set_compact_percent(state: State<Ab>, percent: u8) {
    let changed = state.engine.lock().unwrap().set_compact_recommend_percent(percent);
    if changed {
        state.emit.notify_one();
    }
}

/// Persist the window layout (frontend-owned; saved debounced from the client).
/// Deliberately does NOT re-emit — echoing the blob back would clobber
/// rapid-fire local edits; the client's copy is the live truth.
#[tauri::command]
pub fn ab_save_windows(state: State<Ab>, payload: tt_agentboard::WindowsPayload) {
    state.engine.lock().unwrap().set_windows(payload);
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

/// Open a session's repo directory in the preferred editor. Ports the TS
/// open-in-editor (spawns `<preferredEditor> <dir>`; the TS TMUX-env stripping is
/// desktop-irrelevant and skipped).
#[tauri::command]
pub fn ab_open_in_editor(state: State<Ab>, name: String) -> Result<(), String> {
    let (editor, dir) = {
        let mut engine = state.engine.lock().unwrap();
        (engine.preferred_editor(), engine.repo_dir_for(&name))
    };
    let Some(dir) = dir else {
        return Err(format!("No repo named {name}"));
    };
    if editor.trim().is_empty() {
        return Err("No preferred editor configured".into());
    }
    std::process::Command::new(&editor)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("Failed to launch {editor}: {e}"))?;
    Ok(())
}
