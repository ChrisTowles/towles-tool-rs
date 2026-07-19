//! Black-box tests for `tt slot` against a real checkout built in a tempdir
//! (`<tmp>/demo` + nested `<tmp>/demo/.claude/worktrees/<name>` worktrees).

use assert_cmd::Command as Tt;
use predicates::prelude::PredicateBooleanExt;
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

/// The slot dir for `name` under the checkout: `.claude/worktrees/<name>`.
fn slot_dir(checkout: &Path, name: &str) -> PathBuf {
    checkout.join(".claude").join("worktrees").join(name)
}

/// Build `<tmp>/demo` (a normal clone on main) whose committed
/// `.env.example` carries `${tt:...}` tokens and a declared `TT_SLOT_SETUP`
/// that drops a marker so tests can prove setup ran in-slot.
fn make_checkout(tmp: &Path) -> PathBuf {
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

    git(tmp, &["clone", "seed", "demo"]);
    tmp.join("demo")
}

#[test]
fn lifecycle_new_env_ls_rm() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    // new -b feat/thing → .claude/worktrees/thing on that branch, rendered
    // .env + marker
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
    assert_eq!(created["name"], "feat-thing");
    assert_eq!(created["branch"], "feat/thing");
    assert_eq!(created["base"], "main");
    let slot = slot_dir(&checkout, "feat-thing");
    assert_eq!(created["dir"], slot.to_string_lossy().as_ref());
    let env = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env.contains("NAME=feat-thing"), "env: {env}");
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
    // marker must not dirty the slot's tree, and the nested worktrees dir
    // must not dirty the main checkout's (info/exclude covers both)
    for dir in [&slot, &checkout] {
        let porcelain = Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "status", "--porcelain"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&porcelain.stdout).trim(),
            "",
            "{} must stay clean",
            dir.display()
        );
    }

    // secrets inheritance: fill this slot's SECRET, then create another
    let filled = env.replace("SECRET=", "SECRET=hunter2");
    std::fs::write(slot.join(".env"), filled).unwrap();
    tt().args(["slot", "new", "-b", "fix/other", "--root", &root_s]).assert().success();
    let env2 = std::fs::read_to_string(slot_dir(&checkout, "fix-other").join(".env")).unwrap();
    assert!(env2.contains("SECRET=hunter2"), "new slot inherits sibling secrets: {env2}");
    assert!(!env2.contains(&format!("UI_PORT={ui_port}")), "new slot claims a different port");

    // a second slot for the same branch name is refused
    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("already exists"));

    // env re-render is idempotent: same port, secrets kept
    tt().args(["slot", "env", "feat-thing", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env_again.contains(&format!("UI_PORT={ui_port}")), "re-render keeps the claim");
    assert!(env_again.contains("SECRET=hunter2"), "re-render keeps merged secrets");

    // the main checkout renders its own .env too (it is a checkout like any
    // other) — `primary` still names it
    tt().args(["slot", "env", "primary", "--root", &root_s]).assert().success();
    let env_primary = std::fs::read_to_string(checkout.join(".env")).unwrap();
    assert!(env_primary.contains("NAME=demo"), "env: {env_primary}");
    assert!(!checkout.join(".tt-slot").exists(), "the main checkout gets no marker");

    // running from inside a slot anchors at the main checkout — no
    // worktrees-inside-worktrees
    let slot_s = slot.to_string_lossy().to_string();
    tt().args(["slot", "new", "-b", "feat/from-inside", "--root", &slot_s]).assert().success();
    assert!(slot_dir(&checkout, "feat-from-inside").is_dir());
    assert!(!slot_dir(&slot, "feat-from-inside").exists());
    tt().args(["slot", "rm", "feat-from-inside", "--root", &root_s]).assert().success();

    // ls --json: the main checkout first, then slots by name
    let out = tt().args(["slot", "ls", "--json", "--root", &root_s]).output().unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> =
        listed.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["primary", "feat-thing", "fix-other"]);
    assert_eq!(listed[0]["primary"], true);
    assert_eq!(listed[0]["branch"], "main");

    // rm a clean slot succeeds and releases the dir; the branch survives
    tt().args(["slot", "rm", "fix-other", "--root", &root_s]).assert().success();
    assert!(!slot_dir(&checkout, "fix-other").exists());
    let branches = Command::new("git")
        .args([
            "-C",
            checkout.to_str().unwrap(),
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

    // the main checkout itself is not removable
    tt().args(["slot", "rm", "primary", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("refusing to remove the primary"));
    tt().args(["slot", "rm", "demo", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("refusing to remove the primary"));
}

/// A slot created with `--base <non-default>` records *that* ref, not the
/// main checkout's currently-checked-out branch, in both the rendered
/// `.env`'s `${tt:base}` token and the `.tt-slot` marker — and re-rendering
/// later (`tt slot env`) must not let that drift even if the checkout's
/// branch has since changed.
#[test]
fn new_with_base_records_the_actual_base_not_the_primary_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    git(&checkout, &["checkout", "-b", "develop"]);
    git(&checkout, &["checkout", "main"]);

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

    let slot = slot_dir(&checkout, "feat-off-develop");
    let env = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env.contains("BASE=develop"), "env: {env}");
    let marker = std::fs::read_to_string(slot.join(".tt-slot")).unwrap();
    assert!(marker.contains("base=develop"), "marker: {marker}");

    // The main checkout switches branches after the slot was created (its
    // "current branch" is no longer `develop`, or even `main`) —
    // re-rendering the slot's env must still report the base it was
    // actually created from.
    git(&checkout, &["checkout", "-b", "unrelated"]);
    tt().args(["slot", "env", "feat-off-develop", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env_again.contains("BASE=develop"), "re-render: {env_again}");
    let marker_again = std::fs::read_to_string(slot.join(".tt-slot")).unwrap();
    assert!(marker_again.contains("base=develop"), "re-render marker: {marker_again}");
}

/// If the checkout's base branch has fallen behind `origin/<base>` (the user
/// hasn't pulled `main` in a while), `new` fast-forwards it before branching
/// — so the new slot starts from current history instead of needing a
/// rebase the moment it next syncs with base.
#[test]
fn new_fast_forwards_a_base_behind_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
    let seed = tmp.path().join("seed");

    // origin (seed) moves on after the checkout cloned it — a real-world
    // "someone merged to main since I last pulled" scenario.
    std::fs::write(seed.join("upstream.txt"), "new on origin\n").unwrap();
    git(&seed, &["add", "upstream.txt"]);
    git(&seed, &["commit", "-m", "upstream moves on"]);
    let origin_head = Command::new("git")
        .args(["-C", seed.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;

    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s]).assert().success();

    // the checkout's local `main` was fast-forwarded to match origin...
    let local_head = Command::new("git")
        .args(["-C", checkout.to_str().unwrap(), "rev-parse", "main"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(
        String::from_utf8_lossy(&local_head).trim(),
        String::from_utf8_lossy(&origin_head).trim(),
        "the checkout's main should be fast-forwarded to origin/main"
    );

    // ...so the new slot branches from that current history, not the stale
    // commit the checkout had when the slot command started
    assert!(slot_dir(&checkout, "feat-thing").join("upstream.txt").is_file());
}

/// When the checkout's base branch has diverged from `origin/<base>` (both
/// moved independently), a plain fast-forward is impossible — `new` must
/// warn rather than fail, and still create the slot off the local history as
/// it always did before this check existed.
#[test]
fn new_warns_but_still_creates_when_base_diverged_from_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
    let seed = tmp.path().join("seed");

    // origin moves on...
    std::fs::write(seed.join("upstream.txt"), "origin work\n").unwrap();
    git(&seed, &["add", "upstream.txt"]);
    git(&seed, &["commit", "-m", "upstream moves on"]);

    // ...and so does the local main, independently — a genuine divergence a
    // fast-forward can't resolve.
    std::fs::write(checkout.join("local.txt"), "local work\n").unwrap();
    git(&checkout, &["add", "local.txt"]);
    git(&checkout, &["commit", "-m", "local work"]);

    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("diverged from origin/main"))
        .stdout(contains("could not be fast-forwarded"));

    // creation still succeeds, branching off the checkout's own (unmoved)
    // local main rather than blocking on the divergence
    let slot = slot_dir(&checkout, "feat-thing");
    assert!(slot.join("local.txt").is_file());
    assert!(!slot.join("upstream.txt").is_file());
}

/// A repo with neither a tokenized `.env.example` nor the
/// `.claude/slot-env.template` sidecar — any plain checkout never onboarded
/// onto slots — must still get a slot: the render falls back to an empty
/// template (empty `.env`, no port claims) instead of failing with the old
/// "no template" error (hit for real creating toolbox slots from the app).
#[test]
fn new_works_in_a_repo_with_no_template() {
    let tmp = tempfile::tempdir().unwrap();
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp.path(), &["init", "seed"]);
    std::fs::write(seed.join("README.md"), "plain repo\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);
    git(tmp.path(), &["clone", "seed", "demo"]);
    let checkout = tmp.path().join("demo");
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s]).assert().success();

    let slot = slot_dir(&checkout, "feat-thing");
    assert!(slot.join(".tt-slot").is_file(), "the slot marker must still be written");
    let env = std::fs::read_to_string(slot.join(".env")).unwrap();
    assert!(env.trim().is_empty(), "nothing to template → an empty .env, got: {env}");

    // re-rendering the templateless slot stays a no-op, not an error
    tt().args(["slot", "env", "feat-thing", "--root", &root_s]).assert().success();
}

