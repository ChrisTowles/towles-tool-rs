//! Task lifecycle operations shared by the CLI (`tt task`) and the app.
//!
//! This module owns the IO and process execution (git via `tt-exec`, env-file
//! writes, bind tests, the claim lock); every *decision* stays in the pure
//! modules ([`crate::template`], [`crate::guards`], [`crate::envfile`],
//! [`crate::layout`]). Callers surface [`CreatedTask::warnings`] to the user —
//! nothing here prints.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::{TemplateError, envfile, layout};

pub const TEMPLATE_SIDECAR: &str = "task-env.template";
/// Declared setup command, read from the task's rendered `.env`
/// (e.g. `TT_TASK_SETUP=bun install`). Spawned directly — no shell — so a
/// repo needing more than one command should point this at its own task
/// runner (`make setup`, `npm run bootstrap`).
pub const SETUP_ENV_KEY: &str = "TT_TASK_SETUP";
/// Declared teardown command, read from the task's rendered `.env`
/// (e.g. `TT_TASK_TEARDOWN=docker compose down -v`). Spawned directly — no
/// shell — before the worktree is removed, so it can still see the task's
/// `.env` and working tree. Unset means nothing to run: unlike setup there is
/// no lockfile-style fallback to detect, since there is nothing to teardown by
/// default.
pub const TEARDOWN_ENV_KEY: &str = "TT_TASK_TEARDOWN";
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// The pre-flight `fetch` in [`create_task`] is best-effort freshness, not
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
/// [`create_task`] names the step and its duration in a warning instead of
/// letting it pass silently.
const SLOW_STEP: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum OpsError {
    #[error("no git checkout found walking up from {0}")]
    NoCheckout(String),

    #[error("cannot derive a task name from branch {0}")]
    BadBranchName(String),

    #[error("'{branch}' is not a valid branch name: {detail}")]
    InvalidBranchName { branch: String, detail: String },

    #[error("task {name} already exists at {dir}")]
    TaskExists { name: String, dir: String },

    #[error("git: {0}")]
    Git(String),

    #[error("env template {path}: {source}")]
    Template { path: String, source: TemplateError },

    #[error("{0}")]
    Io(String),

    /// The port-registry file couldn't be written/encoded. Structured (not
    /// folded into [`OpsError::Io`]) because callers degrade differently: a
    /// failed registry write is a warning on an otherwise-successful render,
    /// never a failure of the render itself.
    #[error("port registry {path}: {detail}")]
    Registry { path: String, detail: String },

    #[error("timed out waiting for {0} — another task command may be stuck")]
    LockTimeout(String),

    #[error("refusing to remove the primary checkout — it owns every task's git state")]
    PrimaryRemoval,

    #[error("no task {name} in {tasks_dir}")]
    NoSuchTask { name: String, tasks_dir: String },

    #[error(
        "{name}'s worktree is broken (git fails inside it) — re-run with --force to remove anyway"
    )]
    BrokenWorktree { name: String },

    // NOTE: a guard refusal is deliberately NOT here — see [`RemoveOutcome`].
    #[error(transparent)]
    Port(#[from] crate::ports::PortError),

    #[error(
        "port {port} is not claimed by {name} — refusing to signal a process this task does not own"
    )]
    PortNotClaimed { name: String, port: u16 },

    #[error("{dir} is not a worktree of {repo}")]
    NotAWorktree { dir: String, repo: String },
}

pub type Result<T> = std::result::Result<T, OpsError>;

/// A discovered task root: the repo's main checkout and the repo name (its
/// directory basename). Tasks nest inside the checkout at
/// `.claude/worktrees/<name>` — see the [`crate::layout`] docs.
pub struct TaskRoot {
    /// The main checkout — a normal clone whose `.git` directory owns every
    /// task's git state.
    pub checkout: PathBuf,
    pub repo: String,
}

impl TaskRoot {
    /// The directory holding the worktree tasks (may not exist yet).
    pub fn tasks_dir(&self) -> PathBuf {
        layout::worktrees_dir(&self.checkout)
    }

    pub fn task_dir(&self, name: &str) -> PathBuf {
        self.tasks_dir().join(name)
    }

