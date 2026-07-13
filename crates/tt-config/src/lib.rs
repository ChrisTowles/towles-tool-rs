//! Configuration model for the towles-tool CLI, plus the resolver for every
//! mutable state path the CLI and app touch.
//!
//! This mirrors the zod settings schema used by the TypeScript CLI and reads/writes
//! the *same* file (`~/.config/towles-tool/towles-tool.settings.json`). Because the
//! file is shared, the model deliberately tolerates unknown fields and fills missing
//! ones from defaults via `#[serde(default)]` — never `deny_unknown_fields`.
//!
//! The JSON schema ([`json_schema`]) is **derived from these structs** via
//! `schemars` (`#[derive(JsonSchema)]`) rather than hand-maintained, so the
//! schema cannot silently drift from the serde model. Two invariants keep the
//! shared file safe and are enforced by tests: every schema property name is
//! `camelCase` (matching what the TypeScript CLI reads/writes), and writes to
//! the shared file go through [`save_merge`]/[`save_merge_to`] so TS-owned keys
//! survive.
//!
//! ## Slot-scoped state
//!
//! Chris runs many worktree slot clones of this repo concurrently
//! (`…/towles-tool-rs-slot-N`). To stop concurrent dev instances from clobbering
//! one shared settings file / tt.db / agentboard dir, this module derives a
//! *scope* from the running instance and, when scoped, nests all mutable state
//! under `…/towles-tool/slots/<scope>/…`. See [`state_scope`] for the rule.
//! When unscoped (the installed daily driver) the paths are exactly the historic
//! defaults, so the shared settings file the TypeScript CLI also reads is
//! untouched.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Tool name used for the config directory and file name.
pub const TOOL_NAME: &str = "towles-tool";

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Could not determine home directory")]
    NoHomeDir,

    #[error("Could not determine data directory")]
    NoDataDir,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Position of the AgentBoard sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition {
    Left,
    Right,
}

/// Journal path templates and base folders.
///
/// Path template tokens follow Luxon formatting, e.g. `{yyyy}`, `{MM}`, `{dd}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct JournalSettings {
    /// Base folder where all journal files are stored.
    pub base_folder: String,

    /// Template for daily-note file paths (relative to `base_folder`).
    pub daily_path_template: String,

    /// Template for meeting-note file paths (relative to `base_folder`).
    pub meeting_path_template: String,

    /// Template for note file paths (relative to `base_folder`).
    pub note_path_template: String,

    /// Directory holding external templates (falls back to built-ins when absent).
    pub template_dir: String,
}

impl Default for JournalSettings {
    fn default() -> Self {
        Self {
            base_folder: home_dir_string(),
            daily_path_template:
                "journal/{monday:yyyy}/{monday:MM}/daily-notes/{monday:yyyy}-{monday:MM}-{monday:dd}-daily-notes.md"
                    .to_string(),
            meeting_path_template: "journal/{yyyy}/{MM}/meetings/{yyyy}-{MM}-{dd}-{title}.md"
                .to_string(),
            note_path_template: "journal/{yyyy}/{MM}/notes/{yyyy}-{MM}-{dd}-{title}.md".to_string(),
            template_dir: default_template_dir(),
        }
    }
}

/// AgentBoard UI preferences. Every field is optional; the TS CLI owns most of them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentboardSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mux: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Theme name or an inline theme object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_position: Option<SidebarPosition>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub keybinding: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_panel_heights: Option<HashMap<String, f64>>,

    /// Context-% at/above which a cold (cache-expired) Claude session gets the
    /// "compact" nudge in the app. `None` = the built-in default (30). Only
    /// written once the user changes it, so the shared settings file stays
    /// clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact_recommend_percent: Option<u8>,

    /// Fire a desktop notification when an agent session flips into
    /// "needs you" (waiting/errored/finished-unseen). `None` = the built-in
    /// default (on). Only written once the user changes it, so the shared
    /// settings file stays clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_needs_you: Option<bool>,

    /// Fire a desktop notification when the next meeting's countdown reaches
    /// zero (it starts). `None` = the built-in default (on). Only written once
    /// the user changes it, so the shared settings file stays clean for the TS
    /// CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_meeting_start: Option<bool>,

    /// Fire a desktop notification when a PR newly enters the review-requested
    /// set. `None` = the built-in default (on). Only written once the user
    /// changes it, so the shared settings file stays clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_review_requested: Option<bool>,

    /// Copy the terminal's active selection to the clipboard as soon as a
    /// selection gesture ends (copy-on-select). `None` = the built-in default
    /// (on). Only written once the user changes it, so the shared settings
    /// file stays clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_on_select: Option<bool>,

    /// Font size (px) for the app's canvas terminals. `None` = the built-in
    /// default (13). Written when the user zooms with Ctrl/⌘ +/- or edits the
    /// setting, so the shared settings file stays clean for the TS CLI until
    /// then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_font_size: Option<u8>,

    /// Fire a desktop notification when a collector silently stops succeeding
    /// (its last healthy run ages out, or it fails repeatedly — expired `gh`
    /// auth, a revoked Slack token). `None` = the built-in default (on). Only
    /// written once the user changes it, so the shared settings file stays clean
    /// for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_stale_collector: Option<bool>,

    /// Fire a desktop notification when one of your authored PRs has its CI flip
    /// into failing. `None` = the built-in default (on). Only written once the
    /// user changes it, so the shared settings file stays clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_checks_failed: Option<bool>,

    /// Let board-wide action shortcuts (jump to next/prev session needing you,
    /// close/split session, toggle diff/rail) fire even while a terminal has
    /// focus, instead of being swallowed as shell input. `None` = the built-in
    /// default (on). Only written once the user changes it, so the shared
    /// settings file stays clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcuts_work_in_terminal: Option<bool>,
}

