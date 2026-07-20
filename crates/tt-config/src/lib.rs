//! Configuration model for the towles-tool CLI, plus the resolver for every
//! mutable state path the CLI and app touch.
//!
//! This mirrors the zod settings schema used by the TypeScript CLI and reads/writes
//! the *same* file (`~/.config/towles-tool/towles-tool.settings.json`). Because the
//! file is shared, the model deliberately tolerates unknown fields and fills missing
//! ones from defaults via `#[serde(default)]` — never `deny_unknown_fields`.
//!
//! Two invariants keep the shared file safe and are enforced by tests: every
//! property name is `camelCase` (matching what the TypeScript CLI
//! reads/writes; guarded by a test-only `schemars` schema derived from these
//! structs), and writes to the shared file go through [`save_merge`] so
//! TS-owned keys survive.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition {
    Left,
    Right,
}

/// Journal path templates and base folders.
///
/// Path template tokens follow Luxon formatting, e.g. `{yyyy}`, `{MM}`, `{dd}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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

    /// Group the Board kanban's tasks into per-repo swimlanes. `None` = the
    /// built-in default (on). Only written once the user changes it, so the
    /// shared settings file stays clean for the TS CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board_group_by_repo: Option<bool>,
}

/// Built-in default for [`AgentboardSettings::compact_recommend_percent`].
pub const DEFAULT_COMPACT_RECOMMEND_PERCENT: u8 = 30;

/// Built-in default for [`AgentboardSettings::notify_needs_you`]: notifications on.
pub const DEFAULT_NOTIFY_NEEDS_YOU: bool = true;

/// Built-in default for [`AgentboardSettings::notify_meeting_start`]: on.
pub const DEFAULT_NOTIFY_MEETING_START: bool = true;

/// Built-in default for [`AgentboardSettings::notify_review_requested`]: on.
pub const DEFAULT_NOTIFY_REVIEW_REQUESTED: bool = true;

/// Built-in default for [`AgentboardSettings::notify_stale_collector`]: on.
pub const DEFAULT_NOTIFY_STALE_COLLECTOR: bool = true;

/// Built-in default for [`AgentboardSettings::notify_checks_failed`]: on.
pub const DEFAULT_NOTIFY_CHECKS_FAILED: bool = true;

/// Data-hub collector settings (the Rust CLI/app's tt.db collectors; the TS CLI
/// ignores this block). Each collector is configured independently — enable
/// flag, refresh cadence, and (for the claude-backed calendar) which MCP
/// provider to drive so the same app works at home (Google) and work (Outlook).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct CollectorsSettings {
    pub calendar: CalendarCollector,
    pub prs: PrCollector,
    pub issues: IssueCollector,
    pub slack: SlackDmCollector,
}

/// Calendar collector: shells out to `claude -p` per configured source, so it
/// costs tokens — disabled by default; opt in per machine.
///
/// The collector's only purpose is **focus protection** — knowing when the next
/// meeting is and how much focus time is left — not calendar management. Each
/// [`CalendarSource`] is pulled and stored independently so a personal and a
/// work calendar can be merged into one timeline without clobbering each other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarCollector {
    pub enabled: bool,
    pub refresh_minutes: u64,
    /// Working-hours window that further gates *when* the (already token-costing)
    /// calendar collector may run. Skips nights and weekends when there's no
    /// meeting to count down to. Only narrows an already-`enabled` collector;
    /// disable it (`enabled = false`) to restore 24/7 running.
    pub quiet_hours: CalendarQuietHours,
    /// Calendars to pull, each with its own prompt. Every enabled source is run
    /// separately and written under its own `id`, so adding a second calendar
    /// never displaces the first.
    pub sources: Vec<CalendarSource>,
}

impl Default for CalendarCollector {
    fn default() -> Self {
        Self {
            enabled: false,
            refresh_minutes: 15,
            quiet_hours: CalendarQuietHours::default(),
            sources: CalendarSource::defaults(),
        }
    }
}

