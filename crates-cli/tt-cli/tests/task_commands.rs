//! Black-box tests for `tt task` against a real checkout built in a tempdir
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

/// `tt` with every state path pointed inside `home` under `scope`: tt-config
/// resolves `~/.config` and the data dir through those, and a *forced* scope
/// nests the shared stores too — so nothing reaches the real machine files.
/// The one sandboxing shape in this file; tests that need to inspect the
/// sandbox pass their own `home`, the rest go through [`tt`].
fn tt_scoped(home: &Path, scope: &str) -> Tt {
    let mut cmd = Tt::cargo_bin("tt").expect("binary `tt` should build");
    cmd.env("HOME", home);
    cmd.env("XDG_DATA_HOME", home.join(".local").join("share"));
    cmd.env(tt_config::STATE_SCOPE_ENV, scope);
    cmd
}

/// One throwaway `$HOME` for the whole suite, alive for the process. `tt task
/// new` writes a #339 board row through `Store::open_default()`, so without
/// this the tests would persist rows into the developer's real data dir on
/// every `cargo test` run.
fn tt() -> Tt {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| tempfile::tempdir().expect("sandbox home"));
    tt_scoped(home.path(), "tt-cli-black-box-tests")
}

/// `tt task new` for `branch` in `root`, titled after the branch. The branch is
/// passed explicitly rather than left to the title slug so the task folder is
/// the branch slug the assertions look for.
fn new_task(root: &str, branch: &str) -> Tt {
    let mut cmd = tt();
    cmd.args(["task", "new", branch, "--repo", root, "-b", branch]);
    cmd
}

/// The task dir for `name` under the checkout: `.claude/worktrees/<name>`.
fn task_dir(checkout: &Path, name: &str) -> PathBuf {
    checkout.join(".claude").join("worktrees").join(name)
}

