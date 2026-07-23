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

use tauri::{AppHandle, Emitter, Manager, State};

use tt_store::{Snapshot, Store};

/// Tauri event carrying the full store snapshot (initial mount + after every write).
pub const SNAPSHOT_EVENT: &str = "store://snapshot";

/// Hard cap per `gh` issue mutation (create/close/reopen). These run through
/// `tt_exec` rather than a bare `Command` for two reasons: an unbounded spawn
/// could wedge the caller forever on a stalled network, and `tt_exec` is the
/// single seam where every subprocess is recorded to the telemetry event log —
/// a `gh` call made outside it is invisible there.
const GH_MUTATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Managed Tauri state: the SQLite store, `None` when it could not be opened.
/// `Clone` so the git-info poll loop (`lib.rs`) can hold its own handle to
/// reconcile the tracked-repo identity cache without going through
/// `AppHandle::state`.
#[derive(Clone)]
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

    /// Reconcile the tracked-repo identity cache to exactly `repos` — see
    /// `tt_store::Store::reconcile_repos`. Best-effort: a no-op if the store
    /// never opened.
    pub fn reconcile_repos(&self, repos: &[(String, String)], now_ms: i64) {
        if let Some(store) = self.store.lock().unwrap().as_ref()
            && let Err(e) = store.reconcile_repos(repos, now_ms)
        {
            tracing::warn!(error = %e, "store: failed to reconcile tracked-repo identity cache");
        }
    }

    /// Sync every worktree-backed task's board column to whether its folder
    /// currently has a live, running agent — see
    /// `tt_agentboard::task_status::sync_worktree_task_statuses`. This is the
    /// only path that moves a card between `backlog`/`doing` now that manual
    /// drag-and-drop is gone. Returns how many rows changed (0 if the store
    /// never opened). Best-effort: a write failure just logs and leaves that
    /// row for the next tick to retry.
    pub fn sync_worktree_task_statuses(
        &self,
        payload: &tt_agentboard::StatePayload,
        now_ms: i64,
    ) -> usize {
        let guard = self.store.lock().unwrap();
        let Some(store) = guard.as_ref() else {
            return 0;
        };
        match tt_agentboard::task_status::sync_worktree_task_statuses(store, payload, now_ms) {
            Ok(changed) => changed,
            Err(e) => {
                tracing::warn!(error = %e, "store: failed to sync worktree task statuses");
                0
            }
        }
    }
}

/// Managed flag guarding against overlapping manual "refresh now" runs. A
/// manual refresh shells `gh`/Slack, which can take seconds; without this a
/// jittery double-click (or a second window) could stack redundant sweeps.
#[derive(Default)]
pub struct CollectNowState {
    running: Arc<AtomicBool>,
}

