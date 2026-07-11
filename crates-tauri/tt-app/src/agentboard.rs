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

/// Stamp `SessionData.live`/`shellKind` from the app's PTY registry. The
/// engine assembles them false/None (the Tauri-free crate can't see PTYs);
/// every payload leaving the app — command return or event — passes through
/// here first.
pub fn stamp_pty_state(payload: &mut StatePayload, terms: &crate::terminal::TermState) {
    let live = terms.live_ids();
    let shell_kinds = terms.shell_kinds();
    for repo in &mut payload.repos {
        for folder in &mut repo.folders {
            for session in &mut folder.sessions {
                session.live = live.contains(&session.id);
                session.shell_kind = shell_kinds.get(&session.id).cloned();
            }
        }
    }
    // Now that `live` is truthful, recompute every folder/repo `needs` count
    // — the engine assembled them as 0 placeholders pre-stamp.
    tt_agentboard::bridge::recompute_needs(payload);
}

/// The stamped payload, recomputed now. Shared by `ab_get_state` and emitters.
/// The agent snapshot (claude CLI + `/proc` + transcript reads) is collected
/// BEFORE taking the engine lock so its subprocess work can't stall other
/// `ab_*` commands.
pub fn stamped_payload(app: &AppHandle) -> StatePayload {
    let snapshot = tt_agentboard::engine::collect_agent_snapshot(now_ms());
    let ab = app.state::<Ab>();
    let mut payload = {
        let mut engine = ab.engine.lock().unwrap();
        engine.compute_payload_with(&snapshot, now_ms())
    };
    stamp_pty_state(&mut payload, &app.state::<crate::terminal::TermState>());
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
        stamp_pty_state(&mut payload, &app.state::<crate::terminal::TermState>());
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

/// Remove the repo at `dir` from the rail. Takes the exact dir, not a
/// resolved session name — the client always has the dir on hand, and
/// removing several repos in a row by name is unsafe (see
/// `remove_repo_by_dir`'s doc comment).
#[tauri::command]
pub fn ab_remove_repo(state: State<Ab>, dir: String) {
    state.engine.lock().unwrap().remove_repo(&dir);
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

/// A repo candidate for the manage-repos picker: either already on the rail
/// or discoverable under a scan root.
#[derive(serde::Serialize)]
pub struct RepoCandidate {
    /// Friendly label, e.g. `p/towles-tool` (path relative to the scan root).
    pub name: String,
    /// Absolute path, passed back verbatim to `ab_add_repo`/`ab_remove_repo`.
    pub dir: String,
    /// Whether this repo is currently on the rail.
    pub active: bool,
}

/// Expand a leading `~`/`~/` in a configured scan root to the home dir.
fn expand_tilde(raw: &str, home: Option<&std::path::Path>) -> std::path::PathBuf {
    match (raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~")), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(raw),
    }
}

/// Build the manage-repos picker's candidate list: repos discovered under
/// `roots` unioned with `existing` (repos already on the rail, which may live
/// outside every root, e.g. added by typed path). Each candidate's `name` is
/// its path relative to whichever root it was found under, falling back to
/// the bare dir for repos outside every root; `active` marks whether it's in
/// `existing`, so the picker can render it pre-checked. Pulled out of
/// `ab_discover_repos` so it's testable without a Tauri `State`.
fn build_repo_candidates(existing: &[String], roots: &[std::path::PathBuf]) -> Vec<RepoCandidate> {
    use std::collections::HashSet;
    let existing_set: HashSet<&String> = existing.iter().collect();
    let name_for = |dir: &str| {
        roots
            .iter()
            .find_map(|root| std::path::Path::new(dir).strip_prefix(root).ok())
            .and_then(|p| p.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| dir.to_string())
    };

    let mut dirs: Vec<String> = tt_agentboard::repos::discover_git_repos(roots, 4);
    for dir in existing {
        if !dirs.contains(dir) {
            dirs.push(dir.clone());
        }
    }
    dirs.sort();
    dirs.dedup();

    dirs.into_iter()
        .map(|dir| {
            let name = name_for(&dir);
            let active = existing_set.contains(&dir);
            RepoCandidate { name, dir, active }
        })
        .collect()
}

/// List every repo the manage-repos picker should show (see
/// `build_repo_candidates`) under the configured scan roots (`scanRoots` in
/// repos.json, defaulting to `~/code`).
#[tauri::command]
pub fn ab_discover_repos(state: State<Ab>) -> Vec<RepoCandidate> {
    let (existing, configured): (Vec<String>, Vec<String>) = {
        let mut engine = state.engine.lock().unwrap();
        (engine.repo_dirs(), engine.scan_roots())
    };
    let home = dirs::home_dir();
    let roots: Vec<std::path::PathBuf> = if configured.is_empty() {
        home.iter().map(|h| h.join("code")).collect()
    } else {
        configured.iter().map(|r| expand_tilde(r, home.as_deref())).collect()
    };
    build_repo_candidates(&existing, &roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_union_discovered_and_existing_marking_active() {
        let root = tempfile::TempDir::new().unwrap();
        let base = root.path();
        std::fs::create_dir_all(base.join("p/proj/.git")).unwrap();
        std::fs::create_dir_all(base.join("p/other/.git")).unwrap();

        // "p/other" is already on the rail; "p/proj" is only discovered;
        // "/elsewhere/typed" is on the rail but outside every scan root.
        let other_dir = base.join("p/other").to_str().unwrap().to_string();
        let existing = vec![other_dir.clone(), "/elsewhere/typed".to_string()];
        let candidates = build_repo_candidates(&existing, &[base.to_path_buf()]);

        let proj = candidates.iter().find(|c| c.dir.ends_with("p/proj")).unwrap();
        assert!(!proj.active);
        assert_eq!(proj.name, "p/proj");

        let other = candidates.iter().find(|c| c.dir == other_dir).unwrap();
        assert!(other.active);
        assert_eq!(other.name, "p/other");

        let typed = candidates.iter().find(|c| c.dir == "/elsewhere/typed").unwrap();
        assert!(typed.active);
        assert_eq!(typed.name, "/elsewhere/typed"); // outside every root → bare dir
    }
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

/// The repo dir + new session id opened by [`ab_open_session_for_cwd`], so the
/// client can select the session immediately.
#[derive(serde::Serialize)]
pub struct OpenedSession {
    pub folder_dir: String,
    pub session_id: String,
}

/// Resolve a Claude Code session's real `cwd` to a repo (adding it to the rail
/// first if it isn't already registered), then add a new session there. Used
/// by the Claude Sessions screen's "Open in Agentboard" action.
#[tauri::command]
pub fn ab_open_session_for_cwd(state: State<Ab>, cwd: String) -> Result<OpenedSession, String> {
    if !std::path::Path::new(&cwd).exists() {
        return Err(format!("{cwd} no longer exists on disk"));
    }
    let mut engine = state.engine.lock().unwrap();
    let entries = tt_agentboard::repos::repo_entries(&engine.repo_dirs());
    let dir = tt_agentboard::repos::resolve_repo_dir(&cwd, &entries).unwrap_or_else(|| {
        tt_agentboard::repos::find_repo_root(std::path::Path::new(&cwd))
            .to_string_lossy()
            .to_string()
    });
    if engine.add_repo(&dir) {
        state.scan.notify_one();
    }
    let record = engine.add_session(&dir, None, now_ms());
    drop(engine);
    state.emit.notify_one();
    Ok(OpenedSession { folder_dir: dir, session_id: record.id })
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

/// Set (or clear with `None`/blank) a session's user-authored purpose —
/// captured when starting Claude, so the rail can show why a session exists.
#[tauri::command]
pub fn ab_set_session_purpose(state: State<Ab>, id: String, text: Option<String>) {
    let changed = state.engine.lock().unwrap().set_session_purpose(&id, text.as_deref());
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
/// `touched_folders` are the folder dirs the client actually mutated since its
/// last save — see `WindowsStore::save`'s doc comment for why a whole-blob
/// save can't be applied blindly across every folder.
#[tauri::command]
pub fn ab_save_windows(
    state: State<Ab>,
    payload: tt_agentboard::WindowsPayload,
    touched_folders: Vec<String>,
) {
    state.engine.lock().unwrap().set_windows(payload, &touched_folders);
}

/// Set (or clear) one folder-rail row's collapsed state (issue #52).
/// Deliberately does NOT re-emit — same rationale as `ab_save_windows`.
#[tauri::command]
pub fn ab_save_collapsed(state: State<Ab>, key: String, collapsed: bool) {
    state.engine.lock().unwrap().set_collapsed(&key, collapsed);
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

/// Full unified diff for a folder, for the diff pane. `mode` picks the
/// baseline: `"uncommitted"` diffs the working tree vs HEAD, anything else
/// diffs vs the merge-base with origin/main. Empty string when there's
/// nothing to show. Async: a large working-tree diff is a real subprocess
/// wait that must not stall the main thread.
#[tauri::command]
pub async fn ab_get_diff(dir: String, mode: String) -> String {
    let mode = if mode == "uncommitted" {
        tt_agentboard::DiffMode::Uncommitted
    } else {
        tt_agentboard::DiffMode::Main
    };
    tauri::async_runtime::spawn_blocking(move || tt_agentboard::diff_patch(&dir, mode))
        .await
        .unwrap_or_default()
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