/// Build `<tmp>/demo` (a normal clone on main) whose committed
/// `.env.example` carries `${tt:...}` tokens and a declared `TT_TASK_SETUP`
/// that drops a marker so tests can prove setup ran in-task.
fn make_checkout(tmp: &Path) -> PathBuf {
    let seed = tmp.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(tmp, &["init", "seed"]);
    std::fs::write(
        seed.join(".env.example"),
        "# demo task env\nUI_PORT=${tt:port 42410-42429}\nNAME=${tt:task-name}\nBASE=${tt:base}\nURL=http://localhost:${tt:var UI_PORT}/\nSECRET=\nTT_TASK_SETUP=touch .setup-ran\n",
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
    let out = new_task(&root_s, "feat/thing").arg("--json").output().unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "task new --json emitted bad JSON: {e}\nstdout: {:?}\nstderr: {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(created["name"], "feat-thing");
    assert_eq!(created["branch"], "feat/thing");
    assert_eq!(created["base"], "main");
    let task = task_dir(&checkout, "feat-thing");
    assert_eq!(created["dir"], task.to_string_lossy().as_ref());
    let env = std::fs::read_to_string(task.join(".env")).unwrap();
    assert!(env.contains("NAME=feat-thing"), "env: {env}");
    assert!(env.contains("BASE=main"));
    let ui_port = created["ports"]["UI_PORT"].as_u64().expect("UI_PORT claimed");
    assert!((42410..=42429).contains(&ui_port));
    assert!(env.contains(&format!("URL=http://localhost:{ui_port}/")));
    assert!(task.join(".tt-task").is_file());
    assert!(
        task.join(".setup-ran").is_file(),
        "the declared TT_TASK_SETUP command runs in the new task"
    );
    let branch = Command::new("git")
        .args(["-C", task.to_str().unwrap(), "branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feat/thing");
    // marker must not dirty the task's tree, and the nested worktrees dir
    // must not dirty the main checkout's (info/exclude covers both)
    for dir in [&task, &checkout] {
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

    // secrets inheritance: fill this task's SECRET, then create another
    let filled = env.replace("SECRET=", "SECRET=hunter2");
    std::fs::write(task.join(".env"), filled).unwrap();
    new_task(&root_s, "fix/other").assert().success();
    let env2 = std::fs::read_to_string(task_dir(&checkout, "fix-other").join(".env")).unwrap();
    assert!(env2.contains("SECRET=hunter2"), "new task inherits sibling secrets: {env2}");
    assert!(!env2.contains(&format!("UI_PORT={ui_port}")), "new task claims a different port");

    // a second task for the same branch name is refused
    new_task(&root_s, "feat/thing").assert().failure().stderr(contains("already exists"));

    // env re-render is idempotent: same port, secrets kept
    tt().args(["task", "env", "feat-thing", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(task.join(".env")).unwrap();
    assert!(env_again.contains(&format!("UI_PORT={ui_port}")), "re-render keeps the claim");
    assert!(env_again.contains("SECRET=hunter2"), "re-render keeps merged secrets");

    // the main checkout renders its own .env too (it is a checkout like any
    // other) — `primary` still names it
    tt().args(["task", "env", "primary", "--root", &root_s]).assert().success();
    let env_primary = std::fs::read_to_string(checkout.join(".env")).unwrap();
    assert!(env_primary.contains("NAME=demo"), "env: {env_primary}");
    assert!(!checkout.join(".tt-task").exists(), "the main checkout gets no marker");

    // running from inside a task anchors at the main checkout — no
    // worktrees-inside-worktrees
    let task_s = task.to_string_lossy().to_string();
    new_task(&task_s, "feat/from-inside").assert().success();
    assert!(task_dir(&checkout, "feat-from-inside").is_dir());
    assert!(!task_dir(&task, "feat-from-inside").exists());
    tt().args(["task", "rm", "feat-from-inside", "--root", &root_s]).assert().success();

    // ls --json: the main checkout first, then tasks by name
    let out = tt().args(["task", "ls", "--json", "--root", &root_s]).output().unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> =
        listed.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["primary", "feat-thing", "fix-other"]);
    assert_eq!(listed[0]["primary"], true);
    assert_eq!(listed[0]["branch"], "main");

    // rm a clean task succeeds and releases the dir; the branch survives
    tt().args(["task", "rm", "fix-other", "--root", &root_s]).assert().success();
    assert!(!task_dir(&checkout, "fix-other").exists());
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
        "removing a task never deletes its branch"
    );

    // the main checkout itself is not removable
    tt().args(["task", "rm", "primary", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("refusing to remove the primary"));
    tt().args(["task", "rm", "demo", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("refusing to remove the primary"));
}

/// The claim lock's own regression test: renders racing for *fresh* claims
/// must come away with disjoint ports. Without the lock, all three scan
/// siblings before any of them writes, and they pick the same port.
#[test]
fn concurrent_renders_claim_disjoint_ports() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
    // Own sandbox HOME (not the suite's shared one) so wiping the registry
    // below can't disturb a concurrently running test's claims.
    let home = tempfile::tempdir().unwrap();
    let scope = "race-test";

    let names = ["race-a", "race-b", "race-c"];
    for name in names {
        let out = tt_scoped(home.path(), scope)
            .args([
                "task",
                "new",
                name,
                "--repo",
                &root_s,
                "-b",
                &format!("feat/{name}"),
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    // Erase every trace of the claims — each `.env` and the registry — so
    // the renders below genuinely race for fresh picks from the pool.
    for name in names {
        std::fs::remove_file(task_dir(&checkout, &format!("feat-{name}")).join(".env")).unwrap();
    }
    let registry_dir = home
        .path()
        .join(".config")
        .join("towles-tool")
        .join("tasks")
        .join(scope)
        .join("task-ports");
    if registry_dir.is_dir() {
        std::fs::remove_dir_all(&registry_dir).unwrap();
    }

    let handles: Vec<_> = names
        .iter()
        .map(|name| {
            let root = root_s.clone();
            let home = home.path().to_path_buf();
            let task = format!("feat-{name}");
            std::thread::spawn(move || {
                let out = tt_scoped(&home, scope)
                    .args(["task", "env", &task, "--root", &root])
                    .output()
                    .unwrap();
                (task, out)
            })
        })
        .collect();
    for handle in handles {
        let (task, out) = handle.join().unwrap();
        assert!(
            out.status.success(),
            "env render for {task} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let ports: Vec<String> = names
        .iter()
        .map(|name| {
            let env =
                std::fs::read_to_string(task_dir(&checkout, &format!("feat-{name}")).join(".env"))
                    .unwrap();
            env.lines()
                .find_map(|l| l.strip_prefix("UI_PORT=").map(str::to_string))
                .unwrap_or_else(|| panic!("no UI_PORT rendered for {name}: {env}"))
        })
        .collect();
    let distinct: std::collections::BTreeSet<&String> = ports.iter().collect();
    assert_eq!(distinct.len(), names.len(), "racing renders claimed colliding ports: {ports:?}");
}

/// `tt task ports --json` reports every claim with its owner/var/source, and
/// flags a claim only the registry still knows (its `.env` deleted) as
/// `source: "registry"` — the drift row a doctor check keys off.
#[test]
fn ports_reports_claims_and_flags_env_registry_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    let out = new_task(&root_s, "feat/ports").arg("--json").output().unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "task new --json emitted bad JSON: {e}\nstdout: {:?}\nstderr: {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let port = created["ports"]["UI_PORT"].as_u64().expect("UI_PORT claimed");

    let out = tt().args(["task", "ports", "--json", "--root", &root_s]).output().unwrap();
    assert!(out.status.success(), "ports failed: {}", String::from_utf8_lossy(&out.stderr));
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["port"].as_u64() == Some(port))
        .expect("claimed port reported");
    assert_eq!(row["owner"], "feat-ports");
    assert_eq!(row["var"], "UI_PORT");
    assert_eq!(row["source"], "env+registry");
    assert!(row["claimed_at_ms"].as_i64().unwrap() > 0, "registry stamped the claim time");

    // Delete the task's .env: the claim survives as a registry-only row.
    std::fs::remove_file(task_dir(&checkout, "feat-ports").join(".env")).unwrap();
    let out = tt().args(["task", "ports", "--json", "--root", &root_s]).output().unwrap();
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["port"].as_u64() == Some(port))
        .expect("registry keeps the claim visible");
    assert_eq!(row["source"], "registry");

    // --probe on a port we hold open must read occupied.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let held = listener.local_addr().unwrap().port();
    let out =
        tt().args(["task", "ports", "--probe", &held.to_string(), "--json"]).output().unwrap();
    let probe: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(probe["occupied"], true);
}

/// The port registry's reason to exist: a sibling task whose `.env` is gone
/// (deleted by hand, corrupted) must keep its claimed ports off the table —
/// the live sibling-`.env` scan alone would hand them straight to the next
/// task.
#[test]
fn new_task_avoids_ports_registered_to_a_sibling_whose_env_file_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    let out = new_task(&root_s, "feat/one").arg("--json").output().unwrap();
    assert!(out.status.success(), "new one failed: {}", String::from_utf8_lossy(&out.stderr));
    let one: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let port_one = one["ports"]["UI_PORT"].as_u64().expect("UI_PORT claimed");

    std::fs::remove_file(task_dir(&checkout, "feat-one").join(".env")).unwrap();

    let out = new_task(&root_s, "feat/two").arg("--json").output().unwrap();
    assert!(out.status.success(), "new two failed: {}", String::from_utf8_lossy(&out.stderr));
    let two: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let port_two = two["ports"]["UI_PORT"].as_u64().expect("UI_PORT claimed");

    assert_ne!(
        port_one, port_two,
        "task two must not claim task one's registered port just because its .env is gone"
    );
}

/// `--repo` may point anywhere inside the checkout — including one of its own
/// worktrees. The worktree always anchors at the main checkout, and the board
/// row's `repo` must anchor there too: recorded as the nested path it would key
/// the Board card to a swimlane matching no repo on the rail.
#[test]
fn new_records_the_main_checkout_as_the_repo_even_when_repo_points_inside_a_task() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    new_task(&root_s, "feat/first").assert().success();
    let inside = task_dir(&checkout, "feat-first").to_string_lossy().to_string();

    let out = new_task(&inside, "feat/second").arg("--json").output().unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "task new --json emitted bad JSON: {e}\nstdout: {:?}\nstderr: {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(created["repo"], root_s, "the board row binds to the main checkout");
    assert_eq!(created["dir"], task_dir(&checkout, "feat-second").to_string_lossy().as_ref());
}

/// A task created with `--base <non-default>` records *that* ref, not the
/// main checkout's currently-checked-out branch, in both the rendered
/// `.env`'s `${tt:base}` token and the `.tt-task` marker — and re-rendering
/// later (`tt task env`) must not let that drift even if the checkout's
/// branch has since changed.
#[test]
fn new_with_base_records_the_actual_base_not_the_primary_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    git(&checkout, &["checkout", "-b", "develop"]);
    git(&checkout, &["checkout", "main"]);

    let out = new_task(&root_s, "feat/off-develop")
        .args(["--base", "develop", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "new failed: {}", String::from_utf8_lossy(&out.stderr));
    let created: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "task new --json emitted bad JSON: {e}\nstdout: {:?}\nstderr: {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(created["base"], "develop");

    let task = task_dir(&checkout, "feat-off-develop");
    let env = std::fs::read_to_string(task.join(".env")).unwrap();
    assert!(env.contains("BASE=develop"), "env: {env}");
    let marker = std::fs::read_to_string(task.join(".tt-task")).unwrap();
    assert!(marker.contains("base=develop"), "marker: {marker}");

    // The main checkout switches branches after the task was created (its
    // "current branch" is no longer `develop`, or even `main`) —
    // re-rendering the task's env must still report the base it was
    // actually created from.
    git(&checkout, &["checkout", "-b", "unrelated"]);
    tt().args(["task", "env", "feat-off-develop", "--root", &root_s]).assert().success();
    let env_again = std::fs::read_to_string(task.join(".env")).unwrap();
    assert!(env_again.contains("BASE=develop"), "re-render: {env_again}");
    let marker_again = std::fs::read_to_string(task.join(".tt-task")).unwrap();
    assert!(marker_again.contains("base=develop"), "re-render marker: {marker_again}");
}

/// If the checkout's base branch has fallen behind `origin/<base>` (the user
/// hasn't pulled `main` in a while), `new` fast-forwards it before branching
/// — so the new task starts from current history instead of needing a
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

    new_task(&root_s, "feat/thing").assert().success();

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

    // ...so the new task branches from that current history, not the stale
    // commit the checkout had when the task command started
    assert!(task_dir(&checkout, "feat-thing").join("upstream.txt").is_file());
}

/// When the checkout's base branch has diverged from `origin/<base>` (both
/// moved independently), a plain fast-forward is impossible — `new` must
/// warn rather than fail, and still create the task off the local history as
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

    new_task(&root_s, "feat/thing")
        .assert()
        .success()
        .stdout(contains("diverged from origin/main"))
        .stdout(contains("could not be fast-forwarded"));

    // creation still succeeds, branching off the checkout's own (unmoved)
    // local main rather than blocking on the divergence
    let task = task_dir(&checkout, "feat-thing");
    assert!(task.join("local.txt").is_file());
    assert!(!task.join("upstream.txt").is_file());
}

/// A repo with neither a tokenized `.env.example` nor the
/// `.claude/task-env.template` sidecar — any plain checkout never onboarded
/// onto tasks — must still get a task: the render falls back to an empty
/// template (empty `.env`, no port claims) instead of failing with the old
/// "no template" error (hit for real creating toolbox tasks from the app).
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

    new_task(&root_s, "feat/thing").assert().success();

    let task = task_dir(&checkout, "feat-thing");
    assert!(task.join(".tt-task").is_file(), "the task marker must still be written");
    let env = std::fs::read_to_string(task.join(".env")).unwrap();
    assert!(env.trim().is_empty(), "nothing to template → an empty .env, got: {env}");

    // re-rendering the templateless task stays a no-op, not an error
    tt().args(["task", "env", "feat-thing", "--root", &root_s]).assert().success();
}

/// A `new` that fails after `git worktree add` (e.g. a template render
/// error) must roll the worktree back — leaving one behind blocks every
/// retry with a bogus "already exists" and hides the failed attempt from
/// `task ls`.
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
    new_task(&root_s, "feat/thing")
        .assert()
        .failure()
        .stderr(contains("env template"))
        .stderr(contains(".env.example"))
        .stderr(contains("unknown or malformed token"));

    assert!(!task_dir(&checkout, "feat-thing").exists(), "the worktree must not be left behind");
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
    std::fs::write(checkout.join(".env.example"), "NAME=${tt:task-name}\n").unwrap();
    git(&checkout, &["add", ".env.example"]);
    git(&checkout, &["commit", "-m", "fix template"]);
    new_task(&root_s, "feat/thing").assert().success();
}