/// Managed state guarding against overlapping per-repo manual syncs — the
/// Agentboard rail's "Sync now" action. Keyed by repo dir (unlike
/// [`CollectNowState`]'s single global flag) so syncing two different repos
/// concurrently is fine; only a double-click on the same repo's action is
/// deduped.
#[derive(Default)]
pub struct RepoSyncState {
    running: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// Releases one dir from a [`RepoSyncState`]'s in-flight set when dropped, so
/// the guard clears on every exit path — including a panic.
struct ReleaseDirOnDrop(Arc<Mutex<std::collections::HashSet<String>>>, String);

impl Drop for ReleaseDirOnDrop {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(&self.1);
    }
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
pub fn now_ms() -> i64 {
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

/// Recompute and emit the snapshot given only an [`AppHandle`], for callers
/// that hold no `State` handle of their own.
///
/// The MCP HTTP transport is the one such caller: its dispatcher writes through
/// a *separate* SQLite connection, so a tool call that mutates the store would
/// otherwise leave the UI showing stale data until its next poll. Same
/// best-effort contract as [`emit_snapshot`] — a missing store or a failed emit
/// is swallowed.
pub fn emit_snapshot_from_app(app: &AppHandle) {
    let state = app.state::<StoreState>();
    emit_snapshot(app, &state);
}

/// One board task by id, for callers holding only an [`AppHandle`].
/// `Ok(None)` is "no such task" — a real answer, not a failure.
pub fn task_by_id(app: &AppHandle, id: i64) -> Result<Option<tt_store::TaskItem>, String> {
    let state = app.state::<StoreState>();
    with_store(&state, |store| store.get_task(id).map_err(|e| format!("get_task failed: {e}")))
}

/// The board task bound to a worktree dir, if any. `Ok(None)` is "no task
/// bound" — a real answer, since the rail lists worktrees the board may know
/// nothing about.
///
/// Propagates store errors rather than swallowing them, matching [`task_by_id`]
/// — the two are consumed by adjacent arms of the same match in
/// `delete_task_blocking`, and an unreadable store must not be reported as
/// "this worktree has no task": that would remove the checkout and silently
/// leave its row behind, the exact half-delete the unified path exists to stop.
pub fn task_id_for_worktree_dir(app: &AppHandle, dir: &str) -> Result<Option<i64>, String> {
    let state = app.state::<StoreState>();
    with_store(&state, |store| {
        store.task_for_worktree_dir(dir).map_err(|e| format!("task_for_worktree_dir failed: {e}"))
    })
    .map(|task| task.map(|task| task.id))
}

/// Delete one board row permanently and re-emit the snapshot.
///
/// Deliberately not a Tauri command: a row-only delete is exactly the
/// half-delete that used to strand worktrees on disk, so the only way to reach
/// it is through [`crate::task::delete_task_blocking`], which has already
/// verified no worktree is bound (`purge`) by the time it calls this.
pub fn delete_task_row(app: &AppHandle, id: i64) -> Result<(), String> {
    let state = app.state::<StoreState>();
    with_store(&state, |store| {
        store.delete_task(id).map_err(|e| format!("delete_task failed: {e}"))
    })?;
    tracing::info!(task_id = id, "task.deleted");
    emit_snapshot(app, &state);
    Ok(())
}

/// Close one board row — record how it ended, detach its worktree dir — and
/// re-emit the snapshot. What replaced [`delete_task_row`] as the normal end
/// of a task (2026-07-22).
///
/// Not a Tauri command for the same reason as its sibling: the frontend
/// reaches it only through `task_delete`, which owns the worktree half.
pub fn close_task_row(
    app: &AppHandle,
    id: i64,
    outcome: tt_store::TaskOutcome,
) -> Result<(), String> {
    let state = app.state::<StoreState>();
    let now = now_ms();
    with_store(&state, |store| {
        store.close_task(id, outcome, now).map_err(|e| format!("close_task failed: {e}"))
    })?;
    tracing::info!(task_id = id, outcome = outcome.as_str(), "task.closed");
    emit_snapshot(app, &state);
    Ok(())
}

// --- Tauri commands ---

/// Pull the current snapshot (initial mount).
#[tauri::command]
pub fn store_snapshot(state: State<StoreState>) -> Result<Snapshot, String> {
    snapshot_of(&state)
}

/// Add a manually-entered task, then re-emit the snapshot. `status` picks the
/// column it lands in (quick-add uses `backlog`; the new-task flow creates
/// worktree-backed tasks straight into `doing`).
#[tauri::command]
pub fn store_add_task(
    app: AppHandle,
    state: State<StoreState>,
    text: String,
    status: Option<String>,
) -> Result<i64, String> {
    let status = status.unwrap_or_else(|| "backlog".to_string());
    let task = with_store(&state, |store| {
        store.add_task(&text, &status, None, now_ms()).map_err(|e| format!("add_task failed: {e}"))
    })?;
    tracing::info!(task_id = task.id, %status, "task.created");
    emit_snapshot(&app, &state);
    Ok(task.id)
}

/// Attach a GitHub issue to a task, then re-emit the snapshot.
#[tauri::command]
pub fn store_attach_task_issue(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    repo: String,
    number: i64,
    url: String,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .attach_task_issue(id, &repo, number, &url)
            .map_err(|e| format!("attach_task_issue failed: {e}"))
    })?;
    tracing::info!(task_id = id, %repo, number, "task.issue_attached");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Detach a GitHub issue from a task, then re-emit the snapshot.
#[tauri::command]
pub fn store_detach_task_issue(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    repo: String,
    number: i64,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .detach_task_issue(id, &repo, number)
            .map_err(|e| format!("detach_task_issue failed: {e}"))
    })?;
    tracing::info!(task_id = id, %repo, number, "task.issue_detached");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Attach a GitHub PR to a task, then re-emit the snapshot. (PRs from the
/// task's own task branch attach automatically on collect; this is the manual
/// path for cross-repo or extra PRs.)
#[tauri::command]
pub fn store_attach_task_pr(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    repo: String,
    number: i64,
    url: String,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .attach_task_pr(id, &repo, number, &url)
            .map_err(|e| format!("attach_task_pr failed: {e}"))
    })?;
    tracing::info!(task_id = id, %repo, number, "task.pr_attached");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Detach a GitHub PR from a task, then re-emit the snapshot.