/// Built-in default for [`AgentboardSettings::compact_recommend_percent`].
pub const DEFAULT_COMPACT_RECOMMEND_PERCENT: u8 = 30;

/// Built-in default for [`AgentboardSettings::notify_needs_you`]: notifications on.
pub const DEFAULT_NOTIFY_NEEDS_YOU: bool = true;

/// Built-in default for [`AgentboardSettings::notify_meeting_start`]: on.
pub const DEFAULT_NOTIFY_MEETING_START: bool = true;

/// Built-in default for [`AgentboardSettings::notify_review_requested`]: on.
pub const DEFAULT_NOTIFY_REVIEW_REQUESTED: bool = true;

/// Built-in default for [`AgentboardSettings::copy_on_select`]: on.
pub const DEFAULT_COPY_ON_SELECT: bool = true;

/// Built-in default for [`AgentboardSettings::terminal_font_size`]: 13px.
pub const DEFAULT_TERMINAL_FONT_SIZE: u8 = 13;

/// Built-in default for [`AgentboardSettings::notify_stale_collector`]: on.
pub const DEFAULT_NOTIFY_STALE_COLLECTOR: bool = true;

/// Built-in default for [`AgentboardSettings::notify_checks_failed`]: on.
pub const DEFAULT_NOTIFY_CHECKS_FAILED: bool = true;

/// Built-in default for [`AgentboardSettings::shortcuts_work_in_terminal`]: on.
pub const DEFAULT_SHORTCUTS_WORK_IN_TERMINAL: bool = true;

/// Data-hub collector settings (the Rust CLI/app's tt.db collectors; the TS CLI
/// ignores this block). Each collector is configured independently — enable
/// flag, refresh cadence, and (for the claude-backed calendar) which MCP
/// provider to drive so the same app works at home (Google) and work (Outlook).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct CollectorsSettings {
    pub calendar: CalendarCollector,
    pub prs: PrCollector,
    pub issues: IssueCollector,
    pub slack: SlackDmCollector,
}

/// Calendar collector: shells out to `claude -p` against an MCP calendar, so it
/// costs tokens — disabled by default; opt in per machine.
/// `provider` selects the built-in prompt variant + MCP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarCollector {
    pub enabled: bool,
    /// `"google"` (home) or `"outlook"` (work). Unknown values fall back to Google.
    pub provider: String,
    pub refresh_minutes: u64,
    /// Working-hours window that further gates *when* the (already token-costing)
    /// calendar collector may run. Skips nights and weekends when there's no
    /// meeting to count down to. Only narrows an already-`enabled` collector;
    /// disable it (`enabled = false`) to restore 24/7 running.
    pub quiet_hours: CalendarQuietHours,
}

impl Default for CalendarCollector {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "google".to_string(),
            refresh_minutes: 15,
            quiet_hours: CalendarQuietHours::default(),
        }
    }
}

/// Working-hours gate for the calendar collector: a daily time window plus a
/// weekday mask, evaluated in local time. When `enabled`, the collector runs
/// only inside `[startHour:00, endHour:00)` on a listed weekday; outside it
/// (nights, weekends) the token-costing `claude -p` run is skipped. Set
/// `enabled = false` to run on the plain refresh cadence around the clock.
///
/// `weekdays` are day-of-week numbers with **0 = Monday … 6 = Sunday** (matching
/// chrono's `num_days_from_monday`); the default is Mon–Fri.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarQuietHours {
    /// When false the gate is off entirely and the collector runs 24/7 on its
    /// refresh cadence (the historic behaviour).
    pub enabled: bool,
    /// First local hour (0–23) the collector may run, inclusive of `:00`.
    pub start_hour: u8,
    /// Local hour (0–23) at which the window closes, exclusive — a run at
    /// exactly `endHour:00` is skipped. With the default `18` the last runnable
    /// minute is `17:59`.
    pub end_hour: u8,
    /// Weekdays the collector may run, as `0 = Monday … 6 = Sunday`. Default Mon–Fri.
    pub weekdays: Vec<u8>,
}

impl Default for CalendarQuietHours {
    fn default() -> Self {
        // 8:00–18:00 local, Monday–Friday.
        Self { enabled: true, start_hour: 8, end_hour: 18, weekdays: vec![0, 1, 2, 3, 4] }
    }
}

/// Pull-request collector (via `gh`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct PrCollector {
    pub enabled: bool,
    pub refresh_seconds: u64,
}

impl Default for PrCollector {
    fn default() -> Self {
        Self { enabled: true, refresh_seconds: 120 }
    }
}

/// Slack DM watcher: polls one DM conversation (e.g. spouse) via the Slack Web
/// API and surfaces unanswered messages in the app's attention banner. Needs a
/// user OAuth token (`xoxp-…`) with `im:history` — disabled until one is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct SlackDmCollector {
    pub enabled: bool,
    /// Slack user OAuth token (`xoxp-…`). Empty = collector stays off.
    pub token: String,
    /// Slack app-level token (`xapp-…`) for Socket Mode real-time delivery.
    /// Empty = no socket, poll-only. Distinct from `token`: this authorizes
    /// `apps.connections.open`, not the Web API DM calls.
    #[serde(default)]
    pub app_token: String,
    /// Slack member ID of the person to watch (e.g. `U0123ABCD`).
    pub watch_user_id: String,
    /// Display name shown in the banner (avoids an extra `users.info` call).
    pub watch_name: String,
    pub refresh_seconds: u64,
}

