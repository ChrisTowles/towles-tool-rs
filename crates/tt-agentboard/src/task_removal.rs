//! Removing a worktree task and everything bound to it — the sequence, in one
//! place.
//!
//! A task is three things (#339): a board row, a git worktree, and whatever the
//! host has attached to that directory (terminal panes, a rail entry). Deleting
//! only some of them leaves garbage that nothing in the UI can reach — a row
//! pointing at a directory that no longer exists, or a checkout on disk with no
//! card left to delete it from. So the *order* matters and is policy, not
//! detail:
//!
//! 1. guards run with everything still alive, so a refusal costs nothing;
//! 2. the host tears down what it owns (PTYs, in-memory rail state) only once
//!    removal is really happening;
//! 3. the worktree leaves disk;
//! 4. the dir is untracked from `repos.json` — the repo rule; skip it and the
//!    `prs`/`issues` collectors retry `gh`/`git` against a dead path forever;
//! 5. the board row is **closed last**, when it can no longer strand anything:
//!    it records the caller's [`tt_store::TaskOutcome`] (`done`/`abandoned`)
//!    and detaches from the now-gone dir, but the row itself survives as the
//!    record of the work — deleting it was the old behavior, replaced
//!    2026-07-22.
//!
//! ## Why this crate
//!
//! It wants three things at once: the guarded removal ([`tt_tasks::ops`]), the
//! tracked-repo list ([`crate::repos`]), and the board store ([`tt_store`]).
//! `tt-tasks` cannot host it — this crate already depends on `tt-tasks`, so the
//! edge would be a cycle — and it must stay out of `tt-app`, because the `tt`
//! CLI has no Tauri and would otherwise have to restate the sequence (it did,
//! and the two copies had already drifted in ordering and failure mode).
//! `repos.json` lives here, `tt-store` adds no cycle, so here is where all
//! three meet.
//!
//! Host-specific work enters through [`RemovalHooks`] rather than being
//! reimplemented per shell: the app kills PTYs and closes rail folders, the CLI
//! does neither, and neither one owns the order.

use std::path::Path;

use tt_store::{Store, TaskOutcome};
use tt_tasks::RmBlocked;
use tt_tasks::ops::{self, RemoveOpts, RemoveOutcome};

/// What the host does at the two points in the sequence only it can act on.
///
/// Both default to nothing, so a shell with no panes and no in-memory rail (the
/// CLI) implements neither.
pub trait RemovalHooks {
    /// The guards have passed and the removal is really happening — after the
    /// last point that leaves the task untouched, before the first destructive
    /// step. The app kills the folder's PTYs here, which is why a *refused*
    /// removal never costs a live Claude session.
    fn before_removal(&mut self) {}

    /// The worktree is gone from disk. The app drops the folder's session,
    /// window and pane records here — deliberately not earlier, or a blocked
    /// removal would leave the rail looking clean while the checkout stayed
    /// put, with nothing left to retry from.
    ///
    /// Returns its own progress notes, which land in the outcome *in the order
    /// this step ran* rather than being appended after the later ones.
    fn after_removal(&mut self, _dir: &Path) -> Vec<String> {
        Vec::new()
    }
}

/// What to do when the worktree directory is already gone.
///
/// The two callers mean genuinely different things by it, and collapsing them
/// makes one of them lie:
pub enum MissingDir {
    /// **Fail.** The caller named a task — `tt task rm feat-tpyo` — and a name
    /// that resolves to nothing is a mistake to report, not a no-op to
    /// celebrate. Keeps every guard in [`ops::remove_task`], including its
    /// refusal to remove the main checkout.
    Fail,
    /// **Tear the bindings down anyway.** The caller holds a *record* pointing
    /// at the directory (a board row with a `worktree_dir`), and that record is
    /// precisely what still needs cleaning up after someone removed the
    /// worktree outside the app. Only reachable with a dir the caller read out
    /// of its own store, never a user-typed name.
    TearDownBindings,
}

/// A hook set that does nothing — the CLI's.
pub struct NoHooks;
impl RemovalHooks for NoHooks {}

