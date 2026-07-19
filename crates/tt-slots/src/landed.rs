//! Has a slot branch's work actually reached the base branch?
//!
//! This is the single answer to "is it safe to remove this slot", shared by
//! `tt slot ls`/`rm`/`clean` and the Agentboard rail. Before it existed the
//! repo carried three disagreeing models — `guards::check_removal` (orphaned
//! commits), `clean`'s old ancestor-merge-or-`[gone]` rule (since deleted),
//! and the rail's raw `git cherry` count — so the same slot could read
//! "safe to delete" in one surface and "2 commits unlanded" in another.
//!
//! ## Why one signal is never enough
//!
//! A branch's work reaches the base under three different shapes, and each
//! shape is invisible to the checks that catch the other two:
//!
//! | landing        | `merge-base --is-ancestor` | `git cherry` | tree probe |
//! |----------------|---------------------------|--------------|------------|
//! | merge commit   | yes                       | 0            | —          |
//! | rebase / cherry-pick | no                  | 0            | misses     |
//! | squash         | no                        | **counts every commit** | yes |
//!
//! Squash is the case that matters most here, because it is how this repo's
//! PRs land: GitHub replaces the branch's N commits with one new commit whose
//! SHA *and* patch-id differ from all of them, so both reachability and
//! `git cherry` report the whole branch as unlanded work. That false alarm is
//! exactly what made a merged slot look unsafe to remove.
//!
//! The tree probe is what closes it: synthesise a commit holding the branch's
//! *tree* parented on the merge-base, and ask `git cherry` whether that
//! cumulative diff is already in the base. A squashed branch answers yes.
//!
//! ## Counting what is genuinely left
//!
//! A branch that was squash-merged and then had new commits added is the
//! subtle case: `git cherry` counts the already-squashed commits too, so it
//! over-reports (3 commits when only 1 is really outstanding). Per-commit
//! probes find the watermark instead — the newest commit whose tree already
//! landed — and everything after it is the real remainder.
//!
//! Landedness is *not* monotonic along the branch (an intermediate commit can
//! reproduce a tree the base never had), so this scans newest-first and stops
//! at the first landed commit rather than binary-searching.

use std::path::{Path, PathBuf};

/// How a branch's work reached the base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandedVia {
    /// Reachable from the base — a classic merge commit.
    Ancestor,
    /// Every commit is patch-identical to one already in the base — the
    /// branch was rebased or cherry-picked in.
    Patches,
    /// The branch's cumulative diff is in the base under a different SHA —
    /// a squash merge.
    Squash,
    /// The remote branch was deleted, the usual signature of a merged PR.
    /// Weakest signal: it is about the *remote*, not the local content, so it
    /// only applies when none of the content checks answered.
    UpstreamGone,
}

impl LandedVia {
    /// Short phrase for a one-line summary.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ancestor => "merged",
            Self::Patches => "rebase-merged",
            Self::Squash => "squash-merged",
            Self::UpstreamGone => "upstream gone",
        }
    }

    /// Whether this answer is evidence the branch's *content* is in the base.
    ///
    /// All of them except [`Self::UpstreamGone`], which only observes that the
    /// remote branch disappeared. That is usually a merged PR, but it is also
    /// indistinguishable from a branch deleted while still unmerged — so it
    /// must never be taken as proof the commits are safe. Callers that destroy
    /// history (`clean` runs `git branch -D`) gate on this.
    pub fn is_content_proof(self) -> bool {
        !matches!(self, Self::UpstreamGone)
    }
}

