//! Tauri commands backing the Cockpit issue-queue actions (assign an issue to a
//! sibling task checkout, or create a local branch for it). Thin wrappers over
//! the Tauri-free guard + slugging in `tt_git::task_assign` /
//! `tt_git::branch_name`: this layer only gathers the target task's git state
//! (`remote`, `status`, `stash`) and shells out; every *decision* lives in the
//! pure crate so it stays unit-tested. Mirrors the CLI's `tt gh assign`
//! (`crates-cli/tt-cli/src/commands/gh.rs`), but matches the task against the
//! issue's `owner/name` slug rather than a current-directory checkout — the app
//! has no single cwd repo, the issue names its own.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tt_git::branch_name::create_branch_name_from_issue;
use tt_git::task_assign::validate_task_for_repo;

/// Timeout for git plumbing reads in the task (remote/status/stash) and the
/// local `git checkout -b`.
const TASK_GIT_TIMEOUT: Duration = Duration::from_secs(15);
/// Timeout for `gh issue develop` (talks to the network, then fetches).
const GH_DEVELOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Run git in `dir`, requiring exit 0, returning trimmed stdout. Failures come
/// back as the user-facing error string the frontend surfaces via toast.
fn git_in(dir: &Path, args: &[&str]) -> Result<String, String> {
    match tt_exec::run_in_dir_with_timeout("git", args, dir, TASK_GIT_TIMEOUT) {
        Ok(out) if out.ok() => Ok(out.stdout.trim().to_string()),
        Ok(out) => Err(format!("git {} failed: {}", args.join(" "), out.stderr.trim())),
        Err(e) => Err(format!("failed to run git in {}: {e}", dir.display())),
    }
}

/// Gather the task's remote/status/stash and run the clean-tree + matching-repo
/// guard. Returns `Ok(())` only when `task_dir` is a clean checkout of the same
/// GitHub repo (`owner/name`) the issue belongs to. Hard-fails with no `--force`
/// escape hatch — the whole point is that a dispatch can never trample a task
/// holding in-progress work.
fn guard_task(repo: &str, task_dir: &Path) -> Result<(), String> {
    if !task_dir.join(".git").exists() {
        return Err(format!("{} is not a git checkout (no .git)", task_dir.display()));
    }
    let task_remote = git_in(task_dir, &["remote", "get-url", "origin"])?;
    let status = git_in(task_dir, &["status", "--porcelain"])?;
    let stashes = git_in(task_dir, &["stash", "list"])?;
    validate_task_for_repo(repo, &task_remote, &status, &stashes)
        .map_err(|blocked| format!("Refusing to use {}: {blocked}", task_dir.display()))
}

/// `cockpit_assign_issue`: dispatch issue `#number` of `repo` (`owner/name`)
/// into the task checkout at `task_dir` via `gh issue develop --checkout`, but
/// only after the clean-tree guard passes. Async so the network round-trip runs
/// off the main thread (matches the store's `gh` commands).
#[tauri::command]
pub async fn cockpit_assign_issue(
    repo: String,
    number: u64,
    task_dir: String,
) -> Result<String, String> {
    let dir = PathBuf::from(&task_dir);
    tauri::async_runtime::spawn_blocking(move || {
        guard_task(&repo, &dir)?;
        let issue_arg = number.to_string();
        match tt_exec::run_in_dir_with_timeout(
            "gh",
            &["issue", "develop", &issue_arg, "--checkout"],
            &dir,
            GH_DEVELOP_TIMEOUT,
        ) {
            Ok(out) if out.ok() => {
                tracing::info!(%repo, number, dir = %dir.display(), "cockpit.issue_assigned");
                Ok(format!("Issue #{number} checked out in {}", dir.display()))
            }
            Ok(out) => Err(format!("gh issue develop failed: {}", out.stderr.trim())),
            Err(e) => Err(format!("failed to run gh in {}: {e}", dir.display())),
        }
    })
    .await
    .map_err(|e| format!("assign task failed: {e}"))?
}

/// `cockpit_create_issue_branch`: create a local `feature/<number>-<slug>`
/// branch (from the issue title) in the task checkout at `task_dir` via
/// `git checkout -b`, after the same clean-tree guard. Purely local — no `gh`
/// or network — for starting work without the issue-develop linkage.
#[tauri::command]
pub async fn cockpit_create_issue_branch(
    repo: String,
    number: u64,
    title: String,
    task_dir: String,
) -> Result<String, String> {
    let dir = PathBuf::from(&task_dir);
    tauri::async_runtime::spawn_blocking(move || {
        guard_task(&repo, &dir)?;
        let branch = create_branch_name_from_issue(number, &title);
        match tt_exec::run_in_dir_with_timeout(
            "git",
            &["checkout", "-b", &branch],
            &dir,
            TASK_GIT_TIMEOUT,
        ) {
            Ok(out) if out.ok() => {
                tracing::info!(%repo, number, %branch, "cockpit.issue_branch_created");
                Ok(format!("Created branch {branch} in {}", dir.display()))
            }
            Ok(out) => Err(format!("git checkout -b {branch} failed: {}", out.stderr.trim())),
            Err(e) => Err(format!("failed to run git in {}: {e}", dir.display())),
        }
    })
    .await
    .map_err(|e| format!("create-branch task failed: {e}"))?
}
