//! Tauri bridge for the personal-dashboard store (`tt_store`) and journal logging
//! (`tt_journal`). Mirrors the agentboard bridge shape (see `agentboard.rs`): a
//! managed state wrapping the non-`Sync` `Store` in a `Mutex`, `#[tauri::command]`
//! fns, and a single `store://snapshot` event carrying the full `tt_store::Snapshot`.
//!
//! The store is opened once at startup (`StoreState::open`). Because
//! `Store::open_default` can fail (no data dir), the state holds an `Option<Store>`;
//! commands return `Err("store unavailable: …")` rather than panicking when it is
//! absent. Every successful write recomputes and re-emits the snapshot.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};

use tt_store::{Snapshot, Store};

/// Tauri event carrying the full store snapshot (initial mount + after every write).
pub const SNAPSHOT_EVENT: &str = "store://snapshot";

/// Managed Tauri state: the SQLite store, `None` when it could not be opened.
pub struct StoreState {
    store: Arc<Mutex<Option<Store>>>,
}

impl StoreState {
    /// Open the default store, logging a warning (and leaving the state empty) on
    /// failure so the app still starts. Store commands then return an error.
    pub fn open() -> StoreState {
        let store = match Store::open_default() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("store: unavailable ({e}); store commands will error until restart");
                None
            }
        };
        StoreState { store: Arc::new(Mutex::new(store)) }
    }

    /// Clone the shared store handle. Exposed for the Phase-3 collector scheduler,
    /// which needs to lock the store from its own tokio task (unused until then).
    #[allow(dead_code)]
    pub fn handle(&self) -> Arc<Mutex<Option<Store>>> {
        self.store.clone()
    }

    #[cfg(test)]
    fn from_option(store: Option<Store>) -> StoreState {
        StoreState { store: Arc::new(Mutex::new(store)) }
    }
}

/// Epoch milliseconds from the local wall clock (write-boundary clock).
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Compute a snapshot, or an error string when the store is unavailable.
fn snapshot_of(state: &StoreState) -> Result<Snapshot, String> {
    let guard = state.store.lock().unwrap();
    let store = guard.as_ref().ok_or("store unavailable: no data directory")?;
    store.snapshot().map_err(|e| format!("store snapshot failed: {e}"))
}

/// Recompute and emit the snapshot. Best-effort: a missing store or emit failure is
/// swallowed (the next write, or app restart, recovers).
pub fn emit_snapshot(app: &AppHandle, state: &StoreState) {
    if let Ok(snapshot) = snapshot_of(state) {
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
    }
}

// --- Tauri commands ---

/// Pull the current snapshot (initial mount).
#[tauri::command]
pub fn store_snapshot(state: State<StoreState>) -> Result<Snapshot, String> {
    snapshot_of(&state)
}

/// Add a manually-entered task, then re-emit the snapshot.
#[tauri::command]
pub fn store_add_task(
    app: AppHandle,
    state: State<StoreState>,
    text: String,
    due_ts: Option<i64>,
) -> Result<(), String> {
    {
        let guard = state.store.lock().unwrap();
        let store = guard.as_ref().ok_or("store unavailable: no data directory")?;
        store.add_task(&text, due_ts, now_ms()).map_err(|e| format!("add_task failed: {e}"))?;
    }
    emit_snapshot(&app, &state);
    Ok(())
}

/// Move a todo to a kanban column (backlog/next/doing/review/done), then re-emit.
#[tauri::command]
pub fn store_set_task_status(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    status: String,
) -> Result<(), String> {
    {
        let guard = state.store.lock().unwrap();
        let store = guard.as_ref().ok_or("store unavailable: no data directory")?;
        store
            .set_task_status(id, &status, now_ms())
            .map_err(|e| format!("set_task_status failed: {e}"))?;
    }
    emit_snapshot(&app, &state);
    Ok(())
}

