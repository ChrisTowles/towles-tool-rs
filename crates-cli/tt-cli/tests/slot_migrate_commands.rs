//! Black-box tests for `ttr slot migrate`: a root of full clones (numbered
//! slots plus an unnumbered primary) becomes a bare hub + worktree slots
//! without losing branches, stashes, dirty trees, or env files.

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

fn git_out(dir: &Path, args: &[&str]) -> String {
    let output =
        Command::new("git").arg("-C").arg(dir).args(args).output().expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn ttr() -> Ttr {
    Ttr::cargo_bin("ttr").expect("binary `ttr` should build")
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

/// `<tmp>/demo-repos` holding full clones of a shared origin:
/// - `demo` (primary): clean on main, one stash entry, a local branch
///   `feat/y`, and `extensions.worktreeConfig` enabled (the core.bare gotcha)
/// - `demo-slot-0`: on branch `feat/x` with a commit, a dirty tree
///   (modified + untracked), and a `.env.local`
/// - `demo-slot-1`: detached at main~1
/// - `demo-slot-2`: on its own `feat/y`, diverged from the primary's
fn make_clone_root(tmp: &Path) -> PathBuf {
    let seed = tmp.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp, &["init", "seed"]);
    write(&seed, "README.md", "one\n");
    write(&seed, ".gitignore", ".env\n.env.local\n");
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "c1"]);
    write(&seed, "README.md", "one\ntwo\n");
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "c2"]);
    git(tmp, &["clone", "--bare", "seed", "origin.git"]);
    let origin = tmp.join("origin.git");
    let origin_s = origin.to_string_lossy().to_string();

    let root = tmp.join("demo-repos");
    std::fs::create_dir_all(&root).unwrap();

    let primary = root.join("demo");
    git(&root, &["clone", &origin_s, "demo"]);
    git(&primary, &["checkout", "-b", "feat/y"]);
    write(&primary, "e.txt", "primary side\n");
    git(&primary, &["add", "."]);
    git(&primary, &["commit", "-m", "E"]);
    git(&primary, &["checkout", "main"]);
    write(&primary, "README.md", "one\ntwo\nstashed\n");
    git(&primary, &["stash", "push", "-m", "wip"]);
    git(&primary, &["config", "extensions.worktreeConfig", "true"]);

    let slot0 = root.join("demo-slot-0");
    git(&root, &["clone", &origin_s, "demo-slot-0"]);
    git(&slot0, &["checkout", "-b", "feat/x"]);
    write(&slot0, "feat.txt", "committed\n");
    git(&slot0, &["add", "."]);
    git(&slot0, &["commit", "-m", "C"]);
    write(&slot0, "feat.txt", "committed\nwip\n");
    write(&slot0, "wip.txt", "notes\n");
    write(&slot0, ".env.local", "TT_DEV_PORT=1441\n");

    let slot1 = root.join("demo-slot-1");
    git(&root, &["clone", &origin_s, "demo-slot-1"]);
    git(&slot1, &["checkout", "HEAD~1"]);

    let slot2 = root.join("demo-slot-2");
    git(&root, &["clone", &origin_s, "demo-slot-2"]);
    git(&slot2, &["checkout", "-b", "feat/y"]);
    write(&slot2, "d.txt", "slot-2 side\n");
    git(&slot2, &["add", "."]);
    git(&slot2, &["commit", "-m", "D"]);

    // a hub-side sidecar template so migration renders each slot's .env via
    // the same op `slot new`/`slot env` use (distinct ports, slot identity)
    write(&root, "slot-env.template", "TT_DEV_PORT=${tt:port 4000-4099}\nSLOT=${tt:slot-name}\n");

    root
}