#[test]
fn rm_guards_dirty_and_orphan_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    new_task(&root_s, "feat/work").assert().success();
    let task = task_dir(&checkout, "feat-work");

    // dirty tree refuses — and says what to do about it, not just what's
    // wrong: a refusal with no next step is the dead end this output exists
    // to avoid (the app's blocked-delete dialog renders the same two halves).
    std::fs::write(task.join("junk.txt"), "wip").unwrap();
    tt().args(["task", "rm", "feat-work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("not clean"))
        .stderr(contains("Commit or stash"))
        .stderr(contains("--force"))
        .stderr(contains("discards the work above"));
    std::fs::remove_file(task.join("junk.txt")).unwrap();

    // a commit on a detached HEAD would be orphaned by removal → refused
    // (tasks are created on branches, but a checkout can end up detached)
    git(&task, &["checkout", "--detach"]);
    std::fs::write(task.join("work.txt"), "real work").unwrap();
    git(&task, &["add", "work.txt"]);
    git(&task, &["commit", "-m", "detached work"]);
    tt().args(["task", "rm", "feat-work", "--root", &root_s])
        .assert()
        .failure()
        .stderr(contains("orphan"));

    // parking the commit on a branch makes removal safe (branches live in the
    // main checkout's .git)
    git(&task, &["branch", "parked/detached-work"]);
    tt().args(["task", "rm", "feat-work", "--root", &root_s]).assert().success();
    assert!(!task.exists());

    // --force path: recreate, dirty it, force through
    new_task(&root_s, "feat/redo").assert().success();
    std::fs::write(task_dir(&checkout, "feat-redo").join("junk.txt"), "wip").unwrap();
    tt().args(["task", "rm", "feat-redo", "--force", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("skipping guard"));
    assert!(!task_dir(&checkout, "feat-redo").exists());
}

/// Committed-but-unlanded work does not block removal — the branch keeps it —
/// but the two must never be confused with each other, so removal says which
/// one it is instead of reporting a bare success.
#[test]
fn rm_reports_unlanded_commits_it_does_not_block_on() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    new_task(&root_s, "feat/unlanded").assert().success();
    let task = task_dir(&checkout, "feat-unlanded");
    std::fs::write(task.join("kept.txt"), "committed work").unwrap();
    git(&task, &["add", "kept.txt"]);
    git(&task, &["commit", "-m", "work that never landed"]);

    tt().args(["task", "rm", "feat-unlanded", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("have not reached main").and(contains("stay on the branch")));

    assert!(!task.exists());
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

/// A task whose branch really did land reports that instead, so "removed" and
/// "removed, but you still owe a push" never look the same.
#[test]
fn rm_reports_a_merged_branch_as_having_nothing_outstanding() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    new_task(&root_s, "feat/landed").assert().success();
    let task = task_dir(&checkout, "feat-landed");
    // Two commits: squashing a single commit yields a patch-identical one,
    // which is genuinely indistinguishable from a rebase (and reports as
    // such). Collapsing several is what only the tree probe can recognise.
    std::fs::write(task.join("a.txt"), "a").unwrap();
    git(&task, &["add", "a.txt"]);
    git(&task, &["commit", "-m", "a"]);
    std::fs::write(task.join("b.txt"), "b").unwrap();
    git(&task, &["add", "b.txt"]);
    git(&task, &["commit", "-m", "b"]);
    // Squash it onto main the way a merged PR would, under a fresh SHA.
    git(&checkout, &["merge", "--squash", "feat/landed"]);
    git(&checkout, &["commit", "-m", "squashed feat/landed (#1)"]);

    tt().args(["task", "rm", "feat-landed", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("squash-merged into main").and(contains("nothing outstanding")));
}

#[test]
fn rm_untracks_the_task_and_removes_its_instance_state() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
    // This test asserts on the sandbox's contents, so it needs its own
    // inspectable `$HOME` rather than the suite's throwaway one.
    let home = tmp.path().join("home");

    new_task(&root_s, "feat/tracked").assert().success();
    let task = task_dir(&checkout, "feat-tracked");

    // Give the task checkout this repo's scope marker (committed, so the tree
    // stays clean for the removal guards): its state scope becomes
    // `demo-feat-tracked` (main checkout dir name + task name).
    std::fs::create_dir_all(task.join("crates").join("tt-config")).unwrap();
    std::fs::write(task.join("crates").join("tt-config").join(".gitkeep"), "").unwrap();
    git(&task, &["add", "."]);
    git(&task, &["commit", "-m", "scope marker"]);

    // The app tracks tasks it creates: simulate that in the sandboxed
    // repos.json, alongside a repo that must survive the removal.
    let shared = home.join(".config").join("towles-tool").join("tasks").join("rm-test");
    let repos_json = shared.join("agentboard").join("repos.json");
    std::fs::create_dir_all(repos_json.parent().unwrap()).unwrap();
    let task_s = task.to_string_lossy().to_string();
    std::fs::write(
        &repos_json,
        serde_json::to_string_pretty(&serde_json::json!({
            "repoPaths": [task_s, "/kept/elsewhere"],
        }))
        .unwrap(),
    )
    .unwrap();

    // Leftover instance state the removed task's app instance wrote.
    let state_dir = shared.join("tasks").join("demo-feat-tracked");
    std::fs::create_dir_all(state_dir.join("agentboard")).unwrap();
    std::fs::write(state_dir.join("agentboard").join("sessions.json"), "{}\n").unwrap();

    tt_scoped(&home, "rm-test")
        .args(["task", "rm", "feat-tracked", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("untracked from the agentboard rail"))
        .stdout(contains("removed task state"));

    assert!(!task.exists());
    assert!(!state_dir.exists(), "the task's orphaned instance state is swept");
    let repos: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&repos_json).unwrap()).unwrap();
    assert_eq!(
        repos["repoPaths"],
        serde_json::json!(["/kept/elsewhere"]),
        "the removed task is untracked; other repos survive"
    );
}

/// Board rows are per-checkout instance state (root CLAUDE.md's `tt-config`
/// bullet): a row lives wherever the app instance that created it was
/// scoped, which need not be the primary checkout `tt task rm` anchors to.
/// Concretely: an app instance running *from inside another task's own
/// worktree* (this repo's own dev loop does exactly this) scopes itself to
/// that worktree, not the primary — so a task it creates, and a `tt task rm`
/// run from that same shell later, both see that worktree's own scope via
/// the ambient cwd, never the primary `--root`/discovery resolves to. `dir`
/// (the *removed* task's own directory) is deliberately NOT where the
/// fixture puts the row — that scope is exactly what `ops::remove_task`'s
/// `state_cleanup` step wipes wholesale by task name regardless of this fix,
/// which would make the test pass even against the bug (confirmed by hand:
/// an earlier draft of this test put the row there and stayed green with the
/// fix reverted). Reproduced directly against the real store (no forced
/// `TT_STATE_SCOPE`, so scope resolution is the real `task_scope_from_dir`
/// auto-detection) with both the creating and removing `tt` invocations run
/// with `current_dir` set to the "container" task, rather than through a
/// second real `tt-app` instance.
#[test]
fn rm_closes_a_board_row_scoped_to_the_ambient_cwds_own_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
    let home = tmp.path().join("home");
    let data_home = home.join(".local").join("share");

    // The primary checkout gets its own scope marker too — mirrors this
    // repo's own self-hosting quirk (`task_scope_from_dir` scopes the main
    // checkout by its own dir name, not only its worktrees).
    std::fs::create_dir_all(checkout.join("crates").join("tt-config")).unwrap();
    std::fs::write(checkout.join("crates").join("tt-config").join(".gitkeep"), "").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-m", "primary scope marker"]);

    // No `TT_STATE_SCOPE` — real scope auto-detection, the condition this bug
    // needs to reproduce (a forced scope, as every other test in this file
    // uses for sandboxing, would make every store resolve to the same path
    // and hide the bug).
    let cmd = || {
        let mut c = Tt::cargo_bin("tt").expect("binary `tt` should build");
        c.env("HOME", &home);
        c.env("XDG_DATA_HOME", &data_home);
        c
    };

    // The "container" task: stands in for the worktree a real dev-loop app
    // instance is running from. Created (and left alone) through the normal
    // ambient-cwd path — its own worktree inherits the primary's `tt-config`
    // marker, so it gets its own real scope, `demo-feat-container`.
    cmd()
        .args([
            "task",
            "new",
            "feat/container",
            "--repo",
            &root_s,
            "-b",
            "feat/container",
        ])
        .assert()
        .success();
    let container = task_dir(&checkout, "feat-container");
    assert!(container.join("crates").join("tt-config").is_dir(), "inherited from the primary");

    // The victim task, created *as if* by an app instance running from
    // inside `container` — hence `current_dir(&container)` on this call, not
    // on the primary. `cmd_new`'s own board write lands wherever `tt`'s
    // ambient cwd scopes to, i.e. `demo-feat-container`, matching the scenario.
    let mut new_cmd = cmd();
    new_cmd.current_dir(&container).args([
        "task",
        "new",
        "feat/nested",
        "--repo",
        &root_s,
        "-b",
        "feat/nested",
    ]);
    new_cmd.assert().success();
    let victim = task_dir(&checkout, "feat-nested");
    let victim_s = victim.to_string_lossy().to_string();

    let container_db =
        data_home.join("towles-tool").join("tasks").join("demo-feat-container").join("tt.db");
    let store = tt_store::Store::open(&container_db).unwrap();
    let row = store
        .task_for_worktree_dir(&victim_s)
        .unwrap()
        .expect("the fixture's own board write landed in demo-feat-container's store");
    assert_eq!(row.status, "backlog", "sanity: the row starts open (tt task new's default status)");

    // Remove the victim task the same way it'd really happen: `tt task rm`
    // invoked from a shell sitting in `container`, `--root` pointing at the
    // primary (as every real invocation's discovery does regardless of cwd).
    let mut rm_cmd = cmd();
    rm_cmd.current_dir(&container).args([
        "task",
        "rm",
        "feat-nested",
        "--root",
        &root_s,
        "--force",
    ]);
    rm_cmd.assert().success();

    let row = store.task_for_worktree_dir(&victim_s).unwrap();
    assert!(
        row.is_none(),
        "the row in demo-feat-container's own store must be closed (unbound from the removed \
         worktree), not left dangling as a stale \"doing\" card forever just because it lives \
         in a different scope than the primary `tt task rm` resolves to"
    );
}