/// Promote a local todo into a real GitHub issue in `repo` (owner/name), then
/// link the resulting issue back to the todo and re-emit the snapshot.
///
/// Shells `gh issue create --repo <repo>` with the todo's text as the title.
/// `gh` prints the new issue URL on stdout; the trailing path segment is its
/// number.
#[tauri::command]
pub fn store_promote_task_to_issue(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    repo: String,
) -> Result<(), String> {
    let title = {
        let guard = state.store.lock().unwrap();
        let store = guard.as_ref().ok_or("store unavailable: no data directory")?;
        store
            .get_task(id)
            .map_err(|e| format!("get_task failed: {e}"))?
            .ok_or_else(|| format!("no todo with id {id}"))?
            .text
    };

    let (number, url) = create_gh_issue(&repo, &title)?;

    {
        let guard = state.store.lock().unwrap();
        let store = guard.as_ref().ok_or("store unavailable: no data directory")?;
        store
            .link_task_issue(id, &repo, number, &url)
            .map_err(|e| format!("link_task_issue failed: {e}"))?;
    }
    emit_snapshot(&app, &state);
    Ok(())
}

/// Run `gh issue create` and return the new issue's `(number, url)`.
fn create_gh_issue(repo: &str, title: &str) -> Result<(i64, String), String> {
    let output = std::process::Command::new("gh")
        .args([
            "issue", "create", "--repo", repo, "--title", title, "--body", "",
        ])
        .output()
        .map_err(|e| format!("failed to spawn gh: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "gh issue create failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let number = url
        .rsplit('/')
        .next()
        .and_then(|n| n.parse::<i64>().ok())
        .ok_or_else(|| format!("could not parse issue number from gh output: {url}"))?;
    Ok((number, url))
}

/// Append a timestamped line to today's daily note. Independent of the store (writes a
/// markdown file), so it works even when the store is unavailable. The local date and
/// `HH:MM` are resolved here, at the boundary; journal settings come from `tt_config`.
#[tauri::command]
pub fn journal_log(app: AppHandle, state: State<StoreState>, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("journal text is required".into());
    }
    let settings = tt_config::load().map_err(|e| format!("failed to load settings: {e}"))?;
    let now = chrono::Local::now();
    let date = now.date_naive();
    let time = now.format("%H:%M").to_string();
    tt_journal::entries::append_to_daily(&settings.journal_settings, date, &time, &text)
        .map_err(|e| format!("journal append failed: {e}"))?;
    // Journal writes don't change the store, but re-emit to match the write-command
    // contract (harmless no-op when the store is unavailable).
    emit_snapshot(&app, &state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_positive() {
        assert!(now_ms() > 0);
    }

    #[test]
    fn snapshot_of_empty_store_is_empty() {
        let state = StoreState::from_option(Some(Store::open_in_memory().unwrap()));
        let snap = snapshot_of(&state).unwrap();
        assert!(snap.tasks.is_empty());
        assert!(snap.events.is_empty());
        assert!(snap.issues.is_empty());
    }

    #[test]
    fn snapshot_reflects_writes() {
        let store = Store::open_in_memory().unwrap();
        store.add_task("buy milk", Some(500), 1).unwrap();
        let state = StoreState::from_option(Some(store));
        let snap = snapshot_of(&state).unwrap();
        assert_eq!(snap.tasks.len(), 1);
        assert_eq!(snap.tasks[0].text, "buy milk");
    }

    #[test]
    fn snapshot_of_unavailable_store_errors() {
        let state = StoreState::from_option(None);
        let err = snapshot_of(&state).unwrap_err();
        assert!(err.contains("store unavailable"), "got: {err}");
    }

    #[test]
    fn handle_shares_the_same_store() {
        let store = Store::open_in_memory().unwrap();
        store.add_task("shared", None, 1).unwrap();
        let state = StoreState::from_option(Some(store));
        let handle = state.handle();
        let guard = handle.lock().unwrap();
        let snap = guard.as_ref().unwrap().snapshot().unwrap();
        assert_eq!(snap.tasks.len(), 1);
    }
}
