//! `task_*` commands: worktree-task creation/removal from the app
//! (Agentboard's new-task modal and the rail's delete-worktree action). Thin
//! over `tt_tasks::ops`, which is shared with the `tt task` CLI — the app
//! never reimplements task logic.

use base64::Engine as _;
use serde::Serialize;
use std::path::PathBuf;
use tauri::Manager;

use tt_tasks::guards::RmBlocked;
use tt_tasks::ops::{self, CreateOpts, RemoveOpts};
use tt_tasks::pasted::{self, PastedImage};
use tt_tasks::suggest::Suggested;

/// Fire-and-forget `git fetch` across every tracked repo (deduped, see
/// [`tt_agentboard::git_info::fetch_all`]), then nudge the rail to re-emit.
/// Task lifecycle events (create/remove) are a natural moment to check
/// whether main has moved elsewhere in the fleet too — cheaper than waiting
/// out the periodic poll in `lib.rs`, and kept off the command's own
/// response path so a slow/offline fetch never delays create/remove.
fn refresh_all_git_info_in_background(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ab = app.state::<crate::agentboard::Ab>();
        let targets: Vec<String> =
            ab.engine.lock().unwrap().git_targets().into_iter().map(|(dir, _, _)| dir).collect();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            tt_agentboard::git_info::fetch_all(&targets);
        })
        .await;
        ab.emit.notify_one();
    });
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreated {
    pub name: String,
    pub dir: String,
    pub branch: String,
    pub base: String,
    /// The ref the task effectively branched from (`ops::CreatedTask::base_label`)
    /// — what the dynamic-flow prompt names as its rebase/merge target.
    pub base_label: String,
    pub warnings: Vec<String>,
}

/// Branches available as a base ref in the task root containing `root`
/// (a checkout dir or the root itself), default branch first. See
/// [`ops::BaseBranch`] for the name-vs-label split the form renders.
#[tauri::command]
pub fn task_base_branches(root: String) -> Result<Vec<ops::BaseBranch>, String> {
    let sr = ops::discover_root(Some(&PathBuf::from(root))).map_err(|e| e.to_string())?;
    ops::checkout_branches(&sr.checkout).map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchCheck {
    pub name: Option<String>,
    pub taken: bool,
    pub branch_exists: bool,
    pub error: Option<String>,
}

/// Preflight the new-task dialog's branch field: is it a legal git ref, does
/// it already exist in git (the case `git worktree add` would otherwise
/// reject only after the fetch/worktree-add work has started), and would its
/// derived task name collide with an existing one? Read-only — safe to call
/// on every keystroke (debounced by the caller).
#[tauri::command]
pub fn task_check_branch(root: String, branch: String) -> Result<BranchCheck, String> {
    let sr = ops::discover_root(Some(&PathBuf::from(root))).map_err(|e| e.to_string())?;
    let check = ops::check_branch(&sr, branch.trim());
    Ok(BranchCheck {
        name: check.name,
        taken: check.taken,
        branch_exists: check.branch_exists,
        error: check.error,
    })
}

/// A **prompt improver** button in the new-task dialog: ask `claude -p` (cwd =
/// `dir`, the repo checkout the dialog is open for, so it sees real repo
/// context) to rewrite the text the user typed and propose a branch name for
/// it. `instruction` is the clicked improver's prompt from settings — it
/// decides *how* the goal is rewritten (restate it, turn it into a plan ask, a
/// brainstorm ask); empty falls back to the historic restate-in-one-sentence
/// behavior. The dialog fills its editable fields with the result (Undo
/// restores) — nothing here writes anything or is called automatically.
/// Long-running (a cold `claude` CLI) → off the main thread.
///
/// Returns `tt_tasks::Suggested`, which serializes flat as
/// `{branch, goal, fallback}`.
#[tauri::command]
pub async fn task_suggest(
    dir: String,
    goal: String,
    image_paths: Vec<String>,
    instruction: Option<String>,
) -> Result<Suggested, String> {
    let images = image_paths.len();
    let instruction = instruction.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        tt_tasks::suggest(&PathBuf::from(dir), &goal, &image_paths, &instruction)
    })
    .await
    .map_err(|e| format!("worktree task failed: {e}"))?
    .map_err(|e| e.to_string());
    // A hard failure stays at `warn` — merging the two log sites must not cost
    // the severity an operator filters on.
    match &result {
        Ok(s) => {
            let outcome = if s.fallback.is_some() { "fallback" } else { "ok" };
            tracing::info!(
                images,
                outcome,
                reason = s.fallback.as_deref().unwrap_or(""),
                "task_suggest"
            );
        }
        Err(e) => tracing::warn!(images, outcome = "error", reason = e.as_str(), "task_suggest"),
    }
    result
}