#[tauri::command]
pub fn store_detach_task_pr(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    repo: String,
    number: i64,
) -> Result<(), String> {
    with_store(&state, |store| {
        store.detach_task_pr(id, &repo, number).map_err(|e| format!("detach_task_pr failed: {e}"))
    })?;
    tracing::info!(task_id = id, %repo, number, "task.pr_detached");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Bind a task to its repo, and to the worktree its work happens in
/// once one exists, then re-emit the snapshot. The Agentboard's new-task flow
/// calls this at submit with the repo alone (`branch`/`dir` `None`) so the
/// task has a Board swimlane immediately, then again once `task_create`
/// resolves. `repo` is the GitHub `owner/name` when known — it enables PR
/// auto-attach.
#[tauri::command]
pub fn store_task_set_worktree(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    repo_root: String,
    repo: Option<String>,
    branch: Option<String>,
    dir: Option<String>,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .set_task_worktree(id, &repo_root, repo.as_deref(), branch.as_deref(), dir.as_deref())
            .map_err(|e| format!("set_task_worktree failed: {e}"))
    })?;
    tracing::info!(task_id = id, branch = branch.as_deref().unwrap_or(""), "task.worktree_bound");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Move a todo to a kanban column (backlog/doing/done), then
/// re-emit, and sync GitHub if this crosses the `done` boundary (see
/// [`spawn_gh_status_sync`]). Used by the "Move to" menu.
#[tauri::command]
pub fn store_set_task_status(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    status: String,
) -> Result<(), String> {
    let before = with_store(&state, |store| {
        let before = store.get_task(id).map_err(|e| format!("get_task failed: {e}"))?;
        store
            .set_task_status(id, &status, now_ms())
            .map_err(|e| format!("set_task_status failed: {e}"))?;
        Ok(before)
    })?;
    tracing::info!(
        task_id = id,
        from = before.as_ref().map(|b| b.status.as_str()).unwrap_or(""),
        to = %status,
        "task.status_set"
    );
    emit_snapshot(&app, &state);
    if let Some(before) = before {
        spawn_gh_status_sync(&before.status, &status, &before.issues);
    }
    Ok(())
}

/// Best-effort close/reopen the GitHub issues linked to a task whose status
/// just crossed the `done` boundary (see [`tt_store::gh_close_reopen_targets`]),
/// on a
/// background thread — fire-and-forget so the caller's snapshot emit doesn't
/// wait on the network round-trips. A failed gh call self-heals on the next
/// collector poll via [`tt_collect::rollup_task_statuses`].
///
/// The single call site for this decision: every command that can change a
/// task's status (`store_set_task_status`, `store_set_task_position`) routes
/// through here rather than each re-deriving/spawning its own sync, so the
/// close/reopen behavior can't drift between them (#246 shipped only in
/// `store_set_task_status`, so dragging a card — which goes through
/// `store_set_task_position` — silently skipped the sync). Only the
/// board-originated commands sync: the collectors' rollup writes through
/// `tt_store` directly, so a GitHub-driven status change never echoes back
/// out as a gh mutation.
fn spawn_gh_status_sync(old_status: &str, new_status: &str, issues: &[tt_store::TaskIssueLink]) {
    let targets = tt_store::gh_close_reopen_targets(old_status, new_status, issues);
    if targets.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for (repo, number, close) in targets {
            let verb = if close { "close" } else { "reopen" };
            let result =
                if close { close_gh_issue(&repo, number) } else { reopen_gh_issue(&repo, number) };
            match result {
                Ok(()) => tracing::info!(%repo, number, verb, "task.gh_issue_sync"),
                Err(e) => eprintln!("gh issue {verb} sync failed for {repo}#{number}: {e}"),
            }
        }
    });
}