/// How the sequence reaches the board row bound to a worktree.
///
/// A trait rather than a plain `&Store` because the two hosts hold their store
/// differently: the CLI opens one for the duration of a command, while the
/// app's lives behind a mutex that also serves UI snapshots — and holding that
/// across a minute-long `git worktree remove` would freeze the board it exists
/// to publish. This way each host locks for exactly the row delete and no
/// longer.
pub trait BoardRows {
    /// Close the task bound to `dir`, recording `outcome`. Returns a note
    /// when a row was closed, `None` when the worktree had no task — a real
    /// answer, since a worktree can be discovered on disk without the board
    /// knowing about it.
    fn close_task_for_worktree(
        &self,
        dir: &str,
        outcome: TaskOutcome,
        now_ms: i64,
    ) -> Option<String>;
}

/// The straightforward implementation, for a host holding an open [`Store`].
impl BoardRows for Store {
    fn close_task_for_worktree(
        &self,
        dir: &str,
        outcome: TaskOutcome,
        now_ms: i64,
    ) -> Option<String> {
        let row = self.task_for_worktree_dir(dir).ok()??;
        match self.close_task(row.id, outcome, now_ms) {
            Ok(_) => Some(format!(
                "closed board task #{} as {} ({})",
                row.id,
                outcome.as_str(),
                row.text
            )),
            Err(error) => Some(format!("could not close board task #{}: {error}", row.id)),
        }
    }
}

/// One task to remove: the worktree at `dir`, plus the bindings keyed by it.
pub struct TaskRemoval<'a> {
    /// Which worktree, and whether to skip the guards.
    pub opts: &'a RemoveOpts,
    /// The worktree directory. Passed rather than re-derived because it is also
    /// the key every binding is stored under, and it must be readable after the
    /// directory itself is gone.
    pub dir: &'a Path,
    /// The tracked-repo list to untrack `dir` from.
    pub repos_path: &'a Path,
    /// The board rows bound to `dir`. `None` skips step 5 — for a caller that
    /// cannot resolve which store owns the row, which is a real state
    /// (instance stores are per-checkout) and not an error.
    pub rows: Option<&'a dyn BoardRows>,
    /// How the task ended — recorded on the board row at step 5. Callers with
    /// a user answer pass it through; headless callers infer it (merged PR /
    /// landed work ⇒ done, else abandoned).
    pub outcome: TaskOutcome,
    /// When "now" is, for the close stamp. Injected — the clock read happens
    /// at the call boundary, not here.
    pub now_ms: i64,
    /// What a directory that is already gone means to this caller — see
    /// [`MissingDir`].
    pub on_missing: MissingDir,
}

/// How a removal ended. Mirrors [`ops::RemoveOutcome`], with the bindings
/// teardown folded into the `Removed` arm's notes.
pub enum Outcome {
    Removed {
        name: String,
        messages: Vec<String>,
    },
    /// The guards refused. **Nothing was removed** — not the worktree, not the
    /// bindings, not the row — so the caller can surface the reasons and retry
    /// from exactly where the user was.
    Blocked {
        name: String,
        blocked: Vec<RmBlocked>,
        messages: Vec<String>,
    },
}

/// Steps 4 and 5 alone, for a caller whose worktree is already off disk.
///
/// Bindings outlive the directory they are keyed by, so this is a real
/// standalone operation, not a shortcut: a worktree removed outside the app
/// (`git worktree remove`, a wiped disk, a restored backup) leaves exactly
/// these behind, and `tt task clean` — which removes in bulk through
/// [`ops::clean_tasks`] and cannot route each task through
/// [`remove_task_and_bindings`] — needs the identical teardown.
pub fn remove_bindings(
    repos_path: &Path,
    rows: Option<&dyn BoardRows>,
    dir: &Path,
    outcome: TaskOutcome,
    now_ms: i64,
) -> Vec<String> {
    let dir_s = dir.to_string_lossy().to_string();
    let mut messages = Vec::new();

    if let Ok((_, true)) = crate::repos::remove_repo_persisted(repos_path, &dir_s) {
        messages.push("untracked from the agentboard rail".to_string());
    }

    // Last: the worktree is gone, so closing the row can no longer strand
    // anything on disk.
    if let Some(note) = rows.and_then(|rows| rows.close_task_for_worktree(&dir_s, outcome, now_ms))
    {
        messages.push(note);
    }
    messages
}

