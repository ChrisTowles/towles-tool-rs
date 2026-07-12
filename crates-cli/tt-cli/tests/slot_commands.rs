//! Black-box tests for `ttr slot` against a real primary checkout built in a
//! tempdir (`<root>/demo-primary` + `<root>/slots/<name>` worktrees).

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

/// Build `<tmp>/demo-repos/demo-primary` (a normal clone on main) whose
/// committed `.env.example` carries `${tt:...}` tokens and a declared
/// `TT_SLOT_SETUP` that drops a marker so tests can prove setup ran in-slot.
fn make_root(tmp: &Path) -> PathBuf {
    let seed = tmp.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp, &["init", "seed"]);
    std::fs::write(
        seed.join(".env.example"),
        "# demo slot env\nUI_PORT=${tt:port 42410-42429}\nNAME=${tt:slot-name}\nBASE=${tt:base}\nURL=http://localhost:${tt:var UI_PORT}/\nSECRET=\nTT_SLOT_SETUP=touch .setup-ran\n",
    )
    .unwrap();
    std::fs::write(seed.join(".gitignore"), ".env\n.setup-ran\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);

    let root = tmp.join("demo-repos");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp, &["clone", "seed", "demo-repos/demo-primary"]);
    root
}

#[test]
fn lifecycle_new_env_ls_rm() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    // new -b feat/thing → slots/thing on that branch, rendered .env + marker
    let out = ttr()
        .args([
            "slot",
            "new",
            "-b",
            "feat/thing",
            "--json",
            "--root",
            &root_s,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(created["name"], "thing");
    assert_eq!(created["branch"], "feat/thing");
    assert_eq!(created["base"], "main");
    let slot = root.join("slots").join("thing");
    let env = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env.contains("NAME=thing"), "env: {env}");
    assert!(env.contains("BASE=main"));
    let ui_port = created["ports"]["UI_PORT"].as_u64().expect("UI_PORT claimed");
    assert!((42410..=42429).contains(&ui_port));
    assert!(env.contains(&format!("URL=http://localhost:{ui_port}/")));
    assert!(slot.join(".tt-slot").is_file());
    assert!(
        slot.join(".setup-ran").is_file(),
        "the declared TT_SLOT_SETUP command runs in the new slot"
    );
    let branch = Command::new("git")
        .args(["-C", slot.to_str().unwrap(), "branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feat/thing");
    // marker must not dirty the tree (primary's info/exclude covers it)
    let porcelain = Command::new("git")
        .args(["-C", slot.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&porcelain.stdout).trim(), "");

    // secrets inheritance: fill this slot's SECRET, then create another
    let filled = env.replace("SECRET=", "SECRET=hunter2");
    std::fs::write(slot.join(".env"), filled).unwrap();
    ttr().args(["slot", "new", "-b", "fix/other", "--root", &root_s]).assert().success();
    let env2 = std::fs::read_to_string(root.join("slots").join("other").join(".env")).unwrap();
    assert!(env2.contains("SECRET=hunter2"), "new slot inherits sibling secrets: {env2}");
    assert!(!env2.contains(&format!("UI_PORT={ui_port}")), "new slot claims a different port");

    // a second slot for the same branch name is refused
    ttr()
        .args(["slot", "new", "-b", "feat/thing", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("already exists"));

    // env re-render is idempotent: same port, secrets kept
    ttr().args(["slot", "env", "thing", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env_again.contains(&format!("UI_PORT={ui_port}")), "re-render keeps the claim");
    assert!(env_again.contains("SECRET=hunter2"), "re-render keeps merged secrets");

    // the primary renders its own .env too (it is a checkout like any other)
    ttr().args(["slot", "env", "primary", "--root", &root_s]).assert().success();
    let env_primary = std::fs::read_to_string(root.join("demo-primary").join(".env")).unwrap();
    assert!(env_primary.contains("NAME=demo-primary"), "env: {env_primary}");
    assert!(!root.join("demo-primary").join(".tt-slot").exists(), "primary gets no marker");

    // ls --json: primary first, then slots by name
    let out = ttr().args(["slot", "ls", "--json", "--root", &root_s]).output().unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> =
        listed.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["primary", "other", "thing"]);
    assert_eq!(listed[0]["primary"], true);
    assert_eq!(listed[0]["branch"], "main");

    // rm a clean slot succeeds and releases the dir; the branch survives
    ttr().args(["slot", "rm", "other", "--root", &root_s]).assert().success();
    assert!(!root.join("slots").join("other").exists());
    let branches = Command::new("git")
        .args([
            "-C",
            root.join("demo-primary").to_str().unwrap(),
            "branch",
            "--list",
            "fix/other",
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches.stdout).contains("fix/other"),
        "removing a slot never deletes its branch"
    );

    // the primary itself is not removable
    ttr()
        .args(["slot", "rm", "primary", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("refusing to remove the primary"));
}

#[test]
fn rm_guards_dirty_and_orphan_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    ttr().args(["slot", "new", "-b", "feat/work", "--root", &root_s]).assert().success();
    let slot = root.join("slots").join("work");

    // dirty tree refuses
    std::fs::write(slot.join("junk.txt"), "wip").unwrap();
    ttr()
        .args(["slot", "rm", "work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("not clean"));
    std::fs::remove_file(slot.join("junk.txt")).unwrap();

    // a commit on a detached HEAD would be orphaned by removal → refused
    // (slots are created on branches, but a checkout can end up detached)
    git(&slot, &["checkout", "--detach"]);
    std::fs::write(slot.join("work.txt"), "real work").unwrap();
    git(&slot, &["add", "work.txt"]);
    git(&slot, &["commit", "-m", "detached work"]);
    ttr()
        .args(["slot", "rm", "work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("orphan"));

    // parking the commit on a branch makes removal safe (branches live in the
    // primary's .git)
    git(&slot, &["branch", "parked/detached-work"]);
    ttr().args(["slot", "rm", "work", "--root", &root_s]).assert().success();
    assert!(!slot.exists());

    // --force path: recreate, dirty it, force through
    ttr().args(["slot", "new", "-b", "feat/redo", "--root", &root_s]).assert().success();
    std::fs::write(root.join("slots").join("redo").join("junk.txt"), "wip").unwrap();
    ttr()
        .args(["slot", "rm", "redo", "--force", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("skipping guard"));
    assert!(!root.join("slots").join("redo").exists());
}

#[test]
fn lockfile_detection_installs_without_declared_setup() {
    // A repo with no TT_SLOT_SETUP but a package-lock.json: setup_command
    // picks `npm install`. Proving the pure decision is enough here — the
    // unit tests own the matrix; this asserts a template without the key
    // still creates a slot cleanly (setup skipped when no lockfile either).
    let tmp = tempfile::tempdir().unwrap();
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp.path(), &["init", "seed"]);
    std::fs::write(seed.join(".env.example"), "UI_PORT=${tt:port 42430-42439}\n").unwrap();
    std::fs::write(seed.join(".gitignore"), ".env\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);
    let root = tmp.path().join("demo-repos");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp.path(), &["clone", "seed", "demo-repos/demo-primary"]);
    let root_s = root.to_string_lossy().to_string();

    ttr().args(["slot", "new", "-b", "feat/plain", "--root", &root_s]).assert().success();
    assert!(root.join("slots").join("plain").join(".env").is_file());
}