impl Default for SlackDmCollector {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            app_token: String::new(),
            watch_user_id: String::new(),
            watch_name: String::new(),
            refresh_seconds: 60,
        }
    }
}

/// Issue collector (via `gh`), feeding the cross-repo board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct IssueCollector {
    pub enabled: bool,
    pub refresh_minutes: u64,
}

impl Default for IssueCollector {
    fn default() -> Self {
        Self { enabled: true, refresh_minutes: 5 }
    }
}

/// Top-level user settings, mirroring `UserSettingsSchema` in the TS CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct UserSettings {
    /// Preferred editor command (e.g. `code`).
    pub preferred_editor: String,

    pub journal_settings: JournalSettings,

    pub agentboard: AgentboardSettings,

    pub collectors: CollectorsSettings,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            preferred_editor: "code".to_string(),
            journal_settings: JournalSettings::default(),
            agentboard: AgentboardSettings::default(),
            collectors: CollectorsSettings::default(),
        }
    }
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(Error::NoHomeDir)
}

/// Home directory as a string, falling back to `.` if it can't be resolved.
/// Used only for defaults, which must be infallible.
fn home_dir_string() -> String {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).to_string_lossy().to_string()
}

fn default_template_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join(TOOL_NAME)
        .join("templates")
        .to_string_lossy()
        .to_string()
}

/// Environment variable that overrides slot-scope detection.
///
/// - set to a non-empty value: force that scope name (still sanitized).
/// - set to empty (`TT_STATE_SCOPE=`): force *unscoped* — the shared defaults.
/// - unset: auto-detect from the current working directory (see [`state_scope`]).
pub const STATE_SCOPE_ENV: &str = "TT_STATE_SCOPE";

/// How the active scope was determined. The distinction matters because
/// shared stores (settings, tracked repos) ignore an *auto-detected* scope —
/// they describe the user/machine, not one checkout — but a *forced* scope
/// isolates everything, so tests and quarantined slots never touch the real
/// shared files.
enum Scope {
    /// No scope — the shared daily-driver defaults.
    None,
    /// Auto-detected from the cwd (a checkout of this repo): instance state
    /// is scoped, shared stores stay shared.
    Auto(String),
    /// Forced via [`STATE_SCOPE_ENV`]: full isolation, shared stores included.
    Forced(String),
}

fn detect_scope() -> Scope {
    match std::env::var(STATE_SCOPE_ENV) {
        Ok(v) if !v.trim().is_empty() => Scope::Forced(sanitize_scope(&v)),
        Ok(_) => Scope::None,
        Err(_) => match std::env::current_dir().ok().as_deref().and_then(slot_scope_from_dir) {
            Some(s) => Scope::Auto(s),
            None => Scope::None,
        },
    }
}

/// The active slot scope for the running process, or `None` for the shared
/// (unscoped) defaults.
///
/// Resolution order:
/// 1. [`STATE_SCOPE_ENV`] if set (empty forces unscoped, non-empty forces that name).
/// 2. Otherwise walk up from the current working directory to a checkout of *this*
///    repo and use its root directory name (e.g. `towles-tool-rs-primary`), repo-
///    qualified for `slots/<name>` checkouts (e.g. `towles-tool-rs-slot-migrate`).
///
/// A checkout is recognised by a `crates/tt-config` directory at its root — a
/// marker unique to this workspace — so an installed `ttr` run from an arbitrary
/// project directory stays unscoped and keeps sharing the daily-driver config.
/// The dir-name rule mirrors `scripts/slot-port.mjs` and the app's `slot_label`.
pub fn state_scope() -> Option<String> {
    match detect_scope() {
        Scope::None => None,
        Scope::Auto(s) | Scope::Forced(s) => Some(s),
    }
}

/// Derive a slot scope from `dir`: the nearest ancestor that is a checkout of
/// this repo (contains a `crates/tt-config` directory), or `None`. Split out
/// from [`state_scope`] so it can be unit-tested against temp dirs without
/// touching the real cwd/env.
///
/// Branch-named slots live under a shared `slots/` dir (`<root>/slots/<name>`,
/// see the tt-slots crate), so a bare dir name like `slot-migrate` is not
/// unique across repos/roots — those scopes are qualified with the repo name
/// taken from the sibling `<repo>-primary` checkout (falling back to the
/// root's dir name).
pub fn slot_scope_from_dir(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors() {
        if !ancestor.join("crates").join("tt-config").is_dir() {
            continue;
        }
        let name = ancestor.file_name().and_then(|n| n.to_str())?;
        let in_slots_dir = ancestor
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "slots");
        if !in_slots_dir {
            return Some(sanitize_scope(name));
        }
        let root = ancestor.parent().and_then(Path::parent);
        let repo = root.and_then(primary_repo_name).or_else(|| {
            root.and_then(|r| r.file_name()).and_then(|n| n.to_str()).map(str::to_string)
        });
        return Some(sanitize_scope(&match repo {
            Some(repo) => format!("{repo}-{name}"),
            None => name.to_string(),
        }));
    }
    None
}

