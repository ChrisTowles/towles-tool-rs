//! Migration planning: a root of full clones → bare hub + worktree slots.
//!
//! `ttr slot migrate` converts the pre-hub layout — `<root>/<repo>-slot-N/`
//! full clones plus an optional unnumbered primary `<root>/<repo>/` — into
//! the bare-hub convention the rest of this crate manages. Pure decisions
//! live here (this crate's rule); the CLI layer gathers git state and
//! executes. The rules encode the manual migrations that preceded the
//! command:
//!
//! - The hub is created by *moving* the donor clone's `.git` directory,
//!   never by re-cloning, so stashes, reflogs, and remotes survive. The
//!   donor is the primary when present, else the lowest-numbered slot.
//! - When the donor has `extensions.worktreeConfig` enabled, `core.bare`
//!   must land in the hub's `config.worktree`, not the shared `config`:
//!   the extension stops git from special-casing `core.bare`, so a shared
//!   `true` makes every linked worktree consider itself bare and breaks it
//!   ("fatal: this operation must be run in a work tree").
//! - Every clone-local branch tip is swept into the hub (created, fast-
//!   forwarded, or parked under `migrate/<clone>/<branch>` when diverged),
//!   and every tip must verify present (`git cat-file -e`) before its clone
//!   is deleted.
//! - Dirty trees are saved as patches and re-applied; `.env`/`.env.local`
//!   are carried over; idle slots (clean, on the default branch) are parked
//!   detached at the default branch, matching `slot new`.

use std::collections::BTreeSet;

use thiserror::Error;

use crate::layout;

/// How a candidate directory relates to git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneKind {
    /// Owns a `.git` directory — convertible.
    FullClone,
    /// Has a `.git` file — already a linked worktree; skipped.
    Worktree,
    /// No `.git` at all (or `.git` is a symlink) — blocks migration.
    NotGit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneHead {
    OnBranch {
        branch: String,
        sha: String,
    },
    Detached {
        sha: String,
    },
    /// No commits yet (or the repo is too broken to report a HEAD).
    Unborn,
}

/// Everything the planner needs to know about one clone directory,
/// gathered by the CLI layer.
#[derive(Debug, Clone)]
pub struct CloneInfo {
    pub name: String,
    pub kind: CloneKind,
    pub head: CloneHead,
    /// `(branch name, tip sha)` for every local branch.
    pub branches: Vec<(String, String)>,
    pub dirty: bool,
    /// `(entry count, tip sha)` when the clone has a stash.
    pub stash: Option<(usize, String)>,
    pub has_linked_worktrees: bool,
    /// A rebase/merge/cherry-pick/bisect in flight, by name.
    pub op_in_progress: Option<String>,
}

// ---------------------------------------------------------------------------
// discovery

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoverError {
    #[error("no <repo>-slot-N clones or <repo>.git hub found")]
    NothingFound,

    #[error("multiple repos here ({0}) — pass --repo to pick one")]
    Ambiguous(String),
}

/// The layout discovered from a root's directory names alone.
#[derive(Debug, PartialEq, Eq)]
pub struct MigrationLayout {
    pub repo: String,
    pub hub_exists: bool,
    /// Directory named exactly `<repo>` — the unnumbered primary clone.
    pub primary: Option<String>,
    /// `<repo>-slot-N` directory names, sorted by slot number.
    pub slots: Vec<String>,
}

/// Repo name a slot-style directory implies: `blog-slot-3` → `blog`.
fn slot_repo_of(name: &str) -> Option<&str> {
    let idx = name.rfind("-slot-")?;
    let digits = &name[idx + "-slot-".len()..];
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| &name[..idx])
}