/// One calendar the collector pulls, and the prompt it uses to do so.
///
/// The prompt is user-editable on purpose. The built-in defaults ask for a
/// Google/Outlook MCP, but those MCP servers aren't necessarily configured on a
/// given machine — pointing a source at whatever actually works there (a CLI
/// like `gws calendar events list`, a different MCP, a script) is the intended
/// escape hatch, and the reason the prompt lives in settings rather than in a
/// compiled-in constant.
///
/// The JSON contract the prompt must produce is identical across sources so
/// `tt_collect`'s lenient extraction and [`tt_store::EventInput`] stay the same:
/// an array of `{externalId, title, startTs, endTs, attendees, location,
/// joinUrl}`, or `[]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarSource {
    /// Stable identifier, also written to the store's `events.source` column so
    /// a re-pull replaces only this calendar's rows. Keep it short and stable
    /// (`"google"`, `"outlook"`) — changing it orphans previously stored rows.
    pub id: String,
    /// Human label for the settings UI and event provenance.
    pub label: String,
    pub enabled: bool,
    /// The `claude -p` prompt used to list today's events for this calendar.
    pub prompt: String,
}

impl CalendarSource {
    /// The built-in sources: personal Google (on) and work Outlook (off), each
    /// carrying the prompt that used to be a compiled-in constant.
    ///
    /// **These are defaults, not a migration.** The retired `provider` key is
    /// an unknown field now, so a settings file that still carries
    /// `"provider": "outlook"` and no `sources` gets this list — Google on,
    /// Outlook off — and starts pulling the *other* calendar. That is the
    /// hard-cutover cost, and it is a one-line fix in Settings → Collectors →
    /// Calendar (or in the file), but it is silent: the collector succeeds
    /// against a calendar the user didn't ask for. A machine configured for
    /// Google is unaffected.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self {
                id: "google".to_string(),
                label: "Google (personal)".to_string(),
                enabled: true,
                prompt: DEFAULT_CALENDAR_PROMPT_GOOGLE.to_string(),
            },
            Self {
                id: "outlook".to_string(),
                label: "Outlook (work)".to_string(),
                enabled: false,
                prompt: DEFAULT_CALENDAR_PROMPT_OUTLOOK.to_string(),
            },
        ]
    }
}

/// Default prompt for the personal Google calendar.
///
/// The JSON contract in the second half is what `tt_store::EventInput` parses —
/// keep it in sync with [`DEFAULT_CALENDAR_PROMPT_OUTLOOK`] when editing.
///
/// **Times are asked for as RFC 3339, never epoch milliseconds.** That is a
/// correctness choice before it is a readability one: computing a 13-digit
/// epoch value is arithmetic a model cannot check, and a wrong one is
/// indistinguishable from a right one until the countdown is hours off. An
/// offset-bearing timestamp is something a calendar reports verbatim, and a
/// malformed one is rejected at parse time instead of silently stored.
pub const DEFAULT_CALENDAR_PROMPT_GOOGLE: &str = "\
Using the Google Calendar MCP, list the events on my primary calendar for today \
only, in my local timezone. Respond with ONLY a JSON array, no prose, no code \
fences. Each element: {\"externalId\": string (stable event id), \"title\": \
string, \"start\": string (RFC 3339 with UTC offset, e.g. \
\"2026-07-20T15:00:00-05:00\"), \"end\": string (same format), \"attendees\": \
array of attendee display-name strings, \"location\": string, \"joinUrl\": \
string}. Report each time exactly as the calendar gives it, keeping its UTC \
offset — do not convert to UTC and do not compute epoch numbers. Skip all-day \
events and events I have declined. Omit any field whose value is null or \
unknown. If there are no events, respond with [].";

/// Default prompt for the work Outlook / Microsoft 365 calendar.
pub const DEFAULT_CALENDAR_PROMPT_OUTLOOK: &str = "\
Using the Outlook (Microsoft 365) MCP, list the events on my default calendar \
for today only, in my local timezone. Respond with ONLY a JSON array, no prose, \
no code fences. Each element: {\"externalId\": string (stable event id), \
\"title\": string, \"start\": string (RFC 3339 with UTC offset, e.g. \
\"2026-07-20T15:00:00-05:00\"), \"end\": string (same format), \"attendees\": \
array of attendee display-name strings, \"location\": string, \"joinUrl\": \
string}. Report each time exactly as the calendar gives it, keeping its UTC \
offset — do not convert to UTC and do not compute epoch numbers. Skip all-day \
events and events I have declined. Omit any field whose value is null or \
unknown. If there are no events, respond with [].";

/// Working-hours gate for the calendar collector: a daily time window plus a
/// weekday mask, evaluated in local time. When `enabled`, the collector runs
/// only inside `[startHour:00, endHour:00)` on a listed weekday; outside it
/// (nights, weekends) the token-costing `claude -p` run is skipped. Set
/// `enabled = false` to run on the plain refresh cadence around the clock.
///
/// `weekdays` are day-of-week numbers with **0 = Monday … 6 = Sunday** (matching
/// chrono's `num_days_from_monday`); the default is Mon–Fri.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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

