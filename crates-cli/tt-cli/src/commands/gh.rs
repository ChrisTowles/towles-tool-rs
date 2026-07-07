//! `ttr gh` subcommands: branch, branch-clean, pr (also reachable as `ttr pr`).
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

use crate::cli::{BranchCleanArgs, GhCommands, PrArgs};
use crate::ui;
use std::fmt;
use std::io::IsTerminal;
use tt_git::branch_name::create_branch_name_from_issue;
use tt_git::picker::{ChoiceValue, build_issue_choices, compute_column_layout};
use tt_git::pr::generate_pr_content;
use tt_git::{Issue, branch_clean, issues};

pub fn run(command: GhCommands) -> i32 {
    match command {
        GhCommands::Branch { assigned_to_me } => branch(assigned_to_me),
        GhCommands::BranchClean(args) => branch_clean_cmd(args),
        GhCommands::Pr(args) => pr(args),
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
// gh branch-clean
// ---------------------------------------------------------------------------

fn branch_clean_cmd(args: BranchCleanArgs) -> i32 {
    let current = current_branch().unwrap_or_default();

    let Some(merged) = git(&["branch", "--merged", &args.base]) else {
        return 1;
    };
    let to_delete = branch_clean::branches_to_delete(&merged.stdout, &args.base, &current);

    if to_delete.is_empty() {
        ui::info("No merged branches to clean up");
        return 0;
    }

    println!("Found {} merged branch(es):", to_delete.len());
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
        match tt_exec::run("git", &["branch", "-d", branch]) {
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
