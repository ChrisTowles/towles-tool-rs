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
            ab.engine.lock().unwrap().git_targets().into_iter().map(|(dir, _)| dir).collect();
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
    pub error: Option<String>,
}

/// Preflight the new-task dialog's branch field: is it a legal git ref, and
/// would its derived task name collide with an existing one? Read-only —
/// safe to call on every keystroke (debounced by the caller).
#[tauri::command]
pub fn task_check_branch(root: String, branch: String) -> Result<BranchCheck, String> {
    let sr = ops::discover_root(Some(&PathBuf::from(root))).map_err(|e| e.to_string())?;
    let check = ops::check_branch(&sr, branch.trim());
    Ok(BranchCheck { name: check.name, taken: check.taken, error: check.error })
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
/// branch). Long-running — fetch, worktree add, the install setup step — so
/// it runs off the main thread.
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
        run_setup: true,
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
pub enum TaskRemoveOutcome {
    Removed {
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

/// Remove the worktree at `dir`, guarded — a dirty tree, commits
/// unreachable from any branch/remote, or a foreign listener on the task's
/// claimed ports come back as [`TaskRemoveOutcome::Blocked`] with a typed
/// blocker per reason, which the app renders as a dialog offering each one's
/// remedy (stop the port's process via [`task_stop_port`]) plus a force.
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
/// Long-running → off the main thread.
#[tauri::command]
pub async fn task_remove(
    app: tauri::AppHandle,
    dir: String,
    force: bool,
) -> Result<TaskRemoveOutcome, String> {
    use tracing::Instrument as _;

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
        "task_remove",
        dir = %dir,
        force,
        outcome = tracing::field::Empty,
        blockers = tracing::field::Empty,
    );
    async move {
        let result = task_remove_inner(app, dir, force).await;
        let outcome = match &result {
            Ok(TaskRemoveOutcome::Removed { .. }) => "ok",
            Ok(TaskRemoveOutcome::Blocked { blockers, .. }) => {
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

async fn task_remove_inner(
    app: tauri::AppHandle,
    dir: String,
    force: bool,
) -> Result<TaskRemoveOutcome, String> {
    let mut messages = Vec::new();
    let kill_app = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let (checkout, name) = resolve_task(&dir)?;
        let opts = RemoveOpts { root: Some(checkout), name, force };
        // The PTY kill rides `before_removal`, not the top of this command:
        // the guards run with the panes alive, so a refusal (the dialog the
        // user can back out of) never costs a live Claude session — only a
        // removal that is really happening does. Locks are scoped tight per
        // this crate's rule: never hold the engine lock across a subprocess.
        ops::remove_task(&opts, || {
            let ids = {
                let ab = kill_app.state::<crate::agentboard::Ab>();
                let engine = ab.engine.lock().unwrap();
                engine.session_ids_for(&dir)
            };
            if !ids.is_empty() {
                let term_state = kill_app.state::<crate::terminal::TermState>();
                for id in &ids {
                    term_state.kill(id);
                }
            }
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("worktree task failed: {e}"))??;

    // A refusal ends here: nothing was removed, so none of the rail teardown
    // below applies — the panes stay put for the user to retry from.
    let removed = match outcome {
        ops::RemoveOutcome::Removed(removed) => removed,
        ops::RemoveOutcome::Blocked { name, blocked, messages: notes } => {
            messages.extend(notes);
            return Ok(TaskRemoveOutcome::Blocked {
                name,
                blockers: blocked.iter().map(Blocker::from).collect(),
                messages,
            });
        }
    };

    messages.extend(removed.messages);
    let ab = app.state::<crate::agentboard::Ab>();
    let untracked = {
        let mut engine = ab.engine.lock().unwrap();
        let closed_ids = engine.close_folder(&removed.dir.to_string_lossy());
        if !closed_ids.is_empty() {
            messages.push(format!(
                "closed {} session{} and their panes/windows",
                closed_ids.len(),
                if closed_ids.len() == 1 { "" } else { "s" }
            ));
        }
        engine.remove_repo(&removed.dir.to_string_lossy())
    };
    if untracked {
        messages.push("untracked from the agentboard rail".to_string());
    }
    let detached = crate::store::detach_task_worktree_dir(&app, &removed.dir.to_string_lossy());
    if detached > 0 {
        messages.push("detached the board task from the removed worktree".to_string());
    }
    // Re-emit either way: a fleet-discovered (never-tracked) task also drops
    // off the rail on the next recompute, so don't make the user wait a poll.
    ab.emit.notify_one();
    refresh_all_git_info_in_background(&app);
    Ok(TaskRemoveOutcome::Removed { name: removed.name, messages })
}

/// Resolve a task directory to its checkout root and task name, rejecting
/// anything that isn't a worktree of its own checkout — shared by
/// `task_remove` and `task_stop_port` so both agree on what "this task"
/// means before either acts on it. Returns the identity only; `task_remove`
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
    // `.instrument` rather than a held `enter()` guard, same as `task_remove`:
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
