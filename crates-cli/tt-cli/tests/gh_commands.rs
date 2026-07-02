use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::path::Path;
use std::process::Command as StdCommand;
use tempfile::TempDir;

/// Run a git command in `dir`, asserting success (test setup only).
fn git(dir: &Path, args: &[&str]) {
    let status = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .expect("git should run");
    assert!(status.success(), "git {args:?} failed");
}

/// Create a temp repo on `main` with a merged `feature/done` branch.
fn repo_with_merged_branch() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "test@example.com"]);
    git(p, &["config", "user.name", "Test"]);
    // First commit on the default branch, then normalize to `main`.
    std::fs::write(p.join("README.md"), "hello").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "init"]);
    git(p, &["branch", "-M", "main"]);
    // A branch that gets merged back into main (fast-forward), so it is "merged".
    git(p, &["checkout", "-q", "-b", "feature/done"]);
    std::fs::write(p.join("f.txt"), "x").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "feature work"]);
    git(p, &["checkout", "-q", "main"]);
    git(p, &["merge", "-q", "feature/done"]);
    dir
}

fn tt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("tt").expect("binary `tt` should build");
    cmd.current_dir(dir);
    cmd
}

#[test]
fn branch_clean_dry_run_lists_without_deleting() {
    let repo = repo_with_merged_branch();
    tt(repo.path())
        .args(["gh", "branch-clean", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("feature/done"))
        .stdout(contains("Dry run"));

    // Branch still exists after a dry run.
    let out = StdCommand::new("git")
        .args(["branch", "--list", "feature/done"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("feature/done"));
}

#[test]
fn branch_clean_force_deletes_merged_branch() {
    let repo = repo_with_merged_branch();
    tt(repo.path())
        .args(["gh", "branch-clean", "--force"])
        .assert()
        .success()
        .stdout(contains("Deleted"));

    // Branch is gone.
    let out = StdCommand::new("git")
        .args(["branch", "--list", "feature/done"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("feature/done"));
}

#[test]
fn branch_clean_reports_nothing_to_clean() {
    let dir = TempDir::new().expect("temp dir");
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "test@example.com"]);
    git(p, &["config", "user.name", "Test"]);
    std::fs::write(p.join("README.md"), "hello").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "init"]);
    git(p, &["branch", "-M", "main"]);

    tt(p)
        .args(["gh", "branch-clean"])
        .assert()
        .success()
        .stdout(contains("No merged branches to clean up"));
}

#[test]
fn branch_clean_non_tty_without_force_does_not_delete() {
    let repo = repo_with_merged_branch();
    // No --force and not a TTY: should cancel as a no-op, leaving the branch intact.
    tt(repo.path())
        .args(["gh", "branch-clean"])
        .assert()
        .success()
        .stdout(contains("feature/done"))
        .stdout(contains("Deleted").not());

    let out = StdCommand::new("git")
        .args(["branch", "--list", "feature/done"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("feature/done"));
}