#[test]
fn migrate_converts_clones_preserving_everything() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_clone_root(tmp.path());
    let root_s = root.to_string_lossy().to_string();

    // tips recorded before migration, to verify against afterwards
    let main_sha = git_out(&root.join("demo"), &["rev-parse", "main"]);
    let feat_x = git_out(&root.join("demo-slot-0"), &["rev-parse", "feat/x"]);
    let feat_y_primary = git_out(&root.join("demo"), &["rev-parse", "feat/y"]);
    let feat_y_slot2 = git_out(&root.join("demo-slot-2"), &["rev-parse", "feat/y"]);
    let slot1_head = git_out(&root.join("demo-slot-1"), &["rev-parse", "HEAD"]);

    // dry run touches nothing
    ttr().args(["slot", "migrate", "--dry-run", "--root", &root_s]).assert().success();
    assert!(root.join("demo").join(".git").is_dir());
    assert!(!root.join("demo.git").exists());

    ttr().args(["slot", "migrate", "--root", &root_s]).assert().success();

    // the hub is the primary's moved .git, bare despite worktreeConfig
    let hub = root.join("demo.git");
    assert_eq!(git_out(&hub, &["rev-parse", "--is-bare-repository"]), "true");
    let wt_config = std::fs::read_to_string(hub.join("config.worktree")).unwrap();
    assert!(wt_config.contains("bare = true"), "core.bare must live in config.worktree");

    // every branch tip landed: created, kept, or parked on divergence
    assert_eq!(git_out(&hub, &["rev-parse", "refs/heads/feat/x"]), feat_x);
    assert_eq!(git_out(&hub, &["rev-parse", "refs/heads/feat/y"]), feat_y_primary);
    assert_eq!(
        git_out(&hub, &["rev-parse", "refs/heads/migrate/demo-slot-2/feat/y"]),
        feat_y_slot2
    );

    // slot-0: branch checked out, dirty tree re-applied, .env.local carried
    let slot0 = root.join("demo-slot-0");
    assert_eq!(git_out(&slot0, &["branch", "--show-current"]), "feat/x");
    assert_eq!(std::fs::read_to_string(slot0.join("feat.txt")).unwrap(), "committed\nwip\n");
    assert_eq!(std::fs::read_to_string(slot0.join("wip.txt")).unwrap(), "notes\n");
    assert_eq!(std::fs::read_to_string(slot0.join(".env.local")).unwrap(), "TT_DEV_PORT=1441\n");
    assert!(slot0.join(".tt-slot").is_file());

    // slot-1: still detached at its old commit
    let slot1 = root.join("demo-slot-1");
    assert_eq!(git_out(&slot1, &["branch", "--show-current"]), "");
    assert_eq!(git_out(&slot1, &["rev-parse", "HEAD"]), slot1_head);

    // slot-2: diverged tip → detached at its old commit, old tip parked above
    let slot2 = root.join("demo-slot-2");
    assert_eq!(git_out(&slot2, &["branch", "--show-current"]), "");
    assert_eq!(git_out(&slot2, &["rev-parse", "HEAD"]), feat_y_slot2);

    // primary became the next free slot, idle-parked detached at main,
    // and its stash survived the .git move
    assert!(!root.join("demo").exists());
    let slot3 = root.join("demo-slot-3");
    assert_eq!(git_out(&slot3, &["branch", "--show-current"]), "");
    assert_eq!(git_out(&slot3, &["rev-parse", "HEAD"]), main_sha);
    assert!(git_out(&slot3, &["stash", "list"]).contains("wip"));

    // .env rendered from the sidecar for every slot: identity token resolved
    // and each slot claimed a distinct port from the pool
    let mut ports = Vec::new();
    for (dir, name) in [
        (&slot0, "demo-slot-0"),
        (&slot1, "demo-slot-1"),
        (&slot2, "demo-slot-2"),
        (&slot3, "demo-slot-3"),
    ] {
        let env = std::fs::read_to_string(dir.join(".env")).unwrap();
        assert!(env.contains(&format!("SLOT={name}")), "{name}/.env: {env}");
        let port: u16 = env
            .lines()
            .find_map(|l| l.strip_prefix("TT_DEV_PORT="))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{name}/.env has no TT_DEV_PORT: {env}"));
        assert!((4000..=4099).contains(&port), "{name} port {port} out of pool");
        ports.push(port);
    }
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(ports.len(), 4, "each slot must claim a distinct port");

    // backup keeps the dirty patch; re-running is a no-op; ls works over the result
    assert!(root.join("demo-migrate-backup").join("demo-slot-0.patch").is_file());
    ttr()
        .args(["slot", "migrate", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("nothing to migrate"));
    ttr().args(["slot", "ls", "--root", &root_s]).assert().success();
}

#[test]
fn migrate_refuses_an_operation_in_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp.path(), &["init", "seed"]);
    write(&seed, "README.md", "one\n");
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "c1"]);

    let root = tmp.path().join("demo-repos");
    std::fs::create_dir_all(&root).unwrap();
    let seed_s = seed.to_string_lossy().to_string();
    git(&root, &["clone", &seed_s, "demo-slot-0"]);
    let root_s = root.to_string_lossy().to_string();

    // a fake in-flight merge blocks the whole migration
    let merge_head = root.join("demo-slot-0").join(".git").join("MERGE_HEAD");
    let sha = git_out(&root.join("demo-slot-0"), &["rev-parse", "HEAD"]);
    std::fs::write(&merge_head, format!("{sha}\n")).unwrap();
    ttr()
        .args(["slot", "migrate", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("merge in progress"));
    assert!(!root.join("demo.git").exists(), "a blocked migration must not create the hub");

    // clearing the marker lets it through; with no primary, the lowest slot
    // donates its .git
    std::fs::remove_file(&merge_head).unwrap();
    ttr().args(["slot", "migrate", "--root", &root_s]).assert().success();
    assert!(root.join("demo.git").is_dir());
    assert!(root.join("demo-slot-0").join(".git").is_file(), "slot-0 is now a linked worktree");
}
