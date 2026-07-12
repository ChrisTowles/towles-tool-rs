//! Worktree-slot convention logic (`ttr slot`).
//!
//! The layout convention: a normal primary checkout `<root>/<repo>-primary/`
//! that always holds the default branch (it is where the user runs the app),
//! plus branch-named, ephemeral worktree slots `<root>/slots/<name>/` created
//! from the primary — one per parallel line of work, removed when the branch
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

pub mod envfile;
pub mod guards;
pub mod layout;
pub mod ops;
pub mod template;

pub use guards::{RmBlocked, check_removal, docker_resource_matches};
pub use layout::{MARKER_FILE, PRIMARY_SUFFIX, SLOTS_DIR, marker_contents, slot_name_from_branch};
pub use ops::{CreateOpts, CreatedSlot, OpsError, SlotRoot, create_slot, discover_root};
pub use template::{RenderOutcome, SlotContext, TemplateError, render};
