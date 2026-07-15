//! Worktree-slot convention logic (`tt slot`).
//!
//! The layout convention: any plain git checkout, with branch-named,
//! ephemeral worktree slots nested inside it at
//! `<checkout>/.claude/worktrees/<name>/` — Claude Code's native worktree
//! location, so `claude --worktree` and background sessions (routed through
//! the repo's WorktreeCreate/WorktreeRemove hooks calling
//! `tt slot hook-create`/`hook-remove`) land in the same slots `tt slot`
//! manages. One slot per parallel line of work, removed when its branch
//! merges. A checkout's identity is its directory basename (the same rule
//! `scripts/slot-port.mjs` and `tt_config::state_scope()` use), its per-slot
//! config is a rendered `.env`, and its `.tt-slot` marker records name/base
//! for other tooling.
//!
//! This crate holds the pure logic: the `${tt:...}` template renderer with
//! port-pool claims ([`template`]), env-file parsing/merging ([`envfile`]),
//! slot naming ([`layout`]), setup-command selection ([`ops::setup_command`]),
//! and removal guards ([`guards`]). The CLI layer
//! (`tt-cli/src/commands/slot.rs`) gathers real-world state — git output,
//! bind-tests, docker listings — and hands it here for decisions.

pub mod clean;
pub mod envfile;
pub mod guards;
pub mod layout;
pub mod ops;
pub mod suggest;
pub mod template;

pub use guards::{RmBlocked, check_removal, docker_resource_matches};
pub use layout::{
    CLAUDE_DIR, MARKER_FILE, WORKTREES_DIR, main_checkout_for, marker_contents, parse_marker,
    read_slot_base, slot_name_from_branch, worktrees_dir,
};
pub use ops::{
    CleanOpts, CleanReport, CreateOpts, CreatedSlot, FinishedSlot, KeptSlot, OpsError, RemoveOpts,
    RemovedSlot, SlotRoot, clean_slots, create_slot, discover_root, remove_slot,
};
pub use suggest::{SuggestError, Suggestion, suggest};
pub use template::{RenderOutcome, SlotContext, TemplateError, render};