/// Create the task for `branch` off `base` (empty base = the primary's
/// branch): fetch, worktree add, render `.env`, inherit sibling secrets.
/// Deliberately **not** the install setup step (`TT_TASK_SETUP`, e.g. `npm
/// install`) — that alone can run for minutes (npm's per-file cost on macOS
/// APFS + Gatekeeper scanning is far higher than on Linux), and this command
/// gates the frontend's terminal pane. The caller fires `task_run_setup`
/// separately, after the pane is already open, so the pane and the install
/// aren't sequential: `frontend::createTask` in `agentboard.tsx` is the one
/// call site. Still off the main thread — `git fetch`/`worktree add` are real
/// subprocess work even without the install.
#[tauri::command]
pub async fn task_create(
    app: tauri::AppHandle,
    root: String,
    branch: String,
    base: String,
) -> Result<TaskCreated, String> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        return Err("a task needs a branch — tasks are named after their branch".to_string());
    }
    let opts = CreateOpts {
        root: Some(PathBuf::from(root)),
        branch,
        base: {
            let b = base.trim();
            (!b.is_empty()).then(|| b.to_string())
        },
        run_setup: false,
    };
    let created = tauri::async_runtime::spawn_blocking(move || ops::create_task(&opts))
        .await
        .map_err(|e| format!("worktree task failed: {e}"))?
        .map_err(|e| e.to_string())?;
    tracing::info!(
        name = %created.name,
        branch = %created.branch,
        base = %created.base_label,
        warnings = created.warnings.len(),
        "task.created"
    );
    refresh_all_git_info_in_background(&app);
    Ok(TaskCreated {
        name: created.name,
        dir: created.dir.to_string_lossy().to_string(),
        branch: created.branch,
        base: created.base,
        base_label: created.base_label,
        warnings: created.warnings,
    })
}

/// The system clipboard's image, as a base64 PNG the webview can preview and
/// hand back to `task_write_pasted_images`. `Ok(None)` = the clipboard holds
/// no image (text, or nothing) — an ordinary outcome, not an error.
///
/// This exists because **the DOM can't see an image paste on Linux**: a
/// Ctrl+V there doesn't reach the webview's `paste` event at all (the same
/// behavior `terminal-view.tsx` documents, where Ctrl+V arrives as a plain
/// `keydown` that `encodeKey` turns into `\x16`). So the form drives image
/// attachment off `keydown` and reads the clipboard here instead — the same
/// native-clipboard workaround `term_copy_selection` uses for writes.
///
/// **Must not run on the main thread** — `read_image()` warns that the
/// underlying Linux clipboard libraries can deadlock the whole app there
/// (and on Linux, sync Tauri commands dispatch inline on the GTK thread).
#[tauri::command]
pub async fn read_clipboard_image(app: tauri::AppHandle) -> Result<Option<PastedImage>, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    tauri::async_runtime::spawn_blocking(move || {
        // An empty/non-image clipboard surfaces as an error from the plugin;
        // that's the common case here, so it maps to `None` rather than
        // bubbling up as a failure the user has to read.
        let Ok(image) = app.clipboard().read_image() else {
            return Ok(None);
        };
        let rgba = image.rgba();
        let png =
            pasted::rgba_to_png(image.width(), image.height(), rgba).map_err(|e| e.to_string())?;
        if png.len() > pasted::MAX_IMAGE_BYTES {
            return Err(format!(
                "clipboard image is {} bytes, over the {}-byte limit",
                png.len(),
                pasted::MAX_IMAGE_BYTES
            ));
        }
        Ok(Some(PastedImage {
            mime: "image/png".to_string(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        }))
    })
    .await
    .map_err(|e| format!("clipboard task failed: {e}"))?
}