/// `tt-mcp`'s HTTP transport — Rust-only. (Beware: the legacy TS CLI does not
/// merely ignore this block — its `loadSettings` strips keys its zod schema
/// doesn't model and rewrites the file, so any legacy-CLI run reverts this to
/// its default port.)
///
/// The MCP server is served over loopback HTTP by the desktop app, not by a
/// per-session stdio subprocess. There is **no bearer token**: a token only
/// ever defended against browser-originated requests (any process running as
/// this user can read the token straight out of this file, so it bought
/// nothing against local malware), and it is replaced by the two mitigations
/// the MCP spec recommends for local HTTP servers — rejecting any request
/// carrying an `Origin` header, and requiring `Content-Type: application/json`
/// so a page cannot dodge a CORS preflight. Those live in
/// `crates-tauri/tt-app`; see `crates/tt-mcp/src/lib.rs`'s module doc-comment
/// for the trust boundary they enforce.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct McpSettings {
    /// Loopback TCP port the app serves MCP on.
    ///
    /// A **fixed default** rather than a `${tt:port}` pool claim, and that is
    /// deliberate: the repo's no-hardcoded-ports rule exists because parallel
    /// worktree slots collide over shared resources, but this server is a
    /// machine-wide singleton acquired bind-or-skip — exactly one process ever
    /// holds it, so there is nothing to collide with. A fixed port is also what
    /// lets the `towles-tool-app` plugin ship a static, checked-in `.mcp.json`.
    /// Override here only if something else on the machine wants this port.
    pub port: u16,
}

/// Default loopback port for the app's MCP server. See [`McpSettings::port`].
pub const DEFAULT_MCP_PORT: u16 = 8787;

impl Default for McpSettings {
    fn default() -> Self {
        Self { port: DEFAULT_MCP_PORT }
    }
}

/// Top-level user settings, mirroring `UserSettingsSchema` in the TS CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct UserSettings {
    /// Preferred editor command (e.g. `code`).
    pub preferred_editor: String,

    pub journal_settings: JournalSettings,

    pub agentboard: AgentboardSettings,

    pub collectors: CollectorsSettings,

    /// Lenient on purpose: the docs invite hand-editing this block, and a slip
    /// there (`"mcp": null`, a port as the string `"8787"`) must not fail the
    /// whole settings file — every command loads it, so that would brick the
    /// app, doctor, journal, and collect at once. A malformed block instead
    /// falls back to the default port and the rest of the file keeps working.
    #[serde(default, deserialize_with = "lenient_mcp")]
    pub mcp: McpSettings,
}

/// Deserialize [`McpSettings`] but degrade any shape other than the documented
/// object form to the default — see the field's doc comment on
/// `UserSettings::mcp` for why. Non-object shapes are rejected outright rather
/// than fed to serde: a struct happily deserializes from a JSON *array*
/// positionally (`"mcp": [9999]` would set the port), and the transport config
/// must not be settable through an undocumented shape.
fn lenient_mcp<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<McpSettings, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Object(_) => Ok(serde_json::from_value(value).unwrap_or_default()),
        _ => Ok(McpSettings::default()),
    }
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            preferred_editor: "code".to_string(),
            journal_settings: JournalSettings::default(),
            agentboard: AgentboardSettings::default(),
            collectors: CollectorsSettings::default(),
            mcp: McpSettings::default(),
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
/// marker unique to this workspace — so an installed `tt` run from an arbitrary
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
/// Branch-named slots nest inside their main checkout at
/// `<repo>/.claude/worktrees/<name>` (see the tt-slots crate), so a bare dir
/// name like `slot-migrate` is not unique across repos — those scopes are
/// qualified with the main checkout's dir name (`<repo>-<name>`). The main
/// checkout itself scopes by its own dir name.
pub fn slot_scope_from_dir(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors() {
        if !ancestor.join("crates").join("tt-config").is_dir() {
            continue;
        }
        let name = ancestor.file_name().and_then(|n| n.to_str())?;
        // `<repo>/.claude/worktrees/<name>` → qualify with the repo dir name.
        let main = ancestor
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == "worktrees"))
            .and_then(Path::parent)
            .filter(|p| p.file_name().is_some_and(|n| n == ".claude"))
            .and_then(Path::parent);
        return Some(sanitize_scope(
            &match main.and_then(|m| m.file_name()).and_then(|n| n.to_str()) {
                Some(repo) => format!("{repo}-{name}"),
                None => name.to_string(),
            },
        ));
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
fn config_dir() -> Result<PathBuf> {
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
fn data_dir() -> Result<PathBuf> {
    Ok(instance_under(dirs::data_dir().ok_or(Error::NoDataDir)?.join(TOOL_NAME)))
}

/// Full path to the data-hub SQLite store: `<data_dir>/tt.db`.
pub fn store_db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("tt.db"))
}

