//! Guard logic for `tt gh assign`: dispatching an open issue to a sibling
//! slot checkout (`tt:parallel-slots`). The whole point of the feature is the
//! guard — an issue must never land in a slot that is holding someone's
//! in-progress work, so the checks hard-fail with no `--force` escape hatch.
//!
//! Pure functions only (this crate's rule): the CLI layer gathers the git
//! output (`remote get-url`, `status --porcelain`, `stash list`) in the target
//! slot's directory and hands the raw text here for the decision.

use thiserror::Error;

/// Why an issue may NOT be assigned into the target slot.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SlotBlocked {
    #[error(
        "slot remote does not match this repo's remote\n  this repo: {expected}\n  slot:      {found}"
    )]
    RemoteMismatch { expected: String, found: String },

    #[error("slot working tree is not clean ({entries} changed/untracked path(s))")]
    DirtyTree { entries: usize },

    #[error("slot has {count} stash entr{} — pop or drop them first", if *count == 1 { "y" } else { "ies" })]
    StashNotEmpty { count: usize },
}

/// Normalize a git remote URL for equality: `git@github.com:User/Repo.git`,
/// `https://github.com/user/repo/`, and `ssh://git@github.com/User/repo` all
/// name the same repo. Lowercased, scheme/credentials stripped, trailing
/// `.git` and `/` dropped.
pub fn normalize_remote_url(url: &str) -> String {
    let mut s = url.trim().to_lowercase();
    // scp-like syntax: git@host:path → host/path
    if let Some(rest) = s.strip_prefix("git@") {
        s = rest.replacen(':', "/", 1);
    } else {
        // scheme://[user@]host/path → host/path
        if let Some((_, rest)) = s.split_once("://") {
            s = rest.to_string();
        }
        if let Some((_, rest)) = s.split_once('@') {
            s = rest.to_string();
        }
    }
    let s = s.trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s).to_string()
}

/// Count entries in `git status --porcelain` output — each non-blank line is
/// one changed tracked path or untracked path.
pub fn dirty_entry_count(porcelain: &str) -> usize {
    porcelain.lines().filter(|l| !l.trim().is_empty()).count()
}

/// Count entries in `git stash list` output.
pub fn stash_count(stash_list: &str) -> usize {
    stash_list.lines().filter(|l| !l.trim().is_empty()).count()
}

/// Extract the GitHub `owner/name` slug from a git remote URL. Reuses
/// [`normalize_remote_url`] to fold the ssh/https/scp forms, then keeps the last
/// two path segments (`github.com/owner/name` → `owner/name`), lowercased.
/// `None` when the URL lacks two path segments (e.g. a bare host or a local
/// path), so the caller can treat it as "not a GitHub checkout".
pub fn repo_slug_from_remote(url: &str) -> Option<String> {
    let normalized = normalize_remote_url(url);
    let mut segments = normalized.split('/').filter(|s| !s.is_empty());
    // Drop everything before the final owner/name pair.
    let parts: Vec<&str> = segments.by_ref().collect();
    if parts.len() < 2 {
        return None;
    }
    let name = parts[parts.len() - 1];
    let owner = parts[parts.len() - 2];
    Some(format!("{owner}/{name}"))
}

/// The in-progress-work half of the guard, shared by both entry points: reject
/// a dirty working tree first, then a non-empty stash. Order matters — the
/// failures surface most-to-least obvious.
fn check_clean(status_porcelain: &str, stash_list: &str) -> Result<(), SlotBlocked> {
    let entries = dirty_entry_count(status_porcelain);
    if entries > 0 {
        return Err(SlotBlocked::DirtyTree { entries });
    }
    let count = stash_count(stash_list);
    if count > 0 {
        return Err(SlotBlocked::StashNotEmpty { count });
    }
    Ok(())
}

/// The full assignment guard, in the order the failures should surface:
/// wrong repo first (the assignment makes no sense at all), then in-progress
/// work (uncommitted changes, then stashes). Matches two full remote URLs — the
/// CLI's `tt gh assign` compares the slot against the current checkout's
/// `origin`.
pub fn validate_slot(
    expected_remote: &str,
    slot_remote: &str,
    status_porcelain: &str,
    stash_list: &str,
) -> Result<(), SlotBlocked> {
    let expected = normalize_remote_url(expected_remote);
    let found = normalize_remote_url(slot_remote);
    if expected != found {
        return Err(SlotBlocked::RemoteMismatch { expected, found });
    }
    check_clean(status_porcelain, stash_list)
}