/// The stale-rail bug: a checkout reached through a symlinked path gets
/// tracked (and its board row bound) under that literal string, but `git
/// worktree add` persists the realpath internally — so a removal driven by
/// the *realpath* form (what `git worktree list`, and so the rail, would
/// report) must still find and untrack the literal entry, not silently
/// leave it as a "directory missing" ghost.
#[test]
#[cfg(unix)]
fn rm_untracks_a_symlink_aliased_worktree_and_closes_its_row() {
    let tmp = tempfile::tempdir().unwrap();
    let real_checkout = make_checkout(tmp.path());
    let link_checkout = tmp.path().join("link");
    std::os::unix::fs::symlink(&real_checkout, &link_checkout).unwrap();
    let link_s = link_checkout.to_string_lossy().to_string();
    let home = tmp.path().join("home");

    // Create the task through the symlinked path — this is the literal
    // string that gets tracked (and bound to a board row) in the real app.
    new_task(&link_s, "feat/aliased").assert().success();
    let literal_dir = link_checkout.join(".claude").join("worktrees").join("feat-aliased");
    let literal_dir_s = literal_dir.to_string_lossy().to_string();

    let shared = home.join(".config").join("towles-tool").join("tasks").join("alias-test");
    let repos_json = shared.join("agentboard").join("repos.json");
    std::fs::create_dir_all(repos_json.parent().unwrap()).unwrap();
    std::fs::write(
        &repos_json,
        serde_json::to_string_pretty(&serde_json::json!({
            "repoPaths": [literal_dir_s, "/kept/elsewhere"],
        }))
        .unwrap(),
    )
    .unwrap();

    // Remove it by the *realpath* checkout — what `git worktree list` (and
    // so the rail, and the app's delete-worktree button) would report,
    // never byte-identical to the symlinked literal string above.
    let real_root_s = real_checkout.to_string_lossy().to_string();
    tt_scoped(&home, "alias-test")
        .args(["task", "rm", "feat-aliased", "--root", &real_root_s])
        .assert()
        .success()
        .stdout(contains("untracked from the agentboard rail"));

    assert!(!literal_dir.exists());
    let repos: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&repos_json).unwrap()).unwrap();
    assert_eq!(
        repos["repoPaths"],
        serde_json::json!(["/kept/elsewhere"]),
        "the symlink-aliased entry is untracked by realpath; the unrelated repo survives"
    );
}

