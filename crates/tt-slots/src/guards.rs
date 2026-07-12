//! Removal guards for `ttr slot rm` — a slot must never take work with it.
//!
//! Pure functions (this crate's rule): the CLI gathers git output, bind-test
//! results, and docker listings in the slot's directory and hands the raw
//! data here for the decision, mirroring `tt_git::slot_assign`.
//!
//! Notes from the blog-repos probe that shaped these guards:
//! - Stashes are repo-global in a worktree hub (they live in the shared
//!   `.git`), so a per-slot stash guard is meaningless here — unlike the
//!   clone-era `slot_assign` guard.
//! - The commit guard means *reachable from no branch and no remote*
//!   (`git rev-list --count HEAD --not --branches --remotes`): removing a
//!   worktree never deletes branches (they live in the hub), so only
//!   detached/orphan commits are real data loss. Upstream-based checks
//!   silently pass on never-pushed branches and detached HEADs (both occurred
//!   in the real migration), and remote-only checks block everything in hubs
//!   created by `git clone --bare`, which have no `refs/remotes` at all.

use thiserror::Error;

/// Why a slot may NOT be removed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RmBlocked {
    #[error("slot working tree is not clean ({entries} changed/untracked path(s))")]
    DirtyTree { entries: usize },

    #[error(
        "{count} commit(s) reachable from no branch or remote — removal would orphan them; park them on a branch or push first"
    )]
    UnreachableCommits { count: u64 },

    #[error(
        "port {port} (claimed by this slot) is in use by a process outside the slot's containers — a dev server may be running"
    )]
    ForeignPortListener { port: u16 },
}

/// Count entries in `git status --porcelain` output.
pub fn dirty_entry_count(porcelain: &str) -> usize {
    porcelain.lines().filter(|l| !l.trim().is_empty()).count()
}

/// Parse `git rev-list --count HEAD --not --branches --remotes` output.
pub fn unreachable_commit_count(rev_list_output: &str) -> Option<u64> {
    rev_list_output.trim().parse().ok()
}

/// Every reason removal is blocked, given the gathered state. Empty = safe.
/// `foreign_listener_ports` are claimed ports that are in use by something
/// *other than* the slot's own docker containers (the containers are about to
/// be removed anyway; a foreign listener means a dev server is still running).
pub fn check_removal(
    dirty_entries: usize,
    unreachable_commits: u64,
    foreign_listener_ports: &[u16],
) -> Vec<RmBlocked> {
    let mut blocked = Vec::new();
    if dirty_entries > 0 {
        blocked.push(RmBlocked::DirtyTree { entries: dirty_entries });
    }
    if unreachable_commits > 0 {
        blocked.push(RmBlocked::UnreachableCommits { count: unreachable_commits });
    }
    blocked
        .extend(foreign_listener_ports.iter().map(|&port| RmBlocked::ForeignPortListener { port }));
    blocked
}

/// Whether a docker container/volume name belongs to `slot_name`. Anchored:
/// `blog-slot-1-postgres` and `blog-slot-1_data` match `blog-slot-1`;
/// `blog-slot-10-postgres` does NOT (substring filters caught it in the probe).
pub fn docker_resource_matches(resource: &str, slot_name: &str) -> bool {
    match resource.strip_prefix(slot_name) {
        Some("") => true,
        Some(rest) => rest.starts_with(['-', '_', '.']),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_count_ignores_blank_lines() {
        assert_eq!(dirty_entry_count(" M a.rs\n?? b\n\n"), 2);
        assert_eq!(dirty_entry_count(""), 0);
    }

    #[test]
    fn unreachable_parses_count() {
        assert_eq!(unreachable_commit_count("3\n"), Some(3));
        assert_eq!(unreachable_commit_count("fatal: bad revision"), None);
    }

    #[test]
    fn clean_slot_passes() {
        assert!(check_removal(0, 0, &[]).is_empty());
    }

    #[test]
    fn every_reason_is_reported() {
        let blocked = check_removal(2, 1, &[3000]);
        assert_eq!(
            blocked,
            vec![
                RmBlocked::DirtyTree { entries: 2 },
                RmBlocked::UnreachableCommits { count: 1 },
                RmBlocked::ForeignPortListener { port: 3000 },
            ]
        );
    }

    #[test]
    fn docker_matching_is_anchored() {
        assert!(docker_resource_matches("blog-slot-1-postgres_5433", "blog-slot-1"));
        assert!(docker_resource_matches("blog-slot-1_postgres_data", "blog-slot-1"));
        assert!(docker_resource_matches("blog-slot-1", "blog-slot-1"));
        assert!(!docker_resource_matches("blog-slot-10-postgres", "blog-slot-1"));
        assert!(!docker_resource_matches("other-blog-slot-1", "blog-slot-1"));
    }
}