/// What a slot still holds, on two independent axes: work that was never
/// committed, and commits whose content never reached the base.
///
/// Kept as separate counts on purpose — collapsing them into one "dirty" flag
/// is what made the old output unreadable, because the two have different
/// consequences. Uncommitted work is destroyed by removal and exists nowhere
/// else; unlanded commits survive on the branch and can be pushed later.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkState {
    /// `git status --porcelain` entries — changed or untracked paths.
    pub uncommitted: usize,
    /// Commits since the merge-base whose content is not in the base branch.
    pub unlanded: u64,
    /// Commits since the merge-base, landed or not.
    pub total_commits: u64,
    /// Set when *all* the branch's content is in the base.
    pub landed: Option<LandedVia>,
    /// Commits reachable from no branch and no remote — a detached HEAD's
    /// work, which removal really would destroy. Distinct from `unlanded`,
    /// which is safe on a branch.
    pub orphaned: u64,
}

impl WorkState {
    /// Whether removing this slot would lose anything a user cannot recover.
    pub fn holds_work(&self) -> bool {
        self.uncommitted > 0 || self.unlanded > 0 || self.orphaned > 0
    }

    /// One line naming each axis that is non-zero, so the reason a slot is
    /// held back is never guesswork. Empty string when there is nothing to
    /// report beyond how it landed.
    pub fn headline(&self) -> String {
        let mut parts = Vec::new();
        if self.uncommitted > 0 {
            parts.push(format!("{} uncommitted", self.uncommitted));
        }
        if self.unlanded > 0 {
            parts.push(format!("{} unlanded", self.unlanded));
        }
        if self.orphaned > 0 {
            parts.push(format!("{} orphaned", self.orphaned));
        }
        // A branch with no commits of its own is reported as such rather than
        // as "merged". The two are indistinguishable after the fact — an
        // absorbed branch and a slot created from an older base both have
        // nothing since their merge-base — and "merged" on a slot nobody ever
        // committed to reads as a claim about work that never existed. Either
        // way nothing is at stake, which is what the phrase needs to convey.
        if self.total_commits == 0 {
            if parts.is_empty() {
                return "no commits".to_string();
            }
            parts.push("no commits".to_string());
            return parts.join(", ");
        }
        if let Some(via) = self.landed {
            if parts.is_empty() {
                return via.label().to_string();
            }
            parts.push(via.label().to_string());
        }
        parts.join(", ")
    }
}

/// Count the `+` lines in `git cherry <base> <branch>` output — commits with
/// no patch-identical twin in the base. `-` lines already landed.
pub fn cherry_unlanded(output: &str) -> u64 {
    output.lines().filter(|l| l.trim_start().starts_with('+')).count() as u64
}

/// Whether a single-line `git cherry` result marks its commit as already
/// landed (a leading `-`).
pub fn cherry_says_landed(output: &str) -> bool {
    output.trim_start().starts_with('-')
}

/// Decide how a branch landed, given each independently-gathered signal.
///
/// Ordering is by strength of evidence: reachability and patch-identity are
/// facts about content in the base, the tree probe is a synthesised
/// equivalent, and a gone upstream is only circumstantial — it says the
/// remote branch was deleted, which is *usually* a merged PR but is also what
/// a branch deleted unmerged looks like. It therefore answers last, and only
/// when nothing about the content did.
///
/// `tip_equals_base` suppresses the ancestor answer for a freshly created
/// slot: a branch still sitting on the base tip is trivially reachable from
/// it, and reporting that as "merged" would invite cleaning a slot someone is
/// about to work in.
///
/// One label is fuzzy by nature: squashing a *single* commit produces a
/// patch-identical commit, so it answers [`LandedVia::Patches`] before the
/// tree probe is consulted and reads as "rebase-merged". There is no
/// information in the repository that could tell the two apart — and both mean
/// the same thing for every decision made here — so this is left alone rather
/// than guessed at from commit messages.
pub fn classify(
    ancestor: bool,
    tip_equals_base: bool,
    cherry_plus: u64,
    total_commits: u64,
    tree_landed: bool,
    upstream_gone: bool,
) -> Option<LandedVia> {
    if tip_equals_base || total_commits == 0 {
        return None;
    }
    if ancestor {
        return Some(LandedVia::Ancestor);
    }
    if cherry_plus == 0 {
        return Some(LandedVia::Patches);
    }
    if tree_landed {
        return Some(LandedVia::Squash);
    }
    if upstream_gone {
        return Some(LandedVia::UpstreamGone);
    }
    None
}

