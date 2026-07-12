//! Slot lifecycle operations shared by the CLI (`ttr slot`) and the app.
//!
//! This module owns the IO and process execution (git via `tt-exec`, env-file
//! writes, bind tests, the claim lock); every *decision* stays in the pure
//! modules ([`crate::template`], [`crate::guards`], [`crate::envfile`],
//! [`crate::layout`]). Callers surface [`CreatedSlot::warnings`] to the user —
//! nothing here prints.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::{TemplateError, envfile, layout};

pub const TEMPLATE_SIDECAR: &str = "slot-env.template";
pub const SETUP_HOOK: &str = "slot-setup.sh";
const LOCK_FILE: &str = "tt-slots.lock";
const LOCK_STALE: Duration = Duration::from_secs(60);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Error)]
pub enum OpsError {
    #[error(
        "no slot root found walking up from {0} — a slot root holds exactly one <repo>.git bare hub"
    )]
    NoHub(String),

    #[error("{dir} contains {count} bare hubs — expected exactly one")]
    MultipleHubs { dir: String, count: usize },

    #[error("{name} is not a slot of {repo} (expected {repo}-slot-N)")]
    NotASlot { name: String, repo: String },

    #[error("git: {0}")]
    Git(String),

    #[error("no template: neither a tokenized .env.example in {slot} nor {sidecar}")]
    NoTemplate { slot: String, sidecar: String },

    #[error(transparent)]
    Template(#[from] TemplateError),

    #[error("{0}")]
    Io(String),

    #[error("timed out waiting for {0} — another slot command may be stuck")]
    LockTimeout(String),
}

pub type Result<T> = std::result::Result<T, OpsError>;

/// A discovered slot root: the parent directory, its bare hub, and repo name.
pub struct SlotRoot {
    pub root: PathBuf,
    pub hub: PathBuf,
    pub repo: String,
}

