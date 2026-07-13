//! `tt gh` subcommands: branch, branch-clean, pr (also reachable as `tt pr`).
//!
//! Ports `src/commands/gh/*.ts`. Pure logic (branch names, PR content, merged-branch
//! filtering, issue parsing, picker layout) lives in `tt-git`; this module shells out via
//! `tt-exec` and drives the interactive prompts with `inquire`.
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - Confirmation prompts require a TTY. `pr` refuses to run non-interactively without
//!   `--yes`; `branch-clean` treats a non-TTY (without `--force`/`--dry-run`) as a
//!   no-op cancel. This keeps CI/tests from hanging on a prompt.
//! - The per-command `--debug` flag is replaced by the global `-v/--verbose` flag.

use crate::cli::{AssignArgs, BranchCleanArgs, CoArgs, GhCommands, PrArgs, SyncArgs};
use crate::ui;
use std::fmt;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;
use tt_git::branch_name::create_branch_name_from_issue;
use tt_git::picker::{ChoiceValue, build_issue_choices, compute_column_layout};
use tt_git::pr::generate_pr_content;
use tt_git::pr_list::{PrRow, render_pr_list};
use tt_git::{Issue, branch_clean, issues, slot_assign, sync};

pub fn run(command: GhCommands) -> i32 {
    match command {
        GhCommands::Branch { assigned_to_me } => branch(assigned_to_me),
        GhCommands::BranchClean(args) => branch_clean_cmd(args),
        GhCommands::Pr(args) => pr(args),
        GhCommands::PrList => pr_list(),
        GhCommands::Assign(args) => assign(args),
        GhCommands::Sync(args) => sync_cmd(args),
        GhCommands::Co(args) => co(args),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Run git, capturing trimmed stdout. Returns `None` (after logging) on spawn failure.
fn git(args: &[&str]) -> Option<tt_exec::Output> {
    match tt_exec::run("git", args) {
        Ok(out) => Some(out),
        Err(e) => {
            ui::error(&format!("Failed to run git: {e}"));
            None
        }
    }
}

fn gh_installed() -> bool {
    match tt_exec::run("gh", &["--version"]) {
        Ok(out) => issues::gh_version_indicates_installed(&out.stdout),
        Err(_) => false,
    }
}

fn current_branch() -> Option<String> {
    Some(git(&["branch", "--show-current"])?.stdout.trim().to_string())
}

// ---------------------------------------------------------------------------
// gh pr
// ---------------------------------------------------------------------------

fn pr(args: PrArgs) -> i32 {
    if !gh_installed() {
        ui::error("GitHub CLI not installed");
        return 1;
    }

    let Some(current) = current_branch() else {
        return 1;
    };
    if current.is_empty() {
        ui::error("Not on a branch (detached HEAD?)");
        return 1;
    }
    if current == args.base {
        ui::error(&format!("Already on base branch {}", args.base));
        return 1;
    }

    ui::info(&format!("Current branch: {current}"));
    ui::info(&format!("Base branch: {}", args.base));

    let Some(log) = git(&["log", &format!("{}..HEAD", args.base), "--pretty=format:%s"]) else {
        return 1;
    };
    let commits: Vec<String> =
        log.stdout.trim().split('\n').filter(|l| !l.is_empty()).map(|l| l.to_string()).collect();

    if commits.is_empty() {
        ui::error(&format!("No commits between {} and {current}", args.base));
        return 1;
    }

    ui::info(&format!("Found {} commits", commits.len()));

    let content = generate_pr_content(&current, &commits);

    println!();
    println!("── PR Preview ──");
    println!("Title: {}", content.title);
    println!();
    println!("{}", content.body);
    println!();

    if !args.yes {
        match confirm("Create this PR?", true) {
            Some(true) => {}
            Some(false) => {
                ui::info("Canceled");
                return 0;
            }
            None => {
                ui::error("Not a TTY; pass --yes to create the PR non-interactively.");
                return 1;
            }
        }
    }

    // Push when the branch has no upstream, is ahead of it, or its upstream is
    // gone — otherwise the PR would be opened from a stale remote head.
    if let Some(status) = git(&["status", "-sb"]) {
        let branch_line = status.stdout.lines().next().unwrap_or("");
        if tt_git::pr::should_push(branch_line) {
            ui::info("Pushing branch to remote...");
            if git(&["push", "-u", "origin", &current]).is_none() {
                return 1;
            }
        }
    } else {
        return 1;
    }

    let mut pr_args = vec![
        "pr",
        "create",
        "--title",
        &content.title,
        "--body",
        &content.body,
        "--base",
        &args.base,
    ];
    if args.draft {
        pr_args.push("--draft");
    }

    match tt_exec::run("gh", &pr_args) {
        Ok(out) if out.ok() => {
            ui::success(&format!("PR created: {}", out.stdout.trim()));
            0
        }
        Ok(out) => {
            ui::error(&format!("gh pr create failed: {}", out.stderr.trim()));
            1
        }
        Err(e) => {
            ui::error(&format!("Failed to run gh: {e}"));
            1
        }
    }
}

// ---------------------------------------------------------------------------
// gh pr-list
// ---------------------------------------------------------------------------

/// `tt gh pr-list` (alias `tt prs`): print my open PRs across every tracked
/// repo with their CI check rollup and a needs-you marker — the headless twin
/// of the app's Cockpit "PRs need you" panel.
///
/// Fetching reuses the single `gh` code path in `tt_collect::collect_prs`: it
/// runs the collector against a throwaway in-memory store, then reads the rows
/// back and hands them to `tt_git::pr_list` for rendering (the one Rust home
/// for the "needs you" semantics). A repo whose `gh` call fails contributes no
/// rows and its error is surfaced on stderr; the exit code reflects whether the
/// sweep was clean. Non-interactive by design — never prompts, never hangs.
fn pr_list() -> i32 {
    let repo_dirs = tt_collect::tracked_repo_dirs();
    if repo_dirs.is_empty() {
        ui::warning("No repos configured. Add one with `tt agentboard repos add <dir>`.");
        return 0;
    }

    if !gh_installed() {
        ui::error("GitHub CLI not installed");
        return 1;
    }

    let store = match tt_store::Store::open_in_memory() {
        Ok(store) => store,
        Err(e) => {
            ui::error(&format!("Failed to open in-memory store: {e}"));
            return 1;
        }
    };

    let summary = tt_collect::collect_prs(&store, &repo_dirs, now_ms());
    let prs = match store.prs() {
        Ok(prs) => prs,
        Err(e) => {
            ui::error(&format!("Failed to read collected PRs: {e}"));
            return 1;
        }
    };

    let rows: Vec<PrRow> = prs
        .iter()
        .map(|p| PrRow {
            repo: p.repo.clone(),
            number: p.number,
            title: p.title.clone(),
            state: p.state.clone(),
            checks: p.checks.clone(),
            review_state: p.review_state.clone(),
        })
        .collect();

    println!("{}", render_pr_list(&rows));

    // A failed sweep still lists whatever succeeded; note the failure and let
    // the exit code carry it.
    if !summary.ok {
        ui::warning(summary.message.as_deref().unwrap_or("some repos failed to refresh"));
        return 1;
    }
    0
}

/// Current wall-clock time in epoch milliseconds. Read at the CLI boundary so
/// the library collectors stay clock-injected.
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// gh branch-clean
// ---------------------------------------------------------------------------

fn branch_clean_cmd(args: BranchCleanArgs) -> i32 {
    let current = current_branch().unwrap_or_default();

    // `--gone` targets branches whose upstream was deleted on the remote (via
    // `git branch -vv`), catching GitHub rebase-and-merge branches that
    // `git branch --merged` never lists because the merge landed new SHAs.
    // Those aren't ancestor-merged, so they need `git branch -D`.
    let (to_delete, delete_flag, kind) = if args.gone {
        let Some(vv) = git(&["branch", "-vv"]) else {
            return 1;
        };
        (branch_clean::branches_gone(&vv.stdout, &args.base, &current), "-D", "gone")
    } else {
        let Some(merged) = git(&["branch", "--merged", &args.base]) else {
            return 1;
        };
        (branch_clean::branches_to_delete(&merged.stdout, &args.base, &current), "-d", "merged")
    };

    if to_delete.is_empty() {
        ui::info(&format!("No {kind} branches to clean up"));
        return 0;
    }

    println!("Found {} {kind} branch(es):", to_delete.len());
    for branch in &to_delete {
        println!("  - {branch}");
    }

    if args.dry_run {
        ui::warning("Dry run - no branches deleted");
        return 0;
    }

    if !args.force {
        match confirm(&format!("Delete {} branch(es)?", to_delete.len()), false) {
            Some(true) => {}
            Some(false) => {
                ui::info("Canceled");
                return 0;
            }
            None => {
                ui::info("Not a TTY; pass --force to delete or --dry-run to preview.");
                return 0;
            }
        }
    }

    let mut deleted = 0;
    let mut failed = 0;
    for branch in &to_delete {
        match tt_exec::run("git", &["branch", delete_flag, branch]) {
            Ok(out) if out.ok() => {
                println!("✓ Deleted {branch}");
                deleted += 1;
            }
            _ => {
                println!("✗ Failed to delete {branch}");
                failed += 1;
            }
        }
    }

    println!();
    if deleted > 0 {
        ui::info(&format!("Deleted {deleted} branch(es)"));
    }
    if failed > 0 {
        ui::warning(&format!("Failed to delete {failed} branch(es)"));
    }
    0
}

// ---------------------------------------------------------------------------
// gh assign (dispatch an issue to a sibling slot checkout)
// ---------------------------------------------------------------------------

/// Timeout for git plumbing reads in the slot (status/stash/remote).
const SLOT_GIT_TIMEOUT: Duration = Duration::from_secs(15);
/// Timeout for `gh issue develop` (talks to the network, then fetches).
const GH_DEVELOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Run git in `dir`, requiring exit 0; surfaces the failure as a user error.
fn git_in(dir: &Path, args: &[&str]) -> Option<String> {
    match tt_exec::run_in_dir_with_timeout("git", args, dir, SLOT_GIT_TIMEOUT) {
        Ok(out) if out.ok() => Some(out.stdout),
        Ok(out) => {
            ui::error(&format!(
                "git {} failed in {}: {}",
                args.join(" "),
                dir.display(),
                out.stderr.trim()
            ));
            None
        }
        Err(e) => {
            ui::error(&format!("Failed to run git in {}: {e}", dir.display()));
            None
        }
    }
}

/// `tt gh assign <issue> --slot <dir>`: run `gh issue develop <issue>
/// --checkout` inside the slot, but only after the guard — the slot must be a
/// checkout of this same repo with a completely clean tree (no uncommitted or
/// untracked changes, no stashes). The guard hard-fails with no `--force`
/// escape hatch: its whole purpose is that a dispatch can never trample a
/// slot holding in-progress work.
fn assign(args: AssignArgs) -> i32 {
    let slot = &args.slot;
    if !slot.join(".git").exists() {
        ui::error(&format!("{} is not a git checkout (no .git)", slot.display()));
        return 1;
    }

    // Guard 1: the slot must be a clone of this repo, not something unrelated.
    let Some(expected_remote) = git(&["remote", "get-url", "origin"]) else {
        return 1;
    };
    if !expected_remote.ok() {
        ui::error("Current directory has no `origin` remote to match the slot against");
        return 1;
    }
    let Some(slot_remote) = git_in(slot, &["remote", "get-url", "origin"]) else {
        return 1;
    };
    // Guard 2+3: clean working tree and empty stash list.
    let Some(status) = git_in(slot, &["status", "--porcelain"]) else {
        return 1;
    };
    let Some(stashes) = git_in(slot, &["stash", "list"]) else {
        return 1;
    };

    if let Err(blocked) = slot_assign::validate_slot(
        expected_remote.stdout.trim(),
        slot_remote.trim(),
        &status,
        &stashes,
    ) {
        ui::error(&format!(
            "Refusing to assign issue #{} to {}: {blocked}",
            args.issue,
            slot.display()
        ));
        return 1;
    }

    if !gh_installed() {
        ui::error("GitHub CLI not installed");
        return 1;
    }

    ui::info(&format!("Slot {} is clean; assigning issue #{}...", slot.display(), args.issue));
    let issue_arg = args.issue.to_string();
    match tt_exec::run_in_dir_with_timeout(
        "gh",
        &["issue", "develop", &issue_arg, "--checkout"],
        slot,
        GH_DEVELOP_TIMEOUT,
    ) {
        Ok(out) if out.ok() => {
            let detail = format!("{}{}", out.stdout.trim(), out.stderr.trim());
            if !detail.is_empty() {
                println!("{detail}");
            }
            ui::success(&format!("Issue #{} checked out in {}", args.issue, slot.display()));
            0
        }
        Ok(out) => {
            ui::error(&format!("gh issue develop failed: {}", out.stderr.trim()));
            1
        }
        Err(e) => {
            ui::error(&format!("Failed to run gh in {}: {e}", slot.display()));
            1
        }
    }
}

// ---------------------------------------------------------------------------
// gh sync (bring the current checkout current with the base branch)
// ---------------------------------------------------------------------------

/// The shared clean-tree guard for `sync` and `co`: read `git status
/// --porcelain` and hard-fail with a readable summary if anything is dirty.
/// Returns `false` (after logging) when the tree is dirty or git can't be run,
/// so the caller aborts before touching the branch.
fn require_clean_tree(action: &str) -> bool {
    let Some(status) = git(&["status", "--porcelain"]) else {
        return false;
    };
    if let Some(dirty) = sync::dirty_tree(&status.stdout) {
        ui::error(&format!(
            "Working tree is not clean; commit or stash before {action}.\n{}",
            dirty.summary()
        ));
        return false;
    }
    true
}

/// Ahead/behind of HEAD vs `upstream`, or `None` if it can't be computed
/// (e.g. the upstream ref doesn't exist yet). Purely informational.
fn ahead_behind(upstream: &str) -> Option<sync::AheadBehind> {
    let range = format!("{upstream}...HEAD");
    let out = git(&["rev-list", "--left-right", "--count", &range])?;
    if !out.ok() {
        return None;
    }
    sync::parse_ahead_behind(&out.stdout)
}

fn report_ahead_behind(label: &str, upstream: &str) {
    if let Some(ab) = ahead_behind(upstream) {
        ui::info(&format!("{label}: {} ahead, {} behind {upstream}", ab.ahead, ab.behind));
    }
}

/// `tt gh sync [--base main]`: fetch `origin/<base>` and rebase the current
/// branch onto it. Hard-fails before any fetch/rebase if the tree is dirty, and
/// gives a distinct, actionable error when the rebase stops on a conflict.
fn sync_cmd(args: SyncArgs) -> i32 {
    if !require_clean_tree("syncing") {
        return 1;
    }

    let upstream = format!("origin/{}", args.base);

    ui::info(&format!("Fetching {upstream}..."));
    match tt_exec::run("git", &["fetch", "origin", &args.base]) {
        Ok(out) if out.ok() => {}
        Ok(out) => {
            ui::error(&format!("git fetch failed: {}", out.stderr.trim()));
            return 1;
        }
        Err(e) => {
            ui::error(&format!("Failed to run git: {e}"));
            return 1;
        }
    }

    report_ahead_behind("Before", &upstream);

    ui::info(&format!("Rebasing onto {upstream}..."));
    let outcome = match tt_exec::run("git", &["rebase", &upstream]) {
        Ok(out) => sync::classify_rebase(out.exit_code, &out.stdout, &out.stderr),
        Err(e) => {
            ui::error(&format!("Failed to run git: {e}"));
            return 1;
        }
    };
    match outcome {
        sync::RebaseOutcome::Clean => {}
        sync::RebaseOutcome::Conflict => {
            ui::error(
                "Rebase stopped on conflicts. Resolve them and run `git rebase --continue`, \
                 or run `git rebase --abort` to bail out and leave the branch unchanged.",
            );
            return 1;
        }
        sync::RebaseOutcome::Failed(msg) => {
            ui::error(&format!("Rebase failed: {msg}"));
            return 1;
        }
    }

    report_ahead_behind("After", &upstream);
    ui::success(&format!("In sync with {upstream}"));
    0
}

// ---------------------------------------------------------------------------
// gh co (check out a PR's branch)
// ---------------------------------------------------------------------------

/// `tt gh co <number>` (alias `pr-checkout`): resolve the PR's head branch via
/// `gh pr view`, run the same clean-tree guard, then check the branch out —
/// fetching it from origin first if it isn't known locally.
fn co(args: CoArgs) -> i32 {
    if !gh_installed() {
        ui::error("GitHub CLI not installed");
        return 1;
    }
    if !require_clean_tree("checking out") {
        return 1;
    }

    let number = args.number.to_string();
    let branch = match tt_exec::run("gh", &["pr", "view", &number, "--json", "headRefName"]) {
        Ok(out) if out.ok() => match sync::parse_head_ref_name(&out.stdout) {
            Ok(branch) => branch,
            Err(e) => {
                ui::error(&format!("Could not resolve PR #{number}: {e}"));
                return 1;
            }
        },
        Ok(out) => {
            ui::error(&format!("gh pr view failed: {}", out.stderr.trim()));
            return 1;
        }
        Err(e) => {
            ui::error(&format!("Failed to run gh: {e}"));
            return 1;
        }
    };

    ui::info(&format!("PR #{number} is branch {branch}"));

    // Try a plain checkout first; if the branch isn't local yet, fetch it and retry.
    match tt_exec::run("git", &["checkout", &branch]) {
        Ok(out) if out.ok() => {
            ui::success(&format!("Checked out {branch}"));
            return 0;
        }
        Ok(_) => {}
        Err(e) => {
            ui::error(&format!("Failed to run git: {e}"));
            return 1;
        }
    }

    ui::info("Branch not found locally; fetching from origin...");
    match tt_exec::run("git", &["fetch", "origin", &branch]) {
        Ok(out) if out.ok() => {}
        Ok(out) => {
            ui::error(&format!("git fetch failed: {}", out.stderr.trim()));
            return 1;
        }
        Err(e) => {
            ui::error(&format!("Failed to run git: {e}"));
            return 1;
        }
    }

    match tt_exec::run("git", &["checkout", &branch]) {
        Ok(out) if out.ok() => {
            ui::success(&format!("Checked out {branch}"));
            0
        }
        Ok(out) => {
            ui::error(&format!("git checkout failed: {}", out.stderr.trim()));
            1
        }
        Err(e) => {
            ui::error(&format!("Failed to run git: {e}"));
            1
        }
    }
}

// ---------------------------------------------------------------------------
// gh branch (interactive issue picker)
// ---------------------------------------------------------------------------

fn branch(assigned_to_me: bool) -> i32 {
    if !gh_installed() {
        ui::error("Github CLI not installed");
        return 1;
    }

    println!("Assigned to me: {assigned_to_me}");

    let issue_list = match get_issues(assigned_to_me) {
        Ok(issues) => issues,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };

    if issue_list.is_empty() {
        ui::warning("No issues found, check assignments");
        return 1;
    }
    println!("{} Issues found assigned to you", issue_list.len());

    let layout = compute_column_layout(&issue_list, terminal_columns());
    let choices = build_issue_choices(&issue_list, &layout);

    let items: Vec<ChoiceItem> = choices
        .into_iter()
        .map(|c| {
            let label = match &c.description {
                Some(desc) => format!("{}  {desc}", c.title),
                None => c.title.clone(),
            };
            ChoiceItem { label, value: c.value }
        })
        .collect();

    if !std::io::stdin().is_terminal() {
        ui::error("Not a TTY; the issue picker requires an interactive terminal.");
        return 1;
    }

    let selected = match inquire::Select::new("Github issue to create branch for:", items).prompt()
    {
        Ok(item) => item,
        Err(_) => {
            ui::info("Canceled");
            return 0;
        }
    };

    let number = match selected.value {
        ChoiceValue::Issue(n) => n,
        ChoiceValue::Cancel => {
            ui::info("Canceled");
            return 0;
        }
    };

    let Some(issue) = issue_list.iter().find(|i| i.number == number) else {
        ui::error("Selected issue not found");
        return 1;
    };
    println!("Selected issue {} - {}", issue.number, issue.title);

    let branch_name = create_branch_name_from_issue(issue.number, &issue.title);
    match tt_exec::run("git", &["checkout", "-b", &branch_name]) {
        Ok(out) if out.ok() => 0,
        _ => {
            ui::error(&format!("Failed to create branch {branch_name}"));
            1
        }
    }
}

/// Run `gh issue list` and parse the result. Mirrors `getIssues`.
fn get_issues(assigned_to_me: bool) -> Result<Vec<Issue>, String> {
    let args = issues::issue_list_args(assigned_to_me, None);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = tt_exec::run("gh", &arg_refs).map_err(|e| format!("Failed to run gh: {e}"))?;
    issues::parse_issues(&out.stdout).map_err(|e| e.to_string())
}

/// A row shown in the interactive picker.
struct ChoiceItem {
    label: String,
    value: ChoiceValue,
}

impl fmt::Display for ChoiceItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// Prompt for a yes/no confirmation. Returns `None` when stdin is not a terminal, so the
/// caller can decide the non-interactive behavior.
fn confirm(message: &str, default: bool) -> Option<bool> {
    if !std::io::stdin().is_terminal() {
        return None;
    }
    inquire::Confirm::new(message).with_default(default).prompt().ok()
}

/// Terminal width, falling back to 80 columns off a terminal. Mirrors `getTerminalColumns`.
fn terminal_columns() -> i64 {
    let cols = console::Term::stdout().size().1 as i64;
    if cols <= 0 { 80 } else { cols }
}
