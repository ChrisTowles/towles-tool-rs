//! `slot_*` commands: worktree-slot creation from the app (Agentboard's
//! new-slot modal). Thin over `tt_slots::ops`, which is shared with the
//! `ttr slot` CLI — the app never reimplements slot logic.

use serde::Serialize;
use std::path::PathBuf;

use tt_slots::ops::{self, CreateOpts};

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
