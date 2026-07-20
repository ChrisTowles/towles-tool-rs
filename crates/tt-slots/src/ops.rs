//! Slot lifecycle operations shared by the CLI (`tt slot`) and the app.
//!
//! This module owns the IO and process execution (git via `tt-exec`, env-file
//! writes, bind tests, the claim lock); every *decision* stays in the pure
//! modules ([`crate::template`], [`crate::guards`], [`crate::envfile`],
//! [`crate::layout`]). Callers surface [`CreatedSlot::warnings`] to the user —
//! nothing here prints.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::guards::{ForeignPort, RmBlocked};
use crate::{TemplateError, envfile, layout};

pub const TEMPLATE_SIDECAR: &str = "slot-env.template";
/// Declared setup command, read from the slot's rendered `.env`
/// (e.g. `TT_SLOT_SETUP=bun install`). Spawned directly — no shell — so a
/// repo needing more than one command should point this at its own task
/// runner (`make setup`, `npm run bootstrap`).
pub const SETUP_ENV_KEY: &str = "TT_SLOT_SETUP";
const LOCK_FILE: &str = "tt-slots.lock";
const LOCK_STALE: Duration = Duration::from_secs(60);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// The pre-flight `fetch` in [`create_slot`] is best-effort freshness, not
/// required for correctness (a failure just falls back to local refs with a
/// warning) — so it gets a shorter leash than [`GIT_TIMEOUT`] and fails fast
/// on a slow/inspected network instead of blocking creation for up to 30s.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);
/// Below this, a creation step's duration is unremarkable — normal git/
/// filesystem work on a local repo. Above it, something environmental is
/// probably adding the time (a slow or TLS-inspecting proxy, antivirus/EDR
/// intercepting every file write, Spotlight indexing a freshly checked-out
/// tree — all disproportionately common on a managed corporate laptop), so
/// [`create_slot`] names the step and its duration in a warning instead of
/// letting it pass silently.
const SLOW_STEP: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum OpsError {
    #[error("no git checkout found walking up from {0}")]
    NoCheckout(String),

    #[error("cannot derive a slot name from branch {0}")]
    BadBranchName(String),

    #[error("'{branch}' is not a valid branch name: {detail}")]
    InvalidBranchName { branch: String, detail: String },

    #[error("slot {name} already exists at {dir}")]
    SlotExists { name: String, dir: String },

    #[error("git: {0}")]
    Git(String),

    #[error("env template {path}: {source}")]
    Template { path: String, source: TemplateError },

    #[error("{0}")]
    Io(String),

    #[error("timed out waiting for {0} — another slot command may be stuck")]
    LockTimeout(String),

    #[error("refusing to remove the primary checkout — it owns every slot's git state")]
    PrimaryRemoval,

    #[error("no slot {name} in {slots_dir}")]
    NoSuchSlot { name: String, slots_dir: String },

    #[error(
        "{name}'s worktree is broken (git fails inside it) — re-run with --force to remove anyway"
    )]
    BrokenWorktree { name: String },

    // NOTE: a guard refusal is deliberately NOT here — see [`RemoveOutcome`].
    #[error(transparent)]
    Port(#[from] crate::ports::PortError),

    #[error(
        "port {port} is not claimed by {name} — refusing to signal a process this slot does not own"
    )]
    PortNotClaimed { name: String, port: u16 },
}

pub type Result<T> = std::result::Result<T, OpsError>;

/// A discovered slot root: the repo's main checkout and the repo name (its
/// directory basename). Slots nest inside the checkout at
/// `.claude/worktrees/<name>` — see the [`crate::layout`] docs.
pub struct SlotRoot {
    /// The main checkout — a normal clone whose `.git` directory owns every
    /// slot's git state.
    pub checkout: PathBuf,
    pub repo: String,
}

impl SlotRoot {
    /// The directory holding the worktree slots (may not exist yet).
    pub fn slots_dir(&self) -> PathBuf {
        layout::worktrees_dir(&self.checkout)
    }

    pub fn slot_dir(&self, name: &str) -> PathBuf {
        self.slots_dir().join(name)
    }

    /// Existing slot dirs as (name, path), sorted by name.
    pub fn slots(&self) -> Vec<(String, PathBuf)> {
        let mut slots: Vec<(String, PathBuf)> = dir_names(&self.slots_dir())
            .into_iter()
            .map(|name| {
                let path = self.slots_dir().join(&name);
                (name, path)
            })
            .collect();
        slots.sort();
        slots
    }

    /// The checkout plus every slot — every checkout whose `.env` can hold
    /// port claims.
    pub fn checkouts(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.checkout.clone()];
        dirs.extend(self.slots().into_iter().map(|(_, dir)| dir));
        dirs
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

/// Resolve the *main* checkout for `dir`, which must contain `.git`: a `.git`
/// directory means `dir` is the main checkout itself; a `.git` *file* (a
/// linked worktree) points at `<main>/.git/worktrees/<wt>` — hop to `<main>`,
/// so slot commands anchor at the repo root no matter which worktree they run
/// from. A `.git` file that is not a worktree pointer (a submodule's
/// `gitdir: ../.git/modules/<x>`) keeps `dir` itself as the checkout — a
/// submodule is its own repo and gets its own nested worktrees.
fn main_checkout(dir: &Path) -> PathBuf {
    let dotgit = dir.join(".git");
    if dotgit.is_dir() {
        return dir.to_path_buf();
    }
    let Some(gitdir) = fs::read_to_string(&dotgit)
        .ok()
        .and_then(|text| text.strip_prefix("gitdir:").map(|p| p.trim().to_string()))
    else {
        return dir.to_path_buf();
    };
    let gitdir =
        if Path::new(&gitdir).is_absolute() { PathBuf::from(gitdir) } else { dir.join(gitdir) };
    // `<main>/.git/worktrees/<wt>` → `<main>`; anything else is not a linked
    // worktree.
    for ancestor in gitdir.ancestors() {
        if ancestor.file_name().is_some_and(|n| n == ".git")
            && gitdir
                .strip_prefix(ancestor)
                .ok()
                .and_then(|rest| rest.components().next())
                .is_some_and(|c| c.as_os_str() == "worktrees")
            && let Some(main) = ancestor.parent()
        {
            return main.to_path_buf();
        }
    }
    dir.to_path_buf()
}

/// Find the slot root: walk up from `explicit` (or the current working
/// directory) to the nearest dir containing `.git`, then hop from a linked
/// worktree to its main checkout (see [`main_checkout`]) — so running from
/// inside a slot anchors at the repo root, never nesting worktrees inside
/// worktrees. Any plain git checkout qualifies; there is no layout to set up.
pub fn discover_root(explicit: Option<&Path>) -> Result<SlotRoot> {
    let start = match explicit {
        Some(dir) => dir.to_path_buf(),
        None => {
            std::env::current_dir().map_err(|e| OpsError::Io(format!("cannot read cwd: {e}")))?
        }
    };
    for dir in start.ancestors() {
        if !dir.join(".git").exists() {
            continue;
        }
        let checkout = main_checkout(dir);
        let repo = checkout
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| OpsError::Io(format!("bad checkout path {}", checkout.display())))?
            .to_string();
        return Ok(SlotRoot { checkout, repo });
    }
    Err(OpsError::NoCheckout(start.display().to_string()))
}

/// Bounded like [`git_slot`] — a stalled network op (stuck proxy/VPN, an SSH
/// prompt with nothing to answer it) must fail after `GIT_TIMEOUT`, not hang
/// the caller (`create_slot`/`remove_slot`/`clean_slots`) forever.
pub fn git_checkout(checkout: &Path, args: &[&str]) -> Result<tt_exec::Output> {
    git_checkout_timeout(checkout, args, GIT_TIMEOUT)
}

