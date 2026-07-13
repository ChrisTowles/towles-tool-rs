//! Pure decision/parse logic for `tt gh sync` and `tt gh co`.
//!
//! Both commands guard on a clean working tree, then shell out to `git`/`gh`.
//! Per this crate's rule the decisions live here as pure functions so they can
//! be unit-tested without a real repo: the dirty-tree summary from `git status
//! --porcelain`, the ahead/behind parse from `git rev-list`, the rebase-outcome
//! classification, and the `gh pr view --json headRefName` branch resolve.

use serde::Deserialize;
use thiserror::Error;

/// A summary of an unclean working tree, built from `git status --porcelain`.
/// Produced only when there is something to report — a clean tree yields `None`
/// from [`dirty_tree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyTree {
    /// Total changed or untracked entries.
    pub total: usize,
    /// How many of those entries are untracked (`??`).
    pub untracked: usize,
    /// The reported paths, in porcelain order.
    pub paths: Vec<String>,
}

impl DirtyTree {
    /// A human-readable, multi-line summary for the hard-fail message.
    pub fn summary(&self) -> String {
        let noun = if self.total == 1 { "path" } else { "paths" };
        let mut s = format!("{} changed {noun}", self.total);
        if self.untracked == self.total {
            s.push_str(" (all untracked)");
        } else if self.untracked > 0 {
            s.push_str(&format!(" ({} untracked)", self.untracked));
        }
        s.push(':');
        for p in &self.paths {
            s.push_str("\n  ");
            s.push_str(p);
        }
        s
    }
}

/// Parse `git status --porcelain` output into a [`DirtyTree`], or `None` when
/// the tree is clean. Each non-blank porcelain line is `XY <path>`; `??` marks
/// an untracked entry. Handles an untracked-only tree (every line `??`).
pub fn dirty_tree(porcelain: &str) -> Option<DirtyTree> {
    let mut total = 0;
    let mut untracked = 0;
    let mut paths = Vec::new();
    for line in porcelain.lines() {
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        if line.starts_with("??") {
            untracked += 1;
        }
        // Porcelain v1 is `XY<space><path>`; drop the 3-char status prefix.
        let path = line.get(3..).unwrap_or(line).trim();
        paths.push(path.to_string());
    }
    if total == 0 { None } else { Some(DirtyTree { total, untracked, paths }) }
}

/// Ahead/behind counts of the current branch relative to an upstream, parsed
/// from `git rev-list --left-right --count <upstream>...HEAD`, which prints
/// `<behind>\t<ahead>` (left = commits only on the upstream, right = commits
/// only on HEAD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

/// Parse the two-column `git rev-list --left-right --count` output. Returns
/// `None` if the shape is unexpected (not exactly two integer columns).
pub fn parse_ahead_behind(rev_list_output: &str) -> Option<AheadBehind> {
    let mut parts = rev_list_output.split_whitespace();
    let behind = parts.next()?.parse().ok()?;
    let ahead = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(AheadBehind { ahead, behind })
}

/// The classified outcome of running `git rebase`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// Rebase completed (fast-forwarded or replayed commits cleanly).
    Clean,
    /// Rebase stopped on a conflict; the tree is left mid-rebase and the user
    /// must resolve or `git rebase --abort`.
    Conflict,
    /// Rebase failed for some other reason; the carried message is surfaced.
    Failed(String),
}

/// Classify a `git rebase` result from its exit code and output. A non-zero
/// exit that mentions a conflict is a [`RebaseOutcome::Conflict`]; anything else
/// non-zero is [`RebaseOutcome::Failed`].
pub fn classify_rebase(exit_code: i32, stdout: &str, stderr: &str) -> RebaseOutcome {
    if exit_code == 0 {
        return RebaseOutcome::Clean;
    }
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_lowercase();
    if lower.contains("conflict")
        || lower.contains("could not apply")
        || lower.contains("needs merge")
    {
        return RebaseOutcome::Conflict;
    }
    RebaseOutcome::Failed(combined.trim().to_string())
}

/// Failure resolving a PR's branch from `gh pr view --json headRefName`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SyncError {
    #[error("failed to parse `gh pr view` JSON: {0}")]
    ParsePrView(String),
    #[error("`gh pr view` returned an empty headRefName")]
    EmptyHeadRef,
}