#[test]
fn lockfile_detection_installs_without_declared_setup() {
    // A repo with no TT_TASK_SETUP but a package-lock.json: setup_command
    // picks `npm install`. Proving the pure decision is enough here — the
    // unit tests own the matrix; this asserts a template without the key
    // still creates a task cleanly (setup skipped when no lockfile either).
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

    new_task(&root_s, "feat/plain").assert().success();
    assert!(task_dir(&checkout, "feat-plain").join(".env").is_file());
}

/// The Claude Code WorktreeCreate hook shell: stdin is the hook JSON, stdout
/// is exactly the task path, the requested name IS the branch verbatim, and
/// a re-request for the same name returns the same path instead of failing.
#[test]
fn hook_create_creates_a_task_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let expected = task_dir(&checkout, "auth-flow");

    let hook_input = serde_json::json!({
        "session_id": "abc",
        "hook_event_name": "WorktreeCreate",
        "cwd": checkout.to_string_lossy(),
        "name": "auth-flow",
    })
    .to_string();

    let out = tt().args(["task", "hook-create"]).write_stdin(hook_input.clone()).output().unwrap();
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
    assert!(expected.join(".env").is_file(), "hook-created tasks render .env like tt task new");

    // Same name again → same path, exit 0 (Claude Code re-enters worktrees).
    let again = tt().args(["task", "hook-create"]).write_stdin(hook_input).output().unwrap();
    assert!(again.status.success());
    assert_eq!(String::from_utf8_lossy(&again.stdout).trim(), expected.to_string_lossy());
}

