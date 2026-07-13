//! Pure decisions for `ttr slot clean` — which slots are *finished* (their
//! branch's work has landed) and which per-checkout state directories are
//! stale. The orchestration (git calls, removal, directory sweep) lives in
//! [`crate::ops::clean_slots`]; this module only decides.

use std::collections::BTreeSet;
use std::fmt;

/// Why a slot counts as finished — safe to clean because its branch's work
/// is reachable from somewhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishedReason {
    /// The branch is a strict ancestor of the base branch (a classic merge
    /// landed it).
    MergedInto(String),
    /// The branch's upstream is gone — the remote branch was deleted, the
    /// GitHub squash/rebase-merge signature (`git branch -vv` shows `: gone`).
    UpstreamGone,
}

impl fmt::Display for FinishedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MergedInto(base) => write!(f, "merged into {base}"),
            Self::UpstreamGone => write!(f, "upstream gone (remote branch deleted after merge)"),
        }
    }
}

/// Decide whether a slot's branch is finished.
///
/// `merged_ancestor` is `git merge-base --is-ancestor <branch> <base>`. On its
/// own it also matches a *fresh* slot — a branch just created from the base
/// tip has no commits of its own and is trivially an ancestor — and cleaning
/// a slot someone is about to work in would be hostile, so ancestor-merge only
/// counts when the branch tip differs from the base tip (`tip_equals_base`).
/// A slot created from an older `--base` ref therefore still reads as merged;
/// it holds zero unique commits, so nothing is lost. `upstream_gone` needs no
/// such guard: an upstream only exists after a push, and only reads gone after
/// the remote branch was deleted.
pub fn finished_reason(
    base: &str,
    merged_ancestor: bool,
    tip_equals_base: bool,
    upstream_gone: bool,
) -> Option<FinishedReason> {
    if merged_ancestor && !tip_equals_base {
        return Some(FinishedReason::MergedInto(base.to_string()));
    }
    if upstream_gone {
        return Some(FinishedReason::UpstreamGone);
    }
    None
}

/// Whether `git for-each-ref --format=%(upstream:track)` output marks the
/// branch's upstream as deleted. Git prints exactly `[gone]` for that case
/// (`[ahead 2]`, `[behind 1]`, … otherwise; empty when there is no upstream
/// or it is in sync).
pub fn upstream_gone(track: &str) -> bool {
    track.trim() == "[gone]"
}

/// Which of the `existing` per-scope state dirs (children of a
/// `…/towles-tool/slots/` parent; see `tt_config::state_scope`) are stale:
/// they belong to `repo` — scopes are `<repo>-primary` / `<repo>-<slot>`, so
/// membership is an anchored `<repo>-` prefix (never a bare substring, so
/// repo `blog` doesn't claim `blog2-thing`) — but no live checkout claims
/// them. Scopes of other repos and hand-forced `TT_STATE_SCOPE` names are
/// left alone. Sorted for deterministic output.
pub fn stale_scope_dirs(
    repo: &str,
    live_scopes: &BTreeSet<String>,
    existing: &[String],
) -> Vec<String> {
    let ours = |scope: &str| {
        scope.strip_prefix(repo).is_some_and(|rest| rest.starts_with('-') && rest.len() > 1)
    };
    let mut stale: Vec<String> = existing
        .iter()
        .filter(|scope| ours(scope) && !live_scopes.contains(*scope))
        .cloned()
        .collect();
    stale.sort();
    stale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_slot_is_not_finished() {
        // A branch just created from the base tip: ancestor-merged trivially,
        // tip equals base tip — must be kept for the work about to happen.
        assert_eq!(finished_reason("main", true, true, false), None);
    }

    #[test]
    fn classic_merge_finishes() {
        assert_eq!(
            finished_reason("main", true, false, false),
            Some(FinishedReason::MergedInto("main".into()))
        );
    }

    #[test]
    fn gone_upstream_finishes_even_when_not_ancestor() {
        // Squash/rebase merges land under new SHAs — never ancestor-merged.
        assert_eq!(finished_reason("main", false, false, true), Some(FinishedReason::UpstreamGone));
    }

    #[test]
    fn active_branch_is_kept() {
        assert_eq!(finished_reason("main", false, false, false), None);
    }

    #[test]
    fn upstream_gone_parses_track_output() {
        assert!(upstream_gone("[gone]\n"));
        assert!(!upstream_gone("[ahead 2]"));
        assert!(!upstream_gone("[behind 1]"));
        assert!(!upstream_gone("")); // no upstream / in sync
        assert!(!upstream_gone("[ahead 1, behind 2]"));
    }

    #[test]
    fn stale_scopes_are_anchored_and_exclude_live() {
        let live: BTreeSet<String> = ["demo-primary".to_string(), "demo-wip".to_string()].into();
        let existing = vec![
            "demo-primary".to_string(),    // live primary — kept
            "demo-wip".to_string(),        // live slot — kept
            "demo-old-merged".to_string(), // removed slot — stale
            "demo2-thing".to_string(),     // another repo (anchored!) — kept
            "blog-thing".to_string(),      // another repo — kept
            "demo".to_string(),            // bare repo name, not a scope — kept
            "demo-".to_string(),           // empty slot part, not a scope — kept
        ];
        assert_eq!(stale_scope_dirs("demo", &live, &existing), vec!["demo-old-merged"]);
    }

    #[test]
    fn stale_scopes_sorted() {
        let live = BTreeSet::new();
        let existing = vec!["r-b".to_string(), "r-a".to_string()];
        assert_eq!(stale_scope_dirs("r", &live, &existing), vec!["r-a", "r-b"]);
    }
}