    /// Existing task dirs as (name, path), sorted by name.
    pub fn tasks(&self) -> Vec<(String, PathBuf)> {
        let mut tasks: Vec<(String, PathBuf)> = dir_names(&self.tasks_dir())
            .into_iter()
            .map(|name| {
                let path = self.tasks_dir().join(&name);
                (name, path)
            })
            .collect();
        tasks.sort();
        tasks
    }

    /// The checkout plus every task — every checkout whose `.env` can hold
    /// port claims.
    pub fn checkouts(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.checkout.clone()];
        dirs.extend(self.tasks().into_iter().map(|(_, dir)| dir));
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
/// so task commands anchor at the repo root no matter which worktree they run
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

/// Find the task root: walk up from `explicit` (or the current working
/// directory) to the nearest dir containing `.git`, then hop from a linked
/// worktree to its main checkout (see [`main_checkout`]) — so running from
/// inside a task anchors at the repo root, never nesting worktrees inside
/// worktrees. Any plain git checkout qualifies; there is no layout to set up.
pub fn discover_root(explicit: Option<&Path>) -> Result<TaskRoot> {
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
        return Ok(TaskRoot { checkout, repo });
    }
    Err(OpsError::NoCheckout(start.display().to_string()))
}

/// Resolve a task directory to its checkout root and task name, rejecting
/// anything that isn't a worktree of its own checkout. This is the shared
/// definition of "this dir names a real task of its repo" — the app's delete
/// and stop-port commands both go through it so they agree on what "this task"
/// means before either acts on it. Returns the identity only; a caller that
/// removes attaches its own `force` when building [`RemoveOpts`], so a
/// non-removal caller never constructs a removal config with a meaningless flag.
pub fn resolve_task_dir(dir: &Path) -> Result<(PathBuf, String)> {
    let sr = discover_root(Some(dir))?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| OpsError::Io(format!("bad task path {}", dir.display())))?
        .to_string();
    if sr.task_dir(&name) != dir {
        return Err(OpsError::NotAWorktree { dir: dir.display().to_string(), repo: sr.repo });
    }
    Ok((sr.checkout, name))
}

/// Bounded like [`git_task`] — a stalled network op (stuck proxy/VPN, an SSH
/// prompt with nothing to answer it) must fail after `GIT_TIMEOUT`, not hang
/// the caller (`create_task`/`remove_task`/`clean_tasks`) forever.
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

/// The refs a task's work is judged against: the checkout's base branch, and
/// its remote-tracking twin when one exists.
///
/// Both are needed because they answer at different times. A squash merge
/// lands on `origin/<base>` the moment the PR is merged, while local `<base>`
/// only catches up when the user pulls — so judging against the local ref
/// alone makes every merged task look active until the next `git pull`.
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
/// every task by callers that loop.
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

/// What a task still holds — uncommitted work and commits that never reached
/// the base — as one answer shared by `ls`, `rm`, `clean` and the Agentboard
/// rail. See [`crate::landed`] for why several git signals are combined.
///
/// `branch` is a full ref (`refs/heads/<name>`). Best-effort: git failures
/// degrade to "work is present", never to "safe to delete".
///
/// `uncommitted` and `orphaned` are passed in rather than gathered here. Every
/// caller either already has them (`remove_task` computes both for
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
        git_task(dir, args).ok().filter(|o| o.ok()).map(|o| o.stdout)
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
    // otherwise read every merged task as active. The local ref is still asked
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
    git_task(dir, &["status", "--porcelain"])
        .ok()
        .filter(|o| o.ok())
        .map(|o| crate::guards::dirty_entry_count(&o.stdout))
        .unwrap_or(0)
}

