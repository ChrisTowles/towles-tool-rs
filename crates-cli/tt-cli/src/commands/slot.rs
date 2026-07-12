//! `ttr slot` — worktree-slot lifecycle over a bare hub.
//!
//! Layout: `<root>/<repo>.git` (bare hub) + `<root>/<repo>-slot-N/` worktrees.
//! This layer gathers real-world state (git, bind tests, docker) and delegates
//! every decision to `tt-slots`; see that crate's docs for the convention and
//! the `${tt:...}` template grammar. Ported from the shell probe at
//! `~/code/p/blog-repos/slots.sh`.

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tt_slots::{RmBlocked, SlotContext, envfile, guards, layout};

use crate::cli::SlotCommands;
use crate::ui;

const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const DOCKER_TIMEOUT: Duration = Duration::from_secs(120);
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);
const TEMPLATE_SIDECAR: &str = "slot-env.template";
const SETUP_HOOK: &str = "slot-setup.sh";
const LOCK_FILE: &str = "tt-slots.lock";
const LOCK_STALE: Duration = Duration::from_secs(60);

pub fn run(command: SlotCommands) -> i32 {
    let result = match command {
        SlotCommands::New { branch, json, root } => {
            cmd_new(branch.as_deref(), json, root.as_deref())
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

/// A discovered slot root: the parent directory, its bare hub, and repo name.
struct SlotRoot {
    root: PathBuf,
    hub: PathBuf,
    repo: String,
}

impl SlotRoot {
    fn slot_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Existing slot dirs as (number, name, path), sorted by number.
    fn slots(&self) -> Vec<(u32, String, PathBuf)> {
        let mut slots: Vec<(u32, String, PathBuf)> = dir_names(&self.root)
            .into_iter()
            .filter_map(|name| {
                layout::parse_slot(&self.repo, &name)
                    .map(|n| (n, name.clone(), self.root.join(&name)))
            })
            .collect();
        slots.sort_by_key(|(n, _, _)| *n);
        slots
    }
}

fn dir_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Find the slot root: `--root` if given, else walk up from cwd looking for a
/// directory that contains exactly one `<repo>.git` bare hub.
fn discover_root(explicit: Option<&Path>) -> Result<SlotRoot, String> {
    let start = match explicit {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?,
    };
    for dir in start.ancestors() {
        let hubs: Vec<String> = dir_names(dir)
            .into_iter()
            .filter(|name| layout::repo_from_hub(name).is_some())
            .collect();
        if hubs.len() == 1 {
            let repo = layout::repo_from_hub(&hubs[0]).unwrap_or_default().to_string();
            return Ok(SlotRoot { root: dir.to_path_buf(), hub: dir.join(&hubs[0]), repo });
        }
        if hubs.len() > 1 && explicit.is_some() {
            return Err(format!(
                "{} contains {} bare hubs — expected exactly one",
                dir.display(),
                hubs.len()
            ));
        }
    }
    Err(format!(
        "no slot root found walking up from {} — a slot root holds exactly one <repo>.git bare hub (or pass --root)",
        start.display()
    ))
}

fn git_hub(hub: &Path, args: &[&str]) -> Result<tt_exec::Output, String> {
    let hub_s = hub.to_string_lossy();
    let mut full: Vec<&str> = vec!["-C", hub_s.as_ref()];
    full.extend_from_slice(args);
    tt_exec::run("git", &full).map_err(|e| e.to_string())
}

fn git_slot(dir: &Path, args: &[&str]) -> Result<tt_exec::Output, String> {
    tt_exec::run_in_dir_with_timeout("git", args, dir, GIT_TIMEOUT).map_err(|e| e.to_string())
}

fn base_branch(hub: &Path) -> String {
    git_hub(hub, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

fn port_occupied(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

// ---------------------------------------------------------------------------
// claim lock — serializes port claims across concurrent `slot new` (parallel
// agents create slots together; without this, both scan siblings before
// either writes, and claim the same ports)

struct ClaimLock {
    path: PathBuf,
}

impl ClaimLock {
    fn acquire(hub: &Path) -> Result<Self, String> {
        let path = hub.join(LOCK_FILE);
        for _ in 0..100 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(_) => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Err(format!("timed out waiting for {} — another slot command may be stuck", path.display()))
    }
}

impl Drop for ClaimLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// rendering

struct RenderSummary {
    ports: Vec<(String, u16)>,
    reused: usize,
    claimed: usize,
    preserved: usize,
}

/// Render the slot's `.env`: template → text (reusing existing claims), then
/// merge back any keys the template doesn't know (inherited secrets, local
/// adds), write the marker, and keep `.tt-slot` ignored via the hub.
fn render_slot_env(sr: &SlotRoot, dir: &Path) -> Result<RenderSummary, String> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("bad slot path {}", dir.display()))?
        .to_string();
    let number = layout::parse_slot(&sr.repo, &name).ok_or_else(|| {
        format!("{name} is not a slot of {} (expected {}-slot-N)", sr.repo, sr.repo)
    })?;

    // template: the repo's own .env.example when it carries ${tt:...} tokens
    // (the committed convention), else the hub-side sidecar
    let repo_template = dir.join(".env.example");
    let sidecar = sr.root.join(TEMPLATE_SIDECAR);
    let template_path = match fs::read_to_string(&repo_template) {
        Ok(text) if text.contains("${tt:") => repo_template,
        _ if sidecar.is_file() => sidecar,
        _ => {
            return Err(format!(
                "no template: neither a tokenized .env.example in {name} nor {}",
                sidecar.display()
            ));
        }
    };
    let template = fs::read_to_string(&template_path)
        .map_err(|e| format!("cannot read {}: {e}", template_path.display()))?;

    let _lock = ClaimLock::acquire(&sr.hub)?;

    let env_path = dir.join(".env");
    let old_text = fs::read_to_string(&env_path).unwrap_or_default();
    let existing: BTreeMap<String, String> = envfile::parse(&old_text).into_iter().collect();

    let mut sibling_claims = std::collections::BTreeSet::new();
    for (_, _, sib_dir) in sr.slots() {
        if sib_dir == dir {
            continue;
        }
        if let Ok(text) = fs::read_to_string(sib_dir.join(".env")) {
            sibling_claims.extend(envfile::port_claims(&text));
        }
    }

    let ctx =
        SlotContext { slot_name: &name, slot_number: number, base_branch: &base_branch(&sr.hub) };
    let outcome =
        tt_slots::render(&template, &ctx, &existing, &sibling_claims, |p| !port_occupied(p))
            .map_err(|e| format!("{}: {e}", template_path.display()))?;

    let (merged, preserved) = envfile::merge_missing_keys(&outcome.text, &old_text);
    fs::write(&env_path, &merged)
        .map_err(|e| format!("cannot write {}: {e}", env_path.display()))?;

    let marker = layout::marker_contents(&name, ctx.base_branch, "main");
    fs::write(dir.join(layout::MARKER_FILE), marker)
        .map_err(|e| format!("cannot write {}: {e}", layout::MARKER_FILE))?;
    ensure_hub_excludes(&sr.hub)?;
    if let Ok(out) = git_slot(dir, &["check-ignore", "-q", ".env"])
        && !out.ok()
    {
        ui::warning(".env is NOT gitignored in this repo — it will dirty the slot's tree");
    }

    let ports = outcome.reused.iter().chain(outcome.claimed.iter()).cloned().collect();
    Ok(RenderSummary {
        ports,
        reused: outcome.reused.len(),
        claimed: outcome.claimed.len(),
        preserved,
    })
}

/// Ignore the marker in every worktree via the hub's `info/exclude` — no repo
/// `.gitignore` commit needed.
fn ensure_hub_excludes(hub: &Path) -> Result<(), String> {
    let info = hub.join("info");
    let exclude = info.join("exclude");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|l| l.trim() == layout::MARKER_FILE) {
        return Ok(());
    }
    fs::create_dir_all(&info).map_err(|e| format!("cannot create {}: {e}", info.display()))?;
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(layout::MARKER_FILE);
    next.push('\n');
    fs::write(&exclude, next).map_err(|e| format!("cannot write {}: {e}", exclude.display()))
}

// ---------------------------------------------------------------------------
// commands

fn cmd_new(branch: Option<&str>, json: bool, root: Option<&Path>) -> Result<(), String> {
    let sr = discover_root(root)?;
    let _ = git_hub(&sr.hub, &["worktree", "prune"]);
    if let Ok(out) = git_hub(&sr.hub, &["fetch", "--quiet", "origin"])
        && !out.ok()
    {
        ui::warning("fetch failed (offline?) — using local refs");
    }
    let base = base_branch(&sr.hub);
    let number = layout::next_slot_number(&sr.repo, &dir_names(&sr.root));
    let name = layout::slot_dir_name(&sr.repo, number);
    let dir = sr.slot_dir(&name);
    let dir_s = dir.to_string_lossy().to_string();

    let add_result = match branch {
        Some(b) => git_hub(&sr.hub, &["worktree", "add", "-b", b, &dir_s, &base])?,
        None => git_hub(&sr.hub, &["worktree", "add", "--detach", &dir_s, &base])?,
    };
    if !add_result.ok() {
        return Err(format!("git worktree add failed:\n{}", add_result.stderr.trim()));
    }

    let summary = render_slot_env(&sr, &dir)?;

    // inherit secrets from the lowest-numbered sibling that has a .env
    let mut inherited = 0;
    for (_, _, sib_dir) in sr.slots() {
        if sib_dir == dir {
            continue;
        }
        if let Ok(sib_env) = fs::read_to_string(sib_dir.join(".env")) {
            let env_path = dir.join(".env");
            let current = fs::read_to_string(&env_path).unwrap_or_default();
            let (merged, count) = envfile::merge_missing_keys(&current, &sib_env);
            fs::write(&env_path, merged).map_err(|e| format!("cannot write .env: {e}"))?;
            inherited = count;
            break;
        }
    }

    let hook = sr.root.join(SETUP_HOOK);
    if is_executable(&hook) {
        ui::info("running slot-setup.sh…");
        let hook_s = hook.to_string_lossy().to_string();
        match tt_exec::run_in_dir_with_timeout(&hook_s, &[], &dir, SETUP_TIMEOUT) {
            Ok(out) if out.ok() => {}
            Ok(out) => ui::warning(&format!(
                "slot-setup.sh failed (exit {}) — slot kept, fix and re-run it\n{}",
                out.exit_code,
                out.stderr.trim()
            )),
            Err(e) => ui::warning(&format!("slot-setup.sh failed — slot kept: {e}")),
        }
    }

    if json {
        let ports: serde_json::Map<String, serde_json::Value> =
            summary.ports.iter().map(|(k, p)| (k.clone(), (*p).into())).collect();
        let value = serde_json::json!({
            "name": name,
            "dir": dir_s,
            "branch": branch,
            "detached": branch.is_none(),
            "base": base,
            "ports": ports,
            "inheritedKeys": inherited,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
    } else {
        match branch {
            Some(b) => ui::success(&format!("created {name} on branch {b}")),
            None => ui::success(&format!(
                "created {name} (detached at {base} — create a branch before committing)"
            )),
        }
        for (key, port) in &summary.ports {
            println!("  {key}={port}");
        }
        if inherited > 0 {
            println!("  inherited {inherited} key(s) from a sibling slot");
        }
        println!("slot: {dir_s}");
    }
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn cmd_ls(json: bool, root: Option<&Path>) -> Result<(), String> {
    let sr = discover_root(root)?;
    let _ = git_hub(&sr.hub, &["worktree", "prune"]);
    let mut rows = Vec::new();
    for (_, name, dir) in sr.slots() {
        let broken = !git_slot(&dir, &["rev-parse", "--is-inside-work-tree"])
            .map(|o| o.ok())
            .unwrap_or(false);
        let (branch, detached, dirty) = if broken {
            ("BROKEN".to_string(), false, 0)
        } else {
            let current = git_slot(&dir, &["branch", "--show-current"])
                .map(|o| o.stdout.trim().to_string())
                .unwrap_or_default();
            let dirty = git_slot(&dir, &["status", "--porcelain"])
                .map(|o| guards::dirty_entry_count(&o.stdout))
                .unwrap_or(0);
            if current.is_empty() {
                let sha = git_slot(&dir, &["rev-parse", "--short", "HEAD"])
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
        rows.push((name, branch, detached, broken, dirty, ports));
    }

    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, branch, detached, broken, dirty, ports)| {
                let port_map: serde_json::Map<String, serde_json::Value> =
                    ports.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
                serde_json::json!({
                    "name": name,
                    "branch": branch,
                    "detached": detached,
                    "broken": broken,
                    "dirty": dirty,
                    "ports": port_map,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
    } else {
        println!("{:<20} {:<36} {:<6} PORTS", "SLOT", "BRANCH", "DIRTY");
        for (name, branch, _, _, dirty, ports) in &rows {
            let ports_s: Vec<String> = ports.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("{name:<20} {branch:<36} {dirty:<6} {}", ports_s.join(" "));
        }
    }
    Ok(())
}

fn cmd_env(name: &str, root: Option<&Path>) -> Result<(), String> {
    let sr = discover_root(root)?;
    let dir = sr.slot_dir(name);
    if !dir.is_dir() {
        return Err(format!("no slot {name} in {}", sr.root.display()));
    }
    let summary = render_slot_env(&sr, &dir)?;
    ui::success(&format!(
        "rendered {name}/.env ({} reused, {} fresh claim(s), {} extra key(s) preserved)",
        summary.reused, summary.claimed, summary.preserved
    ));
    Ok(())
}

fn cmd_rm(name: &str, force: bool, root: Option<&Path>) -> Result<(), String> {
    let sr = discover_root(root)?;
    layout::parse_slot(&sr.repo, name)
        .ok_or_else(|| format!("{name} is not a slot of {} — refusing", sr.repo))?;
    let dir = sr.slot_dir(name);
    if !dir.is_dir() {
        return Err(format!("no slot {name} in {}", sr.root.display()));
    }
    let dir_s = dir.to_string_lossy().to_string();

    // broken worktree: git can't even report status
    if git_slot(&dir, &["status", "--porcelain"]).map(|o| !o.ok()).unwrap_or(true) {
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
        let _ = git_hub(&sr.hub, &["worktree", "prune"]);
        ui::success(&format!("removed {name} (broken)"));
        return Ok(());
    }

    let dirty = git_slot(&dir, &["status", "--porcelain"])
        .map(|o| guards::dirty_entry_count(&o.stdout))
        .unwrap_or(0);
    let unreachable = git_slot(
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
        .filter(|&p| port_occupied(p) && !docker_owns_port(name, p))
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
        git_hub(&sr.hub, &["worktree", "remove", "--force", &dir_s])
    } else {
        git_hub(&sr.hub, &["worktree", "remove", &dir_s])
    };
    match remove {
        Ok(out) if out.ok() => {}
        result => {
            let detail = match result {
                Ok(out) => out.stderr.trim().to_string(),
                Err(e) => e,
            };
            if !force {
                return Err(format!("git worktree remove failed:\n{detail}"));
            }
            ui::warning(&format!("git worktree remove failed ({detail}) — removing directory"));
            fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {dir_s}: {e}"))?;
        }
    }
    let _ = git_hub(&sr.hub, &["worktree", "prune"]);
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
            DOCKER_TIMEOUT,
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