/// A `new` that fails after `git worktree add` (e.g. a template render
/// error) must roll the worktree back — leaving one behind blocks every
/// retry with a bogus "already exists" and hides the failed attempt from
/// `slot ls`.
#[test]
fn new_rolls_back_the_worktree_when_env_render_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp.path(), &["init", "seed"]);
    // a committed .env.example with a malformed ${tt:...} token — the render
    // is a hard error, and it happens after the worktree already exists
    std::fs::write(seed.join(".env.example"), "X=${tt:prot 3000-3010}\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);
    git(tmp.path(), &["clone", "seed", "demo"]);
    let checkout = tmp.path().join("demo");
    let root_s = checkout.to_string_lossy().to_string();

    // the error names the template file and the offending line, so the fix
    // is actionable straight from the message
    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("env template"))
        .stderr(contains(".env.example"))
        .stderr(contains("unknown or malformed token"));

    assert!(!slot_dir(&checkout, "feat-thing").exists(), "the worktree must not be left behind");
    let worktrees = Command::new("git")
        .args(["-C", checkout.to_str().unwrap(), "worktree", "list"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&worktrees.stdout).contains("thing"),
        "git must not still track the rolled-back worktree"
    );

    // fixing the template lets the SAME branch be retried — the rollback
    // deleted the branch it had just created along with the worktree, so
    // nothing from the failed attempt blocks the redo (hit for real
    // migrating the blog repo: fix template, retry, "already exists")
    std::fs::write(checkout.join(".env.example"), "NAME=${tt:slot-name}\n").unwrap();
    git(&checkout, &["add", ".env.example"]);
    git(&checkout, &["commit", "-m", "fix template"]);
    tt().args(["slot", "new", "-b", "feat/thing", "--root", &root_s]).assert().success();
}

