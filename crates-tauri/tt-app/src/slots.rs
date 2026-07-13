//! `slot_*` commands: worktree-slot creation/removal from the app
//! (Agentboard's new-slot modal and the rail's delete-worktree action). Thin
//! over `tt_slots::ops`, which is shared with the `tt slot` CLI — the app
//! never reimplements slot logic.

use serde::Serialize;
use std::path::PathBuf;

use tt_slots::ops::{self, CreateOpts, RemoveOpts};

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
    ops::primary_branches(&sr.primary).map_err(|e| e.to_string())
}

/// Create the slot for `branch` off `base` (empty base = the primary's
/// branch). Long-running — fetch, worktree add, the install setup step — so
/// it runs off the main thread.
#[tauri::command]
pub async fn slot_create(
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
    Ok(SlotCreated {
        name: created.name,
        dir: created.dir.to_string_lossy().to_string(),
        branch: created.branch,
        base: created.base,
        warnings: created.warnings,
    })
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
/// use `tt slot rm --force`). Before touching the worktree, closes out the
/// folder's live rail state in order — kill its PTYs (SIGHUPing any Claude
/// Code session running inside), drop its window/pane records — so removal
/// never leaves a shell orphaned on a deleted cwd or a ghost pane/window
/// lingering in the rail until the next poll notices the directory is gone.
/// Then cleans up docker resources, the worktree registration, and the
/// slot's agentboard tracking. Long-running → off the main thread.
#[tauri::command]
pub async fn slot_remove(app: tauri::AppHandle, dir: String) -> Result<SlotRemoved, String> {
    use tauri::Manager;

    let mut messages = Vec::new();
    {
        let ab = app.state::<crate::agentboard::Ab>();
        let closed_ids = ab.engine.lock().unwrap().close_folder(&dir);
        if !closed_ids.is_empty() {
            let term_state = app.state::<crate::terminal::TermState>();
            for id in &closed_ids {
                term_state.kill(id);
            }
            messages.push(format!(
                "closed {} session{} and their panes/windows",
                closed_ids.len(),
                if closed_ids.len() == 1 { "" } else { "s" }
            ));
        }
        ab.emit.notify_one();
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
        ops::remove_slot(&RemoveOpts { root: Some(sr.root.clone()), name, force: false })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("slot task failed: {e}"))??;

    messages.extend(removed.messages);
    let ab = app.state::<crate::agentboard::Ab>();
    let untracked = {
        let mut engine = ab.engine.lock().unwrap();
        engine.remove_repo(&removed.dir.to_string_lossy())
    };
    if untracked {
        messages.push("untracked from the agentboard rail".to_string());
    }
    // Re-emit either way: a fleet-discovered (never-tracked) slot also drops
    // off the rail on the next recompute, so don't make the user wait a poll.
    ab.emit.notify_one();
    Ok(SlotRemoved { name: removed.name, messages })
}