/// Infer the migration layout from a root's directory names. The repo name
/// comes from `--repo` when given, else from the `<repo>-slot-N` /
/// `<repo>.git` names present — exactly one candidate must emerge.
pub fn discover_migration(
    dir_names: &[String],
    repo_override: Option<&str>,
) -> Result<MigrationLayout, DiscoverError> {
    let repo = match repo_override {
        Some(r) => r.to_string(),
        None => {
            let candidates: BTreeSet<&str> = dir_names
                .iter()
                .filter_map(|n| slot_repo_of(n).or_else(|| layout::repo_from_hub(n)))
                .collect();
            match candidates.len() {
                0 => return Err(DiscoverError::NothingFound),
                1 => candidates.into_iter().next().unwrap_or_default().to_string(),
                _ => {
                    let joined = candidates.into_iter().collect::<Vec<_>>().join(", ");
                    return Err(DiscoverError::Ambiguous(joined));
                }
            }
        }
    };
    let mut numbered: Vec<(u32, String)> = dir_names
        .iter()
        .filter_map(|n| layout::parse_slot(&repo, n).map(|num| (num, n.clone())))
        .collect();
    numbered.sort();
    let found = MigrationLayout {
        hub_exists: dir_names.iter().any(|n| *n == format!("{repo}.git")),
        primary: dir_names.iter().find(|n| **n == repo).cloned(),
        slots: numbered.into_iter().map(|(_, n)| n).collect(),
        repo,
    };
    if !found.hub_exists && found.primary.is_none() && found.slots.is_empty() {
        return Err(DiscoverError::NothingFound);
    }
    Ok(found)
}

// ---------------------------------------------------------------------------
// guards

/// Why the root may NOT be migrated. Migration never has a `--force`: every
/// block preserves work that the conversion could not carry.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MigrateBlocked {
    #[error(
        "{name} matches the slot layout but is not a git clone — if a previous migrate was interrupted after taking its .git, the hub already holds its data; move the directory aside and re-run"
    )]
    NotAClone { name: String },

    #[error("{name} has a git {op} in progress — finish or abort it first")]
    OperationInProgress { name: String, op: String },

    #[error("{name} has linked worktrees of its own — remove them first (git worktree list)")]
    HasLinkedWorktrees { name: String },

    #[error("{name} has no commits (unborn or unreadable HEAD) — remove it or commit first")]
    UnbornHead { name: String },

    #[error(
        "{name} has {count} stash entries and only the newest would survive — apply or drop the older ones first (the donor clone's stashes all survive with its .git)"
    )]
    DeepStash { name: String, count: usize },
}

/// Every reason migration is blocked. Empty = safe to proceed. `donor` is the
/// clone whose `.git` becomes the hub — its whole stash stack survives the
/// move, so the deep-stash guard does not apply to it.
pub fn check_migration(clones: &[CloneInfo], donor: Option<&str>) -> Vec<MigrateBlocked> {
    let mut blocked = Vec::new();
    for c in clones {
        match c.kind {
            CloneKind::Worktree => continue, // already converted
            CloneKind::NotGit => {
                blocked.push(MigrateBlocked::NotAClone { name: c.name.clone() });
                continue;
            }
            CloneKind::FullClone => {}
        }
        if let Some(op) = &c.op_in_progress {
            blocked
                .push(MigrateBlocked::OperationInProgress { name: c.name.clone(), op: op.clone() });
        }
        if c.has_linked_worktrees {
            blocked.push(MigrateBlocked::HasLinkedWorktrees { name: c.name.clone() });
        }
        if c.head == CloneHead::Unborn {
            blocked.push(MigrateBlocked::UnbornHead { name: c.name.clone() });
        }
        if Some(c.name.as_str()) != donor
            && let Some((count, _)) = &c.stash
            && *count > 1
        {
            blocked.push(MigrateBlocked::DeepStash { name: c.name.clone(), count: *count });
        }
    }
    blocked
}

/// The clone whose `.git` becomes the hub: the primary when it is a full
/// clone, else the lowest-numbered full-clone slot. `None` when the hub
/// already exists (a resumed migration) or nothing is convertible.
pub fn choose_donor<'a>(
    migration: &MigrationLayout,
    clones: &'a [CloneInfo],
) -> Option<&'a CloneInfo> {
    if migration.hub_exists {
        return None;
    }
    let full_clone =
        |name: &str| clones.iter().find(|c| c.name == name && c.kind == CloneKind::FullClone);
    migration
        .primary
        .as_deref()
        .and_then(full_clone)
        .or_else(|| migration.slots.iter().find_map(|s| full_clone(s)))
}

