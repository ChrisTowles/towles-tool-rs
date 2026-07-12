//! Black-box tests for `ttr slot` against a real bare hub built in a tempdir.

use assert_cmd::Command as Ttr;
use predicates::str::contains;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn ttr() -> Ttr {
    Ttr::cargo_bin("ttr").expect("binary `ttr` should build")
}

/// Build `<tmp>/demo-repos/demo.git` (bare hub) from a seed repo whose
/// committed `.env.example` carries `{tt:...}` tokens.
fn make_root(tmp: &Path) -> PathBuf {
    let seed = tmp.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp, &["init", "seed"]);
    std::fs::write(
        seed.join(".env.example"),
        "# demo slot env\nUI_PORT={tt:port 42410-42429}\nNAME={tt:slot-name}\nBASE={tt:base}\nURL=http://localhost:{tt:var UI_PORT}/\nSECRET=\n",
    )
    .unwrap();
    std::fs::write(seed.join(".gitignore"), ".env\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);

    let root = tmp.join("demo-repos");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp, &["clone", "--bare", "seed", "demo-repos/demo.git"]);
    root
}

#[test]
fn lifecycle_new_env_ls_rm() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    // new → slot-0, detached, rendered .env + marker
    let out = ttr().args(["slot", "new", "--json", "--root", &root_s]).output().unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(created["name"], "demo-slot-0");
    assert_eq!(created["detached"], true);
    assert_eq!(created["base"], "main");
    let slot0 = root.join("demo-slot-0");
    let env = std::fs::read_to_string(slot0.join(".env")).unwrap();
    assert!(env.contains("NAME=demo-slot-0"), "env: {env}");
    assert!(env.contains("BASE=main"));
    let ui_port = created["ports"]["UI_PORT"].as_u64().expect("UI_PORT claimed");
    assert!((42410..=42429).contains(&ui_port));
    assert!(env.contains(&format!("URL=http://localhost:{ui_port}/")));
    assert!(slot0.join(".tt-slot").is_file());
    // marker must not dirty the tree (hub info/exclude covers it)
    let porcelain = Command::new("git")
        .args(["-C", slot0.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&porcelain.stdout).trim(), "");

    // secrets inheritance: fill slot-0's SECRET, then create slot-1
    let filled = env.replace("SECRET=", "SECRET=hunter2");
    std::fs::write(slot0.join(".env"), filled).unwrap();
    ttr().args(["slot", "new", "--root", &root_s]).assert().success();
    let env1 = std::fs::read_to_string(root.join("demo-slot-1").join(".env")).unwrap();
    assert!(env1.contains("SECRET=hunter2"), "slot-1 inherits sibling secrets: {env1}");
    assert!(!env1.contains(&format!("UI_PORT={ui_port}")), "slot-1 claims a different port");

    // env re-render is idempotent: same port, secrets kept
    ttr().args(["slot", "env", "demo-slot-0", "--root", &root_s]).assert().success();
    let env0_again = std::fs::read_to_string(slot0.join(".env")).unwrap();
    assert!(env0_again.contains(&format!("UI_PORT={ui_port}")), "re-render keeps the claim");
    assert!(env0_again.contains("SECRET=hunter2"), "re-render keeps merged secrets");

    // ls --json sees both slots
    let out = ttr().args(["slot", "ls", "--json", "--root", &root_s]).output().unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> =
        listed.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["demo-slot-0", "demo-slot-1"]);

    // rm a clean detached slot succeeds and releases the dir
    ttr().args(["slot", "rm", "demo-slot-1", "--root", &root_s]).assert().success();
    assert!(!root.join("demo-slot-1").exists());
}

#[test]
fn rm_guards_dirty_and_orphan_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    ttr().args(["slot", "new", "--root", &root_s]).assert().success();
    let slot0 = root.join("demo-slot-0");

    // dirty tree refuses
    std::fs::write(slot0.join("junk.txt"), "wip").unwrap();
    ttr()
        .args(["slot", "rm", "demo-slot-0", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("not clean"));
    std::fs::remove_file(slot0.join("junk.txt")).unwrap();

    // a commit on detached HEAD would be orphaned by removal → refused
    std::fs::write(slot0.join("work.txt"), "real work").unwrap();
    git(&slot0, &["add", "work.txt"]);
    git(&slot0, &["commit", "-m", "detached work"]);
    ttr()
        .args(["slot", "rm", "demo-slot-0", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("orphan"));

    // parking the commit on a branch makes removal safe (branches live in the hub)
    git(&slot0, &["branch", "parked/detached-work"]);
    ttr().args(["slot", "rm", "demo-slot-0", "--root", &root_s]).assert().success();
    assert!(!slot0.exists());

    // --force path: recreate, dirty it, force through
    ttr().args(["slot", "new", "--root", &root_s]).assert().success();
    std::fs::write(root.join("demo-slot-0").join("junk.txt"), "wip").unwrap();
    ttr()
        .args(["slot", "rm", "demo-slot-0", "--force", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("skipping guard"));
    assert!(!root.join("demo-slot-0").exists());
}

#[test]
fn new_with_branch_checks_out_that_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    let out =
        ttr().args(["slot", "new", "-b", "feat/x", "--json", "--root", &root_s]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(created["branch"], "feat/x");
    assert_eq!(created["detached"], false);
    let branch = Command::new("git")
        .args([
            "-C",
            root.join("demo-slot-0").to_str().unwrap(),
            "branch",
            "--show-current",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feat/x");
}
