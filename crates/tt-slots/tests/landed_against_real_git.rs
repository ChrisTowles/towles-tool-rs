//! `landed::probe_work_state` against a real git repository.
//!
//! The unit tests in `landed.rs` cover the decision given each signal; these
//! cover the part that can only break against git itself — that the probes
//! actually produce those signals. The squash cases are the reason the module
//! exists, so they are asserted end-to-end rather than trusted.

use std::path::Path;
use std::process::Command;

use tt_slots::landed::{LandedVia, probe_work_state};

/// Run git, mirroring the contract `probe_work_state` documents: `Some(stdout)`
/// only on a zero exit, `None` otherwise.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

fn commit(dir: &Path, file: &str, body: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    run(dir, &["add", "-A"]);
    run(dir, &["commit", "-qm", &format!("add {file}")]);
}

/// A repo on `main` with one base commit.
fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    run(dir, &["init", "-q", "-b", "main"]);
    run(dir, &["config", "user.email", "test@example.com"]);
    run(dir, &["config", "user.name", "Test"]);
    run(dir, &["config", "commit.gpgsign", "false"]);
    commit(dir, "base.txt", "base");
    tmp
}

#[test]
fn squash_merged_branch_reads_as_landed_with_nothing_outstanding() {
    let tmp = repo();
    let dir = tmp.path();

    run(dir, &["checkout", "-qb", "feat/squash"]);
    commit(dir, "a.txt", "a");
    commit(dir, "b.txt", "b");
    run(dir, &["checkout", "-q", "main"]);
    run(dir, &["merge", "--squash", "feat/squash"]);
    run(dir, &["commit", "-qm", "squashed (#1)"]);
    // Unrelated later work, so the base is not merely the branch's tree.
    commit(dir, "other.txt", "other");

    let state = probe_work_state(&git, dir, "main", "feat/squash", 0, 0, false);

    assert_eq!(state.landed, Some(LandedVia::Squash), "squash merge must be detected");
    assert_eq!(state.unlanded, 0, "nothing is outstanding after a squash merge");
    assert_eq!(state.total_commits, 2);
    assert!(!state.holds_work());
    assert_eq!(state.headline(), "squash-merged");
}

/// The probe must not depend on the machine having a git identity.
///
/// `commit-tree` refuses to run without one, and git's fallback to
/// `user@hostname` is unavailable on CI runners and in minimal containers —
/// where this failed with "Author identity unknown", making the probe answer
/// "not landed" for every squash-merged branch. `user.useConfigOnly` disables
/// exactly that fallback, so this reproduces the environment rather than
/// simulating it.
#[test]
fn squash_is_detected_even_when_git_has_no_ambient_identity() {
    let tmp = repo();
    let dir = tmp.path();

    run(dir, &["checkout", "-qb", "feat/no-identity"]);
    commit(dir, "a.txt", "a");
    commit(dir, "b.txt", "b");
    run(dir, &["checkout", "-q", "main"]);
    run(dir, &["merge", "--squash", "feat/no-identity"]);
    run(dir, &["commit", "-qm", "squashed (#1)"]);

    // Strip the identity this repo was set up with, and forbid the
    // `user@hostname` fallback.
    run(dir, &["config", "--unset", "user.name"]);
    run(dir, &["config", "--unset", "user.email"]);
    run(dir, &["config", "user.useConfigOnly", "true"]);

    // Repo config alone is not enough to reproduce a CI runner: the developer
    // running this almost certainly has a global `user.email`, which git would
    // find and the test would pass whether or not the probe supplies its own.
    // Pointing the global and system config at /dev/null (and clearing the
    // identity env vars) leaves git with nothing, which is the actual
    // environment this regressed in.
    let git_without_identity = |dir: &Path, args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL")
            .env_remove("EMAIL")
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };

    let state =
        probe_work_state(&git_without_identity, dir, "main", "feat/no-identity", 0, 0, false);

    assert_eq!(
        state.landed,
        Some(LandedVia::Squash),
        "the probe must supply its own identity to commit-tree"
    );
    assert_eq!(state.unlanded, 0);
}

#[test]
fn work_added_after_a_squash_merge_counts_only_the_new_commit() {
    // The case plain `git cherry` gets wrong: it reports all 3 commits as
    // unlanded because the squash rewrote the first two.
    let tmp = repo();
    let dir = tmp.path();

    run(dir, &["checkout", "-qb", "feat/more"]);
    commit(dir, "a.txt", "a");
    commit(dir, "b.txt", "b");
    run(dir, &["checkout", "-q", "main"]);
    run(dir, &["merge", "--squash", "feat/more"]);
    run(dir, &["commit", "-qm", "squashed (#1)"]);
    run(dir, &["checkout", "-q", "feat/more"]);
    commit(dir, "c.txt", "c");

    let state = probe_work_state(&git, dir, "main", "feat/more", 0, 0, false);

    assert_eq!(state.landed, None, "new work after the merge means it has not fully landed");
    assert_eq!(state.total_commits, 3);
    assert_eq!(state.unlanded, 1, "only the commit made after the squash is outstanding");
    assert!(state.holds_work());
    assert_eq!(state.headline(), "1 unlanded");
}

