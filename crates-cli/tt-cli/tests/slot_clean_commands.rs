//! Black-box tests for `tt slot clean` against a real checkout in a tempdir
//! (nested `.claude/worktrees/<name>` slots). Every invocation fakes
//! HOME/XDG_DATA_HOME: clean sweeps the machine's instance-state tree and
//! prunes the real agentboard store, so an unfaked run would mutate the
//! developer's actual state.

use assert_cmd::Command as Tt;
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

/// A `tt` command with the state tree sandboxed under `home`.
fn tt(home: &Path) -> Tt {
    let mut cmd = Tt::cargo_bin("tt").expect("binary `tt` should build");
    cmd.env("HOME", home);
    cmd.env("XDG_DATA_HOME", home.join(".local").join("share"));
    cmd.env_remove("TT_STATE_SCOPE");
    cmd
}

/// The slot dir for `name` under the checkout: `.claude/worktrees/<name>`.
fn slot_dir(checkout: &Path, name: &str) -> PathBuf {
    checkout.join(".claude").join("worktrees").join(name)
}

/// Build `<tmp>/demo` like the slot lifecycle tests, but with a committed
/// `crates/tt-config/` marker so the checkouts derive state scopes (`demo`,
/// `demo-<slot>`) and the sweep has something to key on.
fn make_checkout(tmp: &Path) -> PathBuf {
    let seed = tmp.join("seed");
    std::fs::create_dir_all(seed.join("crates").join("tt-config")).unwrap();
    git(tmp, &["init", "seed"]);
    std::fs::write(seed.join(".env.example"), "UI_PORT=${tt:port 42440-42469}\n").unwrap();
    std::fs::write(seed.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(seed.join("crates").join("tt-config").join(".gitkeep"), "").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed"]);

    git(tmp, &["clone", "seed", "demo"]);
    tmp.join("demo")
}

fn new_slot(home: &Path, root: &str, branch: &str) {
    tt(home).args(["slot", "new", "-b", branch, "--root", root]).assert().success();
}

fn commit_file(slot: &Path, name: &str) {
    std::fs::write(slot.join(name), "work").unwrap();
    git(slot, &["add", name]);
    git(slot, &["commit", "-m", name]);
}

fn branch_exists(checkout: &Path, branch: &str) -> bool {
    let out = Command::new("git")
        .args(["-C", checkout.to_str().unwrap(), "branch", "--list", branch])
        .output()
        .unwrap();
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

fn clean_json(home: &Path, root: &str, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["slot", "clean", "--json", "--root", root];
    args.extend_from_slice(extra);
    let out = tt(home).args(&args).output().unwrap();
    assert!(out.status.success(), "clean failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("clean --json emits JSON")
}

#[test]
fn clean_removes_merged_slot_and_sweeps_state_keeps_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    // done: committed and classically merged into main → finished.
    new_slot(&home, &root_s, "feat/done");
    commit_file(&slot_dir(&checkout, "done"), "done.txt");
    git(&checkout, &["merge", "--no-ff", "feat/done", "-m", "merge done"]);

    // dirty-done: merged too, but the tree is dirty → guard keeps it.
    new_slot(&home, &root_s, "feat/dirty-done");
    commit_file(&slot_dir(&checkout, "dirty-done"), "dd.txt");
    git(&checkout, &["merge", "--no-ff", "feat/dirty-done", "-m", "merge dd"]);
    std::fs::write(slot_dir(&checkout, "dirty-done").join("junk.txt"), "wip").unwrap();

    // fresh: created from the current tip, no commits → not finished.
    new_slot(&home, &root_s, "feat/fresh");

    // wip: has its own unmerged commit → active.
    new_slot(&home, &root_s, "feat/wip");
    commit_file(&slot_dir(&checkout, "wip"), "wip.txt");

    // Instance-state dirs: the removed slot's scope, an old orphan, a live
    // slot's scope, and a foreign repo's scope.
    let data_slots = home.join(".local/share/towles-tool/slots");
    let cfg_slots = home.join(".config/towles-tool/slots");
    for dir in [
        data_slots.join("demo-done"),
        data_slots.join("demo-stale-old"),
        data_slots.join("demo-wip"),
        data_slots.join("blog-x"),
        cfg_slots.join("demo-stale-cfg"),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
    }

    // Unscoped agentboard store: one window on a live folder, one on a folder
    // that no longer exists.
    let ab = home.join(".config/towles-tool/agentboard");
    std::fs::create_dir_all(&ab).unwrap();
    let live = checkout.to_string_lossy().to_string();
    let dead = "/nonexistent-folder-xyz";
    std::fs::write(
        ab.join("sessions.json"),
        format!(
            r#"{{"folders":{{"{live}":[{{"id":"sa","name":"shell 1","createdAt":1}}],"{dead}":[{{"id":"sb","name":"shell 1","createdAt":2}}]}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        ab.join("windows.json"),
        format!(
            r#"{{"windows":[{{"id":"w1","name":"primary","folderDir":"{live}","panes":["sa"]}},{{"id":"w2","name":"primary","folderDir":"{dead}","panes":["sb"]}}],"activeWindows":{{"{live}":"w1","{dead}":"w2"}}}}"#
        ),
    )
    .unwrap();

    let report = clean_json(&home, &root_s, &[]);

    // Only the merged-and-clean slot goes; its branch goes with it.
    let removed: Vec<&str> =
        report["removed"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(removed, vec!["done"]);
    assert!(report["removed"][0]["reason"].as_str().unwrap().contains("merged into main"));
    assert!(!slot_dir(&checkout, "done").exists());
    assert!(!branch_exists(&checkout, "feat/done"));

    let kept: Vec<&str> =
        report["kept"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(kept, vec!["dirty-done", "fresh", "wip"]);
    let dd = &report["kept"][0];
    assert!(dd["why"][0].as_str().unwrap().contains("not clean"), "got {dd}");
    assert!(slot_dir(&checkout, "dirty-done").exists());
    assert!(slot_dir(&checkout, "fresh").exists());
    assert!(slot_dir(&checkout, "wip").exists());
    assert!(branch_exists(&checkout, "feat/wip"));

    // Sweep: our stale scopes go (including the just-removed slot's), live and
    // foreign scopes stay.
    let swept: Vec<&str> =
        report["sweptStateDirs"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert!(!data_slots.join("demo-done").exists(), "swept: {swept:?}");
    assert!(!data_slots.join("demo-stale-old").exists());
    assert!(!cfg_slots.join("demo-stale-cfg").exists());
    assert!(data_slots.join("demo-wip").exists(), "live slot scope must survive");
    assert!(data_slots.join("blog-x").exists(), "foreign repo scope must survive");

    // Agentboard store: the dead folder's window + session records are gone.
    let windows: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ab.join("windows.json")).unwrap()).unwrap();
    let dirs: Vec<&str> = windows["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["folderDir"].as_str().unwrap())
        .collect();
    assert_eq!(dirs, vec![live.as_str()]);
    assert!(windows["activeWindows"].get(dead).is_none());
    let sessions: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ab.join("sessions.json")).unwrap()).unwrap();
    assert!(sessions["folders"].get(dead).is_none());
    assert!(sessions["folders"].get(&live).is_some());

    // A second run has nothing left to do.
    let report = clean_json(&home, &root_s, &[]);
    assert!(report["removed"].as_array().unwrap().is_empty());
    assert!(report["sweptStateDirs"].as_array().unwrap().is_empty());
    assert!(report["agentboard"].as_array().unwrap().is_empty());
}

#[test]
fn clean_removes_slot_whose_upstream_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    // Push the branch, then delete it on the "remote" (the seed repo) — the
    // squash-merge signature: commits landed under new SHAs, remote branch
    // deleted, so only `fetch --prune` + gone-upstream detection catches it.
    new_slot(&home, &root_s, "feat/push");
    let slot = slot_dir(&checkout, "push");
    commit_file(&slot, "pushed.txt");
    git(&slot, &["push", "-u", "origin", "feat/push"]);
    git(&tmp.path().join("seed"), &["branch", "-D", "feat/push"]);

    let report = clean_json(&home, &root_s, &[]);
    assert_eq!(report["removed"][0]["name"], "push");
    assert!(report["removed"][0]["reason"].as_str().unwrap().contains("upstream gone"));
    assert!(!slot.exists());
    assert!(!branch_exists(&checkout, "feat/push"));
}

#[test]
fn clean_dry_run_touches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    new_slot(&home, &root_s, "feat/done");
    commit_file(&slot_dir(&checkout, "done"), "done.txt");
    git(&checkout, &["merge", "--no-ff", "feat/done", "-m", "merge done"]);
    let stale = home.join(".local/share/towles-tool/slots/demo-gone");
    std::fs::create_dir_all(&stale).unwrap();

    let report = clean_json(&home, &root_s, &["--dry-run"]);
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["removed"][0]["name"], "done");
    assert!(slot_dir(&checkout, "done").exists(), "dry run must not remove");
    assert!(branch_exists(&checkout, "feat/done"));
    assert!(stale.exists(), "dry run must not sweep");
    let swept: Vec<&str> =
        report["sweptStateDirs"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert_eq!(swept.len(), 1, "reports the orphan scope: {swept:?}");
    assert!(swept[0].ends_with("demo-gone"));
}
