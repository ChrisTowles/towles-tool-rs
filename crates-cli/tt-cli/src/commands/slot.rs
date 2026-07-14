//! `tt slot` — worktree-slot lifecycle over a primary checkout.
//!
//! Thin CLI shell: creation/rendering/removal all live in `tt_slots::ops`
//! (shared with the app's `slot_create`/`slot_remove` commands). See the
//! tt-slots crate docs for the convention and the `${tt:...}` template
//! grammar.

use std::fs;
use std::path::Path;

use tt_slots::ops::{self, CleanOpts, CreateOpts, RemoveOpts, SlotRoot};
use tt_slots::{envfile, guards};

use crate::cli::SlotCommands;
use crate::ui;

pub fn run(command: SlotCommands) -> i32 {
    let result = match command {
        SlotCommands::New { branch, base, json, root } => {
            cmd_new(&branch, base.as_deref(), json, root.as_deref())
        }
        SlotCommands::Ls { json, root } => cmd_ls(json, root.as_deref()),
        SlotCommands::Rm { name, force, root } => cmd_rm(&name, force, root.as_deref()),
        SlotCommands::Env { name, root } => cmd_env(&name, root.as_deref()),
        SlotCommands::Clean { dry_run, json, root } => cmd_clean(dry_run, json, root.as_deref()),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            ui::error(&message);
            1
        }
    }
}

fn cmd_new(
    branch: &str,
    base: Option<&str>,
    json: bool,
    root: Option<&Path>,
) -> Result<(), String> {
    let opts = CreateOpts {
        root: root.map(Path::to_path_buf),
        branch: branch.to_string(),
        base: base.map(str::to_string),
        run_setup: true,
    };
    let created = ops::create_slot(&opts).map_err(|e| e.to_string())?;
    for warning in &created.warnings {
        ui::warning(warning);
    }
    let dir_s = created.dir.to_string_lossy().to_string();
    if json {
        let ports: serde_json::Map<String, serde_json::Value> =
            created.ports.iter().map(|(k, p)| (k.clone(), (*p).into())).collect();
        let value = serde_json::json!({
            "name": created.name,
            "dir": dir_s,
            "branch": created.branch,
            "base": created.base,
            "ports": ports,
            "inheritedKeys": created.inherited,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
    } else {
        ui::success(&format!("created {} on branch {}", created.name, created.branch));
        for (key, port) in &created.ports {
            println!("  {key}={port}");
        }
        if created.inherited > 0 {
            println!("  inherited {} key(s) from a sibling checkout", created.inherited);
        }
        println!("slot: {dir_s}");
    }
    Ok(())
}

/// Resolve `name` to a checkout dir: `primary` or a dir under `slots/`.
fn checkout_dir(sr: &SlotRoot, name: &str) -> Result<std::path::PathBuf, String> {
    if name == "primary" || name == sr.primary.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        return Ok(sr.primary.clone());
    }
    let dir = sr.slot_dir(name);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("no slot {name} in {}", sr.slots_dir().display()))
    }
}

fn cmd_env(name: &str, root: Option<&Path>) -> Result<(), String> {
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    let dir = checkout_dir(&sr, name)?;
    let summary = ops::render_slot_env(&sr, &dir, None).map_err(|e| e.to_string())?;
    for warning in &summary.warnings {
        ui::warning(warning);
    }
    ui::success(&format!(
        "rendered {name}/.env ({} reused, {} fresh claim(s), {} extra key(s) preserved)",
        summary.reused, summary.claimed, summary.preserved
    ));
    Ok(())
}