impl SlotRoot {
    pub fn slot_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Existing slot dirs as (number, name, path), sorted by number.
    pub fn slots(&self) -> Vec<(u32, String, PathBuf)> {
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

/// Find the slot root: `explicit` if given, else walk up from the current
/// working directory looking for a directory holding exactly one bare hub.
pub fn discover_root(explicit: Option<&Path>) -> Result<SlotRoot> {
    let start = match explicit {
        Some(dir) => dir.to_path_buf(),
        None => {
            std::env::current_dir().map_err(|e| OpsError::Io(format!("cannot read cwd: {e}")))?
        }
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
            return Err(OpsError::MultipleHubs {
                dir: dir.display().to_string(),
                count: hubs.len(),
            });
        }
    }
    Err(OpsError::NoHub(start.display().to_string()))
}

pub fn git_hub(hub: &Path, args: &[&str]) -> Result<tt_exec::Output> {
    let hub_s = hub.to_string_lossy();
    let mut full: Vec<&str> = vec!["-C", hub_s.as_ref()];
    full.extend_from_slice(args);
    tt_exec::run("git", &full).map_err(|e| OpsError::Git(e.to_string()))
}

pub fn git_slot(dir: &Path, args: &[&str]) -> Result<tt_exec::Output> {
    tt_exec::run_in_dir_with_timeout("git", args, dir, GIT_TIMEOUT)
        .map_err(|e| OpsError::Git(e.to_string()))
}

/// The hub's default branch (its HEAD), falling back to `main`.
pub fn base_branch(hub: &Path) -> String {
    git_hub(hub, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Every branch in the hub, default branch first, the rest sorted.
pub fn hub_branches(hub: &Path) -> Result<Vec<String>> {
    let default = base_branch(hub);
    let out = git_hub(hub, &["for-each-ref", "refs/heads", "--format=%(refname:short)"])?;
    if !out.ok() {
        return Err(OpsError::Git(out.stderr.trim().to_string()));
    }
    let mut rest: Vec<String> = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|b| !b.is_empty() && *b != default)
        .map(str::to_string)
        .collect();
    rest.sort();
    let mut branches = vec![default];
    branches.extend(rest);
    Ok(branches)
}

pub fn port_occupied(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

// ---------------------------------------------------------------------------
// claim lock — serializes port claims across concurrent creations (parallel
// agents create slots together; without this, both scan siblings before
// either writes, and claim the same ports)

struct ClaimLock {
    path: PathBuf,
}

impl ClaimLock {
    fn acquire(hub: &Path) -> Result<Self> {
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
        Err(OpsError::LockTimeout(path.display().to_string()))
    }
}

impl Drop for ClaimLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// rendering

pub struct RenderSummary {
    pub ports: Vec<(String, u16)>,
    pub reused: usize,
    pub claimed: usize,
    pub preserved: usize,
    pub warnings: Vec<String>,
}

/// Render the slot's `.env`: template → text (reusing existing claims), then
/// merge back any keys the template doesn't know (inherited secrets, local
/// adds), write the `.tt-slot` marker, and keep it ignored via the hub.
pub fn render_slot_env(sr: &SlotRoot, dir: &Path) -> Result<RenderSummary> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| OpsError::Io(format!("bad slot path {}", dir.display())))?
        .to_string();
    let number = layout::parse_slot(&sr.repo, &name)
        .ok_or_else(|| OpsError::NotASlot { name: name.clone(), repo: sr.repo.clone() })?;

    // template: the repo's own .env.example when it carries ${tt:...} tokens
    // (the committed convention), else the hub-side sidecar
    let repo_template = dir.join(".env.example");
    let sidecar = sr.root.join(TEMPLATE_SIDECAR);
    let template_path = match fs::read_to_string(&repo_template) {
        Ok(text) if text.contains("${tt:") => repo_template,
        _ if sidecar.is_file() => sidecar,
        _ => {
            return Err(OpsError::NoTemplate {
                slot: name,
                sidecar: sidecar.display().to_string(),
            });
        }
    };
    let template = fs::read_to_string(&template_path)
        .map_err(|e| OpsError::Io(format!("cannot read {}: {e}", template_path.display())))?;

    let _lock = ClaimLock::acquire(&sr.hub)?;

    let env_path = dir.join(".env");
    let old_text = fs::read_to_string(&env_path).unwrap_or_default();
    let existing: BTreeMap<String, String> = envfile::parse(&old_text).into_iter().collect();

    let mut sibling_claims = BTreeSet::new();
    for (_, _, sib_dir) in sr.slots() {
        if sib_dir == dir {
            continue;
        }
        if let Ok(text) = fs::read_to_string(sib_dir.join(".env")) {
            sibling_claims.extend(envfile::port_claims(&text));
        }
    }

    let ctx = crate::SlotContext {
        slot_name: &name,
        slot_number: number,
        base_branch: &base_branch(&sr.hub),
    };
    let outcome =
        crate::render(&template, &ctx, &existing, &sibling_claims, |p| !port_occupied(p))?;

    let (merged, preserved) = envfile::merge_missing_keys(&outcome.text, &old_text);
    fs::write(&env_path, &merged)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", env_path.display())))?;

    let marker = layout::marker_contents(&name, ctx.base_branch, "main");
    fs::write(dir.join(layout::MARKER_FILE), marker)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", layout::MARKER_FILE)))?;
    ensure_hub_excludes(&sr.hub)?;

    let mut warnings = Vec::new();
    if let Ok(out) = git_slot(dir, &["check-ignore", "-q", ".env"])
        && !out.ok()
    {
        warnings.push(".env is NOT gitignored in this repo — it will dirty the slot's tree".into());
    }

    let ports = outcome.reused.iter().chain(outcome.claimed.iter()).cloned().collect();
    Ok(RenderSummary {
        ports,
        reused: outcome.reused.len(),
        claimed: outcome.claimed.len(),
        preserved,
        warnings,
    })
}

/// Ignore the marker in every worktree via the hub's `info/exclude` — no repo
/// `.gitignore` commit needed.
fn ensure_hub_excludes(hub: &Path) -> Result<()> {
    let info = hub.join("info");
    let exclude = info.join("exclude");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|l| l.trim() == layout::MARKER_FILE) {
        return Ok(());
    }
    fs::create_dir_all(&info)
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", info.display())))?;
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(layout::MARKER_FILE);
    next.push('\n');
    fs::write(&exclude, next)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", exclude.display())))
}