/// Run `gh issue close --repo <repo> <number>`.
fn close_gh_issue(repo: &str, number: i64) -> Result<(), String> {
    run_gh_issue_state_change(repo, number, "close")
}

/// Run `gh issue reopen --repo <repo> <number>`.
fn reopen_gh_issue(repo: &str, number: i64) -> Result<(), String> {
    run_gh_issue_state_change(repo, number, "reopen")
}

fn run_gh_issue_state_change(repo: &str, number: i64, verb: &str) -> Result<(), String> {
    let output = tt_exec::run_with_timeout(
        "gh",
        &["issue", verb, "--repo", repo, &number.to_string()],
        GH_MUTATION_TIMEOUT,
    )
    .map_err(|e| format!("failed to run gh: {e}"))?;
    if !output.ok() {
        return Err(format!("gh issue {verb} failed: {}", output.stderr.trim()));
    }
    Ok(())
}

/// Move a todo to `status` at an explicit task (`index`) within that column,
/// renumbering the column's positions — powers drag-to-reorder and
/// position-aware cross-column drops. Then re-emit the snapshot and sync
/// GitHub if this crosses the `done` boundary (see [`spawn_gh_status_sync`]).
#[tauri::command]
pub fn store_set_task_position(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    status: String,
    index: i64,
) -> Result<(), String> {
    let before = with_store(&state, |store| {
        let before = store.get_task(id).map_err(|e| format!("get_task failed: {e}"))?;
        store
            .set_task_position(id, &status, index, now_ms())
            .map_err(|e| format!("set_task_position failed: {e}"))?;
        Ok(before)
    })?;
    tracing::info!(task_id = id, %status, index, "task.position_set");
    emit_snapshot(&app, &state);
    if let Some(before) = before {
        spawn_gh_status_sync(&before.status, &status, &before.issues);
    }
    Ok(())
}

/// Edit a todo's text and notes (a full replace of both fields — `null`
/// clears notes), then re-emit the snapshot.
#[tauri::command]
pub fn store_update_task(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
    text: String,
    notes: Option<String>,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .update_task(id, &text, notes.as_deref())
            .map(|_| ())
            .map_err(|e| format!("update_task failed: {e}"))
    })?;
    tracing::info!(task_id = id, "task.updated");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Archive finished todos older than [`tt_store::ARCHIVE_AFTER_MS`] off the
/// active board, then re-emit the snapshot — the "Archive done" button. Rows
/// are hidden, never deleted (the collector-side sweep in `tt-collect` does
/// the same on its own cadence; this is the don't-wait affordance). The
/// cutoff is derived from the wall clock here, at the command boundary — the
/// store takes plain instants. Returns the number of todos archived.
#[tauri::command]
pub fn store_archive_done(app: AppHandle, state: State<StoreState>) -> Result<usize, String> {
    let now = now_ms();
    let archived = with_store(&state, |store| {
        store
            .archive_closed_tasks(now - tt_store::ARCHIVE_AFTER_MS, now)
            .map_err(|e| format!("archive_closed_tasks failed: {e}"))
    })?;
    tracing::info!(count = archived, "task.done_archived");
    emit_snapshot(&app, &state);
    Ok(archived)
}