/// Stage the images pasted into the new-task form as files, returning their
/// absolute paths for the caller to name in Claude's opening prompt. They
/// land in `tt_config::pasted_images_dir()`, *not* in the repo — see
/// `tt_tasks::pasted` for why (short version: Claude Code reads an
/// out-of-workspace path without prompting, so there's nothing to gain from
/// writing user content into a checkout).
///
/// Called before `task_create`, so a failure here means no task was created
/// and the caller's normal retry path still applies.
///
/// Decoding + writing a handful of megabytes → off the main thread, which on
/// Linux is the GTK thread every other sync command dispatches on.
#[tauri::command]
pub async fn task_write_pasted_images(
    repo: String,
    branch: String,
    images: Vec<PastedImage>,
) -> Result<Vec<String>, String> {
    let base = tt_config::pasted_images_dir();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    tauri::async_runtime::spawn_blocking(move || {
        let scope = pasted::scope_name(&repo, &branch);
        pasted::write_images(&base, &scope, &images, now_ms)
    })
    .await
    .map_err(|e| format!("worktree task failed: {e}"))?
    .map(|paths| paths.iter().map(|p| p.to_string_lossy().to_string()).collect())
    .map_err(|e| e.to_string())
}

/// Re-run a checkout's setup step (declared `TT_TASK_SETUP` or lockfile
/// detection) — the retry affordance for a setup failure surfaced from
/// `task_create`. `Ok(None)` = nothing to run or it succeeded this time;
/// `Ok(Some)` carries the same warning text `task_create` would have shown.
/// Long-running (an install can take a minute) → off the main thread.
#[tauri::command]
pub async fn task_run_setup(dir: String) -> Result<Option<String>, String> {
    tracing::info!(%dir, "task.setup_rerun");
    tauri::async_runtime::spawn_blocking(move || ops::run_setup(&PathBuf::from(dir)))
        .await
        .map_err(|e| format!("worktree task failed: {e}"))?
        .map_err(|e| e.to_string())
}

/// The wire form of [`ops::RemoveOutcome`] — see its doc for why a guard
/// refusal is an `Ok` variant rather than an error. Serialized as a tagged
/// union so the frontend gets real narrowing on `status`.
#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TaskDeleteOutcome {
    Deleted {
        name: String,
        messages: Vec<String>,
    },
    Blocked {
        name: String,
        blockers: Vec<Blocker>,
        /// Caveats gathered before the verdict — carried for the same reason
        /// `Removed` carries them. A refusal computed against stale refs (the
        /// pre-flight `fetch --prune` failed) must not look identical to one
        /// computed online: "commits unreachable from any branch/remote" can
        /// be an artifact of the staleness rather than a fact about the
        /// branch.
        messages: Vec<String>,
    },
}

/// One reason a removal was refused, with everything the UI needs to render
/// it as an actionable row.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    /// Stable discriminant (`dirtyTree` / `unreachableCommits` /
    /// `foreignPort`) — the UI branches on this, never on message text.
    pub kind: String,
    /// What's wrong. Already names the port's holder where there is one, so
    /// there's no separate holder field — the UI renders these two strings
    /// and nothing else.
    pub message: String,
    /// What to do about it.
    pub remedy: String,
    /// Whether forcing past this destroys work that exists nowhere else.
    pub loses_work: bool,
    /// Set for `foreignPort` — the argument to `task_stop_port`.
    pub port: Option<u16>,
}

impl From<&RmBlocked> for Blocker {
    fn from(blocked: &RmBlocked) -> Self {
        Blocker {
            kind: blocked.kind().to_string(),
            message: blocked.to_string(),
            remedy: blocked.remedy(),
            loses_work: blocked.loses_work(),
            port: blocked.port(),
        }
    }
}