/// Distinct branches can slug to the same task folder (`feat/thing` and a
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
        tt().args(["task", "hook-create"]).write_stdin(hook_input("feat/thing")).output().unwrap();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));

    let collided =
        tt().args(["task", "hook-create"]).write_stdin(hook_input("feat-thing")).output().unwrap();
    assert!(!collided.status.success(), "must not silently resume a different branch's task");
    let stderr = String::from_utf8_lossy(&collided.stderr);
    assert!(stderr.contains("feat/thing"), "stderr: {stderr}");
    assert!(stderr.contains("feat-thing"), "stderr: {stderr}");

    // The original task is untouched — still on its own branch.
    let branch = Command::new("git")
        .args([
            "-C",
            task_dir(&checkout, "feat-thing").to_str().unwrap(),
            "branch",
            "--show-current",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feat/thing");
}

/// The WorktreeRemove hook shell runs the same guards as `tt task rm`: a
/// clean task goes away, a dirty one is refused (non-zero, message on
/// stderr) and stays on disk.
#[test]
fn hook_remove_is_guarded_like_rm() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();

    new_task(&root_s, "feat/done").assert().success();
    let task = task_dir(&checkout, "feat-done");
    let hook_input = serde_json::json!({
        "hook_event_name": "WorktreeRemove",
        "cwd": checkout.to_string_lossy(),
        "worktree_path": task.to_string_lossy(),
    })
    .to_string();

    // dirty → refused, task stays
    std::fs::write(task.join("wip.txt"), "unsaved").unwrap();
    tt().args(["task", "hook-remove"])
        .write_stdin(hook_input.clone())
        .assert()
        .failure()
        .stderr(contains("not clean"));
    assert!(task.exists());

    // clean → removed
    std::fs::remove_file(task.join("wip.txt")).unwrap();
    tt().args(["task", "hook-remove"]).write_stdin(hook_input.clone()).assert().success();
    assert!(!task.exists());

    // already gone → a no-op success, not an error (Claude Code may fire the
    // hook for a worktree the user already cleaned up)
    tt().args(["task", "hook-remove"]).write_stdin(hook_input).assert().success();
}