// ---------------------------------------------------------------------------
// creation

#[derive(Debug, Default)]
pub struct CreateOpts {
    /// Slot root; `None` walks up from the current working directory.
    pub root: Option<PathBuf>,
    /// Branch to create and check out; `None` parks detached at the base.
    pub branch: Option<String>,
    /// Base ref for the new branch / detached checkout; `None` = hub HEAD.
    pub base: Option<String>,
    /// Run the repo's committed `slot-setup.sh` in the new slot (deps install
    /// etc.) — the hook is versioned with the repo, not a hub-root sidecar.
    pub run_setup_hook: bool,
}

pub struct CreatedSlot {
    pub name: String,
    pub dir: PathBuf,
    pub branch: Option<String>,
    pub base: String,
    pub ports: Vec<(String, u16)>,
    pub inherited: usize,
    pub warnings: Vec<String>,
}

/// Create the next free slot: worktree (branch or detached at base), rendered
/// `.env` with port claims, sibling-secrets inheritance, setup hook.
pub fn create_slot(opts: &CreateOpts) -> Result<CreatedSlot> {
    let sr = discover_root(opts.root.as_deref())?;
    let mut warnings = Vec::new();
    let _ = git_hub(&sr.hub, &["worktree", "prune"]);
    if let Ok(out) = git_hub(&sr.hub, &["fetch", "--quiet", "origin"])
        && !out.ok()
    {
        warnings.push("fetch failed (offline?) — using local refs".into());
    }
    let base = opts.base.clone().unwrap_or_else(|| base_branch(&sr.hub));
    let number = layout::next_slot_number(&sr.repo, &dir_names(&sr.root));
    let name = layout::slot_dir_name(&sr.repo, number);
    let dir = sr.slot_dir(&name);
    let dir_s = dir.to_string_lossy().to_string();

    let add_result = match &opts.branch {
        Some(b) => git_hub(&sr.hub, &["worktree", "add", "-b", b, &dir_s, &base])?,
        None => git_hub(&sr.hub, &["worktree", "add", "--detach", &dir_s, &base])?,
    };
    if !add_result.ok() {
        return Err(OpsError::Git(format!(
            "git worktree add failed:\n{}",
            add_result.stderr.trim()
        )));
    }

    let summary = render_slot_env(&sr, &dir)?;
    warnings.extend(summary.warnings);

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
            fs::write(&env_path, merged)
                .map_err(|e| OpsError::Io(format!("cannot write .env: {e}")))?;
            inherited = count;
            break;
        }
    }

    if opts.run_setup_hook {
        // The hook is committed in the repo (so it's versioned and travels to
        // teammates), checked out into the slot itself — not a hub-root file.
        let hook = dir.join(SETUP_HOOK);
        if is_executable(&hook) {
            let hook_s = hook.to_string_lossy().to_string();
            match tt_exec::run_in_dir_with_timeout(&hook_s, &[], &dir, SETUP_TIMEOUT) {
                Ok(out) if out.ok() => {}
                Ok(out) => warnings.push(format!(
                    "slot-setup.sh failed (exit {}) — slot kept, fix and re-run it\n{}",
                    out.exit_code,
                    out.stderr.trim()
                )),
                Err(e) => warnings.push(format!("slot-setup.sh failed — slot kept: {e}")),
            }
        }
    }

    Ok(CreatedSlot {
        name,
        dir,
        branch: opts.branch.clone(),
        base,
        ports: summary.ports,
        inherited,
        warnings,
    })
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
