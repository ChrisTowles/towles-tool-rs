//! Black-box tests for `tt slot` against a real primary checkout built in a
//! tempdir (`<root>/demo-primary` + `<root>/slots/<name>` worktrees).

use assert_cmd::Command as Tt;
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

fn tt() -> Tt {
    Tt::cargo_bin("tt").expect("binary `tt` should build")
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
    let out = tt()
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
    tt().args(["slot", "new", "-b", "fix/other", "--root", &root_s]).assert().success();
    let env2 = std::fs::read_to_string(root.join("slots").join("other").join(".env")).unwrap();
    assert!(env2.contains("SECRET=hunter2"), "new slot inherits sibling secrets: {env2}");
    assert!(!env2.contains(&format!("UI_PORT={ui_port}")), "new slot claims a different port");

    // a second slot for the same branch name is refused
    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("already exists"));

    // env re-render is idempotent: same port, secrets kept
    tt().args(["slot", "env", "thing", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env_again.contains(&format!("UI_PORT={ui_port}")), "re-render keeps the claim");
    assert!(env_again.contains("SECRET=hunter2"), "re-render keeps merged secrets");

    // the primary renders its own .env too (it is a checkout like any other)
    tt().args(["slot", "env", "primary", "--root", &root_s]).assert().success();
    let env_primary = std::fs::read_to_string(root.join("demo-primary").join(".env")).unwrap();
    assert!(env_primary.contains("NAME=demo-primary"), "env: {env_primary}");
    assert!(!root.join("demo-primary").join(".tt-slot").exists(), "primary gets no marker");

    // ls --json: primary first, then slots by name
    let out = tt().args(["slot", "ls", "--json", "--root", &root_s]).output().unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> =
        listed.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["primary", "other", "thing"]);
    assert_eq!(listed[0]["primary"], true);
    assert_eq!(listed[0]["branch"], "main");

    // rm a clean slot succeeds and releases the dir; the branch survives
    tt().args(["slot", "rm", "other", "--root", &root_s]).assert().success();
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
    tt().args(["slot", "rm", "primary", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("refusing to remove the primary"));
}

/// A slot created with `--base <non-default>` records *that* ref, not the
/// primary's currently-checked-out branch, in both the rendered `.env`'s
/// `${tt:base}` token and the `.tt-slot` marker — and re-rendering later
/// (`tt slot env`) must not let that drift even if the primary's branch has
/// since changed.
#[test]
fn new_with_base_records_the_actual_base_not_the_primary_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();
    let primary = root.join("demo-primary");

    git(&primary, &["checkout", "-b", "develop"]);
    git(&primary, &["checkout", "main"]);

    let out = tt()
        .args([
            "slot",
            "new",
            "-b",
            "feat/off-develop",
            "--base",
            "develop",
            "--json",
            "--root",
            &root_s,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(created["base"], "develop");

    let slot = root.join("slots").join("off-develop");
    let env = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env.contains("BASE=develop"), "env: {env}");
    let marker = std::fs::read_to_string(slot.join(".tt-slot")).unwrap();
    assert!(marker.contains("base=develop"), "marker: {marker}");

    // The primary switches branches after the slot was created (its "current
    // branch" is no longer `develop`, or even `main`) — re-rendering the
    // slot's env must still report the base it was actually created from.
    git(&primary, &["checkout", "-b", "unrelated"]);
    tt().args(["slot", "env", "off-develop", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env_again.contains("BASE=develop"), "re-render: {env_again}");
    let marker_again = std::fs::read_to_string(slot.join(".tt-slot")).unwrap();
    assert!(marker_again.contains("base=develop"), "re-render marker: {marker_again}");
}

/// A `new` that fails after `git worktree add` (e.g. no env template) must
/// roll the worktree back — leaving one behind blocks every retry with a
/// bogus "already exists" and hides the failed attempt from `slot ls`.
#[test]
fn new_rolls_back_the_worktree_when_env_render_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp.path(), &["init", "seed"]);
    // no .env.example / slot-env.template at all — render_slot_env must fail
    std::fs::write(seed.join("README.md"), "demo\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);
    let root = tmp.path().join("demo-repos");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp.path(), &["clone", "seed", "demo-repos/demo-primary"]);
    let root_s = root.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("no template"));

    assert!(!root.join("slots").join("thing").exists(), "the worktree must not be left behind");
    let worktrees = Command::new("git")
        .args([
            "-C",
            root.join("demo-primary").to_str().unwrap(),
            "worktree",
            "list",
        ])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&worktrees.stdout).contains("thing"),
        "git must not still track the rolled-back worktree"
    );

    // fixing the missing template lets slot creation succeed now that the
    // worktree (not just the branch, which git worktree remove never
    // deletes) isn't left dangling from the failed attempt
    std::fs::write(root.join("slot-env.template"), "NAME=${tt:slot-name}\n").unwrap();
    tt().args(["slot", "new", "-b", "feat/other", "--root", &root_s]).assert().success();
}