#[test]
fn rm_guards_dirty_and_orphan_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/work", "--root", &root_s]).assert().success();
    let slot = slot_dir(&checkout, "feat-work");

    // dirty tree refuses — and says what to do about it, not just what's
    // wrong: a refusal with no next step is the dead end this output exists
    // to avoid (the app's blocked-delete dialog renders the same two halves).
    std::fs::write(slot.join("junk.txt"), "wip").unwrap();
    tt().args(["slot", "rm", "feat-work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("not clean"))
        .stderr(contains("Commit or stash"))
        .stderr(contains("--force"))
        .stderr(contains("discards the work above"));
    std::fs::remove_file(slot.join("junk.txt")).unwrap();

    // a commit on a detached HEAD would be orphaned by removal → refused
    // (slots are created on branches, but a checkout can end up detached)
    git(&slot, &["checkout", "--detach"]);
    std::fs::write(slot.join("work.txt"), "real work").unwrap();
    git(&slot, &["add", "work.txt"]);
    git(&slot, &["commit", "-m", "detached work"]);
    tt().args(["slot", "rm", "feat-work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("orphan"));

    // parking the commit on a branch makes removal safe (branches live in the
    // main checkout's .git)
    git(&slot, &["branch", "parked/detached-work"]);
    tt().args(["slot", "rm", "feat-work", "--root", &root_s]).assert().success();
    assert!(!slot.exists());

    // --force path: recreate, dirty it, force through
    tt().args(["slot", "new", "-b", "feat/redo", "--root", &root_s]).assert().success();
    std::fs::write(slot_dir(&checkout, "feat-redo").join("junk.txt"), "wip").unwrap();
    tt().args(["slot", "rm", "feat-redo", "--force", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("skipping guard"));
    assert!(!slot_dir(&checkout, "feat-redo").exists());
}

/// Committed-but-unlanded work does not block removal — the branch keeps it —
/// but the two must never be confused with each other, so removal says which
/// one it is instead of reporting a bare success.
#[test]
fn rm_reports_unlanded_commits_it_does_not_block_on() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/unlanded", "--root", &root_s]).assert().success();
    let slot = slot_dir(&checkout, "feat-unlanded");
    std::fs::write(slot.join("kept.txt"), "committed work").unwrap();
    git(&slot, &["add", "kept.txt"]);
    git(&slot, &["commit", "-m", "work that never landed"]);

    tt().args(["slot", "rm", "feat-unlanded", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("have not reached main").and(contains("stay on the branch")));

    assert!(!slot.exists());
    // The commit survives on its branch — that is why this is not a guard.
    let branches = std::process::Command::new("git")
        .args([
            "-C",
            checkout.to_str().unwrap(),
            "branch",
            "--list",
            "feat/unlanded",
        ])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&branches.stdout).trim().is_empty());
}

/// A slot whose branch really did land reports that instead, so "removed" and
/// "removed, but you still owe a push" never look the same.
#[test]
fn rm_reports_a_merged_branch_as_having_nothing_outstanding() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/landed", "--root", &root_s]).assert().success();
    let slot = slot_dir(&checkout, "feat-landed");
    // Two commits: squashing a single commit yields a patch-identical one,
    // which is genuinely indistinguishable from a rebase (and reports as
    // such). Collapsing several is what only the tree probe can recognise.
    std::fs::write(slot.join("a.txt"), "a").unwrap();
    git(&slot, &["add", "a.txt"]);
    git(&slot, &["commit", "-m", "a"]);
    std::fs::write(slot.join("b.txt"), "b").unwrap();
    git(&slot, &["add", "b.txt"]);
    git(&slot, &["commit", "-m", "b"]);
    // Squash it onto main the way a merged PR would, under a fresh SHA.
    git(&checkout, &["merge", "--squash", "feat/landed"]);
    git(&checkout, &["commit", "-m", "squashed feat/landed (#1)"]);

    tt().args(["slot", "rm", "feat-landed", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("squash-merged into main").and(contains("nothing outstanding")));
}

#[test]
fn rm_untracks_the_slot_and_removes_its_instance_state() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
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
    let slot = slot_dir(&checkout, "feat-tracked");

    // Give the slot checkout this repo's scope marker (committed, so the tree
    // stays clean for the removal guards): its state scope becomes
    // `demo-feat-tracked` (main checkout dir name + slot name).
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
    let state_dir = shared.join("slots").join("demo-feat-tracked");
    std::fs::create_dir_all(state_dir.join("agentboard")).unwrap();
    std::fs::write(state_dir.join("agentboard").join("sessions.json"), "{}\n").unwrap();

    let mut cmd = tt();
    cmd.envs(scope_env.iter().map(|(k, v)| (*k, v.as_str())));
    cmd.args(["slot", "rm", "feat-tracked", "--root", &root_s])
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
    git(tmp.path(), &["clone", "seed", "demo"]);
    let checkout = tmp.path().join("demo");
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/plain", "--root", &root_s]).assert().success();
    assert!(slot_dir(&checkout, "feat-plain").join(".env").is_file());
}

/// The Claude Code WorktreeCreate hook shell: stdin is the hook JSON, stdout
/// is exactly the slot path, the requested name IS the branch verbatim, and
/// a re-request for the same name returns the same path instead of failing.
#[test]
fn hook_create_creates_a_slot_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let expected = slot_dir(&checkout, "auth-flow");

    let hook_input = serde_json::json!({
        "session_id": "abc",
        "hook_event_name": "WorktreeCreate",
        "cwd": checkout.to_string_lossy(),
        "name": "auth-flow",
    })
    .to_string();

    let out = tt().args(["slot", "hook-create"]).write_stdin(hook_input.clone()).output().unwrap();
    assert!(out.status.success(), "hook-create failed: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        expected.to_string_lossy(),
        "stdout must be exactly the worktree path"
    );
    let branch = Command::new("git")
        .args(["-C", expected.to_str().unwrap(), "branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        "auth-flow",
        "the requested worktree name is the branch, verbatim"
    );
    assert!(expected.join(".env").is_file(), "hook-created slots render .env like tt slot new");

    // Same name again → same path, exit 0 (Claude Code re-enters worktrees).
    let again = tt().args(["slot", "hook-create"]).write_stdin(hook_input).output().unwrap();
    assert!(again.status.success());
    assert_eq!(String::from_utf8_lossy(&again.stdout).trim(), expected.to_string_lossy());
}

/// Distinct branches can slug to the same slot folder (`feat/thing` and a
/// literal `feat-thing` both slug to `feat-thing`). A second WorktreeCreate
/// hitting that same folder on a different requested branch must fail loudly
/// instead of silently resuming into someone else's worktree.
#[test]
fn hook_create_refuses_to_resume_a_slug_collision_on_a_different_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());

    let hook_input = |name: &str| {
        serde_json::json!({
            "hook_event_name": "WorktreeCreate",
            "cwd": checkout.to_string_lossy(),
            "name": name,
        })
        .to_string()
    };

    let first =
        tt().args(["slot", "hook-create"]).write_stdin(hook_input("feat/thing")).output().unwrap();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));

    let collided =
        tt().args(["slot", "hook-create"]).write_stdin(hook_input("feat-thing")).output().unwrap();
    assert!(!collided.status.success(), "must not silently resume a different branch's slot");
    let stderr = String::from_utf8_lossy(&collided.stderr);
    assert!(stderr.contains("feat/thing"), "stderr: {stderr}");
    assert!(stderr.contains("feat-thing"), "stderr: {stderr}");

    // The original slot is untouched — still on its own branch.
    let branch = Command::new("git")
        .args([
            "-C",
            slot_dir(&checkout, "feat-thing").to_str().unwrap(),
            "branch",
            "--show-current",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feat/thing");
}

/// The WorktreeRemove hook shell runs the same guards as `tt slot rm`: a
/// clean slot goes away, a dirty one is refused (non-zero, message on
/// stderr) and stays on disk.
#[test]
fn hook_remove_is_guarded_like_rm() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "new", "-b", "feat/done", "--root", &root_s]).assert().success();
    let slot = slot_dir(&checkout, "feat-done");
    let hook_input = serde_json::json!({
        "hook_event_name": "WorktreeRemove",
        "cwd": checkout.to_string_lossy(),
        "worktree_path": slot.to_string_lossy(),
    })
    .to_string();

    // dirty → refused, slot stays
    std::fs::write(slot.join("wip.txt"), "unsaved").unwrap();
    tt().args(["slot", "hook-remove"])
        .write_stdin(hook_input.clone())
        .assert()
        .failure()
        .stderr(contains("not clean"));
    assert!(slot.exists());

    // clean → removed
    std::fs::remove_file(slot.join("wip.txt")).unwrap();
    tt().args(["slot", "hook-remove"]).write_stdin(hook_input.clone()).assert().success();
    assert!(!slot.exists());

    // already gone → a no-op success, not an error (Claude Code may fire the
    // hook for a worktree the user already cleaned up)
    tt().args(["slot", "hook-remove"]).write_stdin(hook_input).assert().success();
}