/// Bring one archived task back onto the board, then re-emit the snapshot —
/// the card's "Restore" action.
#[tauri::command]
pub fn store_unarchive_task(
    app: AppHandle,
    state: State<StoreState>,
    id: i64,
) -> Result<(), String> {
    with_store(&state, |store| {
        store.unarchive_task(id).map_err(|e| format!("unarchive_task failed: {e}"))
    })?;
    tracing::info!(task_id = id, "task.unarchived");
    emit_snapshot(&app, &state);
    Ok(())
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
    tracing::info!(%channel, "dm.dismissed");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Dismiss one GitHub item (`kind` is `"issue"` or `"pr"`) at `(repo, number)` —
/// it drops out of the attention feed until the collector observes a newer
/// `updatedTs` than the one passed in (see `tt_store::IssueItem::dismissed_ts`).
#[tauri::command]
pub fn store_item_dismiss(
    app: AppHandle,
    state: State<StoreState>,
    kind: String,
    repo: String,
    number: i64,
    updated_ts: i64,
) -> Result<(), String> {
    with_store(&state, |store| {
        store
            .dismiss_item(&kind, &repo, number, updated_ts)
            .map_err(|e| format!("dismiss_item failed: {e}"))
    })?;
    tracing::info!(%kind, %repo, number, "item.dismissed");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Clear every dismissed issue/PR at once — the "clear all dismissals" action.
#[tauri::command]
pub fn store_dismissals_clear(app: AppHandle, state: State<StoreState>) -> Result<usize, String> {
    let count = with_store(&state, |store| {
        store.clear_dismissals().map_err(|e| format!("clear_dismissals failed: {e}"))
    })?;
    tracing::info!(count, "items.dismissals_cleared");
    emit_snapshot(&app, &state);
    Ok(count)
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
        Ok((task.text, render_promoted_issue_body(task.notes.as_deref())))
    })?;

    let gh_repo = repo.clone();
    let (number, url) =
        tauri::async_runtime::spawn_blocking(move || create_gh_issue(&gh_repo, &title, &body))
            .await
            .map_err(|e| format!("gh issue create task failed: {e}"))??;

    with_store(&state, |store| {
        store
            .attach_task_issue(id, &repo, number, &url)
            .map_err(|e| format!("attach_task_issue failed: {e}"))
    })?;
    tracing::info!(task_id = id, %repo, number, "task.promoted_to_issue");
    emit_snapshot(&app, &state);
    Ok(())
}

/// Open issues in the repo checked out at `dir`, for the new-task flow's
/// issue picker: `assigned_to_me` toggles `--assignee @me`. Read-only — no
/// store write.
#[tauri::command]
pub async fn store_gh_issues_list(
    dir: String,
    assigned_to_me: bool,
) -> Result<Vec<tt_store::IssueInput>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        tt_collect::fetch_importable_issues(std::path::Path::new(&dir), assigned_to_me)
    })
    .await
    .map_err(|e| format!("gh issues list task failed: {e}"))?
}

/// Search issues in the repo checked out at `dir` for the attach-to-task
/// flow's picker: `gh issue list --search <query>` across every state, so a
/// task can be linked to any existing issue — not just the open, assigned
/// ones [`store_gh_issues_list`] returns. Read-only — no store write. A blank
/// query returns an empty list without shelling out.
#[tauri::command]
pub async fn store_search_issues(
    dir: String,
    query: String,
) -> Result<Vec<tt_store::IssueInput>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        tt_collect::search_repo_issues(std::path::Path::new(&dir), &query)
    })
    .await
    .map_err(|e| format!("gh issues search task failed: {e}"))?
}

/// Run `gh issue create` and return the new issue's `(number, url)`.
fn create_gh_issue(repo: &str, title: &str, body: &str) -> Result<(i64, String), String> {
    let output = tt_exec::run_with_timeout(
        "gh",
        &[
            "issue", "create", "--repo", repo, "--title", title, "--body", body,
        ],
        GH_MUTATION_TIMEOUT,
    )
    .map_err(|e| format!("failed to run gh: {e}"))?;
    parse_gh_issue_create_output(&output)
}

/// Render the GitHub issue body for a todo promoted from the tt board: the
/// todo's `notes` verbatim (dropped when blank) and a footer marking the
/// origin. Pure and unit-testable.
fn render_promoted_issue_body(notes: Option<&str>) -> String {
    let mut body = String::new();
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        body.push_str(notes);
        body.push_str("\n\n");
    }
    body.push_str("Promoted from tt board");
    body
}

/// Parse a `gh issue create` invocation's output into `(number, url)`. `gh`
/// prints the new issue URL on stdout; the trailing path segment is its number.
fn parse_gh_issue_create_output(output: &tt_exec::Output) -> Result<(i64, String), String> {
    if !output.ok() {
        return Err(format!("gh issue create failed: {}", output.stderr.trim()));
    }
    let url = output.stdout.trim().to_string();
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
    tracing::info!("journal.logged");
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
        tracing::info!(outcome = "already_running", "collect.manual");
        return Ok(CollectNowResult { started: false });
    }
    tracing::info!(outcome = "started", "collect.manual");
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

/// What a `store_sync_repo` call did. `started` is `false` when a sync for
/// this dir was already in flight and this call was a deduped no-op — the
/// frontend should treat that quietly, not as a result to report. Otherwise
/// `ok`/`count`/`message` mirror the combined issues+PRs collector outcome:
/// `ok` is `true` only when both succeeded, `count` is the combined row
/// count written, and `message` carries the first failure's detail.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSyncResult {
    pub started: bool,
    pub ok: bool,
    pub count: usize,
    pub message: Option<String>,
}