/// Directory watched by the app's scheduler for an eager collector nudge: a
/// `prs` or `issues` file touched inside it triggers an immediate collect of
/// that target instead of waiting for the normal poll cadence. Instance-scoped
/// like `data_dir()` so a nudge in one worktree slot only wakes that slot's
/// own running app. Kept as its own subdirectory rather than nested directly
/// under `data_dir()` so a directory-watch on it isn't spammed by tt.db's own
/// WAL/SHM churn.
pub fn nudge_dir_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("nudge"))
}

/// Directory the telemetry event log streams to: `<data_dir>/telemetry`.
/// Instance-scoped like `data_dir()`, which is the point — each worktree slot
/// writes its own event log, so "which slot spawned these commands?" is
/// answerable from the path alone rather than from a field every writer has to
/// remember to stamp. Its own subdirectory so log rotation never walks tt.db.
pub fn telemetry_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("telemetry"))
}

/// Staging directory for images pasted into the app (today: the new-slot
/// form). The bytes have to become a file somewhere before a path to them can
/// go into a Claude prompt, and that somewhere is deliberately *not* the
/// repo — Claude Code reads an absolute path outside its workspace without
/// prompting, so there's nothing to gain by putting user content inside a
/// checkout and a `.gitignore` to maintain if we do.
///
/// Deliberately the OS temp dir (`/tmp` on Linux), not `data_dir()`: a pasted
/// screenshot is throwaway staging, not state worth keeping across a reboot,
/// and `data_dir()`'s per-checkout `slots/<scope>` nesting exists to isolate
/// state a checkout *owns* (tt.db, sessions) — a paste doesn't need that,
/// and every extra layer is a directory the caller's age-based prune has to
/// walk. The OS already reclaims `/tmp` on its own schedule; that prune is a
/// backstop, not the primary cleanup.
pub fn pasted_images_dir() -> PathBuf {
    std::env::temp_dir().join(TOOL_NAME).join("pasted-images")
}

