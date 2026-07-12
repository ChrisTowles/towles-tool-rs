//! `ttr slot` — worktree-slot lifecycle over a primary checkout.
//!
//! Thin CLI shell: creation/rendering lives in `tt_slots::ops` (shared with
//! the app's `slot_create` command); removal guards and docker cleanup stay
//! here since only the CLI removes slots today. See the tt-slots crate docs
//! for the convention and the `${tt:...}` template grammar.

use std::fs;
use std::path::Path;

use tt_slots::ops::{self, CreateOpts, SlotRoot};
use tt_slots::{RmBlocked, envfile, guards};

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
    let summary = ops::render_slot_env(&sr, &dir).map_err(|e| e.to_string())?;
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
    let sr = ops::discover_root(root).map_err(|e| e.to_string())?;
    if name == "primary" || sr.primary.file_name().and_then(|n| n.to_str()) == Some(name) {
        return Err(
            "refusing to remove the primary checkout — it owns every slot's git state".to_string()
        );
    }
    let dir = sr.slot_dir(name);
    if !dir.is_dir() {
        return Err(format!("no slot {name} in {}", sr.slots_dir().display()));
    }
    let dir_s = dir.to_string_lossy().to_string();

    // broken worktree: git can't even report status
    if ops::git_slot(&dir, &["status", "--porcelain"]).map(|o| !o.ok()).unwrap_or(true) {
        if !force {
            return Err(format!(
                "{name}'s worktree is broken (git fails inside it) — re-run with --force to remove anyway"
            ));
        }
        ui::warning(
            "skipping guards (--force): worktree is broken — removing directory + registration",
        );
        docker_cleanup(name, &dir);
        fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {dir_s}: {e}"))?;
        let _ = ops::git_primary(&sr.primary, &["worktree", "prune"]);
        ui::success(&format!("removed {name} (broken)"));
        return Ok(());
    }

    let dirty = ops::git_slot(&dir, &["status", "--porcelain"])
        .map(|o| guards::dirty_entry_count(&o.stdout))
        .unwrap_or(0);
    let unreachable = ops::git_slot(
        &dir,
        &[
            "rev-list",
            "--count",
            "HEAD",
            "--not",
            "--branches",
            "--remotes",
        ],
    )
    .ok()
    .filter(|o| o.ok())
    .and_then(|o| guards::unreachable_commit_count(&o.stdout))
    .unwrap_or(0);
    let env_text = fs::read_to_string(dir.join(".env")).unwrap_or_default();
    let foreign: Vec<u16> = envfile::port_claims(&env_text)
        .into_iter()
        .filter(|&p| ops::port_occupied(p) && !docker_owns_port(name, p))
        .collect();

    let blocked = guards::check_removal(dirty, unreachable, &foreign);
    if !blocked.is_empty() {
        if !force {
            let reasons: Vec<String> = blocked.iter().map(RmBlocked::to_string).collect();
            return Err(format!("refused to remove {name}:\n  {}", reasons.join("\n  ")));
        }
        for reason in &blocked {
            ui::warning(&format!("skipping guard (--force): {reason}"));
        }
    }

    docker_cleanup(name, &dir);

    let remove = if force {
        ops::git_primary(&sr.primary, &["worktree", "remove", "--force", &dir_s])
    } else {
        ops::git_primary(&sr.primary, &["worktree", "remove", &dir_s])
    };
    match remove {
        Ok(out) if out.ok() => {}
        result => {
            let detail = match result {
                Ok(out) => out.stderr.trim().to_string(),
                Err(e) => e.to_string(),
            };
            if !force {
                return Err(format!("git worktree remove failed:\n{detail}"));
            }
            ui::warning(&format!("git worktree remove failed ({detail}) — removing directory"));
            fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {dir_s}: {e}"))?;
        }
    }
    let _ = ops::git_primary(&sr.primary, &["worktree", "prune"]);
    ui::success(&format!("removed {name} (ports released with its .env)"));
    Ok(())
}

/// Whether a docker container owned by this slot publishes `port`.
fn docker_owns_port(slot_name: &str, port: u16) -> bool {
    let publish = format!("publish={port}");
    tt_exec::run("docker", &["ps", "--filter", &publish, "--format", "{{.Names}}"])
        .map(|out| {
            out.ok()
                && out
                    .stdout
                    .lines()
                    .any(|line| guards::docker_resource_matches(line.trim(), slot_name))
        })
        .unwrap_or(false)
}

/// Compose down (containers, networks, volumes) then an anchored sweep of
/// anything else named after the slot. Best-effort: a missing docker is fine.
fn docker_cleanup(slot_name: &str, dir: &Path) {
    let has_compose = [
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ]
    .iter()
    .any(|f| dir.join(f).is_file());
    if has_compose {
        let _ = tt_exec::run_in_dir_with_timeout(
            "docker",
            &["compose", "down", "--volumes", "--remove-orphans"],
            dir,
            std::time::Duration::from_secs(120),
        );
    }
    if let Ok(out) = tt_exec::run("docker", &["ps", "-a", "--format", "{{.Names}}"]) {
        for container in out.stdout.lines().map(str::trim) {
            if guards::docker_resource_matches(container, slot_name) {
                ui::info(&format!("removing container {container}"));
                let _ = tt_exec::run("docker", &["rm", "-f", container]);
            }
        }
    }
    if let Ok(out) = tt_exec::run("docker", &["volume", "ls", "--format", "{{.Name}}"]) {
        for volume in out.stdout.lines().map(str::trim) {
            if guards::docker_resource_matches(volume, slot_name) {
                ui::info(&format!("removing volume {volume}"));
                let _ = tt_exec::run("docker", &["volume", "rm", volume]);
            }
        }
    }
}
