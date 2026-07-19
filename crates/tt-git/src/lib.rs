//! GitHub/git helper domain logic shared by the app and the slot machinery.
//!
//! This crate is deliberately Tauri- and process-free: everything here is a
//! pure function so it can be unit-tested without `gh`, `git`, or a terminal.
//! Callers shell out (via `tt-exec`) and hand the raw text here for decisions.
//!
//! The interactive `tt gh` CLI surface this crate once backed (issue picker,
//! PR content, branch-clean, sync) was removed in the 2026-07-19 CLI trim;
//! its modules went with it. What remains:
//!
//! - [`branch_name`] — `feature/<n>-<slug>` from an issue (`branch-name.ts`).
//! - [`slot_assign`] — clean-tree/remote guard for assigning an issue to a
//!   slot checkout (the app's issue→slot flow).

pub mod branch_name;
pub mod slot_assign;
