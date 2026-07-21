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

/// Detach any task bound to a removed worktree: clears the task's
/// `worktree_dir` (branch + repo root stay as historical fact), re-emitting
/// the snapshot when something changed. The worktree-removal seam calls this
/// right where it already untracks the dir from repos.json. Returns how many
/// tasks were detached; a worktree with no task is a 0-count no-op.
pub fn detach_task_worktree_dir(app: &AppHandle, dir: &str) -> usize {
    let state = app.state::<StoreState>();
    let detached = with_store(&state, |store| {
        store
            .clear_task_worktree_dir(dir)
            .map_err(|e| format!("clear_task_worktree_dir failed: {e}"))
    })
    .unwrap_or_else(|e| {
        eprintln!("worktree detach for {dir} failed: {e}");
        0
    });
    if detached > 0 {
        tracing::info!(%dir, count = detached, "task.worktree_detached");
        emit_snapshot(app, &state);
    }
    detached
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

/// Move a todo to a kanban column (backlog/next/doing/review/done), then
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
/// just crossed the `done` boundary (see [`gh_close_reopen_targets`]), on a
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
    let targets = gh_close_reopen_targets(old_status, new_status, issues);
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

/// Which gh actions a task's status change should trigger: entering `done`
/// closes every linked issue still cached `open`; leaving `done` reopens the
/// ones cached `closed`. Empty for link-less tasks, moves that don't touch
/// `done`, and links already in the target state (so re-running is a no-op
/// and a half-failed batch converges on retry).
fn gh_close_reopen_targets(
    old_status: &str,
    new_status: &str,
    issues: &[tt_store::TaskIssueLink],
) -> Vec<(String, i64, bool)> {
    if old_status == new_status {
        return Vec::new();
    }
    let close = if new_status == "done" {
        true
    } else if old_status == "done" {
        false
    } else {
        return Vec::new();
    };
    issues
        .iter()
        .filter(|link| if close { link.state != "closed" } else { link.state == "closed" })
        .map(|link| (link.repo.clone(), link.number, close))
        .collect()
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

/// Delete a todo permanently, then re-emit the snapshot.
#[tauri::command]
pub fn store_delete_task(app: AppHandle, state: State<StoreState>, id: i64) -> Result<(), String> {
    with_store(&state, |store| {
        store.delete_task(id).map_err(|e| format!("delete_task failed: {e}"))
    })?;
    tracing::info!(task_id = id, "task.deleted");
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
    tracing::info!(count = deleted, "task.done_cleared");
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
    tracing::info!(%channel, "dm.dismissed");
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
        let output = tt_exec::run_in_dir_with_timeout(
            "gh",
            &["issue", "create", "--title", &title, "--body", ""],
            std::path::Path::new(&dir),
            GH_MUTATION_TIMEOUT,
        )
        .map_err(|e| format!("failed to run gh in {dir}: {e}"))?;
        let (number, url) = parse_gh_issue_create_output(&output)?;
        tracing::info!(%dir, number, "issue.created");
        Ok(url)
    })
    .await
    .map_err(|e| format!("gh issue create task failed: {e}"))?
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
    fn gh_close_reopen_targets_only_fire_on_a_done_crossing() {
        let link = |number: i64, state: &str| tt_store::TaskIssueLink {
            repo: "o/r".to_string(),
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            state: state.to_string(),
        };
        // Entering done closes every still-open link; already-closed ones are
        // skipped so a retry after a half-failed batch converges.
        assert_eq!(
            gh_close_reopen_targets("backlog", "done", &[link(7, "open"), link(8, "closed")]),
            vec![("o/r".to_string(), 7, true)]
        );
        // Leaving done reopens only the closed ones.
        assert_eq!(
            gh_close_reopen_targets("done", "backlog", &[link(7, "open"), link(8, "closed")]),
            vec![("o/r".to_string(), 8, false)]
        );
        // A move that never touches done, or a link-less task, is a no-op.
        assert!(gh_close_reopen_targets("backlog", "doing", &[link(7, "open")]).is_empty());
        assert!(gh_close_reopen_targets("backlog", "done", &[]).is_empty());
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