/// What to delete. Both forms resolve to the same pair — a board row and the
/// worktree bound to it — before anything is touched, so "delete the task I
/// clicked on the Board" and "delete the worktree I clicked on the rail" are
/// one operation reached through two handles, not two behaviors that can drift
/// apart. Either half may be absent: a board task can exist with no worktree
/// (the common case for a note-shaped todo), and a worktree discovered on disk
/// may have no board row.
#[derive(Debug, Clone)]
pub enum DeleteTarget {
    /// A board task id — the Board screen and the `task_delete` MCP tool.
    Board(i64),
    /// A worktree directory — the Agentboard rail, which lists worktrees found
    /// on disk whether or not the board knows about them.
    Worktree(String),
}

/// What a [`DeleteTarget`] actually names, resolved once before anything
/// destructive runs so a target that doesn't exist fails while it's still free
/// to fail.
struct Resolved {
    /// The board row, when the target named one. `None` for a rail-initiated
    /// delete, which knows only a directory — there the row is found by the dir
    /// it is bound to (`BoardRows`), and taking it with the worktree is the
    /// whole point of #339's "the worktree is an attribute of the task".
    board_id: Option<i64>,
    /// The worktree bound to it, if any. Present even when the directory has
    /// since vanished — the bindings still need tearing down.
    dir: Option<String>,
    /// What to call this in messages and toasts.
    label: String,
}

fn resolve_delete_target(app: &tauri::AppHandle, target: DeleteTarget) -> Result<Resolved, String> {
    match target {
        DeleteTarget::Board(id) => {
            let task =
                crate::store::task_by_id(app, id)?.ok_or_else(|| format!("no board task #{id}"))?;
            let dir = task.worktree.as_ref().and_then(|w| w.dir.clone());
            Ok(Resolved { board_id: Some(id), dir, label: task.text })
        }
        DeleteTarget::Worktree(dir) => {
            let label = PathBuf::from(&dir)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&dir)
                .to_string();
            Ok(Resolved { board_id: None, dir: Some(dir), label })
        }
    }
}

