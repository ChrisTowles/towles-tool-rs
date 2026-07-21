//! Pure helpers for `tt task clean`: reading a branch's upstream-tracking
//! state, and deciding which per-checkout state directories are stale. The
//! orchestration (git calls, removal, directory sweep) lives in
//! [`crate::ops::clean_tasks`].
//!
//! Whether a task is *finished* is no longer decided here — that moved to
//! [`crate::landed`], which combines several git signals because the
//! ancestor-or-`[gone]` rule this module used to apply could not see a squash
//! merge and treated a deleted remote branch as proof of one.

use std::collections::BTreeSet;

/// Whether `git for-each-ref --format=%(upstream:track)` output marks the
/// branch's upstream as deleted. Git prints exactly `[gone]` for that case
/// (`[ahead 2]`, `[behind 1]`, … otherwise; empty when there is no upstream
/// or it is in sync).
pub fn upstream_gone(track: &str) -> bool {
    track.trim() == "[gone]"
}

/// Which of the `existing` per-scope state dirs (children of a
/// `…/towles-tool/tasks/` parent; see `tt_config::state_scope`) are stale:
/// they belong to `repo` — task scopes are `<repo>-<task>`, so membership is
/// an anchored `<repo>-` prefix (never a bare substring, so repo `blog`
/// doesn't claim `blog2-thing`; the main checkout's own scope is its bare
/// dir name, which the anchored prefix never matches) — but no live checkout
/// claims them. Scopes of other repos and hand-forced `TT_STATE_SCOPE` names
/// are left alone. Sorted for deterministic output.
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
            "demo-wip".to_string(),        // live task — kept
            "demo-old-merged".to_string(), // removed task — stale
            "demo2-thing".to_string(),     // another repo (anchored!) — kept
            "blog-thing".to_string(),      // another repo — kept
            "demo".to_string(),            // bare repo name, not a scope — kept
            "demo-".to_string(),           // empty task part, not a scope — kept
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