#[derive(Debug, Deserialize)]
struct PrView {
    #[serde(rename = "headRefName")]
    head_ref_name: String,
}

/// Extract `headRefName` from `gh pr view <n> --json headRefName` output.
pub fn parse_head_ref_name(json: &str) -> Result<String, SyncError> {
    let view: PrView =
        serde_json::from_str(json).map_err(|e| SyncError::ParsePrView(e.to_string()))?;
    if view.head_ref_name.trim().is_empty() {
        return Err(SyncError::EmptyHeadRef);
    }
    Ok(view.head_ref_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_tree_is_none_when_clean() {
        assert_eq!(dirty_tree(""), None);
        assert_eq!(dirty_tree("\n\n  \n"), None);
    }

    #[test]
    fn dirty_tree_counts_tracked_changes() {
        let d = dirty_tree(" M src/main.rs\nA  added.rs\n").unwrap();
        assert_eq!(d.total, 2);
        assert_eq!(d.untracked, 0);
        assert_eq!(d.paths, vec!["src/main.rs", "added.rs"]);
    }

    #[test]
    fn dirty_tree_handles_untracked_only() {
        let d = dirty_tree("?? new.txt\n?? other/\n").unwrap();
        assert_eq!(d.total, 2);
        assert_eq!(d.untracked, 2);
        assert!(d.summary().contains("all untracked"));
        assert!(d.summary().contains("new.txt"));
    }

    #[test]
    fn dirty_tree_mixes_tracked_and_untracked() {
        let d = dirty_tree(" M a.rs\n?? b.txt\n").unwrap();
        assert_eq!(d.total, 2);
        assert_eq!(d.untracked, 1);
        assert!(d.summary().contains("(1 untracked)"));
    }

    #[test]
    fn summary_uses_singular_for_one_path() {
        let d = dirty_tree(" M only.rs\n").unwrap();
        assert!(d.summary().starts_with("1 changed path:"));
    }

    #[test]
    fn ahead_behind_parses_two_columns() {
        assert_eq!(parse_ahead_behind("3\t2\n"), Some(AheadBehind { ahead: 2, behind: 3 }));
        assert_eq!(parse_ahead_behind("0 0"), Some(AheadBehind { ahead: 0, behind: 0 }));
    }

    #[test]
    fn ahead_behind_rejects_malformed_output() {
        assert_eq!(parse_ahead_behind(""), None);
        assert_eq!(parse_ahead_behind("5"), None);
        assert_eq!(parse_ahead_behind("1 2 3"), None);
        assert_eq!(parse_ahead_behind("a b"), None);
    }

    #[test]
    fn classify_rebase_clean_on_success() {
        assert_eq!(classify_rebase(0, "Successfully rebased", ""), RebaseOutcome::Clean);
    }

    #[test]
    fn classify_rebase_detects_conflict() {
        let stderr = "CONFLICT (content): Merge conflict in src/main.rs";
        assert_eq!(classify_rebase(1, "", stderr), RebaseOutcome::Conflict);
        assert_eq!(
            classify_rebase(1, "could not apply abc123... wip", ""),
            RebaseOutcome::Conflict
        );
    }

    #[test]
    fn classify_rebase_other_failure_carries_message() {
        let outcome = classify_rebase(128, "", "fatal: invalid upstream 'origin/nope'");
        assert_eq!(
            outcome,
            RebaseOutcome::Failed("fatal: invalid upstream 'origin/nope'".to_string())
        );
    }

    #[test]
    fn parse_head_ref_name_from_fixture() {
        let json = r#"{"headRefName":"feat/gh-sync-checkout"}"#;
        assert_eq!(parse_head_ref_name(json).unwrap(), "feat/gh-sync-checkout");
    }

    #[test]
    fn parse_head_ref_name_ignores_extra_fields() {
        let json = r#"{"headRefName":"feat/x","number":42,"title":"whatever"}"#;
        assert_eq!(parse_head_ref_name(json).unwrap(), "feat/x");
    }

    #[test]
    fn parse_head_ref_name_rejects_empty() {
        assert_eq!(parse_head_ref_name(r#"{"headRefName":""}"#), Err(SyncError::EmptyHeadRef));
    }

    #[test]
    fn parse_head_ref_name_rejects_bad_json() {
        assert!(matches!(parse_head_ref_name("not json"), Err(SyncError::ParsePrView(_))));
    }
}