// ---------------------------------------------------------------------------
// branch sweep

/// Where a diverged tip (or a stash / detached HEAD) is parked in the hub.
pub fn park_ref(clone_name: &str, leaf: &str) -> String {
    format!("migrate/{clone_name}/{leaf}")
}

/// How one clone-local branch lands in the hub.
#[derive(Debug, PartialEq, Eq)]
pub enum SweepAction {
    /// The hub has no such branch — create it at the clone's tip.
    Create,
    /// The clone's tip is already reachable from the hub's branch.
    AlreadyPresent,
    /// The hub's branch is a strict ancestor of the clone's tip — advance it.
    FastForward,
    /// Tips diverged — preserve the clone's tip under `migrate/<clone>/<branch>`
    /// instead of clobbering either side.
    Park { ref_name: String },
}

/// Decide how one branch is swept. `is_ancestor(a, b)` is the caller's
/// `git merge-base --is-ancestor a b` (both objects must already be in the
/// hub — fetch into a temp namespace first).
pub fn sweep_action(
    clone_name: &str,
    branch: &str,
    clone_sha: &str,
    hub_sha: Option<&str>,
    mut is_ancestor: impl FnMut(&str, &str) -> bool,
) -> SweepAction {
    match hub_sha {
        None => SweepAction::Create,
        Some(hub) if hub == clone_sha || is_ancestor(clone_sha, hub) => SweepAction::AlreadyPresent,
        Some(hub) if is_ancestor(hub, clone_sha) => SweepAction::FastForward,
        Some(_) => SweepAction::Park { ref_name: park_ref(clone_name, branch) },
    }
}

/// Every commit that must exist in the hub before this clone's directory may
/// be deleted: all branch tips, the HEAD commit, and the stash tip.
pub fn shas_to_verify(info: &CloneInfo) -> BTreeSet<String> {
    let mut shas: BTreeSet<String> = info.branches.iter().map(|(_, sha)| sha.clone()).collect();
    match &info.head {
        CloneHead::OnBranch { sha, .. } | CloneHead::Detached { sha } => {
            shas.insert(sha.clone());
        }
        CloneHead::Unborn => {}
    }
    if let Some((_, sha)) = &info.stash {
        shas.insert(sha.clone());
    }
    shas
}

// ---------------------------------------------------------------------------
// checkout planning

/// Why a converted slot ends up detached instead of on its old branch.
#[derive(Debug, PartialEq, Eq)]
pub enum DetachReason {
    /// The clone was already detached at this commit.
    WasDetached,
    /// Clean and on the default branch — parked, matching `slot new`.
    IdleAtDefault,
    /// Dirty tree: kept at its exact old tip so the saved patch applies.
    KeepDirtyBase,
    /// An earlier slot already checked this branch out (git allows a branch
    /// in only one worktree).
    BranchHeldBySibling { branch: String },
    /// The clone's tip diverged from the hub's branch; the old tip is parked
    /// under `migrate/<clone>/<branch>`.
    TipDiverged { branch: String },
}

impl DetachReason {
    /// Human note for the migration summary.
    pub fn note(&self) -> String {
        match self {
            DetachReason::WasDetached => "was already detached here".to_string(),
            DetachReason::IdleAtDefault => "idle, parked at the default branch".to_string(),
            DetachReason::KeepDirtyBase => {
                "dirty tree kept at its old tip so the patch applies".to_string()
            }
            DetachReason::BranchHeldBySibling { branch } => {
                format!("{branch} is checked out in an earlier slot")
            }
            DetachReason::TipDiverged { branch } => {
                format!("its {branch} tip diverged from the hub's (old tip parked)")
            }
        }
    }
}