/// Directory for single-instance PID lock files (see `InstanceLock` in
/// `tt-app`). Deliberately the OS temp dir, not `config_dir()`: a lock file
/// only means anything while the process that created it is still running —
/// it carries no durable state worth keeping across a reboot, and doesn't
/// belong next to settings a user might back up or sync. Unscoped like
/// `config_dir()` (not nested under `slots/<scope>`) since some holders
/// (e.g. `"slack-socket"`) are intentionally shared across every worktree
/// slot on the machine; per-checkout holders instead vary the lock *name*
/// (e.g. `"app-<identifier>"`).
pub fn locks_dir() -> PathBuf {
    std::env::temp_dir().join(TOOL_NAME).join("locks")
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
/// the data-side one (tt.db) — so `tt slot rm` can delete a removed slot's
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

/// Save settings to an explicit path, creating parent directories as needed.
///
/// Writes only the modeled fields, so any unmodeled keys already on disk (e.g.
/// keys the shared TypeScript CLI owns) are dropped. For the shared settings
/// file use [`save_merge_to`] instead.
fn save_to(path: &Path, settings: &UserSettings) -> Result<()> {
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
fn save_merge_to(path: &Path, settings: &UserSettings) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// JSON Schema derived from the settings structs (test-only: the schema is
    /// no shipped API, but it's the drift guard that catches a struct whose
    /// field names diverge from the shared file's `camelCase` convention).
    fn json_schema() -> serde_json::Value {
        let schema = schemars::schema_for!(UserSettings);
        serde_json::to_value(schema).expect("settings JSON schema should serialize")
    }

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
    fn malformed_mcp_block_falls_back_to_default_without_failing_the_file() {
        // The docs invite hand-editing the `mcp` block, so a slip there must
        // not brick every settings consumer — it degrades to the default port
        // while the rest of the file loads.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        for bad_block in [
            r#"null"#,
            r#"{"port":"8787"}"#, // string, not integer
            r#"[9999]"#,          // array: must not set the port positionally
        ] {
            std::fs::write(&path, format!(r#"{{"preferredEditor":"vim","mcp":{bad_block}}}"#))
                .unwrap();
            let loaded = load_from(&path).unwrap();
            assert_eq!(loaded.preferred_editor, "vim", "rest of the file still loads");
            assert_eq!(loaded.mcp.port, DEFAULT_MCP_PORT, "falls back for {bad_block}");
        }

        // A well-formed block still parses, of course.
        std::fs::write(&path, r#"{"mcp":{"port":9123}}"#).unwrap();
        assert_eq!(load_from(&path).unwrap().mcp.port, 9123);
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
        assert_eq!(c.calendar.refresh_minutes, 15);
        // Two built-in sources; only the personal one is on, matching the
        // single-provider default this replaced.
        let ids: Vec<&str> = c.calendar.sources.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["google", "outlook"]);
        assert!(c.calendar.sources[0].enabled, "google on by default");
        assert!(!c.calendar.sources[1].enabled, "outlook opt-in");
        // Each source carries its own editable prompt, seeded from the built-ins.
        assert_eq!(c.calendar.sources[0].prompt, DEFAULT_CALENDAR_PROMPT_GOOGLE);
        assert_eq!(c.calendar.sources[1].prompt, DEFAULT_CALENDAR_PROMPT_OUTLOOK);
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
        // …and unset means ON (the frontend applies the default).
        assert!(s.agentboard.copy_on_select.unwrap_or(true));
    }

    #[test]
    fn shortcuts_work_in_terminal_defaults_unset_and_on() {
        let s = UserSettings::default();
        // Unset until the user changes it, so the shared file stays clean…
        assert!(s.agentboard.shortcuts_work_in_terminal.is_none());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("shortcutsWorkInTerminal"));
        // …and unset means ON (the frontend applies the default).
        assert!(s.agentboard.shortcuts_work_in_terminal.unwrap_or(true));
    }

    #[test]
    fn terminal_font_size_defaults_unset_and_thirteen() {
        let s = UserSettings::default();
        // Unset until the user changes it, so the shared file stays clean…
        assert!(s.agentboard.terminal_font_size.is_none());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("terminalFontSize"));
        // …and unset means the 13px default (the frontend applies it).
        assert_eq!(s.agentboard.terminal_font_size.unwrap_or(13), 13);
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
        assert!(cal.get("sources").is_some());
        // The per-source prompt is the user-facing escape hatch — it has to
        // reach the schema, or the settings UI can't offer it.
        let src = &defs["CalendarSource"]["properties"];
        assert!(src.get("prompt").is_some());
        assert!(src.get("enabled").is_some());
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
        let dir = slot_checkout("towles-tool-rs");
        let root = dir.path().join("towles-tool-rs");
        // From the root and from a nested subdir, the scope is the root's name.
        assert_eq!(slot_scope_from_dir(&root), Some("towles-tool-rs".to_string()));
        assert_eq!(
            slot_scope_from_dir(&root.join("crates").join("tt-store").join("src")),
            Some("towles-tool-rs".to_string())
        );
    }

    #[test]
    fn nested_worktree_checkout_is_repo_qualified() {
        // <repo>/.claude/worktrees/<name>/crates/tt-config: the slot's scope
        // carries the main checkout's name so same-named slots of different
        // repos never share state.
        let dir = TempDir::new().unwrap();
        let slot =
            dir.path().join("towles-tool-rs").join(".claude").join("worktrees").join("migrate");
        std::fs::create_dir_all(slot.join("crates").join("tt-config")).unwrap();
        assert_eq!(slot_scope_from_dir(&slot), Some("towles-tool-rs-migrate".to_string()));
    }

    #[test]
    fn worktrees_dir_outside_claude_is_not_qualified() {
        // Only the exact `.claude/worktrees` shape qualifies — a checkout that
        // happens to sit under some other `worktrees/` dir scopes by its own
        // name like any main checkout.
        let dir = TempDir::new().unwrap();
        let checkout = dir.path().join("worktrees").join("thing");
        std::fs::create_dir_all(checkout.join("crates").join("tt-config")).unwrap();
        assert_eq!(slot_scope_from_dir(&checkout), Some("thing".to_string()));
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