/// [`git_checkout`] with an explicit timeout — for a step that's best-effort
/// and should fail fast rather than eat the full [`GIT_TIMEOUT`] (see
/// [`FETCH_TIMEOUT`]).
fn git_checkout_timeout(
    checkout: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<tt_exec::Output> {
    let checkout_s = checkout.to_string_lossy();
    let mut full: Vec<&str> = vec!["-C", checkout_s.as_ref()];
    full.extend_from_slice(args);
    tt_exec::run_with_timeout_env("git", &full, tt_exec::GIT_NON_INTERACTIVE_ENV, timeout)
        .map_err(|e| OpsError::Git(e.to_string()))
}

/// Push a warning naming `label` and its duration when a creation step ran
/// long enough that something outside normal git/filesystem work is likely
/// the cause — see [`SLOW_STEP`].
fn note_if_slow(warnings: &mut Vec<String>, label: &str, elapsed: Duration) {
    if elapsed > SLOW_STEP {
        warnings.push(format!("{label} took {:.1}s — slower than expected", elapsed.as_secs_f64()));
    }
}

/// The refs a slot's work is judged against: the checkout's base branch, and
/// its remote-tracking twin when one exists.
///
/// Both are needed because they answer at different times. A squash merge
/// lands on `origin/<base>` the moment the PR is merged, while local `<base>`
/// only catches up when the user pulls — so judging against the local ref
/// alone makes every merged slot look active until the next `git pull`.
#[derive(Debug, Clone)]
pub struct BaseRefs {
    /// Base branch name, e.g. `main`.
    pub base: String,
    /// `refs/heads/<base>`.
    pub local: String,
    /// `refs/remotes/origin/<base>`, when it resolves.
    pub remote: Option<String>,
}

/// Resolve the base refs for a checkout. One set of git calls, reused across
/// every slot by callers that loop.
pub fn base_refs(checkout: &Path) -> BaseRefs {
    let base = base_branch(checkout);
    let local = format!("refs/heads/{base}");
    let candidate = format!("refs/remotes/origin/{base}");
    let remote = git_checkout(checkout, &["rev-parse", "--quiet", "--verify", &candidate])
        .ok()
        .filter(|o| o.ok())
        .map(|_| candidate);
    BaseRefs { base, local, remote }
}

/// What a slot still holds — uncommitted work and commits that never reached
/// the base — as one answer shared by `ls`, `rm`, `clean` and the Agentboard
/// rail. See [`crate::landed`] for why several git signals are combined.
///
/// `branch` is a full ref (`refs/heads/<name>`). Best-effort: git failures
/// degrade to "work is present", never to "safe to delete".
///
/// `uncommitted` and `orphaned` are passed in rather than gathered here. Every
/// caller either already has them (`remove_slot` computes both for
/// [`crate::guards::check_removal`]) or needs them for a checkout this function
/// cannot judge (a detached HEAD has no branch to compare). Re-reading them
/// would also mean two snapshots of one working tree, so the guard could pass
/// on a clean tree while the message reported uncommitted files.
/// [`uncommitted_count`] and [`orphaned_count`] produce them.
pub fn work_state(
    refs: &BaseRefs,
    dir: &Path,
    branch: &str,
    uncommitted: usize,
    orphaned: u64,
) -> crate::landed::WorkState {
    use crate::landed::{LandedVia, WorkState, probe_work_state};

    // `Some` only on a zero exit — `merge-base --is-ancestor` answers through
    // the exit code and prints nothing.
    let probe_git = |dir: &Path, args: &[&str]| -> Option<String> {
        git_slot(dir, args).ok().filter(|o| o.ok()).map(|o| o.stdout)
    };

    let gone = probe_git(dir, &["for-each-ref", branch, "--format=%(upstream:track)"])
        .map(|out| crate::clean::upstream_gone(&out))
        .unwrap_or(false);

    let probe =
        |base: &str| probe_work_state(&probe_git, dir, base, branch, uncommitted, orphaned, gone);
    let proven = |w: &WorkState| w.landed.is_some_and(LandedVia::is_content_proof);

    // Judge against the local base first, then the remote-tracking one. A
    // squash merge lands on `origin/<base>` and nothing here fast-forwards
    // local `<base>`, so a checkout that has not pulled since the merge would
    // otherwise read every merged slot as active. The local ref is still asked
    // first so a repo with no remote, or one merged only locally, keeps
    // working. The retry runs whenever the local base gave no *content* proof
    // — a bare `[gone]` upstream included, since that is exactly the shape a
    // squash merge leaves behind.
    let local = probe(&refs.local);
    if proven(&local) {
        return local;
    }
    match refs.remote.as_deref().map(&probe).filter(&proven) {
        Some(remote) => remote,
        None => local,
    }
}

/// `git status --porcelain` entry count for a checkout — the uncommitted axis.
pub fn uncommitted_count(dir: &Path) -> usize {
    git_slot(dir, &["status", "--porcelain"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| crate::guards::dirty_entry_count(&o.stdout))
        .unwrap_or(0)
}

/// Commits reachable from no branch and no remote — the orphaned axis, the one
/// removal genuinely destroys. Base-independent, so it is meaningful even for a
/// detached HEAD that [`work_state`] cannot otherwise judge.
pub fn orphaned_count(dir: &Path) -> u64 {
    git_slot(
        dir,
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
    .and_then(|o| crate::guards::unreachable_commit_count(&o.stdout))
    .unwrap_or(0)
}

pub fn git_slot(dir: &Path, args: &[&str]) -> Result<tt_exec::Output> {
    tt_exec::run_in_dir_with_timeout_env(
        "git",
        args,
        dir,
        tt_exec::GIT_NON_INTERACTIVE_ENV,
        GIT_TIMEOUT,
    )
    .map_err(|e| OpsError::Git(e.to_string()))
}

/// The checkout's checked-out branch (the repo default), falling back to `main`.
pub fn base_branch(checkout: &Path) -> String {
    git_checkout(checkout, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Fast-forward `base` to `upstream` (its `origin/<base>` counterpart, per
/// [`effective_origin_base`] — the caller decides applicability) — so a new
/// slot branches from current history instead of stale local history, which
/// otherwise means its first sync with base is an unnecessary rebase.
/// `git merge --ff-only` is itself a no-op when already current, so this
/// attempts it unconditionally rather than counting commits behind first.
/// Only ever touches the main checkout's own branch — the predicate returns
/// `None` for a tag, a SHA, a branch checked out in a different worktree, or
/// one with no `origin/<base>` upstream, since moving a ref out from under
/// another checkout would fight whatever's using it. A genuine divergence
/// (or uncommitted local changes ff-only won't overwrite) warns rather than
/// blocks creation — the slot still branches from local history.
fn fast_forward_base_if_behind(
    sr: &SlotRoot,
    base: &str,
    upstream: &str,
    warnings: &mut Vec<String>,
) {
    match git_checkout(&sr.checkout, &["merge", "--ff-only", upstream]) {
        Ok(out) if out.ok() => {}
        Ok(out) => warnings.push(format!(
            "base branch '{base}' has diverged from {upstream} and could not be fast-forwarded \
             ({}) — the new slot may need a rebase later",
            out.stderr.trim()
        )),
        Err(e) => warnings
            .push(format!("base branch '{base}' could not be checked against {upstream}: {e}")),
    }
}

/// The ref slot creation will *effectively* branch from when it applies the
/// fast-forward above: `Some("origin/<base>")` exactly when `base` is the
/// checkout's checked-out branch and that remote-tracking ref exists, `None`
/// otherwise. The single copy of that rule, shared by
/// [`fast_forward_base_if_behind`] (which acts on it) and
/// [`checkout_branches`] (which labels the form with it) — two independent
/// derivations here would let the form's label drift from what creation
/// actually does, the exact bug the label exists to fix.
fn effective_origin_base(checkout: &Path, base: &str) -> Option<String> {
    if base_branch(checkout) != base {
        return None;
    }
    let upstream = format!("origin/{base}");
    let exists = git_checkout(checkout, &["rev-parse", "--verify", "--quiet", &upstream])
        .map(|o| o.ok())
        .unwrap_or(false);
    exists.then_some(upstream)
}

/// One base-branch choice for the new-slot form. `name` is the local branch —
/// what `create_slot` takes as `base` — and `label` is the ref creation will
/// *effectively* branch from ([`effective_origin_base`]), which the UI should
/// show instead of `name`: a form showing plain `main` when creation will
/// branch from `origin/main` would be underselling what actually happens.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BaseBranch {
    pub name: String,
    pub label: String,
}

/// Every local branch as a [`BaseBranch`], default branch first, the rest
/// sorted.
pub fn checkout_branches(checkout: &Path) -> Result<Vec<BaseBranch>> {
    let default = base_branch(checkout);
    let out = git_checkout(checkout, &["for-each-ref", "refs/heads", "--format=%(refname:short)"])?;
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
    // Only the default entry can earn an `origin/` label — the fast-forward
    // only ever applies to the checkout's own checked-out branch.
    let label = effective_origin_base(checkout, &default).unwrap_or_else(|| default.clone());
    let mut branches = vec![BaseBranch { name: default, label }];
    branches.extend(rest.into_iter().map(|b| BaseBranch { name: b.clone(), label: b }));
    Ok(branches)
}

/// Validate `branch` as a git ref via `git check-ref-format --branch` — git
/// is the authority on legal ref names, so this shells out to it rather than
/// reimplementing the rules. Stateless: `check-ref-format` needs no repo.
pub fn validate_branch_name(branch: &str) -> Result<()> {
    let out = tt_exec::run("git", &["check-ref-format", "--branch", branch])
        .map_err(|e| OpsError::Git(e.to_string()))?;
    if out.ok() {
        return Ok(());
    }
    Err(OpsError::InvalidBranchName {
        branch: branch.to_string(),
        detail: out.stderr.trim().to_string(),
    })
}

/// Preflight for the new-slot dialog: is `branch` a legal ref, and would its
/// derived slot name collide with an existing slot? Read-only.
pub struct BranchCheck {
    pub name: Option<String>,
    pub taken: bool,
    pub error: Option<String>,
}

pub fn check_branch(sr: &SlotRoot, branch: &str) -> BranchCheck {
    if let Err(e) = validate_branch_name(branch) {
        return BranchCheck { name: None, taken: false, error: Some(e.to_string()) };
    }
    match layout::slot_name_from_branch(branch) {
        Some(name) => {
            let taken = sr.slot_dir(&name).exists();
            BranchCheck { name: Some(name), taken, error: None }
        }
        None => BranchCheck {
            name: None,
            taken: false,
            error: Some(OpsError::BadBranchName(branch.to_string()).to_string()),
        },
    }
}

pub fn port_occupied(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

// ---------------------------------------------------------------------------
// claim lock — serializes port claims across concurrent creations (parallel
// agents create slots together; without this, both scan siblings before
// either writes, and claim the same ports)

/// Path of the claim lock for `checkout`, in `tt_config::locks_dir()` and
/// keyed by a hash of the checkout path. Deliberately *not* inside the
/// repo's `.git/` — that directory is git's own, and a third-party tool
/// dropping state next to git's index/ref locks is not ours to do. The hash
/// only has to be per-checkout-unique, not cryptographic (a collision would
/// serialize two unrelated repos' claims — slower, never incorrect), so the
/// stdlib hasher is enough; the checkout's basename is kept as a readable
/// prefix so a stuck lock names the repo it belongs to.
fn claim_lock_path(checkout: &Path) -> PathBuf {
    let mut h = DefaultHasher::new();
    checkout.hash(&mut h);
    let repo = checkout.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    tt_config::locks_dir().join(format!("{repo}-{:016x}-{LOCK_FILE}", h.finish()))
}

struct ClaimLock {
    path: PathBuf,
}

impl ClaimLock {
    fn acquire(checkout: &Path) -> Result<Self> {
        let path = claim_lock_path(checkout);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", parent.display())))?;
        }
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

/// The template sidecar's path: `<checkout>/.claude/slot-env.template`,
/// next to the repo's Claude Code settings (committable, but gitignoring it
/// works too — render only reads it).
pub fn template_sidecar_path(sr: &SlotRoot) -> PathBuf {
    sr.checkout.join(layout::CLAUDE_DIR).join(TEMPLATE_SIDECAR)
}

/// Create the [`TEMPLATE_SIDECAR`] for repos that don't commit a tokenized
/// `.env.example` (`tt slot init`). Purely a starting point: a repo with no
/// template at all still renders slots (an empty `.env` — see
/// [`render_slot_env`]), so the sidecar exists to give `${tt:...}` tokens an
/// obvious home when the repo later needs one.
/// Idempotent: an existing sidecar is left untouched.
pub fn init_template_sidecar(sr: &SlotRoot) -> Result<PathBuf> {
    let sidecar = template_sidecar_path(sr);
    if sidecar.is_file() {
        return Ok(sidecar);
    }
    let claude_dir = sr.checkout.join(layout::CLAUDE_DIR);
    fs::create_dir_all(&claude_dir)
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", claude_dir.display())))?;
    fs::write(
        &sidecar,
        "# tt slot env template — this repo declares no ports/env vars for slots.\n\
         # Add ${tt:port A-B} / ${tt:var NAME} / ${tt:slot-name} / ${tt:base} tokens\n\
         # here (or commit a tokenized .env.example in the repo instead) if a slot\n\
         # ever needs one.\n",
    )
    .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", sidecar.display())))?;
    Ok(sidecar)
}

#[derive(Debug)]
pub struct RenderSummary {
    pub ports: Vec<(String, u16)>,
    pub reused: usize,
    pub claimed: usize,
    pub preserved: usize,
    pub warnings: Vec<String>,
}

/// Render a checkout's `.env`: template → text (reusing existing claims),
/// then merge back any keys the template doesn't know (inherited secrets,
/// local adds). Works for slots and for the checkout itself — the checkout is
/// where the user runs the app, so it claims ports like any slot. Slot dirs
/// also get the `.tt-slot` marker.
///
/// `new_slot_base` seeds the marker's `base=` field the *first* time a slot
/// is rendered (at creation, when `dir` has no marker yet) — it should be the
/// actual ref the worktree was created from ([`create_slot`]'s resolved
/// `base`), not the checkout's current branch. A re-render of an *existing*
/// slot (`tt slot env <name>`) ignores this and keeps the marker's already
/// recorded base: it's fixed at creation and must never drift just because
/// the checkout's branch or default has since changed.
pub fn render_slot_env(
    sr: &SlotRoot,
    dir: &Path,
    new_slot_base: Option<&str>,
) -> Result<RenderSummary> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| OpsError::Io(format!("bad slot path {}", dir.display())))?
        .to_string();
    let is_slot = dir.parent().is_some_and(|p| p == sr.slots_dir());

    // template: the repo's own .env.example when it carries ${tt:...} tokens
    // (the committed convention), else the .claude/ sidecar (repos that
    // don't commit tt tokens in their .env.example), else empty — a repo
    // that declares nothing to template (no ports, no per-slot config) still
    // renders (an empty .env), so any plain checkout is slot-capable with no
    // onboarding step.
    let repo_template = dir.join(".env.example");
    let sidecar = template_sidecar_path(sr);
    let (template_path, template) = match fs::read_to_string(&repo_template) {
        Ok(text) if text.contains("${tt:") => (repo_template, text),
        _ if sidecar.is_file() => {
            let text = fs::read_to_string(&sidecar)
                .map_err(|e| OpsError::Io(format!("cannot read {}: {e}", sidecar.display())))?;
            (sidecar, text)
        }
        _ => (PathBuf::new(), String::new()),
    };

    let _lock = ClaimLock::acquire(&sr.checkout)?;

    let env_path = dir.join(".env");
    let old_text = fs::read_to_string(&env_path).unwrap_or_default();
    let existing: BTreeMap<String, String> = envfile::parse(&old_text).into_iter().collect();

    let mut sibling_claims = BTreeSet::new();
    for sib_dir in sr.checkouts() {
        if sib_dir == dir {
            continue;
        }
        if let Ok(text) = fs::read_to_string(sib_dir.join(".env")) {
            sibling_claims.extend(envfile::port_claims(&text));
        }
    }

    // A marker already on disk (re-rendering an existing slot) wins over
    // `new_slot_base` — the base is set once at creation, not re-derived on
    // every `tt slot env`. Only a fresh slot (no marker yet) or the checkout
    // (never gets a marker) falls back to `new_slot_base`/the checkout's branch.
    let recorded_base = layout::read_slot_base(dir);
    let ctx_base = recorded_base
        .clone()
        .or_else(|| new_slot_base.map(str::to_string))
        .unwrap_or_else(|| base_branch(&sr.checkout));
    let ctx = crate::SlotContext { slot_name: &name, base_branch: &ctx_base };
    let outcome = crate::render(&template, &ctx, &existing, &sibling_claims, |p| !port_occupied(p))
        .map_err(|source| OpsError::Template {
            // an empty (no-template) render can't fail, so this always names
            // a real file
            path: template_path.display().to_string(),
            source,
        })?;

    let (merged, preserved) = envfile::merge_missing_keys(&outcome.text, &old_text);
    fs::write(&env_path, &merged)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", env_path.display())))?;

    if is_slot {
        let marker = layout::marker_contents(&name, &ctx_base, "main");
        fs::write(dir.join(layout::MARKER_FILE), marker)
            .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", layout::MARKER_FILE)))?;
    }
    ensure_excludes(&sr.checkout)?;

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

/// Ignore the marker and the nested worktrees dir via the main checkout's
/// `.git/info/exclude` — no repo `.gitignore` commit needed. The worktrees
/// entry keeps `git status` at the checkout root clean even in repos that
/// never added `.claude/worktrees/` to their `.gitignore`.
fn ensure_excludes(checkout: &Path) -> Result<()> {
    let info = checkout.join(".git").join("info");
    let exclude = info.join("exclude");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    let worktrees_entry = format!("{}/{}/", layout::CLAUDE_DIR, layout::WORKTREES_DIR);
    let missing: Vec<&str> = [layout::MARKER_FILE, worktrees_entry.as_str()]
        .into_iter()
        .filter(|entry| !current.lines().any(|l| l.trim() == *entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(&info)
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", info.display())))?;
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    for entry in missing {
        next.push_str(entry);
        next.push('\n');
    }
    fs::write(&exclude, next)
        .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", exclude.display())))
}

// ---------------------------------------------------------------------------
// init — one-shot repo onboarding (`tt slot init`)

/// The two Claude Code worktree hooks `tt slot init` wires so `claude
/// --worktree` and background sessions route through the slot machinery.
const WORKTREE_HOOKS: [(&str, &str); 2] = [
    ("WorktreeCreate", "tt slot hook-create"),
    ("WorktreeRemove", "tt slot hook-remove"),
];

/// What [`init_repo`] did (every step is idempotent, so re-runs report
/// mostly `false`/unchanged).
pub struct InitReport {
    /// The template slots will render from: the repo's tokenized
    /// `.env.example`, or the `.claude/slot-env.template` sidecar.
    pub template: PathBuf,
    pub sidecar_created: bool,
    /// `.env` was appended to the repo's `.gitignore`.
    pub gitignore_added: bool,
    /// The worktree hooks were added to `.claude/settings.json`.
    pub hooks_wired: bool,
    pub settings_path: PathBuf,
    /// The primary checkout's `.env` render (it claims ports like any slot).
    pub render: RenderSummary,
}

/// Onboard a repo onto the slot convention in one idempotent shot: pick (or
/// create) the env template, gitignore `.env`, wire the Claude Code
/// WorktreeCreate/WorktreeRemove hooks into `.claude/settings.json`, and
/// render the primary checkout's `.env` so it claims its ports. The hook
/// wiring only takes effect in new worktrees once the settings file is
/// committed — the caller surfaces that reminder.
pub fn init_repo(sr: &SlotRoot) -> Result<InitReport> {
    // Template: the committed tokenized .env.example wins; otherwise make
    // sure the sidecar exists (empty-but-explained when freshly created).
    let repo_template = sr.checkout.join(".env.example");
    let has_tokenized_example =
        fs::read_to_string(&repo_template).is_ok_and(|text| text.contains("${tt:"));
    let (template, sidecar_created) = if has_tokenized_example {
        (repo_template, false)
    } else {
        let existed = template_sidecar_path(sr).is_file();
        (init_template_sidecar(sr)?, !existed)
    };

    // Gitignore `.env` only when git says it is definitely not ignored
    // (check-ignore exits 1); an errored probe (128 — odd repo state) must
    // not append blindly.
    let mut gitignore_added = false;
    if let Ok(out) = git_checkout(&sr.checkout, &["check-ignore", "-q", ".env"])
        && out.exit_code == 1
    {
        let gitignore = sr.checkout.join(".gitignore");
        let mut current = fs::read_to_string(&gitignore).unwrap_or_default();
        if !current.is_empty() && !current.ends_with('\n') {
            current.push('\n');
        }
        current.push_str(".env\n");
        fs::write(&gitignore, current)
            .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", gitignore.display())))?;
        gitignore_added = true;
    }

    // Hooks: merge into the committed settings file, preserving everything
    // already there.
    let settings_path = sr.checkout.join(layout::CLAUDE_DIR).join("settings.json");
    let current = fs::read_to_string(&settings_path).unwrap_or_default();
    let (wired_text, hooks_wired) = wire_worktree_hooks(&current)?;
    if hooks_wired {
        let claude_dir = sr.checkout.join(layout::CLAUDE_DIR);
        fs::create_dir_all(&claude_dir)
            .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", claude_dir.display())))?;
        fs::write(&settings_path, wired_text)
            .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", settings_path.display())))?;
    }

    let render = render_slot_env(sr, &sr.checkout, None)?;
    Ok(InitReport {
        template,
        sidecar_created,
        gitignore_added,
        hooks_wired,
        settings_path,
        render,
    })
}

/// Merge the [`WORKTREE_HOOKS`] into a `.claude/settings.json` document,
/// preserving every existing key/hook. Returns the new JSON text and whether
/// anything changed (an event already carrying its `tt slot hook-*` command
/// anywhere in its entries is left alone). Empty input starts from `{}`;
/// malformed JSON is an error — never clobber a file we can't parse.
pub fn wire_worktree_hooks(settings: &str) -> Result<(String, bool)> {
    let mut doc: serde_json::Value = if settings.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(settings)
            .map_err(|e| OpsError::Io(format!(".claude/settings.json is not valid JSON: {e}")))?
    };
    if !doc.is_object() {
        return Err(OpsError::Io(".claude/settings.json is not a JSON object".to_string()));
    }

    let mut changed = false;
    let hooks = doc
        .as_object_mut()
        .expect("checked is_object above")
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        return Err(OpsError::Io(".claude/settings.json `hooks` is not an object".to_string()));
    }
    for (event, command) in WORKTREE_HOOKS {
        let entries = hooks
            .as_object_mut()
            .expect("checked is_object above")
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        if !entries.is_array() {
            return Err(OpsError::Io(format!(
                ".claude/settings.json `hooks.{event}` is not an array"
            )));
        }
        let already = entries.as_array().expect("checked is_array above").iter().any(|entry| {
            entry.get("hooks").and_then(|h| h.as_array()).is_some_and(|hs| {
                hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()) == Some(command))
            })
        });
        if !already {
            entries
                .as_array_mut()
                .expect("checked is_array above")
                .push(serde_json::json!({ "hooks": [{ "type": "command", "command": command }] }));
            changed = true;
        }
    }

    let mut text = serde_json::to_string_pretty(&doc)
        .map_err(|e| OpsError::Io(format!("cannot serialize settings.json: {e}")))?;
    text.push('\n');
    Ok((text, changed))
}

// ---------------------------------------------------------------------------
// setup

/// The setup command for a fresh slot, as argv. `env` is the slot's rendered
/// `.env`; `has_file` probes the slot's checkout. Declared `TT_SLOT_SETUP`
/// wins (whitespace-split — point it at a task runner for anything fancier);
/// else the package manager is detected from the committed lockfile. `None`
/// means nothing to run (e.g. a pure-cargo repo whose deps resolve on build).
pub fn setup_command(
    env: &BTreeMap<String, String>,
    mut has_file: impl FnMut(&str) -> bool,
) -> Option<Vec<String>> {
    if let Some(declared) = env.get(SETUP_ENV_KEY) {
        let argv: Vec<String> = declared.split_whitespace().map(str::to_string).collect();
        return (!argv.is_empty()).then_some(argv);
    }
    // npm gets `--prefer-offline`: with a lockfile present, the exact
    // versions are already pinned, so a cache hit needs no network
    // revalidation — a real cut in round-trips behind a slow or
    // TLS-inspecting proxy, with no correctness cost (an uncached package
    // still falls back to the network).
    let by_lockfile: [(&str, &[&str]); 5] = [
        ("bun.lock", &["bun", "install"]),
        ("bun.lockb", &["bun", "install"]),
        ("pnpm-lock.yaml", &["pnpm", "install"]),
        ("yarn.lock", &["yarn", "install"]),
        ("package-lock.json", &["npm", "install", "--prefer-offline"]),
    ];
    for (lockfile, argv) in by_lockfile {
        if has_file(lockfile) {
            return Some(argv.iter().map(|s| s.to_string()).collect());
        }
    }
    None
}

/// Run `dir`'s setup step (declared `TT_SLOT_SETUP` from its rendered
/// `.env`, else lockfile detection — see [`setup_command`]), reading the
/// `.env` itself. `Ok(None)` means nothing to run or it succeeded; `Ok(Some)`
/// carries a warning for a failure the caller should surface but not fail
/// on (the slot/checkout is kept either way). Shared by `create_slot` and
/// the app's setup-retry command, so a failed install always gets exactly
/// one re-run path.
pub fn run_setup(dir: &Path) -> Result<Option<String>> {
    let env_map: BTreeMap<String, String> =
        envfile::parse(&fs::read_to_string(dir.join(".env")).unwrap_or_default())
            .into_iter()
            .collect();
    let Some(argv) = setup_command(&env_map, |f| dir.join(f).is_file()) else {
        return Ok(None);
    };
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let warning = match tt_exec::run_in_dir_with_timeout(&argv[0], &args, dir, SETUP_TIMEOUT) {
        Ok(out) if out.ok() => None,
        Ok(out) => Some(format!(
            "setup `{}` failed (exit {}) — slot kept, fix and re-run it\n{}",
            argv.join(" "),
            out.exit_code,
            out.stderr.trim()
        )),
        Err(e) => Some(format!("setup `{}` failed — slot kept: {e}", argv.join(" "))),
    };
    Ok(warning)
}

// ---------------------------------------------------------------------------
// creation

#[derive(Debug, Default)]
pub struct CreateOpts {
    /// Slot root; `None` walks up from the current working directory.
    pub root: Option<PathBuf>,
    /// Branch to create and check out. Slots are branch-named and ephemeral —
    /// there is no detached/parked mode.
    pub branch: String,
    /// Base ref for the new branch; `None` = the checkout's branch.
    pub base: Option<String>,
    /// Run the setup step in the new slot (declared `TT_SLOT_SETUP` from the
    /// rendered `.env`, else lockfile-detected package-manager install).
    pub run_setup: bool,
}

pub struct CreatedSlot {
    pub name: String,
    pub dir: PathBuf,
    pub branch: String,
    pub base: String,
    /// The ref the slot effectively branched from — `origin/<base>` when the
    /// creation-time fast-forward applied ([`effective_origin_base`]), else
    /// `base`. Display/prompt honesty; `base` stays the branch-name value.
    pub base_label: String,
    pub ports: Vec<(String, u16)>,
    pub inherited: usize,
    pub warnings: Vec<String>,
}

/// Create the slot for `branch`: worktree under `slots/`, rendered `.env`
/// with port claims, sibling-secrets inheritance, setup step.
pub fn create_slot(opts: &CreateOpts) -> Result<CreatedSlot> {
    let sr = discover_root(opts.root.as_deref())?;
    validate_branch_name(&opts.branch)?;
    let mut warnings = Vec::new();
    let _ = git_checkout(&sr.checkout, &["worktree", "prune"]);

    let fetch_start = Instant::now();
    match git_checkout_timeout(&sr.checkout, &["fetch", "--quiet", "origin"], FETCH_TIMEOUT) {
        Ok(out) if out.ok() => {}
        Ok(out) => warnings
            .push(format!("fetch failed (offline?) — using local refs: {}", out.stderr.trim())),
        // Includes a timed-out fetch (a stalled/inspected connection) — the
        // old `if let Ok(..) = .. && !out.ok()` form silently dropped this
        // case instead of warning on it.
        Err(e) => warnings.push(format!("fetch failed — using local refs: {e}")),
    }
    note_if_slow(&mut warnings, "fetch", fetch_start.elapsed());

    let base = opts.base.clone().unwrap_or_else(|| base_branch(&sr.checkout));
    // The ref this slot effectively branches from — probed after the fetch so
    // a just-created remote counterpart counts, and carried on the result as
    // `base_label` so the UI and the dynamic-flow prompt name the same ref
    // creation actually used (agreeing with `checkout_branches`' labels).
    let effective = effective_origin_base(&sr.checkout, &base);
    if let Some(upstream) = &effective {
        fast_forward_base_if_behind(&sr, &base, upstream, &mut warnings);
    }
    let base_label = effective.unwrap_or_else(|| base.clone());
    let name = layout::slot_name_from_branch(&opts.branch)
        .ok_or_else(|| OpsError::BadBranchName(opts.branch.clone()))?;
    let dir = sr.slot_dir(&name);
    if dir.exists() {
        return Err(OpsError::SlotExists { name, dir: dir.display().to_string() });
    }
    fs::create_dir_all(sr.slots_dir())
        .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", sr.slots_dir().display())))?;
    let dir_s = dir.to_string_lossy().to_string();

    let worktree_start = Instant::now();
    let add_result =
        git_checkout(&sr.checkout, &["worktree", "add", "-b", &opts.branch, &dir_s, &base])?;
    if !add_result.ok() {
        return Err(OpsError::Git(format!(
            "git worktree add failed:\n{}",
            add_result.stderr.trim()
        )));
    }
    note_if_slow(&mut warnings, "git worktree add", worktree_start.elapsed());

    // From here on, any failure must remove the worktree just added above —
    // otherwise (e.g. a template render error) it leaves a half-set-up slot
    // behind: a real worktree with no rendered `.env`, invisible as "failed"
    // to `tt slot ls` and blocking a retry with `SlotExists`.
    let created = (|| -> Result<CreatedSlot> {
        let summary = render_slot_env(&sr, &dir, Some(&base))?;
        warnings.extend(summary.warnings);

        // Inherit secrets from the first sibling checkout that has a .env —
        // the main checkout first (`sr.checkouts()` orders it that way; it's
        // the longest-lived and least likely to carry stale branch-specific
        // values), else the alphabetically-first slot. Surfaced in a warning
        // when it wasn't the main checkout, since a slot's secrets can be
        // branch-specific or stale in a way the main checkout's never are.
        let mut inherited = 0;
        for sib_dir in sr.checkouts() {
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
                if count > 0 && sib_dir != sr.checkout {
                    let source =
                        sib_dir.file_name().and_then(|n| n.to_str()).unwrap_or("a sibling slot");
                    warnings.push(format!(
                        "inherited {count} .env key(s) from {source}, not the main checkout — \
                         the main checkout has no .env yet, so these may be branch-specific or stale"
                    ));
                }
                break;
            }
        }

        if opts.run_setup {
            let setup_start = Instant::now();
            let setup_warning = run_setup(&dir)?;
            note_if_slow(&mut warnings, "setup", setup_start.elapsed());
            if let Some(warning) = setup_warning {
                warnings.push(warning);
            }
        }

        Ok(CreatedSlot {
            name,
            dir,
            branch: opts.branch.clone(),
            base,
            base_label,
            ports: summary.ports,
            inherited,
            warnings,
        })
    })();

    created.inspect_err(|_| {
        let _ = git_checkout(&sr.checkout, &["worktree", "remove", "--force", &dir_s]);
        let _ = fs::remove_dir_all(Path::new(&dir_s));
        // `worktree add -b` succeeded, so the branch is ours and still points
        // at base — delete it too, or the retry dies on "branch already
        // exists" after e.g. fixing a template error.
        let _ = git_checkout(&sr.checkout, &["branch", "-D", &opts.branch]);
    })
}

// ---------------------------------------------------------------------------
// removal

#[derive(Debug, Default)]
pub struct RemoveOpts {
    /// Slot root; `None` walks up from the current working directory.
    pub root: Option<PathBuf>,
    /// Slot directory name under `slots/`.
    pub name: String,
    /// Skip guards (each skip lands in [`RemovedSlot::messages`]) and force
    /// worktree removal.
    pub force: bool,
}

pub struct RemovedSlot {
    pub name: String,
    /// The removed checkout's directory (now gone from disk) — callers use it
    /// to untrack the slot from stores keyed by dir (the agentboard rail).
    pub dir: PathBuf,
    /// Progress notes for the user: docker resources removed, guards skipped
    /// under force, fallback paths taken. Callers surface these — nothing
    /// here prints.
    pub messages: Vec<String>,
}

/// How a removal ended.
///
/// A guard refusal is an `Ok` variant, not an [`OpsError`]: it is an expected
/// answer with a next step attached (stash it, stop the dev server, re-run
/// with force), not a failure. Modeling it as an error made every consumer
/// destructure it straight back out — the app to build a dialog from it, the
/// CLI to attach remedies, `clean_slots` to list it as a keep-reason — so
/// three call sites each re-derived "this error isn't an error", and the one
/// thing an error buys you (a `Display` for the boundary) went unused. Errors
/// here stay for what the user genuinely cannot proceed past: a bad path, a
/// broken worktree, git falling over.
pub enum RemoveOutcome {
    Removed(RemovedSlot),
    Blocked {
        name: String,
        blocked: Vec<RmBlocked>,
        /// Caveats gathered before the verdict — carried here for the same
        /// reason `Removed` carries them. A refusal computed against stale
        /// refs (the `fetch --prune` failed, so `origin/*` is whatever was
        /// last seen) is exactly the case a user must be told about: the
        /// blockers are reported as fact, and "unreachable from any
        /// branch/remote" can be an artifact of the staleness rather than a
        /// property of the branch. Dropping these left the offline verdict
        /// indistinguishable from an online one.
        messages: Vec<String>,
    },
}

/// Remove a slot: guarded (clean tree, no commits unreachable from a branch
/// or remote, nothing foreign on its claimed ports), then docker compose
/// down -v, anchored container/volume sweep, `git worktree remove`. Shared by
/// `tt slot rm` and the app's `slot_remove` command.
///
/// `before_removal` runs once the guards have passed (or been forced) and the
/// removal is really about to happen — after the last return that leaves the
/// slot untouched, before the first destructive step. The app hangs its
/// kill-the-slot's-PTYs step here so a *refused* removal never costs a live
/// session; the CLI passes `|| {}`. Deliberately not part of `RemoveOpts`:
/// it's a phase marker in this function's control flow, not a removal
/// setting.
pub fn remove_slot(opts: &RemoveOpts, before_removal: impl FnOnce()) -> Result<RemoveOutcome> {
    let sr = discover_root(opts.root.as_deref())?;
    let name = opts.name.clone();
    if name == "primary" || sr.checkout.file_name().and_then(|n| n.to_str()) == Some(&name) {
        return Err(OpsError::PrimaryRemoval);
    }
    let dir = sr.slot_dir(&name);
    if !dir.is_dir() {
        return Err(OpsError::NoSuchSlot { name, slots_dir: sr.slots_dir().display().to_string() });
    }
    let dir_s = dir.to_string_lossy().to_string();
    let mut messages = Vec::new();
    // The slot's state scope must be read while the checkout still exists —
    // scope detection probes the directory (see `tt_config::slot_scope_from_dir`).
    let state_scope = tt_config::slot_scope_from_dir(&dir);

    // Refresh remote-tracking refs before the unreachable-commit guard below:
    // without this, a branch merged and deleted upstream since the last
    // fetch still looks "unreachable from any branch/remote" against a stale
    // `origin/*`, which is the right call but for the wrong (stale) reason,
    // and a branch merged just now can look falsely safe to remove before
    // its remote ref disappears. `--prune` mirrors `clean_slots` so a
    // deleted remote branch is reflected too.
    match git_checkout(&sr.checkout, &["fetch", "--prune", "--quiet", "origin"]) {
        Ok(out) if out.ok() => {}
        _ => messages
            .push("fetch --prune failed (offline?) — using local refs for guard checks".into()),
    }

    // One `git status --porcelain` answers both questions below — whether git
    // works in there at all, and how dirty the tree is. `clean_slots` calls
    // this for every merged slot, so a second spawn here is per-slot waste.
    let status = git_slot(&dir, &["status", "--porcelain"]).ok().filter(|o| o.ok());

    // broken worktree: git can't even report status
    let Some(status) = status else {
        if !opts.force {
            return Err(OpsError::BrokenWorktree { name });
        }
        messages.push(
            "skipping guards (--force): worktree is broken — removing directory + registration"
                .to_string(),
        );
        before_removal();
        docker_cleanup(&name, &dir, &mut messages);
        fs::remove_dir_all(&dir)
            .map_err(|e| OpsError::Io(format!("cannot remove {dir_s}: {e}")))?;
        let _ = git_checkout(&sr.checkout, &["worktree", "prune"]);
        state_cleanup(state_scope.as_deref(), &mut messages);
        return Ok(RemoveOutcome::Removed(RemovedSlot { name, dir, messages }));
    };

    let dirty = crate::guards::dirty_entry_count(&status.stdout);
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
    .and_then(|o| crate::guards::unreachable_commit_count(&o.stdout))
    .unwrap_or(0);
    // Identify the holder of each foreign port, not just its number: the
    // blocker is only actionable if the user can tell which process to stop
    // (see `ports::holder`). Only for ports that actually block — naming the
    // holder costs two subprocesses, and the common case is no foreign
    // listener at all. On --force the guards are being skipped anyway, so
    // the name would only decorate a "skipping guard" log line — not worth
    // the spawns.
    let foreign: Vec<ForeignPort> = claimed_ports(&dir)
        .into_iter()
        .filter(|&p| port_occupied(p) && !docker_owns_port(&name, p))
        .map(|port| ForeignPort {
            port,
            holder: if opts.force { None } else { crate::ports::holder(port) },
        })
        .collect();

    let blocked = crate::guards::check_removal(dirty, unreachable, &foreign);
    if !blocked.is_empty() {
        if !opts.force {
            return Ok(RemoveOutcome::Blocked { name, blocked, messages });
        }
        for reason in &blocked {
            messages.push(format!("skipping guard (--force): {reason}"));
        }
    }

    // Commits that never reached the base are NOT a removal guard — removing a
    // worktree leaves the branch (and its commits) in the shared `.git`, so
    // nothing is lost. But staying silent about them is what made the old
    // output ambiguous: "removed" read as "that work is dealt with". Say
    // plainly what survives and where, so the difference from the uncommitted
    // work the guard above *does* block on is visible.
    let branch = git_slot(&dir, &["branch", "--show-current"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| o.stdout.trim().to_string())
        .filter(|b| !b.is_empty());
    if let Some(branch) = &branch {
        let refs = base_refs(&sr.checkout);
        if *branch != refs.base {
            let work = work_state(&refs, &dir, &format!("refs/heads/{branch}"), dirty, unreachable);
            match (work.unlanded, work.landed) {
                (0, Some(via)) => {
                    messages.push(format!(
                        "{branch} is {} into {} — nothing outstanding",
                        via.label(),
                        refs.base
                    ));
                }
                (n, _) if n > 0 => {
                    messages.push(format!(
                        "{n} commit(s) on {branch} have not reached {} — they stay on the branch, not in this worktree",
                        refs.base
                    ));
                }
                _ => {}
            }
        }
    }

    // Past every guard, and the state above is already gathered — only now is
    // it safe to kill the folder's PTYs (#366).
    before_removal();
    docker_cleanup(&name, &dir, &mut messages);

    let remove = if opts.force {
        git_checkout(&sr.checkout, &["worktree", "remove", "--force", &dir_s])
    } else {
        git_checkout(&sr.checkout, &["worktree", "remove", &dir_s])
    };
    match remove {
        Ok(out) if out.ok() => {}
        result => {
            let detail = match result {
                Ok(out) => out.stderr.trim().to_string(),
                Err(e) => e.to_string(),
            };
            if !opts.force {
                return Err(OpsError::Git(format!("git worktree remove failed:\n{detail}")));
            }
            messages.push(format!("git worktree remove failed ({detail}) — removing directory"));
            fs::remove_dir_all(&dir)
                .map_err(|e| OpsError::Io(format!("cannot remove {dir_s}: {e}")))?;
        }
    }
    let _ = git_checkout(&sr.checkout, &["worktree", "prune"]);
    state_cleanup(state_scope.as_deref(), &mut messages);
    Ok(RemoveOutcome::Removed(RemovedSlot { name, dir, messages }))
}

/// The ports a checkout claims, from its rendered `.env`.
fn claimed_ports(dir: &Path) -> BTreeSet<u16> {
    envfile::port_claims(&fs::read_to_string(dir.join(".env")).unwrap_or_default())
}

/// Stop whatever is listening on `port` in the slot named `name` under `root`
/// — the remedy for [`RmBlocked::ForeignPortListener`], so a stale dev server
/// can be cleared from the app instead of sending the user to a terminal.
///
/// Takes the slot's identity rather than [`RemoveOpts`]: this removes nothing,
/// and threading a `force` flag through a function that ignores it invites a
/// later caller to believe forcing means something here.
///
/// The claim check is the whole safety story and is not optional: `port` must
/// appear in *this slot's* rendered `.env`. Ports are claimed per-checkout, so
/// a claimed port that's occupied is this slot's own orphan by construction —
/// while an unclaimed one is somebody else's, quite possibly a sibling slot's
/// working dev server, and this function would kill its entire process group.
/// Same reasoning as `scripts/slot-port.mjs`'s "never call `killPort` on a
/// scanned/shared port".
pub fn stop_slot_port(root: Option<&Path>, name: &str, port: u16) -> Result<crate::ports::Stopped> {
    let sr = discover_root(root)?;
    let dir = sr.slot_dir(name);
    if !dir.is_dir() {
        return Err(OpsError::NoSuchSlot {
            name: name.to_string(),
            slots_dir: sr.slots_dir().display().to_string(),
        });
    }
    if !claimed_ports(&dir).contains(&port) {
        return Err(OpsError::PortNotClaimed { name: name.to_string(), port });
    }
    Ok(crate::ports::stop_listeners(port)?)
}

/// Delete the removed slot's instance-state directories (agentboard
/// sessions/windows, tt.db — see `tt_config::instance_state_dirs_for_scope`).
/// Only checkouts of this repo have a scope; other repos' slots have no
/// scoped state and skip cleanly.
fn state_cleanup(scope: Option<&str>, messages: &mut Vec<String>) {
    let Some(scope) = scope else {
        return;
    };
    for dir in tt_config::instance_state_dirs_for_scope(scope) {
        if !dir.is_dir() {
            continue;
        }
        match fs::remove_dir_all(&dir) {
            Ok(()) => messages.push(format!("removed slot state {}", dir.display())),
            Err(e) => messages.push(format!("could not remove slot state {}: {e}", dir.display())),
        }
    }
}

// ---------------------------------------------------------------------------
// clean — remove every finished slot and the state removed checkouts left behind

#[derive(Debug, Default)]
pub struct CleanOpts {
    /// Slot root; `None` walks up from the current working directory.
    pub root: Option<PathBuf>,
    /// Report what would happen without removing or sweeping anything.
    pub dry_run: bool,
    /// Parents of per-scope instance-state dirs to sweep (the
    /// `…/towles-tool/slots/` dirs; the caller resolves them via
    /// `tt_config::instance_state_bases`). Empty = skip the sweep.
    pub scope_parents: Vec<PathBuf>,
}

/// A slot `clean` removed (or, on dry-run, would remove).
pub struct FinishedSlot {
    pub name: String,
    pub branch: String,
    /// How the branch landed, e.g. `"squash-merged into main"`
    /// ([`crate::landed::LandedVia`], rendered against the base).
    pub reason: String,
    /// Removal progress notes (docker resources, branch deletion). Empty on
    /// dry-run.
    pub messages: Vec<String>,
    /// The removed checkout's directory (now gone from disk, except on
    /// dry-run) — callers use it to untrack the slot from stores keyed by dir
    /// (the agentboard rail), the same way `tt slot rm` does.
    pub dir: PathBuf,
}

/// A slot `clean` left alone, and why.
pub struct KeptSlot {
    pub name: String,
    pub branch: String,
    pub why: Vec<String>,
}

pub struct CleanReport {
    pub dry_run: bool,
    /// Removed (dry-run: would-remove) slots.
    pub removed: Vec<FinishedSlot>,
    pub kept: Vec<KeptSlot>,
    /// Orphaned per-scope state dirs swept (dry-run: would sweep).
    pub swept_state_dirs: Vec<PathBuf>,
    /// State scopes of the checkouts that remain (checkout + kept slots) —
    /// callers prune *these* agentboard stores plus the unscoped one.
    pub live_scopes: Vec<String>,
    pub warnings: Vec<String>,
}

/// Remove every *finished* slot — its branch is a strict ancestor of the
/// checkout's branch (classic merge) or its upstream is gone after
/// `fetch --prune` (squash/rebase merge) — via the same guarded
/// [`remove_slot`], never forced: a finished slot with uncommitted changes,
/// orphanable commits, or a live dev server is reported and kept. A removed
/// slot's branch is deleted from the hub (its work is reachable from the
/// base/remote — that's what made it finished). Then sweep `scope_parents`
/// for per-scope state dirs whose checkout no longer exists.
///
/// `scope_of` maps a checkout dir to its instance-state scope
/// (`tt_config::slot_scope_from_dir`); it is injected so the scope rule has
/// exactly one owner. When it can't scope the checkout (a repo that never
/// produces scoped state), the sweep is skipped entirely.
pub fn clean_slots(
    opts: &CleanOpts,
    scope_of: impl Fn(&Path) -> Option<String>,
) -> Result<CleanReport> {
    let sr = discover_root(opts.root.as_deref())?;
    let mut warnings = Vec::new();
    let _ = git_checkout(&sr.checkout, &["worktree", "prune"]);
    // --prune is what flips a merged-and-deleted remote branch to "gone".
    match git_checkout(&sr.checkout, &["fetch", "--prune", "--quiet", "origin"]) {
        Ok(out) if out.ok() => {}
        _ => warnings.push(
            "fetch --prune failed (offline?) — merges that deleted the remote branch may not \
             be detected this run"
                .to_string(),
        ),
    }

    let refs = base_refs(&sr.checkout);
    let base = refs.base.clone();

    let mut removed = Vec::new();
    let mut kept = Vec::new();
    let mut live_scopes: Vec<String> = scope_of(&sr.checkout).into_iter().collect();
    let checkout_scoped = !live_scopes.is_empty();

    for (name, dir) in sr.slots() {
        // Computed before removal — a removed slot's dir is gone afterwards.
        let scope = scope_of(&dir);
        let mut keep = |name: String, branch: String, why: Vec<String>| {
            kept.push(KeptSlot { name, branch, why });
            live_scopes.extend(scope.clone());
        };

        let branch = match git_slot(&dir, &["branch", "--show-current"]) {
            Ok(out) if out.ok() => out.stdout.trim().to_string(),
            _ => {
                keep(
                    name,
                    "BROKEN".to_string(),
                    vec!["worktree is broken — `tt slot rm --force` to drop it".to_string()],
                );
                continue;
            }
        };
        if branch.is_empty() {
            keep(
                name,
                "detached".to_string(),
                vec!["detached HEAD — no branch to judge".to_string()],
            );
            continue;
        }
        if branch == base {
            keep(name, branch, vec!["on the base branch".to_string()]);
            continue;
        }

        let branch_ref = format!("refs/heads/{branch}");
        let work =
            work_state(&refs, &dir, &branch_ref, uncommitted_count(&dir), orphaned_count(&dir));

        let Some(via) = work.landed else {
            keep(name, branch, vec![format!("active: {}", work.headline())]);
            continue;
        };
        // `clean` deletes the branch after removing the worktree, so unlanded
        // commits are unrecoverable here in a way they never are for
        // `tt slot rm` (which leaves the branch behind). Only content-based
        // evidence clears that bar — see `LandedVia::is_content_proof`.
        if work.unlanded > 0 || !via.is_content_proof() {
            keep(
                name,
                branch,
                vec![format!(
                    "{} but {} commit(s) never reached {base} — push or merge before cleaning",
                    via.label(),
                    work.unlanded
                )],
            );
            continue;
        }
        let reason = format!("{} into {base}", via.label());

        if opts.dry_run {
            removed.push(FinishedSlot {
                name,
                branch,
                reason: reason.clone(),
                messages: Vec::new(),
                dir,
            });
            continue;
        }
        let rm = RemoveOpts { root: Some(sr.checkout.clone()), name: name.clone(), force: false };
        match remove_slot(&rm, || {}) {
            Ok(RemoveOutcome::Removed(r)) => {
                let mut messages = r.messages;
                match git_checkout(&sr.checkout, &["branch", "-D", &branch]) {
                    Ok(out) if out.ok() => messages.push(format!("deleted branch {branch}")),
                    _ => messages.push(format!(
                        "could not delete branch {branch} — remove it with `git branch -D`"
                    )),
                }
                removed.push(FinishedSlot {
                    name,
                    branch,
                    reason: reason.clone(),
                    messages,
                    dir: r.dir,
                });
            }
            Ok(RemoveOutcome::Blocked { blocked, .. }) => {
                keep(name, branch, blocked.iter().map(ToString::to_string).collect())
            }
            Err(e) => keep(name, branch, vec![e.to_string()]),
        }
    }

    // Sweep per-scope instance state whose checkout no longer exists — the
    // dirs `tt slot rm` never touches (see tt_config::state_scope). Only in
    // repos that actually produce scopes: if the checkout itself has none,
    // nothing under these parents can be ours.
    let mut swept_state_dirs = Vec::new();
    if checkout_scoped {
        let live: BTreeSet<String> = live_scopes.iter().cloned().collect();
        for parent in &opts.scope_parents {
            let names = dir_names(parent);
            for stale in crate::clean::stale_scope_dirs(&sr.repo, &live, &names) {
                let dir = parent.join(&stale);
                if opts.dry_run {
                    swept_state_dirs.push(dir);
                    continue;
                }
                match fs::remove_dir_all(&dir) {
                    Ok(()) => swept_state_dirs.push(dir),
                    Err(e) => {
                        warnings.push(format!("could not remove {}: {e}", dir.display()));
                    }
                }
            }
        }
    }

    Ok(CleanReport {
        dry_run: opts.dry_run,
        removed,
        kept,
        swept_state_dirs,
        live_scopes,
        warnings,
    })
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
                    .any(|line| crate::guards::docker_resource_matches(line.trim(), slot_name))
        })
        .unwrap_or(false)
}

/// Compose down (containers, networks, volumes) then an anchored sweep of
/// anything else named after the slot. Best-effort: a missing docker is fine.
fn docker_cleanup(slot_name: &str, dir: &Path, messages: &mut Vec<String>) {
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
            Duration::from_secs(120),
        );
    }
    if let Ok(out) = tt_exec::run("docker", &["ps", "-a", "--format", "{{.Names}}"]) {
        for container in out.stdout.lines().map(str::trim) {
            if crate::guards::docker_resource_matches(container, slot_name) {
                messages.push(format!("removing container {container}"));
                let _ = tt_exec::run("docker", &["rm", "-f", container]);
            }
        }
    }
    if let Ok(out) = tt_exec::run("docker", &["volume", "ls", "--format", "{{.Name}}"]) {
        for volume in out.stdout.lines().map(str::trim) {
            if crate::guards::docker_resource_matches(volume, slot_name) {
                messages.push(format!("removing volume {volume}"));
                let _ = tt_exec::run("docker", &["volume", "rm", volume]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn declared_setup_wins_over_lockfiles() {
        let cmd = setup_command(&env(&[(SETUP_ENV_KEY, "make setup")]), |_| true);
        assert_eq!(cmd, Some(vec!["make".to_string(), "setup".to_string()]));
    }

    #[test]
    fn lockfile_detection_orders_bun_first() {
        let cmd = setup_command(&env(&[]), |f| f == "bun.lock" || f == "package-lock.json");
        assert_eq!(cmd, Some(vec!["bun".to_string(), "install".to_string()]));
        let cmd = setup_command(&env(&[]), |f| f == "package-lock.json");
        assert_eq!(
            cmd,
            Some(vec![
                "npm".to_string(),
                "install".to_string(),
                "--prefer-offline".to_string()
            ])
        );
    }

    #[test]
    fn no_lockfile_means_no_setup() {
        assert_eq!(setup_command(&env(&[]), |_| false), None);
        // declared-but-empty disables setup rather than running junk
        assert_eq!(setup_command(&env(&[(SETUP_ENV_KEY, "  ")]), |_| true), None);
    }

    #[test]
    fn validate_branch_name_accepts_legal_refs() {
        assert!(validate_branch_name("feat/hello-world").is_ok());
        assert!(validate_branch_name("standalone").is_ok());
    }

    #[test]
    fn validate_branch_name_rejects_illegal_refs() {
        assert!(validate_branch_name("feat/hello world").is_err());
        assert!(validate_branch_name("bad..name").is_err());
        assert!(validate_branch_name("-leading-dash").is_err());
    }

    /// Minimal slot root under a tempdir: `<tmp>/repo/.git`.
    fn temp_slot_root() -> (tempfile::TempDir, SlotRoot) {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("repo");
        fs::create_dir_all(checkout.join(".git")).unwrap();
        let sr = SlotRoot { checkout, repo: "repo".to_string() };
        (tmp, sr)
    }

    #[test]
    fn discover_root_finds_the_nearest_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("blog");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src").join("deep")).unwrap();

        let sr = discover_root(Some(&repo.join("src").join("deep"))).unwrap();
        assert_eq!(sr.checkout, repo);
        assert_eq!(sr.repo, "blog");
        assert_eq!(sr.slots_dir(), repo.join(".claude").join("worktrees"));
    }

    #[test]
    fn discover_root_hops_from_a_worktree_to_the_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("blog");
        let slot = repo.join(".claude").join("worktrees").join("thing");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("thing")).unwrap();
        fs::create_dir_all(&slot).unwrap();
        fs::write(
            slot.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/thing").display()),
        )
        .unwrap();

        let sr = discover_root(Some(&slot)).unwrap();
        assert_eq!(sr.checkout, repo);
        assert_eq!(sr.repo, "blog");
    }

    #[test]
    fn discover_root_hops_even_from_a_worktree_outside_the_checkout() {
        // Old-layout stragglers (slots that still live in a sibling dir) must
        // still anchor at the main checkout, not become their own root.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("blog");
        let stray = tmp.path().join("elsewhere").join("thing");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("thing")).unwrap();
        fs::create_dir_all(&stray).unwrap();
        fs::write(
            stray.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/thing").display()),
        )
        .unwrap();

        let sr = discover_root(Some(&stray)).unwrap();
        assert_eq!(sr.checkout, repo);
    }

    #[test]
    fn discover_root_keeps_a_submodule_as_its_own_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let superproject = tmp.path().join("super");
        let submodule = superproject.join("vendored");
        fs::create_dir_all(superproject.join(".git").join("modules").join("vendored")).unwrap();
        fs::create_dir_all(&submodule).unwrap();
        fs::write(submodule.join(".git"), "gitdir: ../.git/modules/vendored\n").unwrap();

        let sr = discover_root(Some(&submodule)).unwrap();
        assert_eq!(sr.checkout, submodule);
        assert_eq!(sr.repo, "vendored");
    }

    #[test]
    fn discover_root_errors_outside_any_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("just-a-dir");
        fs::create_dir_all(&plain).unwrap();
        assert!(matches!(discover_root(Some(&plain)), Err(OpsError::NoCheckout(_))));
    }

    #[test]
    fn claim_lock_path_is_per_checkout_and_never_inside_the_repo() {
        let a = claim_lock_path(Path::new("/home/x/code/repo-one"));
        let b = claim_lock_path(Path::new("/home/x/code/repo-two"));
        let a_again = claim_lock_path(Path::new("/home/x/code/repo-one"));

        assert_ne!(a, b, "different checkouts must not share a claim lock");
        assert_eq!(a, a_again, "the same checkout must resolve stably");
        assert!(a.starts_with(tt_config::locks_dir()));
        // The whole point of the move: nothing lands in the repo's .git/.
        assert!(!a.components().any(|c| c.as_os_str() == ".git"));
        assert!(
            a.file_name().unwrap().to_str().unwrap().starts_with("repo-one-"),
            "a stuck lock should name the repo it belongs to"
        );
    }

    #[test]
    fn same_named_checkouts_in_different_parents_get_different_locks() {
        let a = claim_lock_path(Path::new("/home/x/work/api"));
        let b = claim_lock_path(Path::new("/home/x/personal/api"));
        assert_ne!(a, b, "basename collisions must be separated by the path hash");
    }

    #[test]
    fn render_slot_env_with_no_template_renders_an_empty_env() {
        // A repo with neither a tokenized .env.example nor the sidecar — a
        // plain checkout never onboarded — still renders: empty .env, no
        // claims, and for a slot dir the marker. (The old behavior was a hard
        // "no template" error, which made slot creation fail for any such
        // repo.)
        let (_tmp, sr) = temp_slot_root();
        let slot = sr.slot_dir("feat-thing");
        fs::create_dir_all(&slot).unwrap();

        let summary = render_slot_env(&sr, &slot, Some("main")).unwrap();

        assert!(summary.ports.is_empty());
        assert_eq!(fs::read_to_string(slot.join(".env")).unwrap().trim(), "");
        assert!(slot.join(layout::MARKER_FILE).is_file());
    }

    #[test]
    fn render_slot_env_names_the_template_file_on_render_errors() {
        let (_tmp, sr) = temp_slot_root();
        fs::create_dir_all(sr.checkout.join(layout::CLAUDE_DIR)).unwrap();
        fs::write(template_sidecar_path(&sr), "X=${tt:bogus}\n").unwrap();

        let err = render_slot_env(&sr, &sr.checkout, None).unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains(TEMPLATE_SIDECAR), "error must name the template file: {msg}");
        assert!(msg.contains("line 1"), "error must keep the line detail: {msg}");
    }

    #[test]
    fn init_template_sidecar_creates_a_usable_empty_template() {
        let (_tmp, sr) = temp_slot_root();
        let path = init_template_sidecar(&sr).unwrap();
        assert_eq!(path, sr.checkout.join(".claude").join(TEMPLATE_SIDECAR));
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')),
            "an empty sidecar must be comment-only (no ${{tt:...}} tokens to render): {contents}"
        );
    }

    #[test]
    fn init_template_sidecar_is_idempotent() {
        let (_tmp, sr) = temp_slot_root();
        init_template_sidecar(&sr).unwrap();
        fs::write(template_sidecar_path(&sr), "NAME=${tt:slot-name}\n").unwrap();
        // a second call must not clobber a sidecar the user has since edited
        init_template_sidecar(&sr).unwrap();
        let contents = fs::read_to_string(template_sidecar_path(&sr)).unwrap();
        assert!(contents.contains("${tt:slot-name}"));
    }

    #[test]
    fn wire_worktree_hooks_starts_from_empty() {
        let (text, changed) = wire_worktree_hooks("").unwrap();
        assert!(changed);
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        for (event, command) in WORKTREE_HOOKS {
            let entries = doc["hooks"][event].as_array().unwrap();
            assert_eq!(entries[0]["hooks"][0]["command"], command);
        }
    }

    #[test]
    fn wire_worktree_hooks_preserves_existing_settings_and_hooks() {
        let existing = r#"{
            "permissions": {"allow": ["Bash(ls:*)"]},
            "hooks": {
                "PostToolUse": [{"hooks": [{"type": "command", "command": "echo hi"}]}],
                "WorktreeCreate": [{"hooks": [{"type": "command", "command": "other"}]}]
            }
        }"#;
        let (text, changed) = wire_worktree_hooks(existing).unwrap();
        assert!(changed);
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["permissions"]["allow"][0], "Bash(ls:*)");
        assert_eq!(doc["hooks"]["PostToolUse"][0]["hooks"][0]["command"], "echo hi");
        // the unrelated WorktreeCreate entry stays, ours is appended
        let create = doc["hooks"]["WorktreeCreate"].as_array().unwrap();
        assert_eq!(create.len(), 2);
        assert_eq!(create[0]["hooks"][0]["command"], "other");
        assert_eq!(create[1]["hooks"][0]["command"], "tt slot hook-create");
    }

    #[test]
    fn wire_worktree_hooks_is_idempotent() {
        let (once, changed_once) = wire_worktree_hooks("").unwrap();
        assert!(changed_once);
        let (twice, changed_twice) = wire_worktree_hooks(&once).unwrap();
        assert!(!changed_twice);
        assert_eq!(once, twice);
    }

    #[test]
    fn wire_worktree_hooks_refuses_malformed_json() {
        assert!(wire_worktree_hooks("{not json").is_err());
        assert!(wire_worktree_hooks("[]").is_err());
    }

    #[test]
    fn check_branch_reports_invalid_ref() {
        let (_tmp, sr) = temp_slot_root();
        let check = check_branch(&sr, "feat/bad name");
        assert!(check.error.is_some());
        assert!(!check.taken);
    }

    #[test]
    fn check_branch_flags_an_existing_slot_name() {
        let (_tmp, sr) = temp_slot_root();
        fs::create_dir_all(sr.slot_dir("feat-hello-world")).unwrap();
        let check = check_branch(&sr, "feat/hello-world");
        assert_eq!(check.name.as_deref(), Some("feat-hello-world"));
        assert!(check.taken);
        assert!(check.error.is_none());
    }

    #[test]
    fn check_branch_clears_a_free_name() {
        let (_tmp, sr) = temp_slot_root();
        let check = check_branch(&sr, "feat/brand-new");
        assert_eq!(check.name.as_deref(), Some("feat-brand-new"));
        assert!(!check.taken);
        assert!(check.error.is_none());
    }
}