/// What `git worktree add` should do for one converted slot.
#[derive(Debug, PartialEq, Eq)]
pub enum Checkout {
    /// `worktree add <dir> <branch>` — safe because the hub tip carries the
    /// clone's old tip (equal or fast-forwarded).
    Branch(String),
    /// `worktree add --detach <dir> <at>` — `at` is a sha or branch name.
    Detach { at: String, reason: DetachReason },
}

/// Decide one slot's checkout. `hub_tip` is the hub's post-sweep tip of the
/// clone's HEAD branch; `branch_taken` is whether an earlier converted slot
/// already checked that branch out; `is_ancestor` as in [`sweep_action`].
pub fn plan_checkout(
    head: &CloneHead,
    dirty: bool,
    default_branch: &str,
    hub_tip: Option<&str>,
    branch_taken: bool,
    mut is_ancestor: impl FnMut(&str, &str) -> bool,
) -> Checkout {
    let (branch, sha) = match head {
        // Unborn is guarded out earlier; parking at the default branch is the
        // harmless fallback.
        CloneHead::Unborn => {
            return Checkout::Detach {
                at: default_branch.to_string(),
                reason: DetachReason::IdleAtDefault,
            };
        }
        CloneHead::Detached { sha } => {
            return Checkout::Detach { at: sha.clone(), reason: DetachReason::WasDetached };
        }
        CloneHead::OnBranch { branch, sha } => (branch, sha),
    };

    // Slots never hold the default branch checked out (work PRs into it);
    // idle ones park at it, dirty ones keep their exact base for the patch.
    if branch == default_branch {
        return if dirty {
            Checkout::Detach { at: sha.clone(), reason: DetachReason::KeepDirtyBase }
        } else {
            Checkout::Detach { at: default_branch.to_string(), reason: DetachReason::IdleAtDefault }
        };
    }
    if branch_taken {
        return Checkout::Detach {
            at: sha.clone(),
            reason: DetachReason::BranchHeldBySibling { branch: branch.clone() },
        };
    }
    let tip_matches = hub_tip == Some(sha.as_str());
    if tip_matches || (!dirty && hub_tip.is_some_and(|hub| is_ancestor(sha, hub))) {
        return Checkout::Branch(branch.clone());
    }
    if dirty {
        Checkout::Detach { at: sha.clone(), reason: DetachReason::KeepDirtyBase }
    } else {
        Checkout::Detach {
            at: sha.clone(),
            reason: DetachReason::TipDiverged { branch: branch.clone() },
        }
    }
}

// ---------------------------------------------------------------------------
// hub configuration