/// Cap on per-commit probes. Each probe is *three* git subprocesses
/// (`rev-parse`, `commit-tree`, `cherry`) and this runs on the Agentboard's
/// poll, so the worst case here is ~3× this many spawns. A slot branch is
/// short-lived by construction; one past this many commits falls back to the
/// `git cherry` count rather than paying an unbounded cost for a number nobody
/// is reading closely. The scan also stops at the first landed commit, so the
/// cap only binds on branches where nothing has landed at all.
const MAX_PROBES: usize = 64;

/// Run every probe against a real repository and assemble the state.
///
/// Best-effort by design: this feeds a status display and a removal guard, so
/// a git failure degrades to the conservative answer — work is present, the
/// branch has not landed — rather than erroring. Reporting "nothing to lose"
/// because git did not answer is the one outcome that could destroy work.
///
/// `git` must return `Some(stdout)` only when git exited **0**, and `None` on
/// any non-zero exit. That distinction is load-bearing rather than stylistic:
/// `merge-base --is-ancestor` reports its answer purely through the exit code
/// and prints nothing, so a closure that returned `Some("")` for a failed run
/// would read every branch as already merged.
pub fn probe_work_state<G>(
    git: &G,
    dir: &Path,
    base: &str,
    branch: &str,
    uncommitted: usize,
    orphaned: u64,
    upstream_gone: bool,
) -> WorkState
where
    G: Fn(&Path, &[&str]) -> Option<String>,
{
    let mut state =
        WorkState { uncommitted, orphaned, landed: None, unlanded: 0, total_commits: 0 };

    let Some(merge_base) = git(dir, &["merge-base", base, branch]).map(|s| s.trim().to_string())
    else {
        return state;
    };
    if merge_base.is_empty() {
        return state;
    }

    // Where the probe's synthetic commits land, so each can be deleted again
    // once `cherry` has read it. `--git-path` answers with the *common* object
    // store from inside a linked worktree, which is exactly where the objects
    // go; an unresolvable path just means they are left for `git gc`.
    let objects_dir = git(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
    )
    .map(|s| PathBuf::from(s.trim()))
    .filter(|p| p.is_dir());
    let objects_dir = objects_dir.as_deref();

    let range = format!("{merge_base}..{branch}");
    let total = git(dir, &["rev-list", "--count", &range])
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    state.total_commits = total;

    let tip = git(dir, &["rev-parse", branch]).map(|s| s.trim().to_string());
    let base_tip = git(dir, &["rev-parse", base]).map(|s| s.trim().to_string());
    let tip_equals_base = tip.is_some() && tip == base_tip;
    let ancestor = git(dir, &["merge-base", "--is-ancestor", branch, base]).is_some();

    // Zero commits since the merge-base is ambiguous, and the two readings
    // have opposite consequences. A *fresh* slot sits on the base tip and must
    // never read as merged — cleaning it would take a slot someone is about to
    // work in. A *merged* branch also has nothing since its merge-base (the
    // merge-base is its own tip once the base absorbed it), but the base has
    // moved on past it, and it is exactly what `clean` should collect.
    if total == 0 {
        state.landed = (!tip_equals_base && ancestor).then_some(LandedVia::Ancestor);
        return state;
    }
    let cherry_plus =
        git(dir, &["cherry", base, branch]).map(|s| cherry_unlanded(&s)).unwrap_or(total);
    let tree_landed = tree_already_in_base(git, dir, base, &merge_base, branch, objects_dir);

    state.landed =
        classify(ancestor, tip_equals_base, cherry_plus, total, tree_landed, upstream_gone);

    // Only content-based evidence justifies claiming nothing is outstanding.
    // A gone upstream leaves the commits to be counted like any other.
    state.unlanded = if state.landed.is_some_and(LandedVia::is_content_proof) {
        0
    } else if total as usize > MAX_PROBES {
        cherry_plus
    } else {
        let revs = git(dir, &["rev-list", &range]).unwrap_or_default();
        let mut newest_first =
            revs.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>();
        if newest_first.is_empty() {
            // `rev-list` failed despite `total > 0`. Falling through to the
            // scan would return 0 — "nothing outstanding" — on no evidence.
            cherry_plus
        } else {
            // The first entry is the branch tip, whose tree `tree_landed`
            // already probed; and this arm only runs when that came back
            // false, so re-probing it would spend three git calls to learn
            // what we know. Stop at the first landed commit rather than
            // probing them all: everything past the watermark is discarded by
            // the count anyway.
            newest_first.remove(0);
            let landed_at = newest_first.iter().position(|rev| {
                tree_already_in_base(git, dir, base, &merge_base, rev, objects_dir)
            });
            1 + landed_at.unwrap_or(newest_first.len()) as u64
        }
    };

    state
}