/// The assignment guard keyed by a GitHub `owner/name` slug rather than a full
/// remote URL. Used by the desktop app, where the "expected" repo comes from
/// an issue's `repo` field (`owner/name`), not a current-directory checkout.
/// Same failure order as [`validate_slot`].
pub fn validate_slot_for_repo(
    expected_repo: &str,
    slot_remote: &str,
    status_porcelain: &str,
    stash_list: &str,
) -> Result<(), SlotBlocked> {
    let expected = expected_repo.trim().to_lowercase();
    let found = repo_slug_from_remote(slot_remote).unwrap_or_default();
    if expected != found {
        return Err(SlotBlocked::RemoteMismatch { expected, found });
    }
    check_clean(status_porcelain, stash_list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_equates_ssh_https_and_scp_forms() {
        let forms = [
            "git@github.com:ChrisTowles/towles-tool-rs.git",
            "https://github.com/ChrisTowles/towles-tool-rs.git",
            "https://github.com/christowles/towles-tool-rs",
            "https://github.com/ChrisTowles/towles-tool-rs/",
            "ssh://git@github.com/ChrisTowles/towles-tool-rs",
            "  git@github.com:ChrisTowles/towles-tool-rs.git\n",
        ];
        for form in forms {
            assert_eq!(
                normalize_remote_url(form),
                "github.com/christowles/towles-tool-rs",
                "failed for {form}"
            );
        }
    }

    #[test]
    fn normalize_keeps_distinct_repos_distinct() {
        assert_ne!(
            normalize_remote_url("git@github.com:a/repo.git"),
            normalize_remote_url("git@github.com:b/repo.git")
        );
        assert_ne!(
            normalize_remote_url("https://github.com/a/repo"),
            normalize_remote_url("https://gitlab.com/a/repo")
        );
    }

    #[test]
    fn counts_dirty_entries_and_stashes() {
        assert_eq!(dirty_entry_count(""), 0);
        assert_eq!(dirty_entry_count("\n\n"), 0);
        assert_eq!(dirty_entry_count(" M src/main.rs\n?? new.txt\n"), 2);
        assert_eq!(stash_count(""), 0);
        assert_eq!(stash_count("stash@{0}: WIP on main: abc123 msg\n"), 1);
    }

    #[test]
    fn validate_passes_a_clean_matching_slot() {
        assert_eq!(
            validate_slot("git@github.com:u/repo.git", "https://github.com/u/repo", "", ""),
            Ok(())
        );
    }

    #[test]
    fn validate_rejects_remote_mismatch_before_dirty_checks() {
        // Wrong repo wins even when the tree is also dirty — the assignment is
        // nonsensical, not merely unsafe.
        let err = validate_slot(
            "git@github.com:u/repo.git",
            "git@github.com:other/elsewhere.git",
            "?? junk.txt\n",
            "",
        )
        .unwrap_err();
        assert!(matches!(err, SlotBlocked::RemoteMismatch { .. }));
    }

    #[test]
    fn validate_rejects_dirty_tree() {
        let err =
            validate_slot("git@h:u/r.git", "git@h:u/r.git", " M a.rs\n?? b.txt\n", "").unwrap_err();
        assert_eq!(err, SlotBlocked::DirtyTree { entries: 2 });
    }

    #[test]
    fn validate_rejects_stash_entries_even_with_clean_tree() {
        let err = validate_slot(
            "git@h:u/r.git",
            "git@h:u/r.git",
            "",
            "stash@{0}: WIP on main: abc123 wip\n",
        )
        .unwrap_err();
        assert_eq!(err, SlotBlocked::StashNotEmpty { count: 1 });
        // Error text is user-facing; keep the singular/plural readable.
        assert!(err.to_string().contains("1 stash entry"));
    }

    #[test]
    fn repo_slug_extracts_owner_name_from_every_form() {
        let forms = [
            "git@github.com:ChrisTowles/towles-tool-rs.git",
            "https://github.com/ChrisTowles/towles-tool-rs.git",
            "https://github.com/christowles/towles-tool-rs",
            "ssh://git@github.com/ChrisTowles/towles-tool-rs",
        ];
        for form in forms {
            assert_eq!(
                repo_slug_from_remote(form).as_deref(),
                Some("christowles/towles-tool-rs"),
                "failed for {form}"
            );
        }
    }

    #[test]
    fn repo_slug_is_none_without_two_segments() {
        assert_eq!(repo_slug_from_remote("github.com"), None);
        assert_eq!(repo_slug_from_remote(""), None);
    }

    #[test]
    fn validate_for_repo_matches_issue_slug_against_slot_remote() {
        assert_eq!(
            validate_slot_for_repo(
                "ChrisTowles/towles-tool-rs",
                "git@github.com:christowles/towles-tool-rs.git",
                "",
                "",
            ),
            Ok(())
        );
    }

    #[test]
    fn validate_for_repo_rejects_a_different_repo() {
        let err = validate_slot_for_repo(
            "ChrisTowles/towles-tool-rs",
            "git@github.com:someone/other-repo.git",
            "",
            "",
        )
        .unwrap_err();
        assert!(matches!(err, SlotBlocked::RemoteMismatch { .. }));
    }

    #[test]
    fn validate_for_repo_rejects_wrong_repo_before_dirty_checks() {
        // Repo mismatch wins over a dirty tree — same precedence as validate_slot.
        let err = validate_slot_for_repo(
            "u/repo",
            "git@github.com:other/elsewhere.git",
            "?? junk.txt\n",
            "",
        )
        .unwrap_err();
        assert!(matches!(err, SlotBlocked::RemoteMismatch { .. }));
    }

    #[test]
    fn validate_for_repo_rejects_dirty_and_stashed_matching_slots() {
        let dirty = validate_slot_for_repo(
            "u/repo",
            "https://github.com/u/repo.git",
            " M a.rs\n?? b.txt\n",
            "",
        )
        .unwrap_err();
        assert_eq!(dirty, SlotBlocked::DirtyTree { entries: 2 });

        let stashed = validate_slot_for_repo(
            "u/repo",
            "https://github.com/u/repo.git",
            "",
            "stash@{0}: WIP on main: abc123 wip\n",
        )
        .unwrap_err();
        assert_eq!(stashed, SlotBlocked::StashNotEmpty { count: 1 });
    }
}