/// Delete a task outright: its live panes, its worktree on disk, and its board
/// row — the three things that together *are* the task — refusing the whole
/// operation if the worktree still holds work that exists nowhere else.
///
/// This is the single delete path. The Board screen, the Agentboard rail, and
/// the `task_delete` MCP tool all land here, because a delete that took only
/// some of the three left the other parts as garbage: deleting the row alone
/// orphaned a worktree on disk with nothing left in the UI to remove it from,
/// and removing the worktree alone left a board row pointing at a directory
/// that no longer existed.
///
/// **The board row is deleted last, and only if the worktree really went.**
/// A guarded refusal returns [`TaskDeleteOutcome::Blocked`] with the row and
/// the worktree both untouched, so the user can act on the blocker and retry
/// from exactly where they were — a dirty tree, commits
/// unreachable from any branch/remote, or a foreign listener on the task's
/// claimed ports come back as a typed blocker per reason, which the app
/// renders as a dialog offering each one's remedy (stop the port's process via
/// [`task_stop_port`]) plus a force.
/// `force: true` is that force — it skips every guard, so it discards
/// uncommitted changes and unreachable commits for good; only call it behind
/// an explicit confirmation that names what's being lost.
/// Once the guards have passed and the removal is really happening — via
/// `ops::remove_task`'s `before_removal` hook, so a *refused* removal never
/// costs a live session — kills the folder's live PTYs (SIGHUPing any Claude
/// Code session running inside, then SIGKILLing anything else still sharing
/// the shell's session — see `terminal::kill_session_stragglers`) so a
/// backgrounded job that dodged the shell's own signal forwarding doesn't
/// survive as an orphan on a deleted cwd. A side effect of killing only past
/// the guards: a dev server running in the task's own pane now surfaces as a
/// `foreignPort` blocker (one "Stop it" click) instead of dying silently
/// mid-removal. Does NOT drop the session/window/pane records yet — those
/// are only removed (and persisted) once `ops::remove_task` has actually
/// succeeded: dropping them up front used to leave the rail looking like a
/// clean removal — panes gone — while a blocked or failed removal left the
/// worktree sitting on disk forever with nothing left on the rail to retry
/// from. On failure the killed-but-still-tracked sessions surface as dead
/// panes so the user can see the task is still there. Then cleans up docker
/// resources, the worktree registration, and the task's agentboard tracking.
///
/// Blocking, not `async`: every step is a subprocess or a lock, and the two
/// callers already have a blocking thread to spend — the Tauri command below
/// wraps it in `spawn_blocking`, and the MCP host runs inside the transport's
/// existing `spawn_blocking`. Making it `async` would only add a second way to
/// reach the same synchronous work.
pub fn delete_task_blocking(
    app: &tauri::AppHandle,
    target: DeleteTarget,
    force: bool,
) -> Result<TaskDeleteOutcome, String> {
    let Resolved { board_id, dir, label } = resolve_delete_target(app, target)?;

    // No worktree: nothing to guard and nothing bound, so the row is the whole
    // task and goes on its own. Everything else runs the shared sequence.
    let Some(dir) = dir.as_deref() else {
        let mut messages = Vec::new();
        if let Some(id) = board_id {
            crate::store::delete_task_row(app, id)?;
            messages.push("deleted the board task".to_string());
        }
        return Ok(TaskDeleteOutcome::Deleted { name: label, messages });
    };

    // `resolve_task` runs here rather than inside the sequence: it is what
    // rejects a path that isn't a worktree of its own checkout, and the
    // caller-supplied `dir` has not been checked yet.
    //
    // A directory that is already gone can't be resolved, so `root` stays
    // `None` — which is safe **only** because `MissingDir::TearDownBindings`
    // makes the sequence skip the removal step entirely for a missing dir. Were
    // it to call `ops::remove_task` with `root: None`, that would re-discover a
    // root by walking up from this *process's* cwd — a different checkout — and
    // could remove a same-named worktree there.
    let (root, name) = match resolve_task(dir) {
        Ok((checkout, name)) => (Some(checkout), name),
        Err(_) if !std::path::Path::new(dir).is_dir() => (None, task_name_from_dir(dir)),
        Err(error) => return Err(error),
    };
    let opts = RemoveOpts { root, name, force };
    let mut hooks = AppRemovalHooks { app, dir };
    let rows = AppBoardRows { app, board_id };
    let removal = tt_agentboard::task_removal::TaskRemoval {
        opts: &opts,
        dir: std::path::Path::new(dir),
        repos_path: &tt_agentboard::repos::default_repos_path(),
        rows: Some(&rows),
        // The dir came out of the app's own store or the rail, never a typed
        // name, so a missing one means the record outlived the checkout — the
        // record is exactly what still needs clearing.
        on_missing: tt_agentboard::task_removal::MissingDir::TearDownBindings,
    };

    let outcome = tt_agentboard::task_removal::remove_task_and_bindings(removal, &mut hooks)
        .map_err(|e| e.to_string())?;

    match outcome {
        tt_agentboard::task_removal::Outcome::Removed { messages, .. } => {
            // Re-emit either way: a fleet-discovered (never-tracked) task also
            // drops off the rail on the next recompute, so don't make the user
            // wait a poll.
            app.state::<crate::agentboard::Ab>().emit.notify_one();
            refresh_all_git_info_in_background(app);
            Ok(TaskDeleteOutcome::Deleted { name: label, messages })
        }
        // A refusal ends here: nothing was removed — not the worktree, not the
        // panes, not the row — so the user can act on the blocker and retry
        // from exactly where they were.
        tt_agentboard::task_removal::Outcome::Blocked { name, blocked, messages } => {
            Ok(TaskDeleteOutcome::Blocked {
                name,
                blockers: blocked.iter().map(Blocker::from).collect(),
                messages,
            })
        }
    }
}

/// The task name for a worktree whose directory is already gone, so
/// `resolve_task` can't read it off the checkout. The folder basename is the
/// task name by construction (it is the slugged branch).
fn task_name_from_dir(dir: &str) -> String {
    PathBuf::from(dir).file_name().and_then(|n| n.to_str()).unwrap_or(dir).to_string()
}

/// The app's half of the removal sequence: the two steps that need the live
/// process, which is exactly why they are hooks rather than shared code.
struct AppRemovalHooks<'a> {
    app: &'a tauri::AppHandle,
    dir: &'a str,
}