/// Whether `rev`'s tree, taken as a cumulative diff from `merge_base`, is
/// already present in `base`.
///
/// `commit-tree` writes a real (if unreferenced) commit object holding that
/// tree, because `git cherry` needs something with a patch-id to compare.
///
/// Those objects would otherwise accumulate: nothing on this path triggers
/// git's auto-gc, and `git gc` keeps unreachable objects until
/// `gc.pruneExpire` (two weeks) even when it does run — on the Agentboard's
/// poll that is thousands of dead objects a day, each one slowing later
/// lookups. So the loose object is deleted again as soon as `cherry` has read
/// it (see [`remove_loose_object`]).
///
/// Deleting beats redirecting `GIT_OBJECT_DIRECTORY` at scratch storage: that
/// changes how *every* command in the probe resolves objects, which is a far
/// bigger blast radius than removing the single file we know we created.
fn tree_already_in_base<G>(
    git: &G,
    dir: &Path,
    base: &str,
    merge_base: &str,
    rev: &str,
    objects_dir: Option<&Path>,
) -> bool
where
    G: Fn(&Path, &[&str]) -> Option<String>,
{
    let tree_ref = format!("{rev}^{{tree}}");
    let Some(tree) = git(dir, &["rev-parse", &tree_ref]).map(|s| s.trim().to_string()) else {
        return false;
    };
    // The identity is supplied explicitly because `commit-tree` refuses to run
    // without one, and the ambient one cannot be relied on: git falls back to
    // `user@host` from the system, which is absent on CI runners and minimal
    // containers, where it fails with "Author identity unknown". That failure
    // is silent here — the probe would just answer "not landed", turning every
    // squash-merged slot back into a false alarm in exactly the environments
    // nobody is watching. What the identity *is* does not matter: the commit is
    // deleted below, and `git cherry` compares patch-ids, which ignore it.
    let Some(synthetic) = git(
        dir,
        &[
            "-c",
            "user.name=tt",
            "-c",
            "user.email=tt@localhost",
            "commit-tree",
            &tree,
            "-p",
            merge_base,
            "-m",
            "tt-landed-probe",
        ],
    )
    .map(|s| s.trim().to_string()) else {
        return false;
    };
    if synthetic.is_empty() {
        return false;
    }
    let landed =
        git(dir, &["cherry", base, &synthetic]).is_some_and(|out| cherry_says_landed(&out));
    if let Some(objects) = objects_dir {
        remove_loose_object(objects, &synthetic);
    }
    landed
}