/// The repo name from a `<repo>-primary` checkout directly under `root`.
fn primary_repo_name(root: &Path) -> Option<String> {
    for entry in std::fs::read_dir(root).ok()?.filter_map(|e| e.ok()) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some(repo) = name.strip_suffix("-primary")
            && !repo.is_empty()
            && entry.path().join(".git").exists()
        {
            return Some(repo.to_string());
        }
    }
    None
}

/// Reduce a scope name to a single safe path segment: anything outside
/// `[A-Za-z0-9._-]` becomes `-`. Slot dir names already qualify; this only
/// guards a hand-set `TT_STATE_SCOPE`.
fn sanitize_scope(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect()
}

/// The one nesting rule, pure for tests: instance state nests under any
/// scope; shared stores nest only under a forced scope.
fn nest(base: PathBuf, scope: &Scope, instance: bool) -> PathBuf {
    match scope {
        Scope::None => base,
        Scope::Auto(s) if instance => base.join("slots").join(s),
        Scope::Auto(_) => base,
        Scope::Forced(s) => base.join("slots").join(s),
    }
}

/// Nest `base` under `slots/<scope>` for *instance* state (sessions, windows,
/// tt.db — anything one running checkout owns): any scope applies.
fn instance_under(base: PathBuf) -> PathBuf {
    nest(base, &detect_scope(), true)
}

/// Nest `base` under `slots/<scope>` for *shared* stores (settings, tracked
/// repos — they describe the user/machine, so every checkout reads one copy):
/// only a forced [`STATE_SCOPE_ENV`] scopes them.
fn shared_under(base: PathBuf) -> PathBuf {
    nest(base, &detect_scope(), false)
}

/// Config directory for shared stores (the settings file). Shared across
/// checkouts: `~/.config/towles-tool` (matches the TS CLI on every platform);
/// a forced `TT_STATE_SCOPE` nests it under `slots/<scope>`.
pub fn config_dir() -> Result<PathBuf> {
    Ok(shared_under(home_dir()?.join(".config").join(TOOL_NAME)))
}

/// Full path to the settings file:
/// `<config_dir>/towles-tool.settings.json`.
pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(format!("{TOOL_NAME}.settings.json")))
}

/// Data directory, instance-scoped (holds tt.db). Unscoped:
/// `<data_dir>/towles-tool` (e.g. `~/.local/share/towles-tool`). In a slot
/// checkout: `…/towles-tool/slots/<scope>` — a branch's schema experiments
/// must not touch the daily driver's database.
pub fn data_dir() -> Result<PathBuf> {
    Ok(instance_under(dirs::data_dir().ok_or(Error::NoDataDir)?.join(TOOL_NAME)))
}

/// Full path to the data-hub SQLite store: `<data_dir>/tt.db`.
pub fn store_db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("tt.db"))
}

/// Agentboard *instance* persistence directory (sessions.json, windows.json,
/// collapse.json, … — one running app's state): scoped in a slot checkout.
pub fn agentboard_dir() -> Result<PathBuf> {
    Ok(instance_under(home_dir()?.join(".config").join(TOOL_NAME)).join("agentboard"))
}

/// Agentboard *shared* persistence directory (repos.json — which repos exist
/// on this machine is the same fact from every checkout).
pub fn agentboard_shared_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("agentboard"))
}

/// The base directories under which per-checkout *instance* state nests as
/// `slots/<scope>/…` (see [`state_scope`]): the data base (tt.db) and the
/// instance-config base (agentboard sessions/windows/collapse). Cleanup tools
/// use this to reach state belonging to scopes *other than* the running
/// process's — [`data_dir`]/[`agentboard_dir`] only ever resolve the current
/// scope. Deliberately ignores an auto-detected scope (the machine's state is
/// the target even when cleanup runs from a slot checkout), but a *forced*
/// [`STATE_SCOPE_ENV`] nests both bases like every other path, so tests and
/// quarantined slots never see or touch the real state tree.
pub struct InstanceStateBases {
    /// e.g. `~/.local/share/towles-tool` — holds `tt.db` and `slots/<scope>/tt.db`.
    pub data: PathBuf,
    /// e.g. `~/.config/towles-tool` — holds `agentboard/` and
    /// `slots/<scope>/agentboard/`.
    pub config: PathBuf,
}

impl InstanceStateBases {
    /// The `slots/` directories whose children are per-scope state dirs.
    pub fn scope_parents(&self) -> [PathBuf; 2] {
        [self.data.join("slots"), self.config.join("slots")]
    }

    /// The agentboard instance dir for `scope` (`None` = the unscoped store
    /// the installed daily driver writes).
    pub fn agentboard_dir(&self, scope: Option<&str>) -> PathBuf {
        match scope {
            None => self.config.join("agentboard"),
            Some(s) => self.config.join("slots").join(s).join("agentboard"),
        }
    }
}

/// Resolve [`InstanceStateBases`] for this machine (or, under a forced
/// [`STATE_SCOPE_ENV`], for that sandboxed scope).
pub fn instance_state_bases() -> Result<InstanceStateBases> {
    let data = dirs::data_dir().ok_or(Error::NoDataDir)?.join(TOOL_NAME);
    let config = home_dir()?.join(".config").join(TOOL_NAME);
    match detect_scope() {
        Scope::Forced(s) => Ok(InstanceStateBases {
            data: data.join("slots").join(&s),
            config: config.join("slots").join(&s),
        }),
        Scope::None | Scope::Auto(_) => Ok(InstanceStateBases { data, config }),
    }
}