impl tt_agentboard::task_removal::RemovalHooks for AppRemovalHooks<'_> {
    fn before_removal(&mut self) {
        // Kills the folder's live PTYs. Locks are scoped tight per this crate's
        // rule: never hold the engine lock across a subprocess.
        let ids = {
            let ab = self.app.state::<crate::agentboard::Ab>();
            let engine = ab.engine.lock().unwrap();
            engine.session_ids_for(self.dir)
        };
        if !ids.is_empty() {
            let term_state = self.app.state::<crate::terminal::TermState>();
            for id in &ids {
                term_state.kill(id);
            }
        }
    }

    fn after_removal(&mut self, _dir: &std::path::Path) -> Vec<String> {
        let mut notes = Vec::new();
        let ab = self.app.state::<crate::agentboard::Ab>();
        let mut engine = ab.engine.lock().unwrap();
        let closed_ids = engine.close_folder(self.dir);
        if !closed_ids.is_empty() {
            notes.push(format!(
                "closed {} session{} and their panes/windows",
                closed_ids.len(),
                if closed_ids.len() == 1 { "" } else { "s" }
            ));
        }
        // Also reaps the repo's stored identity, which the shared untrack
        // (a plain repos.json rewrite) doesn't know about. Running first makes
        // that untrack a no-op, so it reports nothing and there is no double
        // note.
        engine.remove_repo(self.dir);
        notes
    }
}

/// Reaches the board row through the app's shared store — locking only for the
/// delete itself, never across the worktree removal (see
/// [`tt_agentboard::task_removal::BoardRows`]).
struct AppBoardRows<'a> {
    app: &'a tauri::AppHandle,
    /// The row the caller already resolved, when it had one. Preferred over a
    /// lookup by dir: the Board hands us an explicit id, and re-deriving it
    /// from the directory can quietly miss (a `worktree_dir` that differs by a
    /// trailing slash or a symlink) and leave the very row the user asked to
    /// delete in place while the command reports success.
    board_id: Option<i64>,
}

impl tt_agentboard::task_removal::BoardRows for AppBoardRows<'_> {
    fn delete_task_for_worktree(&self, dir: &str) -> Option<String> {
        // Store errors become a note, never silence: reporting "nothing was
        // bound" when the truth is "the store wouldn't answer" tells the user a
        // row is gone that is still there.
        let id = match self.board_id {
            Some(id) => id,
            None => match crate::store::task_id_for_worktree_dir(self.app, dir) {
                Ok(Some(id)) => id,
                Ok(None) => return None,
                Err(error) => return Some(format!("could not read the board row: {error}")),
            },
        };
        if let Err(error) = crate::store::delete_task_row(self.app, id) {
            return Some(format!("could not delete board task #{id}: {error}"));
        }
        Some("deleted the board task".to_string())
    }
}

/// The Tauri command over [`delete_task_blocking`] — see its doc. Exactly one
/// of `id`/`dir` identifies the task; the Board screen passes an id, the
/// Agentboard rail passes a worktree dir.
///
/// Long-running → off the main thread.
#[tauri::command]
pub async fn task_delete(
    app: tauri::AppHandle,
    id: Option<i64>,
    dir: Option<String>,
    force: bool,
) -> Result<TaskDeleteOutcome, String> {
    use tracing::Instrument as _;

    let target = match (id, dir.clone()) {
        (Some(id), None) => DeleteTarget::Board(id),
        (None, Some(dir)) => DeleteTarget::Worktree(dir),
        _ => return Err("task_delete needs exactly one of id/dir".to_string()),
    };

    // A span (not a bare event) so the event log carries this command's own
    // start/end/duration as one record — otherwise the only visible trace of
    // a slow removal is the `git`/docker `process.spawn` spans nested inside
    // it, with no record of the command boundary itself. Correlate against
    // `window.focus_changed` to see whether an OS focus change landed inside
    // this window (see the worktree-delete-focus investigation).
    //
    // `outcome` distinguishes all three endings, not just ok/err: a guarded
    // refusal is the one the user is most likely to ask about later ("why
    // wouldn't it delete?"), and it looks identical to success in a log that
    // only records `is_ok`. `force` rides along because a forced removal is
    // the only entry in this log that can have destroyed uncommitted work.
    let span = tracing::info_span!(
        "task_delete",
        task_id = id,
        dir = dir.as_deref().unwrap_or(""),
        force,
        outcome = tracing::field::Empty,
        blockers = tracing::field::Empty,
    );
    async move {
        let result =
            tauri::async_runtime::spawn_blocking(move || delete_task_blocking(&app, target, force))
                .await
                .map_err(|e| format!("worktree task failed: {e}"))?;
        let outcome = match &result {
            Ok(TaskDeleteOutcome::Deleted { .. }) => "ok",
            Ok(TaskDeleteOutcome::Blocked { blockers, .. }) => {
                let kinds: Vec<&str> = blockers.iter().map(|b| b.kind.as_str()).collect();
                tracing::Span::current().record("blockers", kinds.join(","));
                "blocked"
            }
            Err(_) => "err",
        };
        tracing::Span::current().record("outcome", outcome);
        result
    }
    .instrument(span)
    .await
}

