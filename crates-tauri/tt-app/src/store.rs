//! Tauri bridge for the personal-dashboard store (`tt_store`) and journal logging
//! (`tt_journal`). Mirrors the agentboard bridge shape (see `agentboard.rs`): a
//! managed state wrapping the non-`Sync` `Store` in a `Mutex`, `#[tauri::command]`
//! fns, and a single `store://snapshot` event carrying the full `tt_store::Snapshot`.
//!
//! The store is opened once at startup (`StoreState::open`). Because
//! `Store::open_default` can fail (no data dir), the state holds an `Option<Store>`;
//! commands return `Err("store unavailable: …")` rather than panicking when it is
//! absent. Every successful write recomputes and re-emits the snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
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

    #[cfg(test)]
    fn from_option(store: Option<Store>) -> StoreState {
        StoreState { store: Arc::new(Mutex::new(store)) }
    }
}

/// Managed flag guarding against overlapping manual "refresh now" runs. A
/// manual refresh shells `gh`/Slack, which can take seconds; without this a
/// jittery double-click (or a second window) could stack redundant sweeps.
#[derive(Default)]
pub struct CollectNowState {
    running: Arc<AtomicBool>,
}

/// Sets the running flag back to `false` when dropped, so the guard releases on
/// every exit path of the blocking worker — including a panic.
struct ReleaseOnDrop(Arc<AtomicBool>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Run `f` against the store, mapping an unavailable store to the stable
/// error string the frontend (and tests) key on.
fn with_store<T>(
    state: &StoreState,
    f: impl FnOnce(&Store) -> Result<T, String>,
) -> Result<T, String> {
    let guard = state.store.lock().unwrap();
    let store = guard.as_ref().ok_or("store unavailable: no data directory")?;
    f(store)
}

/// Epoch milliseconds from the local wall clock (write-boundary clock).
fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Compute a snapshot, or an error string when the store is unavailable.
fn snapshot_of(state: &StoreState) -> Result<Snapshot, String> {
    with_store(state, |store| store.snapshot().map_err(|e| format!("store snapshot failed: {e}")))
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
    repo: Option<String>,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .add_task(&text, due_ts, repo.as_deref(), None, now_ms())
            .map_err(|e| format!("add_task failed: {e}"))
    })?;
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
    with_store(&state, |store| {
        store
            .set_task_status(id, &status, now_ms())
            .map_err(|e| format!("set_task_status failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(())
}

/// Move a todo to `status` at an explicit slot (`index`) within that column,
/// renumbering the column's positions — powers drag-to-reorder and
/// position-aware cross-column drops. Then re-emit the snapshot.
#[tauri::command]
pub fn store_set_task_position(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    status: String,
    index: i64,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .set_task_position(id, &status, index, now_ms())
            .map_err(|e| format!("set_task_position failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(())
}

/// Edit a todo's text, notes, and due date (a full replace of those fields —
/// `null` clears notes/due), then re-emit the snapshot.
#[tauri::command]
pub fn store_update_task(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    text: String,
    notes: Option<String>,
    due_ts: Option<i64>,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .update_task(id, &text, notes.as_deref(), due_ts)
            .map(|_| ())
            .map_err(|e| format!("update_task failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(())
}

/// Delete a todo permanently, then re-emit the snapshot.
#[tauri::command]
pub fn store_delete_task(app: AppHandle, state: State<StoreState>, id: i64) -> Result<(), String> {
    with_store(&state, |store| {
        store.delete_task(id).map_err(|e| format!("delete_task failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(())
}

/// How long a completed todo lingers in Done before "Clear done" can sweep it.
const DONE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Sweep `done` todos completed more than [`DONE_RETENTION_MS`] ago, then
/// re-emit the snapshot. The cutoff is derived from the wall clock here, at the
/// command boundary — the store takes it as a plain `before_ms`. Returns the
/// number of todos removed.
#[tauri::command]
pub fn store_clear_done(app: AppHandle, state: State<StoreState>) -> Result<usize, String> {
    let before_ms = now_ms() - DONE_RETENTION_MS;
    let deleted = with_store(&state, |store| {
        store.clear_done_tasks(before_ms).map_err(|e| format!("clear_done_tasks failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(deleted)
}

/// Mark the watched DM's message at `ts` handled (banner dismissal), then re-emit.
#[tauri::command]
pub fn store_dm_dismiss(
    app: AppHandle,
    state: State<StoreState>,
    channel: String,
    ts: i64,
) -> Result<(), String> {
    with_store(&state, |store| {
        store.dismiss_dm(&channel, ts).map_err(|e| format!("dismiss_dm failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(())
}

/// Promote a local todo into a real GitHub issue in `repo` (owner/name), then
/// link the resulting issue back to the todo and re-emit the snapshot.
///
/// Shells `gh issue create --repo <repo>` with the todo's text as the title.
/// `gh` prints the new issue URL on stdout; the trailing path segment is its
/// number. Async: the network round-trip runs on a blocking worker so a slow
/// GitHub call can't stall the main thread (sync commands run there).
#[tauri::command]
pub async fn store_promote_task_to_issue(
    app: AppHandle,
    state: State<'_, StoreState>,
    id: i64,
    repo: String,
) -> Result<(), String> {
    let (title, body) = with_store(&state, |store| {
        let task = store
            .get_task(id)
            .map_err(|e| format!("get_task failed: {e}"))?
            .ok_or_else(|| format!("no todo with id {id}"))?;
        Ok((task.text, render_promoted_issue_body(task.notes.as_deref(), task.due_ts)))
    })?;

    let gh_repo = repo.clone();
    let (number, url) =
        tauri::async_runtime::spawn_blocking(move || create_gh_issue(&gh_repo, &title, &body))
            .await
            .map_err(|e| format!("gh issue create task failed: {e}"))??;

    with_store(&state, |store| {
        store
            .link_task_issue(id, &repo, number, &url)
            .map_err(|e| format!("link_task_issue failed: {e}"))
    })?;
    emit_snapshot(&app, &state);
    Ok(())
}

/// Create a new GitHub issue directly for the repo checked out at `dir` (no
/// linked todo). `gh` infers the repo from the folder's git remote, mirroring
/// the `.current_dir()` convention `tt-collect`'s issue/PR collectors use.
/// Used by the agentboard repo rail's "New issue" action; the created issue
/// shows up in Board's issue list once the next `issues` collector run picks
/// it up. Async for the same main-thread reason as
/// [`store_promote_task_to_issue`].
#[tauri::command]
pub async fn store_create_issue(dir: String, title: String) -> Result<String, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("issue title is required".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let output = std::process::Command::new("gh")
            .args(["issue", "create", "--title", &title, "--body", ""])
            .current_dir(&dir)
            .output()
            .map_err(|e| format!("failed to spawn gh in {dir}: {e}"))?;
        let (_, url) = parse_gh_issue_create_output(&output)?;
        Ok(url)
    })
    .await
    .map_err(|e| format!("gh issue create task failed: {e}"))?
}

/// Run `gh issue create` and return the new issue's `(number, url)`.
fn create_gh_issue(repo: &str, title: &str, body: &str) -> Result<(i64, String), String> {
    let output = std::process::Command::new("gh")
        .args([
            "issue", "create", "--repo", repo, "--title", title, "--body", body,
        ])
        .output()
        .map_err(|e| format!("failed to spawn gh: {e}"))?;
    parse_gh_issue_create_output(&output)
}

/// Render the GitHub issue body for a todo promoted from the tt board: the
/// todo's `notes` verbatim (dropped when blank), a footer marking the origin,
/// and — when the todo has one — its due date. `due_ts` is epoch milliseconds
/// passed in by the caller; the clock is never read here, so this stays pure
/// and unit-testable.
fn render_promoted_issue_body(notes: Option<&str>, due_ts: Option<i64>) -> String {
    let mut body = String::new();
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        body.push_str(notes);
        body.push_str("\n\n");
    }
    body.push_str("Promoted from tt board");
    if let Some(due) = due_ts.and_then(chrono::DateTime::from_timestamp_millis) {
        body.push_str(&format!("\nDue: {}", due.format("%Y-%m-%d")));
    }
    body
}

/// Parse a `gh issue create` invocation's output into `(number, url)`. `gh`
/// prints the new issue URL on stdout; the trailing path segment is its number.
fn parse_gh_issue_create_output(output: &std::process::Output) -> Result<(i64, String), String> {
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

/// Append a pre-formatted timeline bullet to today's daily note. Independent of the store
/// (writes a markdown file), so it works even when the store is unavailable. The frontend
/// owns the line format (`- HH:MM [context] text`, from `formatLogLine`), so it is written
/// verbatim; only the local date (for section placement) is resolved here. Journal
/// settings come from `tt_config`.
#[tauri::command]
pub fn journal_log(app: AppHandle, state: State<StoreState>, text: String) -> Result<(), String> {
    let line = text.trim();
    if line.is_empty() {
        return Err("journal text is required".into());
    }
    let settings = tt_config::load().map_err(|e| format!("failed to load settings: {e}"))?;
    let date = chrono::Local::now().date_naive();
    tt_journal::entries::append_bullet_to_daily(&settings.journal_settings, date, line)
        .map_err(|e| format!("journal append failed: {e}"))?;
    // Journal writes don't change the store, but re-emit to match the write-command
    // contract (harmless no-op when the store is unavailable).
    emit_snapshot(&app, &state);
    Ok(())
}

/// What a `store_collect_now` call did: `started` is `false` when a manual
/// refresh was already in flight and this call was a no-op (the frontend keeps
/// its spinner off in that case), `true` when this call ran the sweep.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectNowResult {
    pub started: bool,
}

/// Manually run the issues, PRs, and (when configured) Slack collectors right
/// now, then re-emit the snapshot — the "Refresh now" affordance in Config.
/// Calendar is intentionally left out (it spends `claude` tokens), matching
/// [`tt_collect::collect_manual`]. Overlap-guarded: if a manual refresh is
/// already running this returns `started: false` without starting another.
///
/// Runs on a blocking worker with its own store connection (mirroring the
/// scheduler) so the `gh`/Slack round-trips never hold the UI's store mutex.
#[tauri::command]
pub async fn store_collect_now(
    app: AppHandle,
    collect: State<'_, CollectNowState>,
) -> Result<CollectNowResult, String> {
    let running = collect.running.clone();
    // Acquire the guard: swap in `true`; if it was already `true`, bail.
    if running.swap(true, Ordering::SeqCst) {
        return Ok(CollectNowResult { started: false });
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _release = ReleaseOnDrop(running);
        run_collect_now_blocking(&app);
    })
    .await
    .map_err(|e| format!("collect-now worker failed: {e}"))?;
    Ok(CollectNowResult { started: true })
}

/// Open a fresh store, run the manual collector batch, and emit the resulting
/// snapshot. Failures per collector are logged (never surfaced as a command
/// error) so one dead collector doesn't sink the whole refresh.
fn run_collect_now_blocking(app: &AppHandle) {
    let store = match Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            eprintln!("collect-now: store unavailable ({e}); skipping manual refresh");
            return;
        }
    };
    let collectors = tt_config::load().map(|s| s.collectors).unwrap_or_default();
    let repos = tt_collect::tracked_repo_dirs();
    let slack = manual_slack_config(&collectors);
    for summary in tt_collect::collect_manual(&store, &repos, slack.as_ref(), now_ms()) {
        if !summary.ok {
            eprintln!(
                "collect-now: {} failed: {}",
                summary.collector,
                summary.message.as_deref().unwrap_or("unknown")
            );
        }
    }
    if let Ok(snapshot) = store.snapshot() {
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
    }
}

/// The Slack config to feed a manual refresh, or `None` when the collector is
/// disabled or has no token — the same gate the scheduler applies, so a manual
/// refresh never records a Slack failure the scheduled cadence would skip.
fn manual_slack_config(
    collectors: &tt_config::CollectorsSettings,
) -> Option<tt_collect::SlackDmConfig> {
    let slack = &collectors.slack;
    if !slack.enabled || slack.token.trim().is_empty() {
        return None;
    }
    Some(tt_collect::SlackDmConfig {
        token: slack.token.clone(),
        watch_user_id: slack.watch_user_id.clone(),
        watch_name: slack.watch_name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_slack_config_off_when_disabled_or_tokenless() {
        let mut collectors = tt_config::CollectorsSettings::default();
        assert!(manual_slack_config(&collectors).is_none(), "disabled by default");

        collectors.slack.enabled = true;
        assert!(manual_slack_config(&collectors).is_none(), "enabled but no token stays off");

        collectors.slack.token = "  ".to_string();
        assert!(manual_slack_config(&collectors).is_none(), "whitespace token stays off");

        collectors.slack.token = "xoxp-real".to_string();
        collectors.slack.watch_user_id = "U1".to_string();
        let config = manual_slack_config(&collectors).expect("enabled + token → configured");
        assert_eq!(config.token, "xoxp-real");
        assert_eq!(config.watch_user_id, "U1");
    }

    #[test]
    fn promoted_body_carries_notes_verbatim() {
        let body = render_promoted_issue_body(Some("line one\nline two"), None);
        assert_eq!(body, "line one\nline two\n\nPromoted from tt board");
    }

    #[test]
    fn promoted_body_footer_only_when_notes_blank() {
        assert_eq!(render_promoted_issue_body(None, None), "Promoted from tt board");
        assert_eq!(render_promoted_issue_body(Some("   \n  "), None), "Promoted from tt board");
    }

    #[test]
    fn promoted_body_appends_due_date_when_set() {
        // 2026-07-15T00:00:00Z in epoch ms.
        let due_ts = 1_784_073_600_000;
        let body = render_promoted_issue_body(Some("ship it"), Some(due_ts));
        assert_eq!(body, "ship it\n\nPromoted from tt board\nDue: 2026-07-15");

        let body_no_notes = render_promoted_issue_body(None, Some(due_ts));
        assert_eq!(body_no_notes, "Promoted from tt board\nDue: 2026-07-15");
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
        store.add_task("buy milk", Some(500), None, None, 1).unwrap();
        let state = StoreState::from_option(Some(store));
        let snap = snapshot_of(&state).unwrap();
        assert_eq!(snap.tasks.len(), 1);
        assert_eq!(snap.tasks[0].text, "buy milk");
    }

    #[test]
    fn snapshot_reflects_task_edit_and_delete() {
        let store = Store::open_in_memory().unwrap();
        let a = store.add_task("draft", None, None, None, 1).unwrap();
        let b = store.add_task("scrap", None, None, None, 2).unwrap();
        store.update_task(a.id, "final", Some("done"), Some(700)).unwrap();
        store.delete_task(b.id).unwrap();
        let state = StoreState::from_option(Some(store));
        let snap = snapshot_of(&state).unwrap();
        assert_eq!(snap.tasks.len(), 1);
        assert_eq!(snap.tasks[0].text, "final");
        assert_eq!(snap.tasks[0].notes.as_deref(), Some("done"));
        assert_eq!(snap.tasks[0].due_ts, Some(700));
    }

    #[test]
    fn snapshot_of_unavailable_store_errors() {
        let state = StoreState::from_option(None);
        let err = snapshot_of(&state).unwrap_err();
        assert!(err.contains("store unavailable"), "got: {err}");
    }
}