#[test]
fn rebase_merged_branch_reads_as_landed_under_a_different_sha() {
    let tmp = repo();
    let dir = tmp.path();

    run(dir, &["checkout", "-qb", "feat/rebase"]);
    commit(dir, "a.txt", "a");
    run(dir, &["checkout", "-q", "main"]);
    run(dir, &["cherry-pick", "feat/rebase"]);
    // A plain cherry-pick here replays a byte-identical commit (same tree,
    // parent, message and timestamp produce the same SHA), which would make
    // this a reachability test rather than a patch-identity one. Amending the
    // message forces a new SHA while keeping the patch, which is what a real
    // rebase-merge looks like: same change, different commit.
    run(dir, &["commit", "--amend", "-qm", "add a.txt (rebased onto main)"]);
    commit(dir, "later.txt", "later");

    let state = probe_work_state(&git, dir, "main", "feat/rebase", 0, 0, false);

    assert_eq!(
        state.landed,
        Some(LandedVia::Patches),
        "a rebase-merged branch has landed even though its SHA is nowhere in main"
    );
    assert_eq!(state.unlanded, 0);
    assert!(!state.holds_work());
}

#[test]
fn branch_absorbed_by_a_merge_commit_is_not_mistaken_for_a_fresh_slot() {
    // A merged branch has nothing since its merge-base, exactly like a fresh
    // slot; only the base having moved past it tells them apart.
    let tmp = repo();
    let dir = tmp.path();

    run(dir, &["checkout", "-qb", "feat/merged"]);
    commit(dir, "a.txt", "a");
    run(dir, &["checkout", "-q", "main"]);
    run(
        dir,
        &[
            "merge",
            "--no-ff",
            "-q",
            "feat/merged",
            "-m",
            "merge feat/merged",
        ],
    );
    commit(dir, "later.txt", "later");

    let state = probe_work_state(&git, dir, "main", "feat/merged", 0, 0, false);

    // Both a merged branch and a fresh slot have nothing since their
    // merge-base; only the landing evidence tells them apart, so assert on
    // that pair directly rather than a helper that hides the distinction.
    assert_eq!(state.total_commits, 0, "a merged branch has nothing since its merge-base");
    assert_eq!(state.landed, Some(LandedVia::Ancestor), "…and that is what distinguishes it");
    assert!(!state.holds_work());
}

#[test]
fn genuinely_unmerged_branch_reports_its_commits() {
    let tmp = repo();
    let dir = tmp.path();

    run(dir, &["checkout", "-qb", "feat/open"]);
    commit(dir, "a.txt", "a");
    commit(dir, "b.txt", "b");

    let state = probe_work_state(&git, dir, "main", "feat/open", 0, 0, false);

    assert_eq!(state.landed, None);
    assert_eq!(state.unlanded, 2);
    assert!(state.holds_work());
}

#[test]
fn fresh_slot_holds_nothing() {
    let tmp = repo();
    let dir = tmp.path();
    run(dir, &["checkout", "-qb", "feat/fresh"]);

    let state = probe_work_state(&git, dir, "main", "feat/fresh", 0, 0, false);

    assert_eq!(state.total_commits, 0, "a fresh slot has no commits of its own");
    assert_eq!(state.landed, None, "a fresh slot must never read as merged");
    assert_eq!(state.unlanded, 0);
    assert!(!state.holds_work(), "a fresh slot is not protected from removal");
}

#[test]
fn uncommitted_and_unlanded_are_reported_as_separate_axes() {
    let tmp = repo();
    let dir = tmp.path();
    run(dir, &["checkout", "-qb", "feat/both"]);
    commit(dir, "a.txt", "a");

    let state = probe_work_state(&git, dir, "main", "feat/both", 3, 0, false);

    assert_eq!(state.uncommitted, 3);
    assert_eq!(state.unlanded, 1);
    assert_eq!(state.headline(), "3 uncommitted, 1 unlanded");
}

#[test]
fn a_gone_upstream_alone_does_not_outrank_real_unlanded_content() {
    // A branch deleted on the remote *without* being merged still holds work;
    // the content checks must win over the circumstantial `[gone]` signal.
    let tmp = repo();
    let dir = tmp.path();
    run(dir, &["checkout", "-qb", "feat/gone"]);
    commit(dir, "a.txt", "a");

    let state = probe_work_state(&git, dir, "main", "feat/gone", 0, 0, true);

    assert_eq!(
        state.landed,
        Some(LandedVia::UpstreamGone),
        "gone upstream is still the documented fallback answer"
    );
    // But the commit is real, and nothing may pretend otherwise: a branch
    // deleted on the remote looks identical whether it merged or not, so the
    // work stays counted and `clean` must not force-delete the branch.
    assert_eq!(state.total_commits, 1);
    assert_eq!(state.unlanded, 1, "a gone upstream is not proof the commit landed");
    assert!(state.holds_work());
    assert_eq!(state.headline(), "1 unlanded, upstream gone");
}

#[test]
fn a_failing_git_degrades_to_assuming_work_is_present() {
    let tmp = tempfile::tempdir().unwrap(); // not a repo at all
    let state = probe_work_state(&git, tmp.path(), "main", "feat/x", 0, 0, false);
    assert_eq!(state.landed, None, "an unanswerable probe must never read as merged");
    // `landed: None` is the load-bearing half: `clean` only removes a slot —
    // and force-deletes its branch — when it sees content proof, so a repo git
    // could not answer for is kept rather than collected. Assert the counts it
    // reports alongside, so a future change that invents a landing from a
    // failed probe fails here instead of silently deleting a branch.
    assert_eq!(state.total_commits, 0);
    assert_eq!(state.unlanded, 0);
    assert_eq!(state.headline(), "no commits", "and it claims no merge in the UI either");
}