/// Commits reachable from no branch and no remote — the orphaned axis, the one
/// removal genuinely destroys. Base-independent, so it is meaningful even for a
/// detached HEAD that [`work_state`] cannot otherwise judge.
pub fn orphaned_count(dir: &Path) -> u64 {
    git_task(
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

/// Epoch-seconds commit time of the newest commit unique to this worktree's
/// branch — commits in `HEAD` but not in `base` — or `None` when the branch has
/// added no commits of its own. The recency signal behind `tt task ls --stale`
/// ([`crate::staleness`]): deliberately the branch's *own* newest commit, so a
/// fresh empty task off a long-untouched base does not read as stale, and a
/// checkout sitting on the base branch reports `None`. Landedness is a separate
/// axis judged by [`work_state`], not by this age.
pub fn last_own_commit_unix(dir: &Path, base: &str) -> Option<i64> {
    let range = format!("{base}..HEAD");
    git_task(dir, &["log", "-1", "--format=%ct", &range])
        .ok()
        .filter(|o| o.ok())
        .and_then(|o| o.stdout.trim().parse::<i64>().ok())
}

pub fn git_task(dir: &Path, args: &[&str]) -> Result<tt_exec::Output> {
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
/// task branches from current history instead of stale local history, which
/// otherwise means its first sync with base is an unnecessary rebase.
/// `git merge --ff-only` is itself a no-op when already current, so this
/// attempts it unconditionally rather than counting commits behind first.
/// Only ever touches the main checkout's own branch — the predicate returns
/// `None` for a tag, a SHA, a branch checked out in a different worktree, or
/// one with no `origin/<base>` upstream, since moving a ref out from under
/// another checkout would fight whatever's using it. A genuine divergence
/// (or uncommitted local changes ff-only won't overwrite) warns rather than
/// blocks creation — the task still branches from local history.
fn fast_forward_base_if_behind(
    sr: &TaskRoot,
    base: &str,
    upstream: &str,
    warnings: &mut Vec<String>,
) {
    match git_checkout(&sr.checkout, &["merge", "--ff-only", upstream]) {
        Ok(out) if out.ok() => {}
        Ok(out) => warnings.push(format!(
            "base branch '{base}' has diverged from {upstream} and could not be fast-forwarded \
             ({}) — the new task may need a rebase later",
            out.stderr.trim()
        )),
        Err(e) => warnings
            .push(format!("base branch '{base}' could not be checked against {upstream}: {e}")),
    }
}

/// The ref task creation will *effectively* branch from when it applies the
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

/// One base-branch choice for the new-task form. `name` is the local branch —
/// what `create_task` takes as `base` — and `label` is the ref creation will
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

/// Is `branch` already a local ref in `checkout`? Read-only — `git
/// show-ref` needs no fetch and never mutates anything.
pub fn branch_exists(checkout: &Path, branch: &str) -> bool {
    git_checkout(
        checkout,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .map(|out| out.ok())
    .unwrap_or(false)
}

/// Preflight for the new-task dialog: is `branch` a legal ref, does it
/// already exist in git (the case `git worktree add` would otherwise reject
/// after the fact), and would its derived task name collide with an
/// existing task? Read-only.
pub struct BranchCheck {
    pub name: Option<String>,
    pub taken: bool,
    pub branch_exists: bool,
    pub error: Option<String>,
}

pub fn check_branch(sr: &TaskRoot, branch: &str) -> BranchCheck {
    if let Err(e) = validate_branch_name(branch) {
        return BranchCheck {
            name: None,
            taken: false,
            branch_exists: false,
            error: Some(e.to_string()),
        };
    }
    match layout::task_name_from_branch(branch) {
        Some(name) => {
            let taken = sr.task_dir(&name).exists();
            let exists = branch_exists(&sr.checkout, branch);
            BranchCheck { name: Some(name), taken, branch_exists: exists, error: None }
        }
        None => BranchCheck {
            name: None,
            taken: false,
            branch_exists: false,
            error: Some(OpsError::BadBranchName(branch.to_string()).to_string()),
        },
    }
}

// ---------------------------------------------------------------------------
// setup

/// The setup command for a fresh task, as argv. `env` is the task's rendered
/// `.env`; `has_file` probes the task's checkout. Declared `TT_TASK_SETUP`
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

/// Run `dir`'s setup step (declared `TT_TASK_SETUP` from its rendered
/// `.env`, else lockfile detection — see [`setup_command`]), reading the
/// `.env` itself. `Ok(None)` means nothing to run or it succeeded; `Ok(Some)`
/// carries a warning for a failure the caller should surface but not fail
/// on (the task/checkout is kept either way). Shared by `create_task` and
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
            "setup `{}` failed (exit {}) — task kept, fix and re-run it\n{}",
            argv.join(" "),
            out.exit_code,
            out.stderr.trim()
        )),
        Err(e) => Some(format!("setup `{}` failed — task kept: {e}", argv.join(" "))),
    };
    Ok(warning)
}