#[test]
fn rm_guards_dirty_and_orphan_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/work", "--root", &root_s]).assert().success();
    let slot = root.join("slots").join("work");

    // dirty tree refuses
    std::fs::write(slot.join("junk.txt"), "wip").unwrap();
    tt().args(["slot", "rm", "work", "--root", &root_s])
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
    tt().args(["slot", "rm", "work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("orphan"));

    // parking the commit on a branch makes removal safe (branches live in the
    // primary's .git)
    git(&slot, &["branch", "parked/detached-work"]);
    tt().args(["slot", "rm", "work", "--root", &root_s]).assert().success();
    assert!(!slot.exists());

    // --force path: recreate, dirty it, force through
    tt().args(["slot", "new", "-b", "feat/redo", "--root", &root_s]).assert().success();
    std::fs::write(root.join("slots").join("redo").join("junk.txt"), "wip").unwrap();
    tt().args(["slot", "rm", "redo", "--force", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("skipping guard"));
    assert!(!root.join("slots").join("redo").exists());
}

#[test]
fn rm_untracks_the_slot_and_removes_its_instance_state() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();
    // Sandbox every state path: fake HOME (tt-config resolves ~/.config and
    // the data dir through it) plus a forced TT_STATE_SCOPE — a forced scope
    // nests shared stores too, so nothing touches the real machine files.
    let home = tmp.path().join("home");
    let scope_env: Vec<(&str, String)> = vec![
        ("HOME", home.to_string_lossy().to_string()),
        ("XDG_DATA_HOME", home.join(".local").join("share").to_string_lossy().to_string()),
        (tt_config::STATE_SCOPE_ENV, "rm-test".to_string()),
    ];

    tt().args(["slot", "new", "-b", "feat/tracked", "--root", &root_s]).assert().success();
    let slot = root.join("slots").join("tracked");

    // Give the slot checkout this repo's scope marker (committed, so the tree
    // stays clean for the removal guards): its state scope becomes
    // `demo-tracked` (repo name from the sibling demo-primary + slot name).
    std::fs::create_dir_all(slot.join("crates").join("tt-config")).unwrap();
    std::fs::write(slot.join("crates").join("tt-config").join(".gitkeep"), "").unwrap();
    git(&slot, &["add", "."]);
    git(&slot, &["commit", "-m", "scope marker"]);

    // The app tracks slots it creates: simulate that in the sandboxed
    // repos.json, alongside a repo that must survive the removal.
    let shared = home.join(".config").join("towles-tool").join("slots").join("rm-test");
    let repos_json = shared.join("agentboard").join("repos.json");
    std::fs::create_dir_all(repos_json.parent().unwrap()).unwrap();
    let slot_s = slot.to_string_lossy().to_string();
    std::fs::write(
        &repos_json,
        serde_json::to_string_pretty(&serde_json::json!({
            "repoPaths": [slot_s, "/kept/elsewhere"],
        }))
        .unwrap(),
    )
    .unwrap();

    // Leftover instance state the removed slot's app instance wrote.
    let state_dir = shared.join("slots").join("demo-tracked");
    std::fs::create_dir_all(state_dir.join("agentboard")).unwrap();
    std::fs::write(state_dir.join("agentboard").join("sessions.json"), "{}\n").unwrap();

    let mut cmd = tt();
    cmd.envs(scope_env.iter().map(|(k, v)| (*k, v.as_str())));
    cmd.args(["slot", "rm", "tracked", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("untracked from the agentboard rail"))
        .stdout(contains("removed slot state"));

    assert!(!slot.exists());
    assert!(!state_dir.exists(), "the slot's orphaned instance state is swept");
    let repos: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&repos_json).unwrap()).unwrap();
    assert_eq!(
        repos["repoPaths"],
        serde_json::json!(["/kept/elsewhere"]),
        "the removed slot is untracked; other repos survive"
    );
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

    tt().args(["slot", "new", "-b", "feat/plain", "--root", &root_s]).assert().success();
    assert!(root.join("slots").join("plain").join(".env").is_file());
}