#[test]
fn init_onboards_a_bare_repo_idempotently() {
    let tmp = tempfile::tempdir().unwrap();
    // A repo with no tokenized .env.example, no .gitignore, no settings.json.
    let checkout = tmp.path().join("bare");
    std::fs::create_dir_all(&checkout).unwrap();
    git(tmp.path(), &["init", "bare"]);
    std::fs::write(checkout.join("README.md"), "hi\n").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-m", "seed"]);
    let root_s = checkout.to_string_lossy().to_string();

    tt().args(["slot", "init", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("slot-env.template"))
        .stdout(contains("gitignore: added .env"))
        .stdout(contains("hooks: wired"));

    assert!(checkout.join(".claude").join("slot-env.template").is_file());
    assert!(checkout.join(".env").is_file());
    let gitignore = std::fs::read_to_string(checkout.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".env"));
    let settings = std::fs::read_to_string(checkout.join(".claude").join("settings.json")).unwrap();
    assert!(settings.contains("tt slot hook-create"));
    assert!(settings.contains("tt slot hook-remove"));

    // Re-run: nothing to do, nothing clobbered.
    tt().args(["slot", "init", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("hooks: already wired"));
    let settings_again =
        std::fs::read_to_string(checkout.join(".claude").join("settings.json")).unwrap();
    assert_eq!(settings, settings_again);
}