/// Infallible [`agentboard_dir`], falling back to `./agentboard` when the home
/// directory can't be resolved. For callers that build default paths without a
/// `Result` (they historically fell back to `.`).
pub fn agentboard_dir_lossy() -> PathBuf {
    agentboard_dir().unwrap_or_else(|_| PathBuf::from(".").join("agentboard"))
}

/// Infallible [`agentboard_shared_dir`] (see [`agentboard_dir_lossy`]).
pub fn agentboard_shared_dir_lossy() -> PathBuf {
    agentboard_shared_dir().unwrap_or_else(|_| PathBuf::from(".").join("agentboard"))
}

/// The instance-state directories owned by `scope` — the config-side
/// `…/towles-tool/slots/<scope>` (agentboard sessions/windows/collapse) and
/// the data-side one (tt.db) — so `ttr slot rm` can delete a removed slot's
/// leftover state. This targets *another* checkout's scope, so the ambient
/// auto-detected scope is deliberately ignored (running the command from
/// inside a slot must not nest the target under the runner's own scope);
/// a forced [`STATE_SCOPE_ENV`] still nests, keeping tests fully isolated.
pub fn instance_state_dirs_for_scope(scope: &str) -> Vec<PathBuf> {
    let scope = sanitize_scope(scope);
    if scope.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(home) = home_dir() {
        out.push(shared_under(home.join(".config").join(TOOL_NAME)).join("slots").join(&scope));
    }
    if let Some(data) = dirs::data_dir() {
        out.push(shared_under(data.join(TOOL_NAME)).join("slots").join(&scope));
    }
    out
}

/// Load settings from the standard location, creating defaults if the file is missing.
pub fn load() -> Result<UserSettings> {
    load_from(&config_path()?)
}

/// Load settings from an explicit path, creating defaults if the file is missing.
///
/// Accepting a path keeps this testable without touching the real home directory.
pub fn load_from(path: &Path) -> Result<UserSettings> {
    if !path.exists() {
        let settings = UserSettings::default();
        save_to(path, &settings)?;
        return Ok(settings);
    }
    let raw = std::fs::read_to_string(path)?;
    let settings = serde_json::from_str(&raw)?;
    Ok(settings)
}

/// Save settings to the standard location, **serializing only the fields this
/// model captures**. Any keys the shared TypeScript CLI owns that this model
/// does not model are dropped — for writes to the shared settings file prefer
/// [`save_merge`], which preserves them.
pub fn save(settings: &UserSettings) -> Result<()> {
    save_to(&config_path()?, settings)
}

/// Save settings to an explicit path, creating parent directories as needed.
///
/// Writes only the modeled fields, so any unmodeled keys already on disk (e.g.
/// keys the shared TypeScript CLI owns) are dropped. For the shared settings
/// file use [`save_merge_to`] instead.
pub fn save_to(path: &Path, settings: &UserSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Save to the standard location while preserving unknown keys already on disk.
pub fn save_merge(settings: &UserSettings) -> Result<()> {
    save_merge_to(&config_path()?, settings)
}

/// Save `settings` to `path`, **preserving any keys already in the file that this
/// model doesn't capture** (keys the shared TypeScript CLI may own). Known fields
/// win; unknown fields on disk survive. Use this for writes to the shared settings
/// file — unlike [`save_to`], which serializes only the modeled fields and would
/// silently drop anything the other tool wrote.
pub fn save_merge_to(path: &Path, settings: &UserSettings) -> Result<()> {
    let mut base = if path.exists() {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(path)?)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    merge_json(&mut base, &serde_json::to_value(settings)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&base)?)?;
    Ok(())
}