fn cmd_ls(json: bool, root: Option<&Path>) -> Result<(), String> {
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    let _ = ops::git_primary(&sr.primary, &["worktree", "prune"]);
    let mut checkouts: Vec<(String, std::path::PathBuf, bool)> =
        vec![("primary".to_string(), sr.primary.clone(), true)];
    checkouts.extend(sr.slots().into_iter().map(|(name, dir)| (name, dir, false)));

    let mut rows = Vec::new();
    for (name, dir, is_primary) in checkouts {
        let broken = !ops::git_slot(&dir, &["rev-parse", "--is-inside-work-tree"])
            .map(|o| o.ok())
            .unwrap_or(false);
        let (branch, detached, dirty) = if broken {
            ("BROKEN".to_string(), false, 0)
        } else {
            let current = ops::git_slot(&dir, &["branch", "--show-current"])
                .map(|o| o.stdout.trim().to_string())
                .unwrap_or_default();
            let dirty = ops::git_slot(&dir, &["status", "--porcelain"])
                .map(|o| guards::dirty_entry_count(&o.stdout))
                .unwrap_or(0);
            if current.is_empty() {
                let sha = ops::git_slot(&dir, &["rev-parse", "--short", "HEAD"])
                    .map(|o| o.stdout.trim().to_string())
                    .unwrap_or_else(|_| "?".to_string());
                (format!("detached:{sha}"), true, dirty)
            } else {
                (current, false, dirty)
            }
        };
        let env_text = fs::read_to_string(dir.join(".env")).unwrap_or_default();
        let ports: Vec<(String, String)> = envfile::parse(&env_text)
            .into_iter()
            .filter(|(k, v)| {
                k.ends_with("PORT") && v.bytes().all(|b| b.is_ascii_digit()) && !v.is_empty()
            })
            .collect();
        rows.push((name, branch, detached, broken, dirty, ports, is_primary));
    }

    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, branch, detached, broken, dirty, ports, is_primary)| {
                let port_map: serde_json::Map<String, serde_json::Value> =
                    ports.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
                serde_json::json!({
                    "name": name,
                    "branch": branch,
                    "detached": detached,
                    "broken": broken,
                    "dirty": dirty,
                    "ports": port_map,
                    "primary": is_primary,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
    } else {
        println!("{:<20} {:<36} {:<6} PORTS", "CHECKOUT", "BRANCH", "DIRTY");
        for (name, branch, _, _, dirty, ports, _) in &rows {
            let ports_s: Vec<String> = ports.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("{name:<20} {branch:<36} {dirty:<6} {}", ports_s.join(" "));
        }
    }
    Ok(())
}

fn cmd_rm(name: &str, force: bool, root: Option<&Path>) -> Result<(), String> {
    let opts = RemoveOpts { root: root.map(Path::to_path_buf), name: name.to_string(), force };
    let removed = ops::remove_slot(&opts).map_err(|e| e.to_string())?;
    for message in &removed.messages {
        ui::warning(message);
    }
    // The slot may be tracked on the agentboard rail (the app tracks slots it
    // creates); drop the now-dangling entry so the rail doesn't keep a
    // "missing" ghost with its pane and windows.
    let dir_s = removed.dir.to_string_lossy();
    if let Ok((_, untracked)) = tt_agentboard::repos::remove_repo_persisted(
        &tt_agentboard::repos::default_repos_path(),
        &dir_s,
    ) && untracked
    {
        println!("  untracked from the agentboard rail");
    }
    ui::success(&format!("removed {} (ports released with its .env)", removed.name));
    Ok(())
}

fn cmd_clean(dry_run: bool, json: bool, root: Option<&Path>) -> Result<(), String> {
    let bases = tt_config::instance_state_bases().map_err(|e| e.to_string())?;
    let opts = CleanOpts {
        root: root.map(Path::to_path_buf),
        dry_run,
        scope_parents: bases.scope_parents().to_vec(),
    };
    let report =
        ops::clean_slots(&opts, tt_config::slot_scope_from_dir).map_err(|e| e.to_string())?;

    // Agentboard stores that survive the sweep: the unscoped daily driver's
    // plus every remaining checkout's scope. Removed scopes' stores just got
    // deleted wholesale with their state dir.
    let mut store_dirs = vec![bases.agentboard_dir(None)];
    store_dirs.extend(report.live_scopes.iter().map(|s| bases.agentboard_dir(Some(s))));
    let mut prunes = Vec::new();
    for dir in store_dirs {
        match tt_agentboard::cleanup::prune_store(&dir, dry_run) {
            Ok(Some(prune)) => prunes.push(prune),
            Ok(None) => {}
            Err(e) => {
                ui::warning(&format!("agentboard prune failed for {}: {e}", dir.display()));
            }
        }
    }

    if json {
        let value = serde_json::json!({
            "dryRun": report.dry_run,
            "removed": report.removed.iter().map(|s| serde_json::json!({
                "name": s.name,
                "branch": s.branch,
                "reason": s.reason,
                "messages": s.messages,
            })).collect::<Vec<_>>(),
            "kept": report.kept.iter().map(|s| serde_json::json!({
                "name": s.name,
                "branch": s.branch,
                "why": s.why,
            })).collect::<Vec<_>>(),
            "sweptStateDirs": report.swept_state_dirs.iter()
                .map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "agentboard": prunes.iter().map(|p| serde_json::json!({
                "dir": p.dir.display().to_string(),
                "sessionFoldersDropped": p.session_folders_dropped,
                "windowsDropped": p.windows_dropped,
                "panesDropped": p.panes_dropped,
            })).collect::<Vec<_>>(),
            "warnings": report.warnings,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
        return Ok(());
    }

    for warning in &report.warnings {
        ui::warning(warning);
    }
    let verb = if dry_run { "would remove" } else { "removed" };
    for slot in &report.removed {
        ui::success(&format!("{verb} {} ({} — {})", slot.name, slot.branch, slot.reason));
        for message in &slot.messages {
            println!("  {message}");
        }
    }
    for slot in &report.kept {
        println!("kept {} ({}): {}", slot.name, slot.branch, slot.why.join("; "));
    }
    if !report.swept_state_dirs.is_empty() {
        let verb = if dry_run { "would sweep" } else { "swept" };
        println!("{verb} stale instance state:");
        for dir in &report.swept_state_dirs {
            println!("  {}", dir.display());
        }
    }
    for prune in &prunes {
        let verb = if dry_run { "would prune" } else { "pruned" };
        println!(
            "{verb} agentboard store {}: {} window(s), {} pane(s), {} session folder(s)",
            prune.dir.display(),
            prune.windows_dropped,
            prune.panes_dropped,
            prune.session_folders_dropped.len()
        );
    }
    if report.removed.is_empty() && report.swept_state_dirs.is_empty() && prunes.is_empty() {
        println!("nothing to clean");
    }
    Ok(())
}
