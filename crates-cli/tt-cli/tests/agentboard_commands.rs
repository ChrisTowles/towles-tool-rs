mod common;

use common::cli_cmd;
use std::path::Path;
use tempfile::TempDir;

/// A `ttr agentboard` command with HOME redirected into the sandbox, so it reads
/// and writes the fixture `~/.config/towles-tool/agentboard/repos.json` instead of
/// the real one.
fn ab_cmd(temp: &Path) -> assert_cmd::Command {
    let mut cmd = cli_cmd(temp);
    cmd.env("HOME", temp);
    cmd
}

fn repos_json(temp: &Path) -> std::path::PathBuf {
    temp.join(".config").join("towles-tool").join("agentboard").join("repos.json")
}

/// Create a directory under the sandbox, optionally making it a git repo.
fn make_repo(temp: &Path, name: &str, git: bool) -> std::path::PathBuf {
    let dir = temp.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    if git {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }
    dir
}

#[test]
fn repos_empty_reports_none() {
    let temp = TempDir::new().unwrap();
    ab_cmd(temp.path())
        .args(["agentboard", "repos"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No repos configured"));
}

#[test]
fn repos_add_git_repo_and_list() {
    let temp = TempDir::new().unwrap();
    let repo = make_repo(temp.path(), "proj", true);

    ab_cmd(temp.path())
        .args(["agentboard", "repos", "add", repo.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("Added"));

    // repos.json now holds the canonicalized path.
    let content = std::fs::read_to_string(repos_json(temp.path())).unwrap();
    assert!(content.contains("\"repoPaths\""));
    let canon = std::fs::canonicalize(&repo).unwrap();
    assert!(content.contains(canon.to_str().unwrap()));

    // Listing shows the basename as the session name.
    ab_cmd(temp.path())
        .args(["agentboard", "repos"])
        .assert()
        .success()
        .stdout(predicates::str::contains("proj"));
}

#[test]
fn repos_add_non_git_warns_but_adds() {
    let temp = TempDir::new().unwrap();
    let repo = make_repo(temp.path(), "plain", false);

    ab_cmd(temp.path())
        .args(["agentboard", "repos", "add", repo.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("not a git repository"));

    let content = std::fs::read_to_string(repos_json(temp.path())).unwrap();
    let canon = std::fs::canonicalize(&repo).unwrap();
    assert!(content.contains(canon.to_str().unwrap()));
}

#[test]
fn repos_add_missing_path_errors() {
    let temp = TempDir::new().unwrap();
    ab_cmd(temp.path())
        .args(["agentboard", "repos", "add", "/nope/does/not/exist"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("does not exist"));
}

#[test]
fn repos_remove_by_name() {
    let temp = TempDir::new().unwrap();
    let repo = make_repo(temp.path(), "proj", true);
    ab_cmd(temp.path())
        .args(["agentboard", "repos", "add", repo.to_str().unwrap()])
        .assert()
        .success();

    // Remove by session name (basename).
    ab_cmd(temp.path())
        .args(["agentboard", "repos", "remove", "proj"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Removed"));

    let content = std::fs::read_to_string(repos_json(temp.path())).unwrap();
    assert!(!content.contains("proj"));
}

#[test]
fn repos_remove_by_path() {
    let temp = TempDir::new().unwrap();
    let repo = make_repo(temp.path(), "proj", true);
    let canon = std::fs::canonicalize(&repo).unwrap().to_string_lossy().to_string();
    ab_cmd(temp.path()).args(["agentboard", "repos", "add", &canon]).assert().success();

    ab_cmd(temp.path())
        .args(["agentboard", "repos", "remove", &canon])
        .assert()
        .success()
        .stdout(predicates::str::contains("Removed"));
}

#[test]
fn repos_remove_unknown_errors() {
    let temp = TempDir::new().unwrap();
    ab_cmd(temp.path())
        .args(["agentboard", "repos", "remove", "ghost"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("No watched repo matching"));
}

#[test]
fn ag_alias_works() {
    let temp = TempDir::new().unwrap();
    ab_cmd(temp.path())
        .args(["ag", "repos"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No repos configured"));
}