/// Deep-merge `incoming` into `base`: objects merge key-by-key (recursively);
/// every other value (scalars, arrays) is replaced wholesale by `incoming`.
fn merge_json(base: &mut serde_json::Value, incoming: &serde_json::Value) {
    match (base, incoming) {
        (serde_json::Value::Object(b), serde_json::Value::Object(i)) => {
            for (k, v) in i {
                merge_json(b.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, i) => *b = i.clone(),
    }
}

/// JSON Schema for the settings file, as a `serde_json::Value`.
pub fn json_schema() -> serde_json::Value {
    let schema = schemars::schema_for!(UserSettings);
    serde_json::to_value(schema).expect("settings JSON schema should serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_match_ts_cli() {
        let settings = UserSettings::default();
        assert_eq!(settings.preferred_editor, "code");
        assert!(settings.journal_settings.daily_path_template.contains("daily-notes"));
        assert!(settings.agentboard.mux.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");

        let settings = UserSettings { preferred_editor: "nvim".to_string(), ..Default::default() };
        save_to(&path, &settings).unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn load_from_missing_creates_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("towles-tool.settings.json");

        assert!(!path.exists());
        let loaded = load_from(&path).unwrap();
        assert!(path.exists());
        assert_eq!(loaded, UserSettings::default());
    }

    #[test]
    fn tolerates_unknown_fields_and_fills_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        // Unknown top-level key + a partial journalSettings object.
        std::fs::write(
            &path,
            r#"{"preferredEditor":"vim","futureFlag":true,"journalSettings":{"baseFolder":"/tmp/j"}}"#,
        )
        .unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.preferred_editor, "vim");
        assert_eq!(loaded.journal_settings.base_folder, "/tmp/j");
        // Missing journal fields fall back to defaults.
        assert!(loaded.journal_settings.daily_path_template.contains("daily-notes"));
    }

    #[test]
    fn save_merge_preserves_unknown_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        // A file with keys this model doesn't capture — top-level and nested.
        std::fs::write(
            &path,
            r#"{"preferredEditor":"vim","futureFlag":true,"journalSettings":{"baseFolder":"/old","tsOnly":42}}"#,
        )
        .unwrap();

        let mut settings = load_from(&path).unwrap();
        settings.preferred_editor = "code".to_string();
        settings.journal_settings.base_folder = "/new".to_string();
        save_merge_to(&path, &settings).unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Known edits win.
        assert_eq!(raw["preferredEditor"], "code");
        assert_eq!(raw["journalSettings"]["baseFolder"], "/new");
        // Unknown keys survive, at both levels.
        assert_eq!(raw["futureFlag"], true);
        assert_eq!(raw["journalSettings"]["tsOnly"], 42);
    }

    #[test]
    fn serializes_camel_case() {
        let json = serde_json::to_string(&UserSettings::default()).unwrap();
        assert!(json.contains("\"preferredEditor\""));
        assert!(json.contains("\"journalSettings\""));
        assert!(json.contains("\"dailyPathTemplate\""));
        assert!(json.contains("\"collectors\""));
        assert!(json.contains("\"refreshMinutes\""));
    }

    #[test]
    fn collectors_defaults() {
        let c = UserSettings::default().collectors;
        // Off by default: the calendar collector burns tokens (`claude -p`).
        assert!(!c.calendar.enabled);
        assert_eq!(c.calendar.provider, "google");
        assert_eq!(c.calendar.refresh_minutes, 15);
        // Quiet hours default to 8:00–18:00 local, Mon–Fri, and are on.
        assert!(c.calendar.quiet_hours.enabled);
        assert_eq!(c.calendar.quiet_hours.start_hour, 8);
        assert_eq!(c.calendar.quiet_hours.end_hour, 18);
        assert_eq!(c.calendar.quiet_hours.weekdays, vec![0, 1, 2, 3, 4]);
        assert!(c.prs.enabled);
        assert_eq!(c.prs.refresh_seconds, 120);
        assert!(c.issues.enabled);
        assert_eq!(c.issues.refresh_minutes, 5);
        // Off by default: the Slack watcher needs a user token first.
        assert!(!c.slack.enabled);
        assert!(c.slack.token.is_empty());
        // Socket Mode is opt-in on top of the poll; no app token by default.
        assert!(c.slack.app_token.is_empty());
        assert_eq!(c.slack.refresh_seconds, 60);
    }

    #[test]
    fn notify_needs_you_defaults_unset_and_on() {
        let s = UserSettings::default();
        // Unset until the user changes it, so the shared file stays clean…
        assert!(s.agentboard.notify_needs_you.is_none());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("notifyNeedsYou"));
        // …and unset means ON.
        assert!(s.agentboard.notify_needs_you.unwrap_or(DEFAULT_NOTIFY_NEEDS_YOU));
    }

    #[test]
    fn copy_on_select_defaults_unset_and_on() {
        let s = UserSettings::default();
        // Unset until the user changes it, so the shared file stays clean…
        assert!(s.agentboard.copy_on_select.is_none());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("copyOnSelect"));
        // …and unset means ON.
        assert!(s.agentboard.copy_on_select.unwrap_or(DEFAULT_COPY_ON_SELECT));
    }

    #[test]
    fn shortcuts_work_in_terminal_defaults_unset_and_on() {
        let s = UserSettings::default();
        // Unset until the user changes it, so the shared file stays clean…
        assert!(s.agentboard.shortcuts_work_in_terminal.is_none());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("shortcutsWorkInTerminal"));
        // …and unset means ON.
        assert!(
            s.agentboard.shortcuts_work_in_terminal.unwrap_or(DEFAULT_SHORTCUTS_WORK_IN_TERMINAL)
        );
    }

    #[test]
    fn terminal_font_size_defaults_unset_and_thirteen() {
        let s = UserSettings::default();
        // Unset until the user changes it, so the shared file stays clean…
        assert!(s.agentboard.terminal_font_size.is_none());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("terminalFontSize"));
        // …and unset means the 13px default.
        assert_eq!(s.agentboard.terminal_font_size.unwrap_or(DEFAULT_TERMINAL_FONT_SIZE), 13);
    }

    #[test]
    fn terminal_font_size_tolerates_unknown_and_roundtrips() {
        // An unknown sibling key (written by the TS CLI or a newer app) must
        // not break deserialization, and a set value round-trips as camelCase.
        let json = r#"{"agentboard":{"terminalFontSize":17,"someFutureKey":true}}"#;
        let s: UserSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.agentboard.terminal_font_size, Some(17));
        let out = serde_json::to_string(&s).unwrap();
        assert!(out.contains("\"terminalFontSize\":17"));
    }

    #[test]
    fn json_schema_has_properties() {
        let schema = json_schema();
        let props = &schema["properties"];
        assert!(props.get("preferredEditor").is_some());
        assert!(props.get("journalSettings").is_some());
        assert!(props.get("agentboard").is_some());
    }

    /// Drift guard: because the schema is derived from the serde structs, a
    /// struct whose field names diverge from the shared file's `camelCase`
    /// convention (e.g. a new struct missing `#[serde(rename_all = "camelCase")]`,
    /// which would serialize `snake_case`) shows up as a `snake_case` property
    /// name here. The TS CLI reads/writes this same file expecting `camelCase`,
    /// so any underscore in a property name is a break waiting to happen.
    #[test]
    fn json_schema_property_names_are_camel_case() {
        fn walk(node: &serde_json::Value, offenders: &mut Vec<String>) {
            match node {
                serde_json::Value::Object(map) => {
                    if let Some(serde_json::Value::Object(props)) = map.get("properties") {
                        for name in props.keys() {
                            if name.contains('_') {
                                offenders.push(name.clone());
                            }
                        }
                    }
                    for value in map.values() {
                        walk(value, offenders);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, offenders);
                    }
                }
                _ => {}
            }
        }

        let schema = json_schema();
        let mut offenders = Vec::new();
        walk(&schema, &mut offenders);
        assert!(
            offenders.is_empty(),
            "schema has non-camelCase property names (would break the shared TS-CLI file): {offenders:?}",
        );
    }

    /// Drift guard: the schema must reach the nested collector tree, not just the
    /// top level — a real read/write surface the TS CLI shares. Spot-check the
    /// generated definitions and their `camelCase` fields.
    #[test]
    fn json_schema_covers_nested_collectors() {
        let schema = json_schema();
        let defs = &schema["definitions"];
        assert!(defs.get("CollectorsSettings").is_some());
        let cal = &defs["CalendarCollector"]["properties"];
        assert!(cal.get("refreshMinutes").is_some());
        assert!(cal.get("provider").is_some());
        let prs = &defs["PrCollector"]["properties"];
        assert!(prs.get("refreshSeconds").is_some());
    }

    /// Pins the shared-file write contract honestly: [`save_to`] serializes only
    /// modeled fields and therefore **drops** unmodeled TS-owned keys, whereas
    /// [`save_merge_to`] preserves them (see `save_merge_preserves_unknown_keys`).
    /// If a future refactor makes `save_to` merge, this test forces the change to
    /// be deliberate.
    #[test]
    fn save_to_drops_unknown_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        std::fs::write(
            &path,
            r#"{"preferredEditor":"vim","futureFlag":true,"journalSettings":{"baseFolder":"/old","tsOnly":42}}"#,
        )
        .unwrap();

        let settings = load_from(&path).unwrap();
        save_to(&path, &settings).unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Modeled fields survive.
        assert_eq!(raw["preferredEditor"], "vim");
        assert_eq!(raw["journalSettings"]["baseFolder"], "/old");
        // Unmodeled TS-owned keys are dropped — the reason the shared file must
        // be written with `save_merge_to`.
        assert!(raw.get("futureFlag").is_none());
        assert!(raw["journalSettings"].get("tsOnly").is_none());
    }

    /// Serializes the process-global `TT_STATE_SCOPE` mutations so env-touching
    /// tests don't race each other (cargo runs tests on parallel threads).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A temp dir laid out like a slot checkout: `<root>/crates/tt-config`
    /// (plus a nested crate dir to test detection from a subdirectory).
    fn slot_checkout(root_name: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(root_name);
        std::fs::create_dir_all(root.join("crates").join("tt-config")).unwrap();
        std::fs::create_dir_all(root.join("crates").join("tt-store").join("src")).unwrap();
        dir
    }

    #[test]
    fn slot_checkout_dir_derives_scope() {
        let dir = slot_checkout("towles-tool-rs-primary");
        let root = dir.path().join("towles-tool-rs-primary");
        // From the root and from a nested subdir, the scope is the root's name.
        assert_eq!(slot_scope_from_dir(&root), Some("towles-tool-rs-primary".to_string()));
        assert_eq!(
            slot_scope_from_dir(&root.join("crates").join("tt-store").join("src")),
            Some("towles-tool-rs-primary".to_string())
        );
    }

    #[test]
    fn slots_dir_checkout_is_repo_qualified() {
        // <tmp>/towles-tool-rs-primary/.git + <tmp>/slots/slot-migrate/crates/tt-config:
        // the slot's scope carries the repo name so same-named slots of
        // different repos never share state.
        let dir = TempDir::new().unwrap();
        let primary = dir.path().join("towles-tool-rs-primary");
        std::fs::create_dir_all(primary.join(".git")).unwrap();
        let slot = dir.path().join("slots").join("slot-migrate");
        std::fs::create_dir_all(slot.join("crates").join("tt-config")).unwrap();
        assert_eq!(slot_scope_from_dir(&slot), Some("towles-tool-rs-slot-migrate".to_string()));
    }

    #[test]
    fn slots_dir_without_primary_falls_back_to_root_name() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("my-repos");
        let slot = root.join("slots").join("thing");
        std::fs::create_dir_all(slot.join("crates").join("tt-config")).unwrap();
        assert_eq!(slot_scope_from_dir(&slot), Some("my-repos-thing".to_string()));
    }

    #[test]
    fn non_repo_dir_is_unscoped() {
        let dir = TempDir::new().unwrap();
        assert_eq!(slot_scope_from_dir(dir.path()), None);
    }

    #[test]
    fn arbitrary_git_repo_is_unscoped() {
        // A checkout that isn't THIS repo: it has a .git and Cargo.toml but no
        // `crates/tt-config` marker, so it must not be scoped.
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("some-other-project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("crates").join("their-crate")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        assert_eq!(slot_scope_from_dir(&root), None);
    }

    #[test]
    fn sanitize_scope_keeps_slot_names_and_strips_others() {
        assert_eq!(sanitize_scope("towles-tool-rs-slot-2"), "towles-tool-rs-slot-2");
        assert_eq!(sanitize_scope("  weird/name space "), "weird-name-space");
    }

    #[test]
    fn env_override_forces_scope_and_empty_forces_unscoped() {
        let _guard = ENV_LOCK.lock().unwrap();
        let base = PathBuf::from("/home/x/.config/towles-tool");

        // Non-empty → that scope — and a FORCED scope nests shared stores
        // too, so tests and quarantined slots never touch real shared files.
        // SAFETY: guarded by ENV_LOCK; no other threads read env concurrently here.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "my-scope") };
        assert_eq!(state_scope(), Some("my-scope".to_string()));
        assert_eq!(instance_under(base.clone()), base.join("slots").join("my-scope"));
        assert_eq!(shared_under(base.clone()), base.join("slots").join("my-scope"));

        // Empty → forced unscoped, regardless of cwd.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "") };
        assert_eq!(state_scope(), None);
        assert_eq!(instance_under(base.clone()), base);
        assert_eq!(shared_under(base.clone()), base);

        unsafe { std::env::remove_var(STATE_SCOPE_ENV) };
    }

    /// An auto-detected checkout scope splits: instance state nests, shared
    /// stores (settings, repos.json) stay at the machine-wide default. Uses
    /// the pure resolvers with a hand-built Scope since auto-detection reads
    /// the real cwd.
    #[test]
    fn auto_scope_nests_instance_but_not_shared() {
        let base = PathBuf::from("/home/x/.config/towles-tool");
        let auto = Scope::Auto("towles-tool-rs-thing".into());
        assert_eq!(
            nest(base.clone(), &auto, true),
            base.join("slots").join("towles-tool-rs-thing")
        );
        assert_eq!(nest(base.clone(), &auto, false), base);
    }

    #[test]
    fn config_dir_override_wins_via_env_but_scoped_paths_nest() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "slot-9") };
        let cfg = config_dir().unwrap();
        assert!(cfg.ends_with("towles-tool/slots/slot-9"), "got {}", cfg.display());
        assert!(config_path().unwrap().ends_with("slot-9/towles-tool.settings.json"));
        assert!(store_db_path().unwrap().ends_with("towles-tool/slots/slot-9/tt.db"));
        assert!(agentboard_dir().unwrap().ends_with("slots/slot-9/agentboard"));
        unsafe { std::env::remove_var(STATE_SCOPE_ENV) };
    }

    /// The cleanup bases point at the machine-wide state tree even when the
    /// process itself runs auto-scoped from a slot checkout — that tree is
    /// what cleanup sweeps — but a forced scope sandboxes them entirely.
    #[test]
    fn instance_state_bases_ignore_auto_but_honor_forced_scope() {
        let _guard = ENV_LOCK.lock().unwrap();

        // SAFETY: guarded by ENV_LOCK.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "") };
        let bases = instance_state_bases().unwrap();
        assert!(bases.data.ends_with("towles-tool"), "got {}", bases.data.display());
        assert!(bases.config.ends_with(".config/towles-tool"), "got {}", bases.config.display());
        let [data_slots, config_slots] = bases.scope_parents();
        assert!(data_slots.ends_with("towles-tool/slots"));
        assert!(config_slots.ends_with(".config/towles-tool/slots"));
        assert!(bases.agentboard_dir(None).ends_with("towles-tool/agentboard"));
        assert!(
            bases
                .agentboard_dir(Some("repo-thing"))
                .ends_with("towles-tool/slots/repo-thing/agentboard")
        );

        // SAFETY: guarded by ENV_LOCK.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "sandbox") };
        let bases = instance_state_bases().unwrap();
        assert!(bases.data.ends_with("towles-tool/slots/sandbox"));
        assert!(bases.config.ends_with(".config/towles-tool/slots/sandbox"));

        unsafe { std::env::remove_var(STATE_SCOPE_ENV) };
    }

    #[test]
    fn instance_state_dirs_target_the_named_scope_not_the_ambient_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "") };
        let dirs = instance_state_dirs_for_scope("towles-tool-rs-thing");
        assert!(!dirs.is_empty());
        for dir in &dirs {
            assert!(
                dir.ends_with("towles-tool/slots/towles-tool-rs-thing"),
                "got {}",
                dir.display()
            );
        }
        assert!(instance_state_dirs_for_scope("  ").is_empty());

        // A FORCED scope nests the targets too — a test world's slot state
        // lives under the forced nest, never at the real machine paths.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "test-world") };
        for dir in instance_state_dirs_for_scope("towles-tool-rs-thing") {
            assert!(
                dir.ends_with("slots/test-world/slots/towles-tool-rs-thing"),
                "got {}",
                dir.display()
            );
        }
        unsafe { std::env::remove_var(STATE_SCOPE_ENV) };
    }

    #[test]
    fn unscoped_paths_match_historic_defaults() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK.
        unsafe { std::env::set_var(STATE_SCOPE_ENV, "") };
        assert!(config_dir().unwrap().ends_with(".config/towles-tool"));
        assert!(config_path().unwrap().ends_with("towles-tool/towles-tool.settings.json"));
        assert!(store_db_path().unwrap().ends_with("towles-tool/tt.db"));
        assert!(agentboard_dir().unwrap().ends_with("towles-tool/agentboard"));
        unsafe { std::env::remove_var(STATE_SCOPE_ENV) };
    }
}