/// Delete the loose object `sha` from `objects_dir`.
///
/// Only ever called on a commit this module just synthesised: unreferenced by
/// any ref, and holding a tree that already exists elsewhere in the store, so
/// removing it cannot lose data. Best-effort — if git packed it, or the path
/// isn't writable, the object simply survives to the next `git gc`.
fn remove_loose_object(objects_dir: &Path, sha: &str) {
    if sha.len() < 3 {
        return;
    }
    let (shard, rest) = sha.split_at(2);
    let _ = std::fs::remove_file(objects_dir.join(shard).join(rest));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cherry_counts_only_plus_lines() {
        assert_eq!(cherry_unlanded("+ abc\n- def\n+ 123\n"), 2);
        assert_eq!(cherry_unlanded("- abc\n- def\n"), 0);
        assert_eq!(cherry_unlanded(""), 0);
    }

    #[test]
    fn squash_merge_is_recognised_when_reachability_and_cherry_both_miss() {
        // The headline case: GitHub squashed 2 commits, so the branch is not
        // an ancestor and `git cherry` reports both as unlanded.
        assert_eq!(
            classify(false, false, 2, 2, true, false),
            Some(LandedVia::Squash),
            "a squash-merged branch must not read as outstanding work"
        );
    }

    #[test]
    fn rebase_merge_is_recognised_by_patch_identity() {
        assert_eq!(classify(false, false, 0, 1, false, false), Some(LandedVia::Patches));
    }

    #[test]
    fn classic_merge_is_recognised_by_reachability() {
        assert_eq!(classify(true, false, 0, 3, false, false), Some(LandedVia::Ancestor));
    }

    #[test]
    fn genuinely_open_branch_is_not_landed() {
        assert_eq!(classify(false, false, 2, 2, false, false), None);
    }

    #[test]
    fn fresh_slot_is_never_landed() {
        // Branch sitting on the base tip: trivially an ancestor, but cleaning
        // it would take a slot someone is about to work in.
        assert_eq!(classify(true, true, 0, 0, false, false), None);
        assert_eq!(classify(true, true, 0, 0, false, true), None);
    }

    #[test]
    fn gone_upstream_answers_only_when_content_checks_do_not() {
        assert_eq!(classify(false, false, 1, 1, false, true), Some(LandedVia::UpstreamGone));
        // Content evidence outranks it.
        assert_eq!(classify(false, false, 2, 2, true, true), Some(LandedVia::Squash));
    }

    #[test]
    fn headline_separates_the_two_axes() {
        // total_commits must be set: `unlanded: 1` with no commits at all is
        // not a state the probe can produce, and defaulting it to 0 would
        // exercise the "no commits" wording instead of the two axes.
        let s = WorkState { uncommitted: 2, unlanded: 1, total_commits: 1, ..Default::default() };
        assert_eq!(s.headline(), "2 uncommitted, 1 unlanded");
    }

    #[test]
    fn headline_of_a_merged_clean_slot_names_how_it_landed() {
        let s =
            WorkState { total_commits: 2, landed: Some(LandedVia::Squash), ..Default::default() };
        assert_eq!(s.headline(), "squash-merged");
        assert!(!s.holds_work());
    }

    #[test]
    fn headline_flags_work_added_after_a_squash_merge() {
        let s =
            WorkState { uncommitted: 0, unlanded: 1, total_commits: 3, landed: None, orphaned: 0 };
        assert_eq!(s.headline(), "1 unlanded");
        assert!(s.holds_work());
    }

    #[test]
    fn orphaned_commits_are_reported_separately_from_unlanded() {
        let s = WorkState { orphaned: 2, total_commits: 2, ..Default::default() };
        assert_eq!(s.headline(), "2 orphaned");
        assert!(s.holds_work());

        // A detached slot whose commits are on no branch at all: nothing since
        // a merge-base, but the orphan axis still has to be reported — that is
        // the work removal really would destroy.
        let detached = WorkState { orphaned: 2, ..Default::default() };
        assert_eq!(detached.headline(), "2 orphaned, no commits");
        assert!(detached.holds_work());
    }
}
