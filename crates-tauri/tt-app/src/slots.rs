//! `slot_*` commands: worktree-slot creation/removal from the app
//! (Agentboard's new-slot modal and the rail's delete-worktree action). Thin
//! over `tt_slots::ops`, which is shared with the `tt slot` CLI — the app
//! never reimplements slot logic.

use serde::Serialize;
use std::path::PathBuf;
use tauri::Manager;

use tt_slots::ops::{self, CreateOpts, RemoveOpts};

/// Fire-and-forget `git fetch` across every tracked repo (deduped, see
/// [`tt_agentboard::git_info::fetch_all`]), then nudge the rail to re-emit.
/// Slot lifecycle events (create/remove) are a natural moment to check
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
pub struct SlotCreated {
    pub name: String,
    pub dir: String,
    pub branch: String,
    pub base: String,
    pub warnings: Vec<String>,
}

/// Branches available as a base ref in the slot root containing `root`
/// (a checkout dir or the root itself), default branch first.
#[tauri::command]
pub fn slot_base_branches(root: String) -> Result<Vec<String>, String> {
    let sr = ops::discover_root(Some(&PathBuf::from(root))).map_err(|e| e.to_string())?;
    ops::checkout_branches(&sr.checkout).map_err(|e| e.to_string())
}

/// Create the `.claude/slot-env.template` sidecar for a repo that has
/// neither it nor a tokenized `.env.example` — the New Slot dialog's
/// one-click fix for `slot_create`'s "no template" error. Returns the
/// created (or already-existing) sidecar path.
#[tauri::command]
pub fn slot_init_template(root: String) -> Result<String, String> {
    let sr = ops::discover_root(Some(&PathBuf::from(root))).map_err(|e| e.to_string())?;
    ops::init_template_sidecar(&sr)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchCheck {
    pub name: Option<String>,
    pub taken: bool,
    pub error: Option<String>,
}

/// Preflight the new-slot dialog's branch field: is it a legal git ref, and
/// would its derived slot name collide with an existing one? Read-only —
/// safe to call on every keystroke (debounced by the caller).
#[tauri::command]
pub fn slot_check_branch(root: String, branch: String) -> Result<BranchCheck, String> {
    let sr = ops::discover_root(Some(&PathBuf::from(root))).map_err(|e| e.to_string())?;
    let check = ops::check_branch(&sr, branch.trim());
    Ok(BranchCheck { name: check.name, taken: check.taken, error: check.error })
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SlotSuggestion {
    pub branch: String,
    pub goal: String,
}

/// Manual "Suggest" button in the new-slot dialog: ask `claude -p` (cwd =
/// `dir`, the repo checkout the dialog is open for, so it sees real repo
/// context) to propose a better branch name and a cleaned-up goal for the
/// text the user typed. The dialog fills its editable fields with the
/// result — nothing here writes anything or is called automatically.
/// Long-running (a cold `claude` CLI) → off the main thread.
#[tauri::command]
pub async fn slot_suggest(dir: String, goal: String) -> Result<SlotSuggestion, String> {
    tauri::async_runtime::spawn_blocking(move || tt_slots::suggest(&PathBuf::from(dir), &goal))
        .await
        .map_err(|e| format!("slot task failed: {e}"))?
        .map(|s| SlotSuggestion { branch: s.branch, goal: s.goal })
        .map_err(|e| e.to_string())
}

/// Create the slot for `branch` off `base` (empty base = the primary's
/// branch). Long-running — fetch, worktree add, the install setup step — so
/// it runs off the main thread.
#[tauri::command]
pub async fn slot_create(
    app: tauri::AppHandle,
    root: String,
    branch: String,
    base: String,
) -> Result<SlotCreated, String> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        return Err("a slot needs a branch — slots are named after their branch".to_string());
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
    let created = tauri::async_runtime::spawn_blocking(move || ops::create_slot(&opts))
        .await
        .map_err(|e| format!("slot task failed: {e}"))?
        .map_err(|e| e.to_string())?;
    refresh_all_git_info_in_background(&app);
    Ok(SlotCreated {
        name: created.name,
        dir: created.dir.to_string_lossy().to_string(),
        branch: created.branch,
        base: created.base,
        warnings: created.warnings,
    })
}

/// Re-run a checkout's setup step (declared `TT_SLOT_SETUP` or lockfile
/// detection) — the retry affordance for a setup failure surfaced from
/// `slot_create`. `Ok(None)` = nothing to run or it succeeded this time;
/// `Ok(Some)` carries the same warning text `slot_create` would have shown.
/// Long-running (an install can take a minute) → off the main thread.
#[tauri::command]
pub async fn slot_run_setup(dir: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || ops::run_setup(&PathBuf::from(dir)))
        .await
        .map_err(|e| format!("slot task failed: {e}"))?
        .map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SlotRemoved {
    pub name: String,
    pub messages: Vec<String>,
}

/// Remove the worktree slot at `dir`, guarded — a dirty tree, commits
/// unreachable from any branch/remote, or a foreign listener on the slot's
/// claimed ports block with an explanatory error (no force path in the app;
/// use `tt slot rm --force`). Before touching the worktree, kills the
/// folder's live PTYs (SIGHUPing any Claude Code session running inside,
/// then SIGKILLing anything else still sharing the shell's session — see
/// `terminal::kill_session_stragglers`) so a backgrounded job that dodged the
/// shell's own signal forwarding doesn't survive as an orphan on a deleted
/// cwd — but does NOT drop the session/window/pane records yet. Those are
/// only removed (and persisted) once `ops::remove_slot` has actually
/// succeeded: dropping them up front used to leave the rail looking like a
/// clean removal — panes gone — while a blocked or failed removal left the
/// worktree sitting on disk forever with nothing left on the rail to retry
/// from. On failure the killed-but-still-tracked sessions surface as dead
/// panes so the user can see the slot is still there. Then cleans up docker
/// resources, the worktree registration, and the slot's agentboard tracking.
/// Long-running → off the main thread.
#[tauri::command]
pub async fn slot_remove(app: tauri::AppHandle, dir: String) -> Result<SlotRemoved, String> {
    let mut messages = Vec::new();
    {
        let ab = app.state::<crate::agentboard::Ab>();
        let ids = ab.engine.lock().unwrap().session_ids_for(&dir);
        if !ids.is_empty() {
            let term_state = app.state::<crate::terminal::TermState>();
            for id in &ids {
                term_state.kill(id);
            }
        }
    }

    let removed = tauri::async_runtime::spawn_blocking(move || {
        let path = PathBuf::from(&dir);
        let sr = ops::discover_root(Some(&path)).map_err(|e| e.to_string())?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("bad slot path {dir}"))?
            .to_string();
        if sr.slot_dir(&name) != path {
            return Err(format!("{dir} is not a worktree slot of {}", sr.repo));
        }
        ops::remove_slot(&RemoveOpts { root: Some(sr.checkout.clone()), name, force: false })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("slot task failed: {e}"))??;

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
    // Re-emit either way: a fleet-discovered (never-tracked) slot also drops
    // off the rail on the next recompute, so don't make the user wait a poll.
    ab.emit.notify_one();
    refresh_all_git_info_in_background(&app);
    Ok(SlotRemoved { name: removed.name, messages })
}