/// Run `dir`'s teardown step (declared `TT_TASK_TEARDOWN` from its rendered
/// `.env` — see [`TEARDOWN_ENV_KEY`]), reading the `.env` itself. `Ok(None)`
/// means nothing declared or it succeeded; `Ok(Some)` carries a warning for a
/// failure the caller should surface but not fail on — removal proceeds
/// either way, since a stuck teardown command must never be what blocks a
/// worktree from coming off disk. Called from [`crate::ops::remove_task`]
/// while `dir` still exists, before it is deleted.
pub fn run_teardown(dir: &Path) -> Result<Option<String>> {
    let env_map: BTreeMap<String, String> =
        envfile::parse(&fs::read_to_string(dir.join(".env")).unwrap_or_default())
            .into_iter()
            .collect();
    let Some(declared) = env_map.get(TEARDOWN_ENV_KEY) else {
        return Ok(None);
    };
    let argv: Vec<String> = declared.split_whitespace().map(str::to_string).collect();
    let Some((cmd, args)) = argv.split_first() else {
        return Ok(None);
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let warning = match tt_exec::run_in_dir_with_timeout(cmd.as_str(), &args, dir, SETUP_TIMEOUT) {
        Ok(out) if out.ok() => None,
        Ok(out) => Some(format!(
            "teardown `{declared}` failed (exit {}) — continuing removal\n{}",
            out.exit_code,
            out.stderr.trim()
        )),
        Err(e) => Some(format!("teardown `{declared}` failed — continuing removal: {e}")),
    };
    Ok(warning)
}

// ---------------------------------------------------------------------------
// submodules — the lifecycle phases. Public API is re-exported here so
// callers keep using `tt_tasks::ops::*` paths; `pub(crate)` re-exports keep
// this file's tests (and sibling modules) reaching internals without the
// submodule paths leaking anywhere else.

mod claims;
mod create;
mod init;
mod remove;
mod render;

pub use claims::{PortClaim, PortRegistry, PortStatus, port_occupied, port_report};
pub use create::{CreateOpts, CreatedTask, create_task};
pub use init::{InitReport, init_repo, wire_worktree_hooks};
pub use remove::{
    CleanOpts, CleanReport, FinishedTask, KeptTask, RemoveOpts, RemoveOutcome, RemovedTask,
    clean_tasks, remove_task, stop_task_port,
};
pub use render::{RenderSummary, init_template_sidecar, render_task_env, template_sidecar_path};

#[cfg(test)]
pub(crate) use claims::{
    PORT_REGISTRY_FILE, claim_lock_path, record_task_ports, registry_claims, release_task_ports,
};

/// Write via temp-file + rename so a crash mid-write can never leave a
/// truncated file behind: the registry parses-or-reads-empty (silently
/// dropping every claim), and a half-written `.env` loses claims the same
/// way. Callers hold the claim lock during writes, so the fixed `.tmp`
/// sibling name can't collide across processes.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::init::WORKTREE_HOOKS;
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
    fn run_teardown_with_no_declared_command_does_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".env"), "").unwrap();
        assert!(run_teardown(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn run_teardown_runs_the_declared_command() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("teardown-ran");
        fs::write(
            tmp.path().join(".env"),
            format!("{TEARDOWN_ENV_KEY}=touch {}\n", marker.display()),
        )
        .unwrap();
        assert!(run_teardown(tmp.path()).unwrap().is_none());
        assert!(marker.is_file());
    }

    #[test]
    fn run_teardown_reports_a_failing_command_without_erroring() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".env"), format!("{TEARDOWN_ENV_KEY}=false\n")).unwrap();
        let warning = run_teardown(tmp.path()).unwrap();
        assert!(warning.is_some_and(|w| w.contains("teardown")));
    }

    #[test]
    fn port_occupied_catches_an_ipv6_only_listener() {
        // A listener bound only to ::1 must still mark the port occupied —
        // checking just 127.0.0.1 (the old behavior) let a fresh claim
        // collide with a sibling task's server bound the other stack.
        // Machines without an IPv6 loopback can't stage the scenario at all —
        // skip rather than fail, mirroring port_occupied's own tolerance for
        // an absent stack.
        let Ok(listener) = TcpListener::bind(("::1", 0)) else {
            return;
        };
        let port = listener.local_addr().unwrap().port();
        assert!(port_occupied(port));
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

    /// Minimal task root under a tempdir: `<tmp>/repo/.git`.
    fn temp_task_root() -> (tempfile::TempDir, TaskRoot) {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("repo");
        fs::create_dir_all(checkout.join(".git")).unwrap();
        let sr = TaskRoot { checkout, repo: "repo".to_string() };
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
        assert_eq!(sr.tasks_dir(), repo.join(".claude").join("worktrees"));
    }

    #[test]
    fn discover_root_hops_from_a_worktree_to_the_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("blog");
        let task = repo.join(".claude").join("worktrees").join("thing");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("thing")).unwrap();
        fs::create_dir_all(&task).unwrap();
        fs::write(
            task.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/thing").display()),
        )
        .unwrap();

        let sr = discover_root(Some(&task)).unwrap();
        assert_eq!(sr.checkout, repo);
        assert_eq!(sr.repo, "blog");
    }

    #[test]
    fn discover_root_hops_even_from_a_worktree_outside_the_checkout() {
        // Old-layout stragglers (tasks that still live in a sibling dir) must
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
    fn resolve_task_dir_accepts_a_worktree_of_its_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("blog");
        let task = repo.join(".claude").join("worktrees").join("feat-thing");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("feat-thing")).unwrap();
        fs::create_dir_all(&task).unwrap();
        fs::write(
            task.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/feat-thing").display()),
        )
        .unwrap();

        let (checkout, name) = resolve_task_dir(&task).unwrap();
        assert_eq!(checkout, repo);
        assert_eq!(name, "feat-thing");
    }

    #[test]
    fn resolve_task_dir_rejects_a_dir_that_is_not_a_worktree_of_its_checkout() {
        // A directory that resolves to a checkout but doesn't sit at that
        // checkout's `.claude/worktrees/<name>` is not this repo's task — e.g.
        // the checkout root itself, or a stray sibling.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("blog");
        fs::create_dir_all(repo.join(".git")).unwrap();

        let err = resolve_task_dir(&repo).unwrap_err();
        assert!(matches!(err, OpsError::NotAWorktree { .. }), "got {err:?}");
    }

    #[test]
    fn resolve_task_dir_errors_without_a_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve_task_dir(&tmp.path().join("nowhere")).unwrap_err();
        assert!(matches!(err, OpsError::NoCheckout(_)), "got {err:?}");
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

    /// `(var, port)` pairs for the registry tests — var names are synthesized
    /// (`P<port>`), which is all the metadata plumbing needs.
    fn port_pairs(ports: &[u16]) -> Vec<(String, u16)> {
        ports.iter().map(|&p| (format!("P{p}"), p)).collect()
    }

    /// A temp registry file next to the fixture checkout — the production
    /// path lives under the real config dir ([`port_registry_path`]), which
    /// tests must never touch.
    fn temp_registry(sr: &TaskRoot) -> PathBuf {
        sr.checkout.parent().unwrap().join(PORT_REGISTRY_FILE)
    }

    #[test]
    fn port_registry_blocks_reuse_until_release() {
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);
        fs::create_dir_all(sr.task_dir("feat-one")).unwrap();
        fs::create_dir_all(sr.task_dir("feat-two")).unwrap();

        record_task_ports(&sr, &reg, "feat-one", &port_pairs(&[4001, 4002]), 7).unwrap();

        // A different task must see both ports as taken...
        let claims = registry_claims(&sr, &reg, "feat-two");
        assert!(claims.contains(&4001));
        assert!(claims.contains(&4002));
        // ...but the owning task itself must not see its own ports as taken.
        assert!(registry_claims(&sr, &reg, "feat-one").is_empty());

        // The claim survives a no-op release for an unrelated task — it's
        // keyed by owner, not just "does something call release".
        release_task_ports(&sr.checkout, &reg, "feat-two");
        assert!(registry_claims(&sr, &reg, "feat-two").contains(&4001));

        // Only releasing the actual owner frees the ports.
        release_task_ports(&sr.checkout, &reg, "feat-one");
        assert!(registry_claims(&sr, &reg, "feat-two").is_empty());
    }

    #[test]
    fn record_task_ports_replaces_a_tasks_previous_entries() {
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);
        fs::create_dir_all(sr.task_dir("feat-one")).unwrap();

        record_task_ports(&sr, &reg, "feat-one", &port_pairs(&[5001]), 7).unwrap();
        record_task_ports(&sr, &reg, "feat-one", &port_pairs(&[5002]), 7).unwrap();

        let claims = registry_claims(&sr, &reg, "other");
        assert!(!claims.contains(&5001), "stale entry from the earlier render must be gone");
        assert!(claims.contains(&5002));
    }

    #[test]
    fn record_task_ports_with_no_claims_creates_no_registry_file() {
        // A repo with no port template renders empty claims on every task —
        // that must not litter the config dir with one empty ledger per
        // checkout ever rendered.
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);

        record_task_ports(&sr, &reg, "feat-one", &[], 7).unwrap();
        assert!(!reg.exists());
    }

    #[test]
    fn registry_self_heals_when_a_tasks_directory_vanishes_outside_tt_task_rm() {
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);
        let task_dir = sr.task_dir("feat-gone");
        fs::create_dir_all(&task_dir).unwrap();

        record_task_ports(&sr, &reg, "feat-gone", &port_pairs(&[6001]), 7).unwrap();
        assert!(registry_claims(&sr, &reg, "someone-else").contains(&6001));

        // Simulate the directory vanishing some way other than `tt task rm`
        // (which requires it to still exist to run at all) — the registry
        // must notice on its own next touch, not stay stuck forever.
        fs::remove_dir_all(&task_dir).unwrap();
        assert!(registry_claims(&sr, &reg, "someone-else").is_empty());
    }

    #[test]
    fn registry_records_claim_metadata_and_its_checkout() {
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);
        fs::create_dir_all(sr.task_dir("feat-one")).unwrap();

        record_task_ports(&sr, &reg, "feat-one", &[("UI_PORT".to_string(), 4001)], 1234).unwrap();

        let registry = super::claims::load_port_registry(&reg);
        assert_eq!(registry.checkout, sr.checkout.display().to_string());
        let claim = &registry.ports[&4001];
        assert_eq!(claim.owner, "feat-one");
        assert_eq!(claim.var, "UI_PORT");
        assert_eq!(claim.claimed_at_ms, 1234);
    }

    #[test]
    fn registry_drops_owners_that_are_not_valid_task_names() {
        // A hand-edited (or corrupt) owner like "../x" must never survive a
        // load — load_live_registry would otherwise path-join it under the
        // worktrees dir on the liveness probe.
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);
        fs::write(
            &reg,
            serde_json::json!({
                "checkout": sr.checkout.display().to_string(),
                "ports": { "4001": { "owner": "../escape", "var": "P", "claimed_at_ms": 0 } },
            })
            .to_string(),
        )
        .unwrap();

        assert!(registry_claims(&sr, &reg, "other").is_empty());
    }

    #[test]
    fn pre_metadata_registry_files_read_as_empty() {
        // Hard cutover from the flat `{port: owner}` format: an old file
        // must block nothing (claims re-record on the next render), never
        // error or half-parse.
        let (_tmp, sr) = temp_task_root();
        let reg = temp_registry(&sr);
        fs::write(&reg, r#"{ "4001": "feat-old" }"#).unwrap();

        assert!(registry_claims(&sr, &reg, "other").is_empty());
    }

    #[test]
    fn render_task_env_with_no_template_renders_an_empty_env() {
        // A repo with neither a tokenized .env.example nor the sidecar — a
        // plain checkout never onboarded — still renders: empty .env, no
        // claims, and for a task dir the marker. (The old behavior was a hard
        // "no template" error, which made task creation fail for any such
        // repo.)
        let (_tmp, sr) = temp_task_root();
        let task = sr.task_dir("feat-thing");
        fs::create_dir_all(&task).unwrap();

        let summary = render_task_env(&sr, &task, Some("main"), 7).unwrap();

        assert!(summary.ports.is_empty());
        assert_eq!(fs::read_to_string(task.join(".env")).unwrap().trim(), "");
        assert!(task.join(layout::MARKER_FILE).is_file());
    }

    #[test]
    fn render_task_env_names_the_template_file_on_render_errors() {
        let (_tmp, sr) = temp_task_root();
        fs::create_dir_all(sr.checkout.join(layout::CLAUDE_DIR)).unwrap();
        fs::write(template_sidecar_path(&sr), "X=${tt:bogus}\n").unwrap();

        let err = render_task_env(&sr, &sr.checkout, None, 7).unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains(TEMPLATE_SIDECAR), "error must name the template file: {msg}");
        assert!(msg.contains("line 1"), "error must keep the line detail: {msg}");
    }

    #[test]
    fn init_template_sidecar_creates_a_usable_empty_template() {
        let (_tmp, sr) = temp_task_root();
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
        let (_tmp, sr) = temp_task_root();
        init_template_sidecar(&sr).unwrap();
        fs::write(template_sidecar_path(&sr), "NAME=${tt:task-name}\n").unwrap();
        // a second call must not clobber a sidecar the user has since edited
        init_template_sidecar(&sr).unwrap();
        let contents = fs::read_to_string(template_sidecar_path(&sr)).unwrap();
        assert!(contents.contains("${tt:task-name}"));
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
        assert_eq!(create[1]["hooks"][0]["command"], "tt task hook-create");
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
        let (_tmp, sr) = temp_task_root();
        let check = check_branch(&sr, "feat/bad name");
        assert!(check.error.is_some());
        assert!(!check.taken);
    }

    #[test]
    fn check_branch_flags_an_existing_task_name() {
        let (_tmp, sr) = temp_task_root();
        fs::create_dir_all(sr.task_dir("feat-hello-world")).unwrap();
        let check = check_branch(&sr, "feat/hello-world");
        assert_eq!(check.name.as_deref(), Some("feat-hello-world"));
        assert!(check.taken);
        assert!(check.error.is_none());
    }

    #[test]
    fn check_branch_clears_a_free_name() {
        let (_tmp, sr) = temp_task_root();
        let check = check_branch(&sr, "feat/brand-new");
        assert_eq!(check.name.as_deref(), Some("feat-brand-new"));
        assert!(!check.taken);
        assert!(!check.branch_exists);
        assert!(check.error.is_none());
    }

    /// Runs `git <args>` in `dir` and asserts it actually succeeded (exit
    /// code 0) — a CI runner has no ambient git identity, so a bare `commit`
    /// silently fails without `-c user.{name,email}=...`, and `is_ok()` alone
    /// only proves the process spawned, not that it did anything.
    fn git_ok(dir: &Path, args: &[&str]) {
        let dir = dir.to_str().unwrap();
        let mut full = vec![
            "-C",
            dir,
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test",
        ];
        full.extend_from_slice(args);
        let out = tt_exec::run("git", &full).unwrap();
        assert!(out.ok(), "git {full:?} failed: {}", out.stderr);
    }

    #[test]
    fn check_branch_flags_an_existing_git_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("repo");
        fs::create_dir_all(&checkout).unwrap();
        git_ok(&checkout, &["init", "-q"]);
        git_ok(&checkout, &["commit", "-q", "--allow-empty", "-m", "x"]);
        git_ok(&checkout, &["branch", "feat/taken-elsewhere"]);
        let sr = TaskRoot { checkout, repo: "repo".to_string() };

        let check = check_branch(&sr, "feat/taken-elsewhere");
        assert!(check.branch_exists);
        assert!(!check.taken);
        assert!(check.error.is_none());
    }
}