/// Run the whole sequence at the top of this module.
///
/// A directory that is already gone is handled per [`MissingDir`], and *only*
/// [`MissingDir::TearDownBindings`] skips step 3. That skip is deliberately
/// gated rather than universal: `ops::remove_task` is also what refuses to
/// remove the main checkout and what reports an unknown task name, so an
/// unconditional `is_dir()` pre-flight would turn `tt task rm <typo>` into a
/// cheerful "removed" and could let a bad name reach the removal path at all.
///
/// Skipping it — rather than calling `ops::remove_task` and catching
/// `NoSuchTask` — is what keeps a caller with a stale recorded dir from having
/// its root re-discovered from the *process's* cwd, which resolves a different
/// checkout entirely and can match a same-named worktree there.
pub fn remove_task_and_bindings(
    task: TaskRemoval<'_>,
    hooks: &mut dyn RemovalHooks,
) -> Result<Outcome, ops::OpsError> {
    let mut messages = Vec::new();
    let name;

    let bindings_only =
        !task.dir.is_dir() && matches!(task.on_missing, MissingDir::TearDownBindings);
    if bindings_only {
        name = task.opts.name.clone();
        messages.push(format!("worktree {} was already gone", task.dir.display()));
    } else {
        match ops::remove_task(task.opts, || hooks.before_removal())? {
            RemoveOutcome::Removed(removed) => {
                name = removed.name;
                messages.extend(removed.messages);
            }
            RemoveOutcome::Blocked { name, blocked, messages: notes } => {
                messages.extend(notes);
                return Ok(Outcome::Blocked { name, blocked, messages });
            }
        }
    }

    messages.extend(hooks.after_removal(task.dir));
    messages.extend(remove_bindings(
        task.repos_path,
        task.rows,
        task.dir,
        task.outcome,
        task.now_ms,
    ));
    Ok(Outcome::Removed { name, messages })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    /// A store holding one task bound to `dir`.
    fn store_with_task_at(dir: &str) -> Store {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("wire up the thing", "doing", None, NOW).unwrap();
        store
            .set_task_worktree(task.id, "/repos/demo", None, Some("feat/thing"), Some(dir))
            .unwrap();
        store
    }

    fn repos_json(tmp: &tempfile::TempDir, dirs: &[&str]) -> std::path::PathBuf {
        let path = tmp.path().join("repos.json");
        let config = serde_json::json!({ "repoPaths": dirs });
        std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        path
    }

    /// The whole point of step 4 + 5 being one function: a removed worktree
    /// leaves both a tracked path and a board row, and handling only one of
    /// them is what used to strand the other.
    #[test]
    fn remove_bindings_untracks_the_dir_and_closes_the_bound_row() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = "/repos/demo/.claude/worktrees/feat-thing";
        let path = repos_json(&tmp, &[dir, "/repos/other"]);
        let store = store_with_task_at(dir);
        let id = store.task_for_worktree_dir(dir).unwrap().unwrap().id;

        let notes = remove_bindings(
            &path,
            Some(&store),
            std::path::Path::new(dir),
            TaskOutcome::Abandoned,
            NOW + 5,
        );

        assert!(notes.iter().any(|n| n.contains("untracked")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("closed board task")), "{notes:?}");
        assert!(store.task_for_worktree_dir(dir).unwrap().is_none(), "the binding went");
        // …but the row survives as the record, closed with the outcome.
        let row = store.task_by_id(id).unwrap();
        assert_eq!(row.outcome.as_deref(), Some("abandoned"));
        assert_eq!(row.status, "doing", "abandoned freezes the status");
        assert_eq!(row.worktree.as_ref().unwrap().dir, None);
        let left = crate::repos::load_repos(&path);
        assert_eq!(left, vec!["/repos/other".to_string()], "only this task's path was untracked");
    }

    /// A worktree the board never knew about is the rail's normal case, not an
    /// error — it still has to be untracked.
    #[test]
    fn remove_bindings_untracks_a_worktree_with_no_board_row() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = "/repos/demo/.claude/worktrees/feat-orphan";
        let path = repos_json(&tmp, &[dir]);
        let store = Store::open_in_memory().unwrap();

        let notes =
            remove_bindings(&path, Some(&store), std::path::Path::new(dir), TaskOutcome::Done, NOW);

        assert_eq!(notes, vec!["untracked from the agentboard rail".to_string()]);
        assert!(crate::repos::load_repos(&path).is_empty());
    }

    /// A caller that cannot resolve which store owns the row (instance stores
    /// are per-checkout) still gets the untrack — the half it *can* do.
    #[test]
    fn remove_bindings_without_a_store_still_untracks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = "/repos/demo/.claude/worktrees/feat-thing";
        let path = repos_json(&tmp, &[dir]);

        let notes = remove_bindings(&path, None, std::path::Path::new(dir), TaskOutcome::Done, NOW);

        assert_eq!(notes, vec!["untracked from the agentboard rail".to_string()]);
    }

    /// Nothing tracked and nothing bound: silent, not an error. `tt task clean`
    /// runs this for every task it sweeps, most of which were never tracked.
    #[test]
    fn remove_bindings_is_quiet_when_there_is_nothing_to_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let path = repos_json(&tmp, &["/repos/other"]);
        let store = Store::open_in_memory().unwrap();

        let notes = remove_bindings(
            &path,
            Some(&store),
            std::path::Path::new("/repos/gone"),
            TaskOutcome::Done,
            NOW,
        );

        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(crate::repos::load_repos(&path), vec!["/repos/other".to_string()]);
    }

    /// The stale-record path must never reach `ops::remove_task`.
    ///
    /// `opts.root` here is `None` with a name that resolves to nothing — the
    /// shape the app produces when a board row outlived its checkout. If the
    /// removal step ran, it would re-discover a root by walking up from the
    /// *test process's* cwd (this repo), resolve a completely different
    /// checkout, and act on `<that checkout>/.claude/worktrees/<name>`. Succeeding
    /// here is the proof that step is skipped; the bindings still get cleaned.
    #[test]
    fn tear_down_bindings_never_runs_the_removal_step() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("gone-worktree");
        let path = repos_json(&tmp, &[dir.to_str().unwrap()]);
        let store = store_with_task_at(dir.to_str().unwrap());
        let opts = RemoveOpts { root: None, name: "gone-worktree".to_string(), force: false };

        let outcome = remove_task_and_bindings(
            TaskRemoval {
                opts: &opts,
                dir: &dir,
                repos_path: &path,
                rows: Some(&store),
                outcome: TaskOutcome::Done,
                now_ms: NOW,
                on_missing: MissingDir::TearDownBindings,
            },
            &mut NoHooks,
        )
        .expect("a missing dir is not an error for this caller");

        let Outcome::Removed { messages, .. } = outcome else {
            panic!("nothing on disk to block on");
        };
        assert!(messages.iter().any(|m| m.contains("was already gone")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("untracked")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("closed board task")), "{messages:?}");
        assert!(crate::repos::load_repos(&path).is_empty());
    }

    /// The same missing dir, named on a command line instead: a typo must be
    /// reported, not celebrated as a removal.
    #[test]
    fn fail_on_missing_reports_the_unknown_task() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("demo");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let dir = checkout.join(".claude/worktrees/feat-tpyo");
        let path = repos_json(&tmp, &[]);
        let opts = RemoveOpts { root: Some(checkout), name: "feat-tpyo".to_string(), force: false };

        let error = remove_task_and_bindings(
            TaskRemoval {
                opts: &opts,
                dir: &dir,
                repos_path: &path,
                rows: None,
                outcome: TaskOutcome::Abandoned,
                now_ms: NOW,
                on_missing: MissingDir::Fail,
            },
            &mut NoHooks,
        );
        let Err(error) = error else {
            panic!("an unknown task name is an error, not a no-op")
        };
        assert!(matches!(error, ops::OpsError::NoSuchTask { .. }), "{error}");
    }

    /// The row is found by the dir it is bound to, so a different worktree's
    /// removal can never take it.
    #[test]
    fn remove_bindings_leaves_another_worktrees_row_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let mine = "/repos/demo/.claude/worktrees/feat-mine";
        let theirs = "/repos/demo/.claude/worktrees/feat-theirs";
        let path = repos_json(&tmp, &[mine, theirs]);
        let store = store_with_task_at(theirs);

        let notes = remove_bindings(
            &path,
            Some(&store),
            std::path::Path::new(mine),
            TaskOutcome::Done,
            NOW,
        );

        assert!(!notes.iter().any(|n| n.contains("closed board task")), "{notes:?}");
        let theirs_row = store.task_for_worktree_dir(theirs).unwrap();
        assert!(theirs_row.is_some(), "their row stayed bound");
        assert_eq!(theirs_row.unwrap().outcome, None, "…and open");
    }
}
