//! GitHub/git helper domain logic for the towles-tool CLI.
//!
//! Ports the pure logic from `src/commands/gh/` and `src/lib/git/` in the TypeScript
//! CLI. This crate is deliberately Tauri- and process-free: everything here is a pure
//! function so it can be unit-tested without `gh`, `git`, or a terminal. The `tt-cli`
//! layer shells out (via `tt-exec`) and drives the interactive prompts.
//!
//! Modules:
//! - [`branch_name`] — `feature/<n>-<slug>` from an issue (`branch-name.ts`).
//! - [`pr`] — PR title/body generation (`pr.ts`).
//! - [`branch_clean`] — merged-branch filtering (`branch-clean.ts`).
//! - [`issues`] — `gh issue list` arg building + JSON parsing (`gh-cli-wrapper.ts`).
//! - [`picker`] — issue-picker column layout and rendering (`branch.ts`, `render.ts`).
//! - [`slot_assign`] — clean-tree/remote guard for assigning an issue to a slot checkout.
//! - [`pr_list`] — `ttr gh pr-list` rendering + the "needs you" PR semantics.

use serde::Deserialize;
use thiserror::Error;

pub mod branch_clean;
pub mod branch_name;
pub mod issues;
pub mod picker;
pub mod pr;
pub mod pr_list;
pub mod slot_assign;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to parse GitHub CLI output as JSON. Raw output: {0}")]
    ParseIssues(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A GitHub issue label, mirroring the `labels` entries in the TS `Issue` type.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Label {
    pub name: String,
    pub color: String,
}

/// A GitHub issue as returned by `gh issue list --json labels,number,title,state`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Issue {
    #[serde(default)]
    pub labels: Vec<Label>,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub state: String,
}