/// Resolve a task directory to its checkout root and task name, rejecting
/// anything that isn't a worktree of its own checkout — shared by
/// `delete_task_blocking` and `task_stop_port` so both agree on what "this
/// task" means before either acts on it. Returns the identity only; the delete
/// attaches its own `force` when building [`RemoveOpts`], so a non-removal
/// caller never constructs a removal config with a meaningless flag.
fn resolve_task(dir: &str) -> Result<(PathBuf, String), String> {
    let path = PathBuf::from(dir);
    let sr = ops::discover_root(Some(&path)).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("bad task path {dir}"))?
        .to_string();
    if sr.task_dir(&name) != path {
        return Err(format!("{dir} is not a worktree of {}", sr.repo));
    }
    Ok((sr.checkout.clone(), name))
}

/// Stop whatever is listening on `port` — the remedy the app offers for a
/// `foreignPort` blocker, so a stale dev server doesn't send the user to a
/// terminal to finish a delete they started in the app. Returns the note to
/// show; the caller retries the removal afterwards.
///
/// `ops::stop_task_port` refuses any port the task doesn't claim in its own
/// `.env`, which is what keeps this from being a "kill any port" primitive
/// reachable from the UI. SIGTERM, then SIGKILL if the port is still held.
#[tauri::command]
pub async fn task_stop_port(dir: String, port: u16) -> Result<String, String> {
    use tracing::Instrument as _;

    // A user-initiated action that signals processes: it gets its own record
    // (see the telemetry rule in the root CLAUDE.md) — after the fact, "the
    // dev server died" should be answerable from the log, not a repro.
    // `.instrument` rather than a held `enter()` guard, same as `task_delete`:
    // an entered span across an `.await` stays entered while the task is
    // parked, attributing whatever else runs on this thread to it.
    let span = tracing::info_span!(
        "task_stop_port",
        dir = %dir,
        port,
        outcome = tracing::field::Empty,
        pgids = tracing::field::Empty,
    );
    async move {
        let stopped = tauri::async_runtime::spawn_blocking(move || {
            let (checkout, name) = resolve_task(&dir)?;
            ops::stop_task_port(Some(&checkout), &name, port).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("worktree task failed: {e}"))?;

        let span = tracing::Span::current();
        match stopped {
            Ok(stopped) => {
                let count = stopped.pgids.len();
                let s = if count == 1 { "" } else { "s" };
                // One decision for both the log field and the toast, so they
                // can't drift into describing the same event differently.
                // Nothing signaled = the port was already free, which happens
                // when the user quit the dev server themselves after reading
                // the blocker.
                let (outcome, message) = if count == 0 {
                    ("already_free", format!("Port {port} was already free"))
                } else if stopped.graceful {
                    ("terminated", format!("Port {port}: stopped {count} process group{s}"))
                } else {
                    ("killed", format!("Port {port}: force-killed {count} process group{s}"))
                };
                let pgids: Vec<String> = stopped.pgids.iter().map(i32::to_string).collect();
                span.record("pgids", pgids.join(","));
                span.record("outcome", outcome);
                Ok(message)
            }
            Err(e) => {
                span.record("outcome", "err");
                Err(e)
            }
        }
    }
    .instrument(span)
    .await
}