/// The hub's per-worktree config file (holds `core.bare` when
/// `extensions.worktreeConfig` is enabled).
pub const WORKTREE_CONFIG_FILE: &str = "config.worktree";

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigScope {
    /// The hub's shared `config` — read by every worktree.
    Shared,
    /// The hub's own `config.worktree` — read only in the hub itself.
    WorktreePrivate,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfigEdit {
    pub scope: ConfigScope,
    pub key: &'static str,
    pub value: &'static str,
}

/// How to mark the moved `.git` bare. With `extensions.worktreeConfig`
/// enabled git stops special-casing `core.bare`, so a `true` in the shared
/// config would make every linked worktree consider itself bare and break it
/// — the flag must live in the hub's own `config.worktree` instead.
pub fn bare_config_edits(worktree_config_enabled: bool) -> Vec<ConfigEdit> {
    if worktree_config_enabled {
        vec![
            ConfigEdit { scope: ConfigScope::WorktreePrivate, key: "core.bare", value: "true" },
            ConfigEdit { scope: ConfigScope::Shared, key: "core.bare", value: "false" },
        ]
    } else {
        vec![ConfigEdit { scope: ConfigScope::Shared, key: "core.bare", value: "true" }]
    }
}

/// The hub's default branch: what `origin/HEAD` names when that branch exists
/// locally, else `main`/`master` when present, else the donor's checked-out
/// branch, else `main`.
pub fn pick_default_branch(
    origin_head: Option<&str>,
    local_branches: &[String],
    head_branch: Option<&str>,
) -> String {
    let local = |b: &str| local_branches.iter().any(|l| l == b);
    if let Some(b) = origin_head.and_then(|h| h.strip_prefix("origin/"))
        && !b.is_empty()
        && local(b)
    {
        return b.to_string();
    }
    for candidate in ["main", "master"] {
        if local(candidate) {
            return candidate.to_string();
        }
    }
    head_branch.unwrap_or("main").to_string()
}

// ---------------------------------------------------------------------------
// backup naming

/// Directory (under the root) holding saved patches and env copies. Kept
/// after a successful migration so the user deletes it once satisfied.
pub fn backup_dir_name(repo: &str) -> String {
    format!("{repo}-migrate-backup")
}

pub fn patch_file_name(clone_name: &str) -> String {
    format!("{clone_name}.patch")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn clone(name: &str, kind: CloneKind) -> CloneInfo {
        CloneInfo {
            name: name.to_string(),
            kind,
            head: CloneHead::OnBranch { branch: "main".into(), sha: "aaa".into() },
            branches: vec![("main".into(), "aaa".into())],
            dirty: false,
            stash: None,
            has_linked_worktrees: false,
            op_in_progress: None,
        }
    }

    // -- discovery ----------------------------------------------------------

    #[test]
    fn discover_infers_repo_primary_and_sorted_slots() {
        let found = discover_migration(
            &names(&[
                "blog-slot-2",
                "blog",
                "blog-slot-0",
                "blog-slot-3.old",
                "junk",
            ]),
            None,
        )
        .unwrap();
        assert_eq!(found.repo, "blog");
        assert_eq!(found.primary.as_deref(), Some("blog"));
        assert_eq!(found.slots, vec!["blog-slot-0", "blog-slot-2"]);
        assert!(!found.hub_exists);
    }

    #[test]
    fn discover_sees_an_existing_hub() {
        let found = discover_migration(&names(&["blog.git", "blog-slot-1"]), None).unwrap();
        assert!(found.hub_exists);
        assert_eq!(found.slots, vec!["blog-slot-1"]);
    }

    #[test]
    fn discover_errors_on_ambiguity_and_nothing() {
        assert_eq!(
            discover_migration(&names(&["blog-slot-0", "zine-slot-1"]), None),
            Err(DiscoverError::Ambiguous("blog, zine".to_string()))
        );
        assert_eq!(
            discover_migration(&names(&["misc", "stuff"]), None),
            Err(DiscoverError::NothingFound)
        );
    }

    #[test]
    fn discover_repo_override_still_requires_matches() {
        let found = discover_migration(&names(&["blog-slot-1", "other"]), Some("blog")).unwrap();
        assert_eq!(found.slots, vec!["blog-slot-1"]);
        assert_eq!(
            discover_migration(&names(&["other"]), Some("blog")),
            Err(DiscoverError::NothingFound)
        );
    }

    // -- guards & donor -----------------------------------------------------

    #[test]
    fn guards_block_the_unconvertible() {
        let mut in_merge = clone("blog-slot-0", CloneKind::FullClone);
        in_merge.op_in_progress = Some("merge".into());
        let mut nested = clone("blog-slot-1", CloneKind::FullClone);
        nested.has_linked_worktrees = true;
        let mut unborn = clone("blog-slot-2", CloneKind::FullClone);
        unborn.head = CloneHead::Unborn;
        let blocked = check_migration(
            &[
                in_merge,
                nested,
                unborn,
                clone("blog-slot-3", CloneKind::NotGit),
                clone("blog-slot-4", CloneKind::Worktree), // skipped, not blocked
            ],
            None,
        );
        assert_eq!(blocked.len(), 4);
        assert!(
            matches!(&blocked[0], MigrateBlocked::OperationInProgress { op, .. } if op == "merge")
        );
        assert!(matches!(&blocked[1], MigrateBlocked::HasLinkedWorktrees { .. }));
        assert!(matches!(&blocked[2], MigrateBlocked::UnbornHead { .. }));
        assert!(matches!(&blocked[3], MigrateBlocked::NotAClone { name } if name == "blog-slot-3"));
    }

    #[test]
    fn deep_stash_blocks_everywhere_but_the_donor() {
        let mut donor = clone("blog", CloneKind::FullClone);
        donor.stash = Some((3, "ddd".into()));
        let mut shallow = clone("blog-slot-0", CloneKind::FullClone);
        shallow.stash = Some((1, "eee".into()));
        let mut deep = clone("blog-slot-1", CloneKind::FullClone);
        deep.stash = Some((2, "fff".into()));
        let blocked = check_migration(&[donor, shallow, deep], Some("blog"));
        assert_eq!(
            blocked,
            vec![MigrateBlocked::DeepStash { name: "blog-slot-1".into(), count: 2 }]
        );
    }

    #[test]
    fn donor_prefers_primary_then_lowest_full_slot() {
        let migration =
            discover_migration(&names(&["blog", "blog-slot-0", "blog-slot-1"]), None).unwrap();
        let clones = [
            clone("blog-slot-0", CloneKind::Worktree),
            clone("blog-slot-1", CloneKind::FullClone),
            clone("blog", CloneKind::FullClone),
        ];
        assert_eq!(choose_donor(&migration, &clones).unwrap().name, "blog");

        let no_primary = discover_migration(&names(&["blog-slot-0", "blog-slot-1"]), None).unwrap();
        assert_eq!(choose_donor(&no_primary, &clones).unwrap().name, "blog-slot-1");
    }

    #[test]
    fn no_donor_once_the_hub_exists() {
        let migration = discover_migration(&names(&["blog.git", "blog-slot-0"]), None).unwrap();
        assert!(choose_donor(&migration, &[clone("blog-slot-0", CloneKind::FullClone)]).is_none());
    }

    // -- sweep --------------------------------------------------------------

    #[test]
    fn sweep_covers_all_relationships() {
        let no = |_: &str, _: &str| false;
        assert_eq!(sweep_action("s0", "b", "abc", None, no), SweepAction::Create);
        assert_eq!(sweep_action("s0", "b", "abc", Some("abc"), no), SweepAction::AlreadyPresent);
        // clone tip behind the hub's → already reachable
        let clone_behind = |a: &str, _: &str| a == "abc";
        assert_eq!(
            sweep_action("s0", "b", "abc", Some("def"), clone_behind),
            SweepAction::AlreadyPresent
        );
        // hub tip behind the clone's → fast-forward
        let hub_behind = |a: &str, _: &str| a == "def";
        assert_eq!(
            sweep_action("s0", "b", "abc", Some("def"), hub_behind),
            SweepAction::FastForward
        );
        assert_eq!(
            sweep_action("blog-slot-2", "feat/y", "abc", Some("def"), no),
            SweepAction::Park { ref_name: "migrate/blog-slot-2/feat/y".to_string() }
        );
    }

    #[test]
    fn verify_set_covers_branches_head_and_stash_deduped() {
        let mut c = clone("blog-slot-0", CloneKind::FullClone);
        c.branches = vec![("main".into(), "aaa".into()), ("feat".into(), "bbb".into())];
        c.head = CloneHead::Detached { sha: "ccc".into() };
        c.stash = Some((1, "bbb".into())); // duplicate of a branch tip
        let shas = shas_to_verify(&c);
        assert_eq!(shas, ["aaa", "bbb", "ccc"].iter().map(|s| s.to_string()).collect());
    }

    // -- checkout planning ---------------------------------------------------

    fn on(branch: &str, sha: &str) -> CloneHead {
        CloneHead::OnBranch { branch: branch.into(), sha: sha.into() }
    }

    #[test]
    fn idle_default_parks_and_dirty_default_keeps_its_base() {
        let no = |_: &str, _: &str| false;
        assert_eq!(
            plan_checkout(&on("main", "abc"), false, "main", Some("abc"), false, no),
            Checkout::Detach { at: "main".into(), reason: DetachReason::IdleAtDefault }
        );
        assert_eq!(
            plan_checkout(&on("main", "abc"), true, "main", Some("xyz"), false, no),
            Checkout::Detach { at: "abc".into(), reason: DetachReason::KeepDirtyBase }
        );
    }

    #[test]
    fn feature_branch_is_claimed_when_the_hub_tip_carries_it() {
        let no = |_: &str, _: &str| false;
        // tip equal — claim even when dirty (the patch applies at the tip)
        assert_eq!(
            plan_checkout(&on("feat/x", "abc"), true, "main", Some("abc"), false, no),
            Checkout::Branch("feat/x".into())
        );
        // clean and merely behind the hub tip — claim (fast-forwarded view)
        let behind = |a: &str, _: &str| a == "abc";
        assert_eq!(
            plan_checkout(&on("feat/x", "abc"), false, "main", Some("def"), false, behind),
            Checkout::Branch("feat/x".into())
        );
    }

    #[test]
    fn moved_or_contested_tips_detach() {
        let no = |_: &str, _: &str| false;
        // dirty and the hub tip moved — stay at the old base
        assert_eq!(
            plan_checkout(&on("feat/x", "abc"), true, "main", Some("def"), false, no),
            Checkout::Detach { at: "abc".into(), reason: DetachReason::KeepDirtyBase }
        );
        // clean but diverged
        assert_eq!(
            plan_checkout(&on("feat/x", "abc"), false, "main", Some("def"), false, no),
            Checkout::Detach {
                at: "abc".into(),
                reason: DetachReason::TipDiverged { branch: "feat/x".into() }
            }
        );
        // branch held by an earlier slot
        assert_eq!(
            plan_checkout(&on("feat/x", "abc"), false, "main", Some("abc"), true, no),
            Checkout::Detach {
                at: "abc".into(),
                reason: DetachReason::BranchHeldBySibling { branch: "feat/x".into() }
            }
        );
    }

    #[test]
    fn detached_and_unborn_heads() {
        let no = |_: &str, _: &str| false;
        assert_eq!(
            plan_checkout(
                &CloneHead::Detached { sha: "abc".into() },
                false,
                "main",
                None,
                false,
                no
            ),
            Checkout::Detach { at: "abc".into(), reason: DetachReason::WasDetached }
        );
        assert_eq!(
            plan_checkout(&CloneHead::Unborn, false, "main", None, false, no),
            Checkout::Detach { at: "main".into(), reason: DetachReason::IdleAtDefault }
        );
    }

    // -- hub config -----------------------------------------------------------

    #[test]
    fn bare_lands_in_config_worktree_when_the_extension_is_on() {
        assert_eq!(
            bare_config_edits(false),
            vec![ConfigEdit { scope: ConfigScope::Shared, key: "core.bare", value: "true" }]
        );
        assert_eq!(
            bare_config_edits(true),
            vec![
                ConfigEdit { scope: ConfigScope::WorktreePrivate, key: "core.bare", value: "true" },
                ConfigEdit { scope: ConfigScope::Shared, key: "core.bare", value: "false" },
            ]
        );
    }

    #[test]
    fn default_branch_prefers_origin_head_when_local() {
        let locals = names(&["main", "feat/x"]);
        assert_eq!(pick_default_branch(Some("origin/main"), &locals, Some("feat/x")), "main");
        // origin/HEAD names a branch with no local counterpart → fall through
        assert_eq!(pick_default_branch(Some("origin/trunk"), &locals, Some("feat/x")), "main");
        assert_eq!(pick_default_branch(None, &names(&["master", "dev"]), Some("dev")), "master");
        assert_eq!(pick_default_branch(None, &names(&["dev"]), Some("dev")), "dev");
        assert_eq!(pick_default_branch(None, &[], None), "main");
    }

    #[test]
    fn backup_names_are_stable() {
        assert_eq!(backup_dir_name("blog"), "blog-migrate-backup");
        assert_eq!(patch_file_name("blog-slot-2"), "blog-slot-2.patch");
        assert_eq!(park_ref("blog-slot-2", "stash"), "migrate/blog-slot-2/stash");
    }
}
