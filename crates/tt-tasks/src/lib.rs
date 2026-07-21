//! Worktree-task convention logic (`tt task`).
//!
//! The layout convention: any plain git checkout, with branch-named,
//! ephemeral worktree tasks nested inside it at
//! `<checkout>/.claude/worktrees/<name>/` — Claude Code's native worktree
//! location, so `claude --worktree` and background sessions (routed through
//! the repo's WorktreeCreate/WorktreeRemove hooks calling
//! `tt task hook-create`/`hook-remove`) land in the same tasks `tt task`
//! manages. One task per parallel line of work, removed when its branch
//! merges. A checkout's identity is its directory basename (the same rule
//! `scripts/task-port.mjs` and `tt_config::state_scope()` use), its per-task
//! config is a rendered `.env`, and its `.tt-task` marker records name/base
//! for other tooling.
//!
//! This crate holds the pure logic: the `${tt:...}` template renderer with
//! port-pool claims ([`template`]), env-file parsing/merging ([`envfile`]),
//! task naming ([`layout`]), setup-command selection ([`ops::setup_command`]),
//! and removal guards ([`guards`]). The CLI layer
//! (`tt-cli/src/commands/task.rs`) gathers real-world state — git output,
//! bind-tests, docker listings — and hands it here for decisions.

pub mod clean;
pub mod envfile;
pub mod guards;
pub mod landed;
pub mod layout;
pub mod ops;
pub mod pasted;
pub mod ports;
pub mod suggest;
pub mod template;

pub use guards::{RmBlocked, check_removal, docker_resource_matches};
pub use landed::{LandedVia, WorkState, classify, probe_work_state};
pub use layout::{
    CLAUDE_DIR, MARKER_FILE, WORKTREES_DIR, is_managed_task, main_checkout_for, marker_contents,
    parse_marker, read_task_base, task_name_from_branch, worktrees_dir,
};
pub use ops::{
    CleanOpts, CleanReport, CreateOpts, CreatedTask, FinishedTask, KeptTask, OpsError, RemoveOpts,
    RemovedTask, TaskRoot, clean_tasks, create_task, discover_root, remove_task,
};
pub use pasted::{PastedError, PastedImage, write_images};
pub use suggest::{SuggestError, Suggestion, suggest};
pub use template::{RenderOutcome, TaskContext, TemplateError, render};
