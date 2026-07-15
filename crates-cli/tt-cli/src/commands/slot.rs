//! `tt slot` — worktree-slot lifecycle over any git checkout.
//!
//! Thin CLI shell: creation/rendering/removal all live in `tt_slots::ops`
//! (shared with the app's `slot_create`/`slot_remove` commands). See the
//! tt-slots crate docs for the convention and the `${tt:...}` template
//! grammar. `hook-create`/`hook-remove` are the Claude Code
//! WorktreeCreate/WorktreeRemove hook shells — stdin is the hook JSON and
//! (for create) stdout is *only* the worktree path, per the hook contract.

use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use tt_slots::ops::{self, CleanOpts, CreateOpts, OpsError, RemoveOpts, SlotRoot};
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
        SlotCommands::HookCreate => cmd_hook_create(),
        SlotCommands::HookRemove => cmd_hook_remove(),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            ui::error(&message);
            1
        }
    }
}

/// The hook JSON Claude Code writes to the hook's stdin. TTY-guarded so a
/// hand-run `tt slot hook-create` fails fast instead of hanging on a read.
fn read_hook_input() -> Result<serde_json::Value, String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err("hook-create/hook-remove read Claude Code's hook JSON on stdin — \
                    they are not meant to be run by hand"
            .to_string());
    }
    let mut raw = String::new();
    stdin.read_to_string(&mut raw).map_err(|e| format!("cannot read hook stdin: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("hook stdin is not valid JSON: {e}"))
}

fn hook_str<'a>(input: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| input.get(k).and_then(|v| v.as_str())).filter(|s| !s.is_empty())
}

/// WorktreeCreate hook: create (or reuse) the slot for the requested name and
/// print its path — the one line of stdout Claude Code parses. The requested
/// name IS the branch, verbatim (`claude -w feat/thing` → branch
/// `feat/thing`, slot folder `feat-thing` — the folder is a one-way slug of
/// the branch, never parsed back) — and never the native `worktree-<name>`
/// scheme or a guessed prefix. Claude Code observed (2.1.210) sends
/// `{session_id, transcript_path, cwd, hook_event_name, name}` with `cwd`
/// already the main checkout root; `worktree_name`/`source_ref` are accepted
/// too for the documented shape.
fn cmd_hook_create() -> Result<(), String> {
    let input = read_hook_input()?;
    let name = hook_str(&input, &["name", "worktree_name"])
        .ok_or("hook input has no worktree name (`name`/`worktree_name`)")?;
    let root = hook_str(&input, &["cwd"]).map(PathBuf::from);
    let branch = name.to_string();

    let opts = CreateOpts {
        root: root.clone(),
        branch,
        base: hook_str(&input, &["source_ref"]).map(str::to_string),
        run_setup: true,
    };
    let dir = match ops::create_slot(&opts) {
        Ok(created) => {
            for warning in &created.warnings {
                eprintln!("tt slot: {warning}");
            }
            created.dir
        }
        // An existing slot is a resume, not an error — Claude Code re-enters
        // worktrees by name; hand back the same path.
        Err(OpsError::SlotExists { dir, .. }) => PathBuf::from(dir),
        Err(e) => return Err(e.to_string()),
    };
    println!("{}", dir.display());
    Ok(())
}

/// WorktreeRemove hook: the same guarded removal as `tt slot rm` (never
/// forced — a slot with unpushed work stays on disk and the refusal lands in
/// Claude Code's hook log on stderr), plus the agentboard untracking every
/// removal path owes.
fn cmd_hook_remove() -> Result<(), String> {
    let input = read_hook_input()?;
    let path = hook_str(&input, &["worktree_path", "path"])
        .map(PathBuf::from)
        .ok_or("hook input has no worktree path (`worktree_path`/`path`)")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("bad worktree path {}", path.display()))?
        .to_string();
    if !path.exists() {
        eprintln!("tt slot: {} is already gone — nothing to remove", path.display());
        return Ok(());
    }
    let opts = RemoveOpts { root: Some(path.clone()), name, force: false };
    let removed = ops::remove_slot(&opts).map_err(|e| e.to_string())?;
    for message in &removed.messages {
        eprintln!("tt slot: {message}");
    }
    untrack_from_agentboard(&removed.dir);
    Ok(())
}

/// Drop a removed checkout's now-dangling agentboard rail entry (see the
/// repo rule: every removal path must untrack the dir from repos.json).
fn untrack_from_agentboard(dir: &Path) {
    let dir_s = dir.to_string_lossy();
    if let Ok((_, true)) = tt_agentboard::repos::remove_repo_persisted(
        &tt_agentboard::repos::default_repos_path(),
        &dir_s,
    ) {
        eprintln!("tt slot: untracked {dir_s} from the agentboard rail");
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

    // Each removed slot may be tracked on the agentboard rail (same rationale
    // as `tt slot rm`'s untracking below) — drop its now-dangling repos.json
    // entry so collectors (`prs`/`issues`) don't keep retrying a gone dir.
    if !dry_run {
        let repos_path = tt_agentboard::repos::default_repos_path();
        for slot in &report.removed {
            let dir_s = slot.dir.to_string_lossy();
            if let Ok((_, true)) = tt_agentboard::repos::remove_repo_persisted(&repos_path, &dir_s)
            {
                ui::warning(&format!("untracked {} from the agentboard rail", slot.name));
            }
        }
    }

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
