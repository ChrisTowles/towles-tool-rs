//! The `tt collect nudge` contract: which collectors an external process can
//! eagerly refresh, and the nudge-dir filename each one touches.
//!
//! Tauri-free like the rest of this crate, so both halves of the contract live
//! in one place: the CLI (`tt collect nudge <key>`) writes the file, and the
//! app's scheduler (`crates-tauri/tt-app/src/scheduler.rs`) watches the dir and
//! diffs each target's mtime. Keeping the key ↔ filename mapping here — rather
//! than duplicated as string literals on each side — is what stops the two from
//! drifting.

/// A collector `tt collect nudge` can refresh ahead of its normal poll cadence.
/// The `SlackDm` target lets a running app refresh the watched Slack DM
/// immediately (e.g. after a manual poke) the same way `Prs`/`Issues` already
/// react to a `gh pr`/`gh issue` mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NudgeTarget {
    Prs,
    Issues,
    SlackDm,
}

impl NudgeTarget {
    /// Every nudge target, in a stable order (used to build the CLI's accepted
    /// value list and to drive exhaustive tests).
    pub const ALL: [NudgeTarget; 3] = [NudgeTarget::Prs, NudgeTarget::Issues, NudgeTarget::SlackDm];

    /// The collector key this target refreshes — the same stable string
    /// `Store::record_run` uses and the frontend matches on (`prs`, `issues`,
    /// `slack:dm`). This is also the value accepted on the CLI.
    pub fn key(self) -> &'static str {
        match self {
            NudgeTarget::Prs => "prs",
            NudgeTarget::Issues => "issues",
            NudgeTarget::SlackDm => "slack:dm",
        }
    }

    /// Filename inside the nudge dir this target touches. Kept free of the
    /// collector key's `:` so it's a plain, portable filename; the scheduler's
    /// `changed_nudge_batches` reads these exact names.
    pub fn file_name(self) -> &'static str {
        match self {
            NudgeTarget::Prs => "prs",
            NudgeTarget::Issues => "issues",
            NudgeTarget::SlackDm => "slack",
        }
    }

    /// Parse a collector key (`prs`, `issues`, `slack:dm`) into a target.
    /// Returns `None` for any key that isn't nudgeable.
    pub fn from_key(key: &str) -> Option<NudgeTarget> {
        NudgeTarget::ALL.into_iter().find(|t| t.key() == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_key_round_trips_every_target() {
        for target in NudgeTarget::ALL {
            assert_eq!(NudgeTarget::from_key(target.key()), Some(target));
        }
    }

    #[test]
    fn from_key_rejects_unknown_or_non_nudgeable_keys() {
        assert_eq!(NudgeTarget::from_key("claude:calendar"), None);
        assert_eq!(NudgeTarget::from_key("slack"), None);
        assert_eq!(NudgeTarget::from_key("PRS"), None);
        assert_eq!(NudgeTarget::from_key(""), None);
    }

    #[test]
    fn slack_key_matches_the_collector_key_and_filename_stays_plain() {
        assert_eq!(NudgeTarget::SlackDm.key(), "slack:dm");
        assert_eq!(NudgeTarget::SlackDm.file_name(), "slack");
    }

    #[test]
    fn file_names_are_distinct() {
        let names: Vec<_> = NudgeTarget::ALL.iter().map(|t| t.file_name()).collect();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "nudge filenames must be unique");
    }
}