/// Claude Code sometimes removes the worktree from disk itself before firing
/// WorktreeRemove — the hook must still untrack it from the agentboard rail
/// rather than no-op, or the rail strands a "directory missing" ghost that
/// only a manual Untrack can clear (the bug this test guards against).
#[test]
fn hook_remove_untracks_a_worktree_already_gone_from_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = make_checkout(tmp.path());
    let root_s = checkout.to_string_lossy().to_string();
    let home = tmp.path().join("home");

    new_task(&root_s, "feat/ghost").assert().success();
    let task = task_dir(&checkout, "feat-ghost");
    let task_s = task.to_string_lossy().to_string();

    let shared = home.join(".config").join("towles-tool").join("tasks").join("ghost-test");
    let repos_json = shared.join("agentboard").join("repos.json");
    std::fs::create_dir_all(repos_json.parent().unwrap()).unwrap();
    std::fs::write(
        &repos_json,
        serde_json::to_string_pretty(&serde_json::json!({
            "repoPaths": [task_s, "/kept/elsewhere"],
        }))
        .unwrap(),
    )
    .unwrap();

    // The worktree is gone from disk already — e.g. Claude Code's own
    // teardown ran before the hook fired.
    std::fs::remove_dir_all(&task).unwrap();
    assert!(!task.exists());

    let hook_input = serde_json::json!({
        "hook_event_name": "WorktreeRemove",
        "cwd": checkout.to_string_lossy(),
        "worktree_path": task_s,
    })
    .to_string();
    tt_scoped(&home, "ghost-test")
        .args(["task", "hook-remove"])
        .write_stdin(hook_input)
        .assert()
        .success();

    let repos: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&repos_json).unwrap()).unwrap();
    assert_eq!(
        repos["repoPaths"],
        serde_json::json!(["/kept/elsewhere"]),
        "the gone worktree's entry is dropped, the other repo survives"
    );
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

    tt().args(["task", "init", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("task-env.template"))
        .stdout(contains("gitignore: added .env"))
        .stdout(contains("hooks: wired"));

    assert!(checkout.join(".claude").join("task-env.template").is_file());
    assert!(checkout.join(".env").is_file());
    let gitignore = std::fs::read_to_string(checkout.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".env"));
    let settings = std::fs::read_to_string(checkout.join(".claude").join("settings.json")).unwrap();
    assert!(settings.contains("tt task hook-create"));
    assert!(settings.contains("tt task hook-remove"));

    // Re-run: nothing to do, nothing clobbered.
    tt().args(["task", "init", "--root", &root_s])
        .assert()
        .success()
        .stdout(contains("hooks: already wired"));
    let settings_again =
        std::fs::read_to_string(checkout.join(".claude").join("settings.json")).unwrap();
    assert_eq!(settings, settings_again);
}
