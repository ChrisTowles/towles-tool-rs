//! Worktree-slot convention logic (`ttr slot`).
//!
//! The layout convention: a bare hub `<root>/<repo>.git` plus worktree slots
//! `<root>/<repo>-slot-N/`, one per parallel line of work. A slot's identity
//! is its directory basename (the same rule `scripts/slot-port.mjs` and
//! `tt_config::state_scope()` use), its per-slot config is a rendered `.env`,
//! and its `.tt-slot` marker records name/base for other tooling.
//!
//! This crate holds the pure logic: the `${tt:...}` template renderer with
//! port-pool claims ([`template`]), env-file parsing/merging ([`envfile`]),
//! slot naming ([`layout`]), removal guards ([`guards`]), and the
//! full-clones→hub migration planner ([`migrate`]). The CLI layer
//! (`tt-cli/src/commands/slot/`) gathers real-world state — git output,
//! bind-tests, docker listings — and hands it here for decisions, mirroring
//! the `tt_git::slot_assign` pattern.
//!
//! Derived from the shell probe at `~/code/p/blog-repos/slots.sh`, which
//! migrated a real repo first; the guard and idempotence rules encode that
//! probe's findings (see the git history for the write-up).

pub mod envfile;
pub mod guards;
pub mod layout;
pub mod migrate;
pub mod template;

pub use guards::{RmBlocked, check_removal, docker_resource_matches};
pub use layout::{MARKER_FILE, marker_contents, next_slot_number, parse_slot, slot_dir_name};
pub use template::{RenderOutcome, SlotContext, TemplateError, render};