/// Manually sync one repo's issues + PRs right now, bypassing the poll
/// cadence — the Agentboard rail's "Sync now" action, for pulling in GitHub
/// updates the scheduler hasn't picked up yet. Re-emits the snapshot on
/// completion. Overlap-guarded per dir: a sync already running for `dir`
/// returns `started: false` without starting another; syncing a different dir
/// concurrently is unaffected.
///
/// Runs on a blocking worker with its own store connection (mirroring
/// [`store_collect_now`]) so the `gh` round-trip never holds the UI's store
/// mutex.
#[tauri::command]
pub async fn store_sync_repo(
    app: AppHandle,
    sync: State<'_, RepoSyncState>,
    dir: String,
) -> Result<RepoSyncResult, String> {
    let running = sync.running.clone();
    {
        let mut guard = running.lock().unwrap();
        if !guard.insert(dir.clone()) {
            tracing::info!(%dir, outcome = "already_running", "repo.synced");
            return Ok(RepoSyncResult { started: false, ok: true, count: 0, message: None });
        }
    }
    tracing::info!(%dir, outcome = "started", "repo.synced");
    tauri::async_runtime::spawn_blocking(move || {
        let _release = ReleaseDirOnDrop(running, dir.clone());
        run_sync_repo_blocking(&app, &dir)
    })
    .await
    .map_err(|e| format!("repo sync worker failed: {e}"))
}

/// Open a fresh store, sync one repo's issues + PRs, emit the resulting
/// snapshot, and summarize the outcome for the caller.
fn run_sync_repo_blocking(app: &AppHandle, dir: &str) -> RepoSyncResult {
    let store = match Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            let msg = format!("store unavailable: {e}");
            eprintln!("repo-sync: {msg}");
            return RepoSyncResult { started: true, ok: false, count: 0, message: Some(msg) };
        }
    };
    let summaries = tt_collect::collect_repo_now(&store, std::path::Path::new(dir), now_ms());
    let ok = summaries.iter().all(|s| s.ok);
    let count = summaries.iter().map(|s| s.count).sum();
    let message = summaries.iter().find(|s| !s.ok).and_then(|s| s.message.clone());
    if !ok {
        eprintln!("repo-sync: sync failed for {dir}: {}", message.as_deref().unwrap_or("unknown"));
    }
    if let Ok(snapshot) = store.snapshot() {
        let _ = app.emit(SNAPSHOT_EVENT, snapshot);
    }
    RepoSyncResult { started: true, ok, count, message }
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
        let body = render_promoted_issue_body(Some("line one\nline two"));
        assert_eq!(body, "line one\nline two\n\nPromoted from tt board");
    }

    #[test]
    fn promoted_body_footer_only_when_notes_blank() {
        assert_eq!(render_promoted_issue_body(None), "Promoted from tt board");
        assert_eq!(render_promoted_issue_body(Some("   \n  ")), "Promoted from tt board");
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
        store.add_task("buy milk", "backlog", None, 1).unwrap();
        let state = StoreState::from_option(Some(store));
        let snap = snapshot_of(&state).unwrap();
        assert_eq!(snap.tasks.len(), 1);
        assert_eq!(snap.tasks[0].text, "buy milk");
    }

    #[test]
    fn snapshot_reflects_task_edit_and_delete() {
        let store = Store::open_in_memory().unwrap();
        let a = store.add_task("draft", "backlog", None, 1).unwrap();
        let b = store.add_task("scrap", "backlog", None, 2).unwrap();
        store.update_task(a.id, "final", Some("done")).unwrap();
        store.delete_task(b.id).unwrap();
        let state = StoreState::from_option(Some(store));
        let snap = snapshot_of(&state).unwrap();
        assert_eq!(snap.tasks.len(), 1);
        assert_eq!(snap.tasks[0].text, "final");
        assert_eq!(snap.tasks[0].notes.as_deref(), Some("done"));
    }

    #[test]
    fn snapshot_of_unavailable_store_errors() {
        let state = StoreState::from_option(None);
        let err = snapshot_of(&state).unwrap_err();
        assert!(err.contains("store unavailable"), "got: {err}");
    }
}
