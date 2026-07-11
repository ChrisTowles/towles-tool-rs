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

fn ttr(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ttr").expect("binary `ttr` should build");
    cmd.current_dir(dir);
    cmd
}

#[test]
fn branch_clean_dry_run_lists_without_deleting() {
    let repo = repo_with_merged_branch();
    ttr(repo.path())
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
    ttr(repo.path())
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

    ttr(p)
        .args(["gh", "branch-clean"])
        .assert()
        .success()
        .stdout(contains("No merged branches to clean up"));
}

/// Create a plain temp repo on `main` with one commit and `origin` set to `url`.
fn repo_with_remote(url: &str) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "test@example.com"]);
    git(p, &["config", "user.name", "Test"]);
    std::fs::write(p.join("README.md"), "hello").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "init"]);
    git(p, &["branch", "-M", "main"]);
    git(p, &["remote", "add", "origin", url]);
    dir
}

const REMOTE: &str = "git@github.com:user/repo.git";

#[test]
fn assign_rejects_slot_that_is_not_a_git_checkout() {
    let current = repo_with_remote(REMOTE);
    let not_a_repo = TempDir::new().unwrap();
    ttr(current.path())
        .args(["gh", "assign", "1", "--slot"])
        .arg(not_a_repo.path())
        .assert()
        .failure()
        .stderr(contains("not a git checkout"));
}

#[test]
fn assign_rejects_slot_with_mismatched_remote() {
    let current = repo_with_remote(REMOTE);
    let slot = repo_with_remote("git@github.com:someone-else/other.git");
    ttr(current.path())
        .args(["gh", "assign", "1", "--slot"])
        .arg(slot.path())
        .assert()
        .failure()
        .stderr(contains("remote does not match"));
}

#[test]
fn assign_rejects_dirty_slot() {
    let current = repo_with_remote(REMOTE);
    // Same repo (https form of the same remote — must still match), but with
    // an untracked file: the guard the feature exists for.
    let slot = repo_with_remote("https://github.com/user/repo");
    std::fs::write(slot.path().join("wip.txt"), "uncommitted").unwrap();
    ttr(current.path())
        .args(["gh", "assign", "1", "--slot"])
        .arg(slot.path())
        .assert()
        .failure()
        .stderr(contains("not clean"));
}

#[test]
fn assign_rejects_slot_with_stash() {
    let current = repo_with_remote(REMOTE);
    let slot = repo_with_remote(REMOTE);
    // Clean tree, but a stash entry — still blocked.
    std::fs::write(slot.path().join("README.md"), "changed").unwrap();
    git(slot.path(), &["stash", "push", "-q", "-m", "wip"]);
    ttr(current.path())
        .args(["gh", "assign", "1", "--slot"])
        .arg(slot.path())
        .assert()
        .failure()
        .stderr(contains("stash"));
}

#[test]
fn assign_runs_gh_develop_in_the_slot_when_clean() {
    let current = repo_with_remote(REMOTE);
    let slot = repo_with_remote(REMOTE);

    // Stub `gh` on PATH: answers --version, and for `issue develop` prints its
    // args + cwd so the test can assert it ran inside the SLOT's directory.
    let bin = TempDir::new().unwrap();
    let stub = bin.path().join("gh");
    std::fs::write(
        &stub,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'gh version 2.0.0 (https://github.com/cli/cli)'; exit 0; fi\necho \"gh-args: $*\"\npwd\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&stub).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&stub, perms).unwrap();
    let path = format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap());

    let slot_real = std::fs::canonicalize(slot.path()).unwrap();
    ttr(current.path())
        .env("PATH", path)
        .args(["gh", "assign", "42", "--slot"])
        .arg(slot.path())
        .assert()
        .success()
        .stdout(contains("gh-args: issue develop 42 --checkout"))
        .stdout(contains(slot_real.to_str().unwrap()))
        .stdout(contains("checked out in"));
}

#[test]
fn branch_clean_non_tty_without_force_does_not_delete() {
    let repo = repo_with_merged_branch();
    // No --force and not a TTY: should cancel as a no-op, leaving the branch intact.
    ttr(repo.path())
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
