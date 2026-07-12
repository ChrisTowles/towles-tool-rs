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

/// Write a stub `gh` on a fresh PATH dir that answers the calls `pr-list`
/// makes: `--version`, `repo view`, and the authored / review-requested
/// `pr list` searches. Returns the `PATH` string to hand to the command.
fn stub_gh_for_pr_list(bin: &TempDir) -> String {
    let stub = bin.path().join("gh");
    let script = r#"#!/bin/sh
case "$*" in
  "--version"*) echo "gh version 2.0.0 (https://github.com/cli/cli)"; exit 0;;
  *"repo view"*) echo '{"nameWithOwner":"o/r"}'; exit 0;;
  *"review-requested:@me"*) echo '[{"number":7,"title":"Please review","headRefName":"feat/rev","state":"OPEN","statusCheckRollup":[{"conclusion":"SUCCESS"}],"url":"https://github.com/o/r/pull/7","updatedAt":"2024-01-02T03:04:05Z"}]'; exit 0;;
  *"--author @me"*) echo '[{"number":42,"title":"Fix the thing","headRefName":"feat/x","state":"OPEN","statusCheckRollup":[{"conclusion":"FAILURE"}],"url":"https://github.com/o/r/pull/42","updatedAt":"2024-01-03T00:00:00Z"}]'; exit 0;;
  *) echo '[]'; exit 0;;
esac
"#;
    std::fs::write(&stub, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&stub).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&stub, perms).unwrap();
    format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap())
}

#[test]
fn pr_list_with_no_repos_configured_is_a_clean_noop() {
    // Empty HOME → no agentboard repos.json → nothing to list, exit 0.
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    ttr(cwd.path())
        .env("HOME", home.path())
        .env("TT_STATE_SCOPE", "") // force unscoped config paths
        .args(["gh", "pr-list"])
        .assert()
        .success()
        .stdout(contains("No repos configured"));
}

#[test]
fn pr_list_renders_prs_with_glyphs_and_needs_you() {
    let home = TempDir::new().unwrap();
    // A tracked repo dir (contents don't matter; gh is stubbed).
    let repo = TempDir::new().unwrap();
    let agentboard = home.path().join(".config/towles-tool/agentboard");
    std::fs::create_dir_all(&agentboard).unwrap();
    std::fs::write(
        agentboard.join("repos.json"),
        format!(r#"{{"repoPaths": ["{}"]}}"#, repo.path().display()),
    )
    .unwrap();

    let bin = TempDir::new().unwrap();
    let path = stub_gh_for_pr_list(&bin);

    ttr(repo.path())
        .env("HOME", home.path())
        .env("TT_STATE_SCOPE", "")
        .env("PATH", path)
        .args(["gh", "pr-list"])
        .assert()
        .success()
        .stdout(contains("o/r#42"))
        .stdout(contains("Fix the thing"))
        .stdout(contains("o/r#7"))
        .stdout(contains("Please review"))
        .stdout(contains("(review requested)"))
        .stdout(contains("2 open PRs · 2 need you"));
}

#[test]
fn prs_alias_reaches_pr_list() {
    // The top-level `ttr prs` alias resolves to the same command.
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    ttr(cwd.path())
        .env("HOME", home.path())
        .env("TT_STATE_SCOPE", "")
        .args(["prs"])
        .assert()
        .success()
        .stdout(contains("No repos configured"));
}

/// Build an `origin` bare remote for `local`, push `main`, then advance the
/// remote by one commit so `local` is one commit behind. Returns the local repo
/// (its `origin` already points at the bare remote).
fn repo_behind_remote() -> TempDir {
    let remote_dir = TempDir::new().expect("remote dir");
    // Leak the bare remote for the test's lifetime — the local repo's `origin`
    // points into it, and we don't need to clean it up in a unit test.
    let remote = Box::leak(Box::new(remote_dir)).path();
    StdCommand::new("git")
        .args(["init", "-q", "--bare", "--initial-branch=main"])
        .arg(remote)
        .status()
        .unwrap();

    let local = TempDir::new().expect("local dir");
    let p = local.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "test@example.com"]);
    git(p, &["config", "user.name", "Test"]);
    std::fs::write(p.join("README.md"), "hello").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "init"]);
    git(p, &["branch", "-M", "main"]);
    git(p, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(p, &["push", "-q", "-u", "origin", "main"]);

    // Advance the remote via a throwaway clone so `local` falls one behind.
    let other = TempDir::new().expect("other clone");
    let o = other.path();
    StdCommand::new("git").args(["clone", "-q", remote.to_str().unwrap()]).arg(o).status().unwrap();
    git(o, &["config", "user.email", "test@example.com"]);
    git(o, &["config", "user.name", "Test"]);
    std::fs::write(o.join("upstream.txt"), "new").unwrap();
    git(o, &["add", "."]);
    git(o, &["commit", "-q", "-m", "upstream commit"]);
    git(o, &["push", "-q", "origin", "main"]);

    local
}

#[test]
fn sync_fast_forwards_a_clean_behind_branch() {
    let repo = repo_behind_remote();
    ttr(repo.path())
        .args(["gh", "sync"])
        .assert()
        .success()
        .stdout(contains("Before: 0 ahead, 1 behind"))
        .stdout(contains("After: 0 ahead, 0 behind"))
        .stdout(contains("In sync with origin/main"));

    // The upstream commit is now present locally.
    assert!(repo.path().join("upstream.txt").exists());
}

#[test]
fn sync_hard_fails_on_a_dirty_tree_before_rebasing() {
    let repo = repo_behind_remote();
    // Dirty the tree with an untracked file: the guard must trip first.
    std::fs::write(repo.path().join("wip.txt"), "uncommitted").unwrap();
    ttr(repo.path())
        .args(["gh", "sync"])
        .assert()
        .failure()
        .stderr(contains("Working tree is not clean"))
        .stderr(contains("wip.txt"));

    // The rebase never ran, so we're still behind (upstream commit absent).
    assert!(!repo.path().join("upstream.txt").exists());
}

#[test]
fn co_with_non_numeric_arg_is_a_clap_error() {
    let repo = repo_with_remote(REMOTE);
    ttr(repo.path())
        .args(["gh", "co", "not-a-number"])
        .assert()
        .failure()
        .stderr(contains("invalid value").or(contains("invalid digit")));
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
