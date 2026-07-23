//! SQLite-backed store for the towles-tool "personal dashboard" data: calendar
//! events, kanban todos, issues, PR status, and collector run bookkeeping.
//!
//! This crate is deliberately Tauri-free (the shared-crate rule): both the CLI and
//! the Tauri app depend on it. Clocks are injected as `now_ms` parameters (epoch
//! milliseconds) so logic stays deterministic under test.
//!
//! **Calendar events are the one exception to epoch-ms storage.** Their
//! `starts_at`/`ends_at` are RFC 3339 strings that keep the offset the calendar
//! reported (`2026-07-20T15:00:00+01:00`), because an epoch integer throws that
//! away — it can say *when* a meeting is but never that it was booked as 3pm
//! London. Everything else here (`updated_at`, run timestamps, task times) is
//! still epoch ms; see [`Store::replace_events_for_source`] for how the two meet.
//!
//! The public output structs serialize with `camelCase` keys to match the TypeScript
//! contract consumed by the frontend / Tauri commands.

use std::path::Path;

use chrono::{DateTime, FixedOffset};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod attention;
pub use attention::{
    ChecksFailedEdge, ChecksFailedWatch, FAIL_STREAK, MeetingStartEdge, MeetingStartWatch,
    ReviewRequestedEdge, ReviewRequestedWatch, StaleCollectorEdge, StaleCollectorWatch,
    WatchedCollector,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("could not resolve a data directory")]
    NoDataDir,

    #[error("no task with id {0}")]
    TaskNotFound(i64),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Current on-disk schema version, stored in the `meta` table.
const SCHEMA_VERSION: i64 = 15;

/// The sort/range key format for calendar events: UTC, fixed width, matching
/// the `starts_at_utc`/`ends_at_utc` generated columns' `strftime` format
/// **exactly**.
///
/// Fixed width is the whole point. These keys are compared lexically by SQLite's
/// default `BINARY` collation, and that only equals chronological order when
/// every value has identical shape and zone. `2026-07-20T09:00:00-05:00` sorts
/// before `2026-07-20T10:00:00+01:00` byte-wise while being an hour *later* in
/// real time — which is exactly why the authored column is never the sort key.
/// If this format and the DDL's ever disagree, range queries silently return
/// wrong rows, so the two are asserted equal in `utc_key_matches_the_generated_column`.
const UTC_KEY_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

/// An instant as its `starts_at_utc` sort key — the bridge between the injected
/// `now_ms` clock and the events table's text columns.
fn utc_key(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(dt) => dt.format(UTC_KEY_FORMAT).to_string(),
        // Beyond what chrono can represent — callers pass `i64::MIN`/`i64::MAX`
        // to mean "no bound". Clamp to a key that sorts outside every real
        // value; collapsing to the epoch instead (the obvious `unwrap_or`)
        // would turn an unbounded window into an empty one.
        None if ms < 0 => "0000-01-01T00:00:00.000Z".to_string(),
        None => "9999-12-31T23:59:59.999Z".to_string(),
    }
}

/// Parse a stored event time, keeping its offset. `None` for anything that
/// isn't RFC 3339 — see the call site in `query_events` for why that is skipped
/// rather than propagated.
fn parse_rfc3339(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text).ok()
}

/// One unparseable event row, logged so a hand-edit is discoverable instead of
/// silently shrinking the calendar.
fn log_unparseable_event(external_id: &str, value: &str) {
    tracing::warn!(%external_id, %value, "tt-store: unparseable event time; row skipped");
}

/// How many MCP call-log rows are retained; older rows are pruned on insert.
const MCP_CALL_RETAIN: i64 = 500;

/// How far back calendar events are kept, swept on each calendar write.
/// Public so writers can refuse a backfill their own sweep would reclaim.
///
/// Events are a cache in service of "when is my next meeting", so history has
/// no value here — but a few days of slack means a clock skew or a late-running
/// pull can't discard a meeting that hasn't happened yet.
pub const EVENT_RETAIN_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How many MCP call-log rows ride along in a [`Snapshot`] (newest first).
const MCP_CALL_SNAPSHOT_LIMIT: usize = 100;

/// Kanban columns a todo can live in, in board order. `next`/`review` (Up
/// Next / In Review) were removed 2026-07-23: in practice every task was
/// either not started (`backlog`), had an agent running on it (`doing`), or
/// was finished (`done`) — the two middle columns never held anything.
pub const TASK_STATUSES: [&str; 3] = ["backlog", "doing", "done"];

/// How a closed task ended. Orthogonal to [`TASK_STATUSES`]: `status` is where
/// the card sits on the board, `outcome` is the record of how the work
/// finished — set once when the task is closed (usually as its worktree is
/// deleted), `NULL` while the task is open.
pub const TASK_OUTCOMES: [&str; 2] = ["done", "abandoned"];

/// A parsed task outcome — the typed form of [`TASK_OUTCOMES`]. String input
/// (MCP args, CLI flags, IPC payloads) parses at the boundary via
/// [`TaskOutcome::parse`]; everything past it carries the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    Done,
    Abandoned,
}

impl TaskOutcome {
    /// The stored/wire spelling, one of [`TASK_OUTCOMES`].
    pub fn as_str(self) -> &'static str {
        match self {
            TaskOutcome::Done => "done",
            TaskOutcome::Abandoned => "abandoned",
        }
    }

    /// Parse the stored/wire spelling; `None` for anything else.
    pub fn parse(s: &str) -> Option<TaskOutcome> {
        match s {
            "done" => Some(TaskOutcome::Done),
            "abandoned" => Some(TaskOutcome::Abandoned),
            _ => None,
        }
    }
}

/// How long a finished task stays visible in the terminal column before
/// [`Store::archive_closed_tasks`] hides it. One constant for every sweeper —
/// the app's manual "Archive done" button and the collector-side auto-sweep
/// must agree on what "old enough" means.
pub const ARCHIVE_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Schema v1. Every statement is `IF NOT EXISTS` so `migrate` is idempotent.
const SCHEMA_V1: &str = "\
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    starts_at_utc TEXT GENERATED ALWAYS AS (strftime('%Y-%m-%dT%H:%M:%fZ', starts_at)) STORED,
    ends_at TEXT,
    ends_at_utc TEXT GENERATED ALWAYS AS (strftime('%Y-%m-%dT%H:%M:%fZ', ends_at)) STORED,
    attendees TEXT NOT NULL DEFAULT '[]',
    location TEXT,
    join_url TEXT,
    updated_at INTEGER NOT NULL,
    UNIQUE(source, external_id)
);
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'backlog',
    position INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    notes TEXT,
    worktree_repo_root TEXT,
    worktree_repo TEXT,
    worktree_branch TEXT,
    worktree_dir TEXT,
    outcome TEXT,
    archived_at INTEGER
);
CREATE TABLE IF NOT EXISTS issues (
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    title TEXT NOT NULL,
    labels TEXT NOT NULL DEFAULT '[]',
    state TEXT NOT NULL,
    url TEXT NOT NULL,
    updated_ts INTEGER NOT NULL,
    PRIMARY KEY (repo, number)
);
CREATE TABLE IF NOT EXISTS pr_status (
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    title TEXT NOT NULL,
    branch TEXT NOT NULL,
    state TEXT NOT NULL,
    checks TEXT NOT NULL,
    review_state TEXT NOT NULL,
    url TEXT NOT NULL,
    updated_ts INTEGER NOT NULL,
    PRIMARY KEY (repo, number)
);
CREATE TABLE IF NOT EXISTS collect_runs (
    collector TEXT PRIMARY KEY,
    ran_at INTEGER NOT NULL,
    ok INTEGER NOT NULL,
    message TEXT
);
CREATE TABLE IF NOT EXISTS dm_status (
    channel TEXT PRIMARY KEY,
    from_name TEXT NOT NULL,
    text TEXT NOT NULL,
    ts INTEGER NOT NULL,
    from_me INTEGER NOT NULL,
    url TEXT,
    fetched_at INTEGER NOT NULL,
    dismissed_ts INTEGER NOT NULL DEFAULT 0
);
";

/// v5: the MCP server's incoming-call log (one row per JSON-RPC request the
/// MCP dispatcher handled). `IF NOT EXISTS`, so `migrate` stays
/// idempotent and pre-v5 dbs gain the table in place.
const SCHEMA_MCP_CALLS_V5: &str = "\
CREATE TABLE IF NOT EXISTS mcp_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    method TEXT NOT NULL,
    tool TEXT,
    args TEXT,
    ok INTEGER NOT NULL,
    error TEXT,
    duration_ms INTEGER,
    client TEXT
);
";

/// v7 (#339): a task links 0..N GitHub issues and 0..N PRs. Link rows cache
/// the last observed `state` (and `checks` for PRs) because the collector
/// snapshot only holds open-assigned issues and a bounded merged-PR list —
/// absence from the snapshot is ambiguous, so once a ref is observed
/// closed/merged that fact must survive the ref leaving the snapshot.
/// `state_ts` is when the state was last confirmed.
const SCHEMA_TASK_LINKS_V7: &str = "\
CREATE TABLE IF NOT EXISTS task_issues (
    task_id INTEGER NOT NULL,
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    url TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open',
    state_ts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (task_id, repo, number)
);
CREATE TABLE IF NOT EXISTS task_prs (
    task_id INTEGER NOT NULL,
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    url TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open',
    checks TEXT NOT NULL DEFAULT 'none',
    state_ts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (task_id, repo, number)
);
";

/// v12: tracked-repo identity cache (repo root -> GitHub `owner/repo` slug).
/// Reconciled wholesale by the Agentboard poll loop from the shared
/// `repos.json` tracked-repo list plus each repo's freshly-derived git
/// origin (see [`Store::reconcile_repos`]), so `repos.json` stays the sole
/// source of truth for "which repos are tracked" and this table is a pure,
/// self-healing cache — there is no separate untrack path to keep in sync.
const SCHEMA_REPOS_V12: &str = "\
CREATE TABLE IF NOT EXISTS repos (
    repo_root TEXT PRIMARY KEY,
    owner_repo TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
";

/// v15: per-item dismissals for the `issues`/`pr_status` tables. Those two
/// tables are fully replaced by every collector run (see
/// [`Store::replace_issues`]/[`Store::replace_prs`]), so a dismissal can't
/// live as a column on them the way `dm_status.dismissed_ts` does — it would
/// vanish the moment the row is reinserted. This table is independent and
/// keyed on `(kind, repo, number)` (`kind` is `"issue"` or `"pr"` — plain
/// numbers collide across the two per repo), diffed against the live rows at
/// read time in [`Store::issues`]/[`Store::get_issue`]/[`Store::prs`].
const SCHEMA_ITEM_DISMISSALS_V15: &str = "\
CREATE TABLE IF NOT EXISTS item_dismissals (
    kind TEXT NOT NULL,
    repo TEXT NOT NULL,
    number INTEGER NOT NULL,
    dismissed_ts INTEGER NOT NULL,
    PRIMARY KEY (kind, repo, number)
);
";

/// Local midnight of `date` as epoch ms, resolving DST edges rather than
/// giving up on them: an ambiguous midnight takes the earlier instant, and a
/// nonexistent one (spring-forward at 00:00) walks forward to the first valid
/// minute of that day. `None` only if the whole day is unrepresentable.
fn local_midnight(date: chrono::NaiveDate) -> Option<i64> {
    use chrono::{Local, LocalResult, TimeZone};

    match date.and_hms_opt(0, 0, 0).map(|dt| Local.from_local_datetime(&dt)) {
        Some(LocalResult::Single(dt)) => return Some(dt.timestamp_millis()),
        // Fall-back fold: two instants map to this local time. Take the earlier
        // so the day window still starts at the first occurrence of midnight.
        Some(LocalResult::Ambiguous(earlier, _)) => return Some(earlier.timestamp_millis()),
        _ => {}
    }
    // Spring-forward at 00:00: midnight doesn't exist. Step forward a minute at
    // a time to the first instant that does — bounded, since a DST jump is
    // never more than a couple of hours.
    for minute in 1..=180 {
        if let Some(dt) = date.and_hms_opt(0, 0, 0).map(|dt| dt + chrono::Duration::minutes(minute))
            && let Some(resolved) = Local.from_local_datetime(&dt).earliest()
        {
            return Some(resolved.timestamp_millis());
        }
    }
    None
}

// Column lists, kept in sync with the row-mapping closures below.
const EVENT_COLS: &str =
    "id, source, external_id, title, starts_at, ends_at, attendees, location, join_url";
const TASK_COLS: &str = "id, text, status, position, created_at, completed_at, notes, \
     worktree_repo_root, worktree_repo, worktree_branch, worktree_dir, outcome, archived_at";
// Aliased to `i`/`p` and joined against `item_dismissals` in the read paths
// below, so each column list carries its own dismissed_ts.
const ISSUE_COLS: &str = "i.repo, i.number, i.title, i.labels, i.state, i.url, i.updated_ts, COALESCE(d.dismissed_ts, 0)";
const PR_COLS: &str = "p.repo, p.number, p.title, p.branch, p.state, p.checks, p.review_state, \
     p.url, p.updated_ts, COALESCE(d.dismissed_ts, 0)";
const RUN_COLS: &str = "collector, ran_at, ok, message";
const DM_COLS: &str = "channel, from_name, text, ts, from_me, url, fetched_at, dismissed_ts";
const MCP_CALL_COLS: &str = "id, ts, method, tool, args, ok, error, duration_ms, client";

/// Kanban ordering used across queries: board column, then manual position, then age.
const TASK_ORDER: &str = "\
ORDER BY CASE status
    WHEN 'backlog' THEN 0 WHEN 'doing' THEN 1 WHEN 'done' THEN 2 ELSE 3 END,
  position ASC, created_at ASC";

// ---------------------------------------------------------------------------
// Output structs (camelCase, matching the TypeScript contract).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalEvent {
    pub id: i64,
    /// Which configured calendar this came from (`tt_config::CalendarSource::id`
    /// — e.g. `"google"`, `"outlook"`). Provenance for the UI, and the scope key
    /// for [`Store::replace_events_for_source`].
    pub source: String,
    pub external_id: String,
    pub title: String,
    /// When the meeting starts, with the offset the calendar reported.
    pub start: DateTime<FixedOffset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<FixedOffset>>,
    pub attendees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_url: Option<String>,
}

impl CalEvent {
    /// The start instant as epoch ms, for arithmetic against an injected
    /// `now_ms`. Lossy on purpose — the offset is presentation, not instant.
    pub fn start_ms(&self) -> i64 {
        self.start.timestamp_millis()
    }

    /// The end instant as epoch ms, when the event has one.
    pub fn end_ms(&self) -> Option<i64> {
        self.end.map(|end| end.timestamp_millis())
    }
}

/// A task on the board (#339): the unit of work. Local by default; it can
/// link any number of GitHub issues and PRs, and usually gets a worktree
/// worktree (its [`TaskWorktree`] binding).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: i64,
    pub text: String,
    pub status: String,
    pub position: i64,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// How the task ended (see [`TASK_OUTCOMES`]); `None` while it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// When the closed task was archived off the active board; `None` while
    /// it is open or still visible in the terminal column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<TaskWorktree>,
    #[serde(default)]
    pub issues: Vec<TaskIssueLink>,
    #[serde(default)]
    pub prs: Vec<TaskPrLink>,
    /// A closed task renders in the terminal ("Closed") column regardless of
    /// its frozen kanban `status` — true once the task carries an `outcome`
    /// or its `status` itself is `done`. Presentation, computed once here so
    /// every consumer (app UI, MCP) reads the same answer instead of
    /// re-deriving it from `status`/`outcome`.
    #[serde(default)]
    pub closed: bool,
    /// The outcome badge a closed card shows: the recorded `outcome`, or
    /// `done` implied by `status` for a task that rolled/dragged into the
    /// done column without an explicit close. `None` while the task is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_outcome: Option<String>,
    /// Whether this task has a live worktree checkout on disk right now — as
    /// opposed to a "task only" card that was never given one, or a closed
    /// task whose worktree was torn down. Drives whether the UI offers "jump
    /// to the running session" vs. "start/reopen this task".
    #[serde(default)]
    pub has_worktree: bool,
}

impl TaskItem {
    /// The best-evidence default outcome for closing this task without a user
    /// answer: a merged linked PR — or already sitting in `done` — closes as
    /// [`TaskOutcome::Done`], anything else as [`TaskOutcome::Abandoned`].
    /// Strictly `merged`, never `closed`: an unmerged-closed PR is evidence
    /// of abandonment, not completion. Every headless close path (CLI, MCP)
    /// shares this one inference; interactive paths ask the user and use it
    /// only to pre-answer.
    pub fn inferred_outcome(&self) -> TaskOutcome {
        if self.status == "done" || self.prs.iter().any(|pr| pr.state == "merged") {
            TaskOutcome::Done
        } else {
            TaskOutcome::Abandoned
        }
    }

    /// Materialize `closed`/`display_outcome`/`has_worktree` from the raw
    /// `status`/`outcome`/`worktree` fields — the one place this
    /// presentation logic is computed, called right after every row maps
    /// into a `TaskItem` (see `Store::query_tasks`).
    fn with_derived_fields(mut self) -> Self {
        self.closed = self.status == "done" || self.outcome.is_some();
        self.display_outcome = self
            .outcome
            .clone()
            .or_else(|| (self.status == "done").then(|| TaskOutcome::Done.as_str().to_string()));
        self.has_worktree = self.worktree.as_ref().is_some_and(|w| w.dir.is_some());
        self
    }
}

/// A task's repo binding, and the worktree its work happens in once one exists.
///
/// `repo_root` is the only required part: a task created from the Agentboard
/// knows its repo from the moment of submit, including a "task only" submit
/// that never creates a worktree. `branch` is therefore `None` until the worktree
/// is created — which is what lets every task land in a repo swimlane on the
/// Board rather than an "unassigned" bucket.
///
/// `repo_root` and `branch` survive worktree removal as historical fact; `dir` is
/// cleared when the worktree is removed (a "detached" task). `repo` is the
/// GitHub `owner/name`, used to auto-attach collected PRs whose head branch
/// matches `branch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorktree {
    pub repo_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// One GitHub issue linked to a task. `state` is the last observed state
/// (`open` | `closed`), cached on the link (see [`SCHEMA_TASK_LINKS_V7`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskIssueLink {
    pub repo: String,
    pub number: i64,
    pub url: String,
    pub state: String,
}

/// One GitHub PR linked to a task. `state` is the last observed state
/// (`open` | `merged` | `closed`); `checks` mirrors [`PrItem::checks`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPrLink {
    pub repo: String,
    pub number: i64,
    pub url: String,
    pub state: String,
    pub checks: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueItem {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub labels: Vec<String>,
    pub state: String,
    pub url: String,
    pub updated_ts: i64,
    /// The `updated_ts` this item had the last time the user dismissed it
    /// (see [`Store::dismiss_item`]); `0` if never dismissed. The UI hides an
    /// item while `dismissed_ts >= updated_ts` and re-shows it the moment the
    /// collector observes a newer `updated_ts` — a dismissal survives the
    /// item leaving and re-entering the collector snapshot the same way
    /// [`DmItem::dismissed_ts`] does, but expires on its own once the item
    /// actually changes rather than needing a matching new "message".
    pub dismissed_ts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrItem {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub branch: String,
    pub state: String,
    pub checks: String,
    pub review_state: String,
    pub url: String,
    pub updated_ts: i64,
    /// See [`IssueItem::dismissed_ts`] — same semantics, keyed `(kind = "pr", repo, number)`.
    pub dismissed_ts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectRun {
    pub collector: String,
    pub ran_at: i64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The latest state of one watched DM conversation. `from_me` means the most
/// recent message in the channel is the user's own (i.e. already answered);
/// `dismissed_ts` is the `ts` of the last message the user marked handled, so
/// the UI shows a banner only when `!from_me && dismissed_ts < ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmItem {
    pub channel: String,
    pub from_name: String,
    pub text: String,
    pub ts: i64,
    pub from_me: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub fetched_at: i64,
    pub dismissed_ts: i64,
}

/// One handled MCP request: what came in (method, tool, compacted args), how it
/// went (`ok`/`error`), how long the handler took, and which client sent it
/// (from the session's `initialize`). Written by the MCP dispatcher,
/// read by the app's MCP screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCall {
    pub id: i64,
    pub ts: i64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

/// Full-store snapshot for the dashboard UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub events: Vec<CalEvent>,
    pub tasks: Vec<TaskItem>,
    pub issues: Vec<IssueItem>,
    pub prs: Vec<PrItem>,
    pub runs: Vec<CollectRun>,
    pub dms: Vec<DmItem>,
    #[serde(default)]
    pub mcp_calls: Vec<McpCall>,
}

// ---------------------------------------------------------------------------
// Input structs (what collectors hand to the store).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventInput {
    pub external_id: String,
    pub title: String,
    /// RFC 3339 with an offset (`2026-07-20T15:00:00+01:00` or `…Z`).
    ///
    /// A `DateTime`, not a `String`, so an unparseable value is rejected by
    /// serde with a real message at the edge rather than reaching the store as
    /// text nothing can order. `FixedOffset` (not `Utc`) because the offset is
    /// data: normalizing on the way in would discard the one thing this type
    /// exists to carry.
    pub start: DateTime<FixedOffset>,
    #[serde(default)]
    pub end: Option<DateTime<FixedOffset>>,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub join_url: Option<String>,
}

impl EventInput {
    /// The start instant as epoch ms.
    pub fn start_ms(&self) -> i64 {
        self.start.timestamp_millis()
    }

    /// The end instant as epoch ms, when the event has one.
    pub fn end_ms(&self) -> Option<i64> {
        self.end.map(|end| end.timestamp_millis())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueInput {
    pub repo: String,
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub state: String,
    pub url: String,
    pub updated_ts: i64,
}

/// What the Slack collector hands the store for one watched DM conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmInput {
    pub channel: String,
    pub from_name: String,
    pub text: String,
    pub ts: i64,
    pub from_me: bool,
    #[serde(default)]
    pub url: Option<String>,
}

/// What the MCP dispatcher hands the store for one handled request. The row's
/// `ts` comes from the dispatcher's injected `now_ms`, never a clock read here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallInput {
    pub method: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub args: Option<String>,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub client: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrInput {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub branch: String,
    pub state: String,
    pub checks: String,
    pub review_state: String,
    pub url: String,
    pub updated_ts: i64,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// A handle to the SQLite store.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the store at `path`, running migrations. Parent
    /// directories are created if absent.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        // The file is shared by several connections at once (app UI, the app's
        // collector scheduler, the CLI, the MCP server); WAL plus a busy timeout
        // lets their writes interleave instead of failing with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open the store at the resolved default location. Unscoped this is
    /// `<data_dir>/towles-tool/tt.db` (e.g. `~/.local/share/towles-tool/tt.db`);
    /// in a worktree checkout it nests under `…/tasks/<scope>/` (see [`tt_config`]).
    pub fn open_default() -> Result<Store> {
        let path = tt_config::store_db_path().map_err(|_| Error::NoDataDir)?;
        Store::open(&path)
    }

    /// Open an ephemeral in-memory store (for tests).
    pub fn open_in_memory() -> Result<Store> {
        let store = Store { conn: Connection::open_in_memory()? };
        store.migrate()?;
        Ok(store)
    }

    /// Create tables and record the schema version. Idempotent.
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_V1)?;
        self.migrate_tasks_v2()?;
        self.migrate_tasks_notes_v4()?;
        self.conn.execute_batch(SCHEMA_TASK_LINKS_V7)?;
        self.migrate_tasks_v7()?;
        self.migrate_tasks_v8_drop_due()?;
        self.migrate_tasks_v11_worktree_cols()?;
        self.migrate_tasks_v13_outcome()?;
        self.migrate_tasks_v14_drop_next_review()?;
        self.migrate_events_v9()?;
        self.migrate_events_v10_iso()?;
        // After v10, never inside SCHEMA_V1: that batch runs before the
        // migrations, so on an upgrading db the column would not exist yet.
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_events_starts_at_utc ON events(starts_at_utc);",
        )?;
        self.conn.execute_batch(SCHEMA_MCP_CALLS_V5)?;
        self.migrate_collect_runs_v6()?;
        self.conn.execute_batch(SCHEMA_REPOS_V12)?;
        self.conn.execute_batch(SCHEMA_ITEM_DISMISSALS_V15)?;
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    /// `CREATE TABLE IF NOT EXISTS` is a no-op on a `tasks` table that already
    /// existed under the pre-kanban schema (`source`/`source_ref`/`done`, no
    /// `status`/`position`/`repo`/`issue_number`/`issue_url`), so a db created
    /// before the day-screens pivot never gained the new columns. Rebuild such
    /// a table to the v2 shape. A rebuild — not `ALTER TABLE ADD COLUMN` — is
    /// required because the legacy `source` column is `NOT NULL` without a
    /// default: left in place it fails every future `INSERT INTO tasks`
    /// (SQLite can't drop a column's NOT NULL in place). The rebuild also
    /// repairs dbs that were half-migrated by the old ALTER-based migration
    /// (new columns added, `source` still present). Drops the `emails` table,
    /// dead since the same pivot.
    fn migrate_tasks_v2(&self) -> Result<()> {
        let mut has_status = false;
        let mut has_source = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "status" {
                    has_status = true;
                }
                if name == "source" {
                    has_source = true;
                }
            }
        }
        if has_source {
            // Legacy rows carry their kanban fields either in the old `done`
            // flag (never migrated) or in already-added v2 columns
            // (half-migrated by the old ALTER-based migration).
            let (status_expr, position_expr, link_exprs) = if has_status {
                ("status", "position", "repo, issue_number, issue_url")
            } else {
                ("CASE WHEN done = 1 THEN 'done' ELSE 'backlog' END", "0", "NULL, NULL, NULL")
            };
            self.conn.execute_batch(&format!(
                "BEGIN;
                 CREATE TABLE tasks_v2 (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER
                 );
                 INSERT INTO tasks_v2 (id, text, status, position, due_ts, repo, issue_number,
                                       issue_url, created_at, completed_at)
                   SELECT id, text, {status_expr}, {position_expr}, due_ts, {link_exprs},
                          created_at, completed_at
                   FROM tasks;
                 DROP TABLE tasks;
                 ALTER TABLE tasks_v2 RENAME TO tasks;
                 COMMIT;"
            ))?;
        }
        self.conn.execute_batch("DROP TABLE IF EXISTS emails;")?;
        Ok(())
    }

    /// v6: drop `collect_runs` freshness rows for collectors that no longer
    /// exist (`claude:email` and `claude:tasks` died in the day-screens pivot,
    /// but their rows lingered forever). Kept as a `NOT IN` sweep against the
    /// live collector keys — the same set `tt-collect` records under — so any
    /// future retired collector is cleaned up the same way. Idempotent.
    fn migrate_collect_runs_v6(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM collect_runs
             WHERE collector NOT IN ('claude:calendar', 'issues', 'prs', 'slack:dm')",
            [],
        )?;
        Ok(())
    }

    /// v4: free-form `notes` on todos. Dbs created before v4 (including ones the
    /// v2 rebuild just produced) lack the column; a nullable ADD COLUMN brings
    /// them forward in place. Idempotent via the `PRAGMA table_info` check.
    fn migrate_tasks_notes_v4(&self) -> Result<()> {
        let mut has_notes = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "notes" {
                    has_notes = true;
                }
            }
        }
        if !has_notes {
            self.conn.execute_batch("ALTER TABLE tasks ADD COLUMN notes TEXT;")?;
        }
        Ok(())
    }

    /// v7 (#339): tasks become the unit of work. The single issue link
    /// (`repo`/`issue_number`/`issue_url` columns) generalizes into the
    /// `task_issues` link table, and the task gains its worktree binding
    /// (`worktree_repo_root`/`worktree_repo`/`worktree_branch`/`worktree_dir`). A rebuild —
    /// not `ALTER` — for the same reason as the v2 repair (dropping columns
    /// in place is off the table). Existing single links are ported into
    /// `task_issues` with state `open`; the next collect pass refreshes
    /// their real state. Detects a pre-v7 table by the `repo` column, so it
    /// is idempotent and a no-op on fresh dbs.
    fn migrate_tasks_v7(&self) -> Result<()> {
        let mut has_repo = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "repo" {
                    has_repo = true;
                }
            }
        }
        if !has_repo {
            return Ok(());
        }
        self.conn.execute_batch(
            "BEGIN;
             CREATE TABLE tasks_v7 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'backlog',
                position INTEGER NOT NULL DEFAULT 0,
                due_ts INTEGER,
                created_at INTEGER NOT NULL,
                completed_at INTEGER,
                notes TEXT,
                worktree_repo_root TEXT,
                worktree_repo TEXT,
                worktree_branch TEXT,
                worktree_dir TEXT
             );
             INSERT INTO tasks_v7 (id, text, status, position, due_ts, created_at, completed_at,
                                   notes)
               SELECT id, text, status, position, due_ts, created_at, completed_at, notes
               FROM tasks;
             INSERT OR IGNORE INTO task_issues (task_id, repo, number, url, state, state_ts)
               SELECT id, repo, issue_number, COALESCE(issue_url, ''), 'open', 0
               FROM tasks
               WHERE repo IS NOT NULL AND issue_number IS NOT NULL;
             DROP TABLE tasks;
             ALTER TABLE tasks_v7 RENAME TO tasks;
             COMMIT;",
        )?;
        Ok(())
    }

    /// v9: calendar events gained a `source` column so a personal and a work
    /// calendar can be merged into one timeline without clobbering each other
    /// (the old write path was a `DELETE FROM events` full-table swap, so
    /// whichever calendar pushed second wiped the first).
    ///
    /// A rebuild — not `ALTER TABLE ADD COLUMN` — because the uniqueness rule
    /// changes too: `external_id` was `UNIQUE` on its own, and two providers can
    /// legitimately issue the same event id. SQLite cannot alter a constraint in
    /// place.
    ///
    /// **Pre-v9 rows are dropped, not migrated.** There is no honest source to
    /// attribute them to (the old schema didn't record one), and a wrong guess
    /// is worse than no row: a row labelled with a source that never pushes
    /// again is never replaced by anything, so it would linger in the countdown
    /// forever. Events are a pure collector-owned cache, fully rebuilt on the
    /// next pull, so the cost is at most one refresh interval of staleness.
    /// Detected by column presence, so it's idempotent and a no-op on fresh dbs.
    fn migrate_events_v9(&self) -> Result<()> {
        let mut has_source = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(events)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "source" {
                    has_source = true;
                }
            }
        }
        if !has_source {
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS events;
                 CREATE TABLE events (
                     id INTEGER PRIMARY KEY,
                     source TEXT NOT NULL,
                     external_id TEXT NOT NULL,
                     title TEXT NOT NULL,
                     start_ts INTEGER NOT NULL,
                     end_ts INTEGER,
                     attendees TEXT NOT NULL DEFAULT '[]',
                     location TEXT,
                     join_url TEXT,
                     updated_at INTEGER NOT NULL,
                     UNIQUE(source, external_id)
                 );",
            )?;
        }
        Ok(())
    }

    /// v10: event times became RFC 3339 text that keeps its offset, replacing
    /// the `start_ts`/`end_ts` epoch-ms integers.
    ///
    /// An epoch integer answers "when" and nothing else. The calendar knows a
    /// meeting was booked as 3pm London; storing `1784732400000` discards that,
    /// and every read then renders it in whatever zone the machine happens to be
    /// in — so the same row reads differently after a flight.
    ///
    /// A rebuild, because SQLite cannot `ALTER TABLE ADD COLUMN` a **STORED**
    /// generated column, and the sort key has to be stored to be indexable.
    ///
    /// **Rows are converted, not dropped** — unlike v9, which had no honest
    /// source to attribute old rows to. Here the instant is known exactly; only
    /// the authored offset is unknown, and `Z` states that truthfully rather
    /// than inventing a zone. Dropping would blank the next-meeting countdown
    /// until something writes, and with the pull collector off by default that
    /// may not be until the user notices it is wrong.
    ///
    /// Detected by column presence, so it is idempotent and a no-op on fresh dbs.
    fn migrate_events_v10_iso(&self) -> Result<()> {
        let mut has_starts_at = false;
        let mut has_source = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(events)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                match name.as_str() {
                    "starts_at" => has_starts_at = true,
                    "source" => has_source = true,
                    _ => {}
                }
            }
        }
        // `has_source` false means the table is pre-v9 and `migrate_events_v9`
        // just rebuilt it in the v9 shape; either way the epoch columns are what
        // exist, so the conversion below is the same.
        let _ = has_source;
        if !has_starts_at {
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE events_v10 (
                     id INTEGER PRIMARY KEY,
                     source TEXT NOT NULL,
                     external_id TEXT NOT NULL,
                     title TEXT NOT NULL,
                     starts_at TEXT NOT NULL,
                     starts_at_utc TEXT GENERATED ALWAYS AS
                         (strftime('%Y-%m-%dT%H:%M:%fZ', starts_at)) STORED,
                     ends_at TEXT,
                     ends_at_utc TEXT GENERATED ALWAYS AS
                         (strftime('%Y-%m-%dT%H:%M:%fZ', ends_at)) STORED,
                     attendees TEXT NOT NULL DEFAULT '[]',
                     location TEXT,
                     join_url TEXT,
                     updated_at INTEGER NOT NULL,
                     UNIQUE(source, external_id)
                 );
                 -- Epoch ms -> RFC 3339 UTC. `Z` is the honest offset for a row
                 -- whose authored zone was never recorded.
                 INSERT INTO events_v10
                     (id, source, external_id, title, starts_at, ends_at,
                      attendees, location, join_url, updated_at)
                 SELECT id, source, external_id, title,
                        strftime('%Y-%m-%dT%H:%M:%fZ', start_ts / 1000.0, 'unixepoch'),
                        CASE WHEN end_ts IS NULL THEN NULL
                             ELSE strftime('%Y-%m-%dT%H:%M:%fZ', end_ts / 1000.0, 'unixepoch')
                        END,
                        attendees, location, join_url, updated_at
                 FROM events;
                 DROP TABLE events;
                 ALTER TABLE events_v10 RENAME TO events;
                 CREATE INDEX IF NOT EXISTS idx_events_starts_at_utc
                     ON events(starts_at_utc);
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    /// v8: due dates are gone from tasks (2026-07-19 — GitHub issues carry no
    /// native due date, and the Board leans on status + links for urgency), so
    /// drop the column from dbs that predate the removal. Detected by column
    /// presence, so it's idempotent and a no-op on fresh dbs; `due_ts` was
    /// nullable and unindexed, so a plain `DROP COLUMN` is safe.
    fn migrate_tasks_v8_drop_due(&self) -> Result<()> {
        let mut has_due = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "due_ts" {
                    has_due = true;
                }
            }
        }
        if has_due {
            self.conn.execute("ALTER TABLE tasks DROP COLUMN due_ts", [])?;
        }
        Ok(())
    }

    /// v11: the worktree-"slot" vocabulary rename (2026-07-20). A task's
    /// worktree binding used to be stored in `slot_repo_root`/`slot_repo`/
    /// `slot_branch`/`slot_dir`; those columns become `worktree_*`. Dbs created
    /// at v7 (before the rename) carry the `slot_*` names — rename them in place
    /// with `ALTER TABLE … RENAME COLUMN` (SQLite ≥ 3.25), which preserves data
    /// and is cheaper than a rebuild. Detected by the `slot_repo_root` column,
    /// so it's idempotent and a no-op on fresh dbs (built straight to `worktree_*`).
    fn migrate_tasks_v11_worktree_cols(&self) -> Result<()> {
        let mut has_slot = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "slot_repo_root" {
                    has_slot = true;
                }
            }
        }
        if has_slot {
            self.conn.execute_batch(
                "ALTER TABLE tasks RENAME COLUMN slot_repo_root TO worktree_repo_root;
                 ALTER TABLE tasks RENAME COLUMN slot_repo TO worktree_repo;
                 ALTER TABLE tasks RENAME COLUMN slot_branch TO worktree_branch;
                 ALTER TABLE tasks RENAME COLUMN slot_dir TO worktree_dir;",
            )?;
        }
        Ok(())
    }

    /// v13: closing a task stopped deleting its row (2026-07-22). `outcome`
    /// records how it ended (see [`TASK_OUTCOMES`]) and `archived_at` hides it
    /// from active views — both `NULL` for every pre-existing (open) row.
    /// Detected by the `outcome` column, so it's idempotent and a no-op on
    /// fresh dbs (built straight to the full shape).
    fn migrate_tasks_v13_outcome(&self) -> Result<()> {
        let mut has_outcome = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "outcome" {
                    has_outcome = true;
                }
            }
        }
        if !has_outcome {
            self.conn.execute_batch(
                "ALTER TABLE tasks ADD COLUMN outcome TEXT;
                 ALTER TABLE tasks ADD COLUMN archived_at INTEGER;",
            )?;
        }
        Ok(())
    }

    /// v14: the "Up Next" and "In Review" columns are gone (2026-07-23) —
    /// they never held a task in practice, since a card was always either
    /// untouched, actively worked (an agent running on its worktree), or
    /// done. Remap existing rows onto the columns that remain: `next` folds
    /// back to `backlog` (never started), `review` folds forward to `doing`
    /// (work had begun). A plain `UPDATE`, not a rebuild, since no column
    /// shape changes — and it's idempotent: after the first pass no row
    /// matches `next`/`review` again.
    fn migrate_tasks_v14_drop_next_review(&self) -> Result<()> {
        self.conn.execute_batch(
            "UPDATE tasks SET status = 'backlog' WHERE status = 'next';
             UPDATE tasks SET status = 'doing' WHERE status = 'review';",
        )?;
        Ok(())
    }

    // --- Writes -----------------------------------------------------------

    /// The `[start, end)` epoch-ms bounds of the local calendar day containing
    /// `reference_ms` — the window callers pass to
    /// [`Store::replace_events_for_source`].
    ///
    /// This lives here, beside the delete it scopes, because **every writer
    /// must agree on it**. It previously existed twice — once in the collector,
    /// once in the MCP tool — with different DST fallbacks: one widened to a
    /// ±1-day window, the other collapsed to an empty one. Both fed the same
    /// scoped `DELETE`, so on a DST-transition day the same calendar day would
    /// sweep two days of rows when written by the collector and none when
    /// written over MCP. One destructive window, one implementation.
    ///
    /// DST is handled rather than punted on:
    /// - An **ambiguous** local midnight (a fall-back fold, real in zones like
    ///   Brazil, Chile and Cuba that transition at midnight) resolves to the
    ///   *earlier* instant, so the window still covers the whole civil day. The
    ///   old code used `.single()` here, which returned `None` for this case and
    ///   silently skipped the delete twice a year.
    /// - A **nonexistent** local midnight (spring-forward at 00:00) walks
    ///   forward to the first valid instant of that day.
    /// - Only if both boundaries are unresolvable does it fall back — to the
    ///   empty window, never a wider one. Deleting nothing leaves stale rows a
    ///   later pull fixes; deleting too much destroys data no pull restores.
    pub fn local_day_bounds(reference_ms: i64) -> (i64, i64) {
        use chrono::{Duration, Local, TimeZone};

        let Some(reference) = Local.timestamp_millis_opt(reference_ms).single() else {
            return (reference_ms, reference_ms);
        };
        let date = reference.date_naive();
        let start = local_midnight(date);
        let end = local_midnight(date + Duration::days(1));
        match (start, end) {
            (Some(start), Some(end)) => (start, end),
            _ => (reference_ms, reference_ms),
        }
    }

    /// Drop calendar events older than the retention window, independent of any
    /// write.
    ///
    /// [`Store::replace_events_for_source`] sweeps as a side effect, which is
    /// enough while some calendar is still being pulled — but not when the last
    /// one is switched off. Then no write ever happens again, the sweep never
    /// runs, and whatever was in the table stays forever: `calendar_next` keeps
    /// returning a meeting from the day the user turned collection off, with an
    /// ever-more-negative `minutesUntil` feeding the countdown and the
    /// meeting-start notification. The collector calls this even on the
    /// nothing-to-do path for exactly that reason.
    ///
    /// Returns how many rows were removed.
    pub fn sweep_old_events(&self, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM events WHERE starts_at_utc < ?1",
            params![utc_key(now_ms.saturating_sub(EVENT_RETAIN_MS))],
        )?)
    }

    /// Replace one calendar's events within one day window, leaving every other
    /// calendar — and every other day — untouched.
    ///
    /// This is deliberately *not* a full-table swap. Several calendars (personal
    /// Google, work Outlook) are pulled independently and merged into a single
    /// timeline; a global `DELETE FROM events` meant whichever pulled second
    /// erased the first. Scoping the delete to `(source, day)` makes each pull
    /// idempotent within its own lane.
    ///
    /// `source` is assigned by the *caller*, never by the data: it identifies
    /// which configured calendar this pull represents, and [`EventInput`]
    /// therefore has no `source` field — a model-authored payload must not be
    /// able to name the lane it writes into.
    ///
    /// The window is `[day_start_ms, day_end_ms)` against `start_ts`, passed in
    /// rather than derived here so the local-day boundary (and DST) stays the
    /// caller's decision and tests stay deterministic. Events outside it are
    /// inserted but will not be swept by this call — pass a window that actually
    /// contains them.
    pub fn replace_events_for_source(
        &self,
        source: &str,
        day_start_ms: i64,
        day_end_ms: i64,
        events: &[EventInput],
        now_ms: i64,
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM events WHERE source = ?1
               AND starts_at_utc >= ?2 AND starts_at_utc < ?3",
            params![source, utc_key(day_start_ms), utc_key(day_end_ms)],
        )?;
        // Retention. The delete above is scoped to one lane and one day, so
        // unlike the full-table swap it replaced it bounds nothing over time:
        // yesterday's meetings, and every row belonging to a calendar the user
        // has since renamed or removed, would otherwise accumulate forever.
        // Sweeping by age (not by source) is what catches the orphaned-lane
        // case, since no per-source write will ever visit those rows again.
        // Cheap to run here — this path fires per collector tick, not per read.
        // Delegated to `sweep_old_events` rather than repeating its SQL, so
        // write-time and standalone sweeping cannot drift apart; `tx` is an
        // `unchecked_transaction` on `self.conn`, so the call joins it.
        self.sweep_old_events(now_ms)?;
        // De-duplicate by external_id before inserting. The upsert below would
        // otherwise let a repeated id overwrite its own earlier row inside this
        // loop — one row lands, the other vanishes, and the returned count still
        // claims both were written. A model emitting the same recurring-meeting
        // instance twice is exactly how that happens, so collapse it here and
        // report what actually landed. Last occurrence wins, matching the
        // upsert's own semantics.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let deduped: Vec<&EventInput> = events
            .iter()
            .rev()
            .filter(|e| seen.insert(e.external_id.as_str()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO events
                   (source, external_id, title, starts_at, ends_at, attendees, location, join_url,
                    updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(source, external_id) DO UPDATE SET
                   title = excluded.title,
                   starts_at = excluded.starts_at,
                   ends_at = excluded.ends_at,
                   attendees = excluded.attendees,
                   location = excluded.location,
                   join_url = excluded.join_url,
                   updated_at = excluded.updated_at",
            )?;
            for e in &deduped {
                stmt.execute(params![
                    source,
                    e.external_id,
                    e.title,
                    e.start.to_rfc3339(),
                    e.end.map(|end| end.to_rfc3339()),
                    serde_json::to_string(&e.attendees)?,
                    e.location,
                    e.join_url,
                    now_ms,
                ])?;
            }
        }
        tx.commit()?;
        Ok(deduped.len())
    }

    /// Replace only the named repos' issue rows, leaving other repos' rows
    /// intact. Collectors use this when a sweep partially failed: repos that
    /// errored keep their last-known-good rows instead of being wiped.
    pub fn replace_issues_for_repos(
        &self,
        repos: &[String],
        issues: &[IssueInput],
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM issues WHERE repo = ?1")?;
            for repo in repos {
                del.execute(params![repo])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO issues (repo, number, title, labels, state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for i in issues {
                stmt.execute(params![
                    i.repo,
                    i.number,
                    i.title,
                    serde_json::to_string(&i.labels)?,
                    i.state,
                    i.url,
                    i.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(issues.len())
    }

    /// Full-snapshot replace of issue rows.
    pub fn replace_issues(&self, issues: &[IssueInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM issues", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO issues (repo, number, title, labels, state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for i in issues {
                stmt.execute(params![
                    i.repo,
                    i.number,
                    i.title,
                    serde_json::to_string(&i.labels)?,
                    i.state,
                    i.url,
                    i.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(issues.len())
    }

    /// Add a task. Lands in `status` (validated against [`TASK_STATUSES`]) at
    /// the end of that column; `notes` is free-form context. Issues/PRs are
    /// attached separately ([`Store::attach_task_issue`] /
    /// [`Store::attach_task_pr`]), the worktree via [`Store::set_task_worktree`].
    pub fn add_task(
        &self,
        text: &str,
        status: &str,
        notes: Option<&str>,
        now_ms: i64,
    ) -> Result<TaskItem> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE status = ?1",
            params![status],
            |r| r.get(0),
        )?;
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        self.conn.execute(
            "INSERT INTO tasks (text, status, position, notes, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![text, status, position, notes, now_ms, completed_at],
        )?;
        self.task_by_id(self.conn.last_insert_rowid())
    }

    /// Move a todo to a kanban column, appending it at the end of the target
    /// column (position = max there + 1, ignoring the task itself). Sets
    /// `completed_at` when entering `done`, clears it otherwise. Moving to any
    /// non-`done` column also reopens a closed task — `outcome` and
    /// `archived_at` clear, since the card is active again. Unknown statuses
    /// are rejected.
    pub fn set_task_status(&self, id: i64, status: &str, now_ms: i64) -> Result<()> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        let tx = self.conn.unchecked_transaction()?;
        let position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE status = ?1 AND id <> ?2",
            params![status, id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2, position = ?3,
                    outcome = CASE WHEN ?1 = 'done' THEN outcome ELSE NULL END,
                    archived_at = CASE WHEN ?1 = 'done' THEN archived_at ELSE NULL END
             WHERE id = ?4",
            params![status, completed_at, position, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Move a todo to `status` at an explicit `index` within that column,
    /// renumbering the column's `position`s to be contiguous (`0..n`). `index`
    /// is clamped to `[0, n]`, where `n` is the number of *other* todos already
    /// in the column, so out-of-range values land the card at the top or
    /// bottom rather than erroring. Sets `completed_at` when entering `done`
    /// and clears it otherwise (matching [`Store::set_task_status`]).
    ///
    /// Unlike `set_task_status` (which always appends), this reaches an
    /// arbitrary position — it powers drag-to-reorder within a column and
    /// position-aware drops across columns. The source column is left with a
    /// gap in its `position`s, which is harmless: ordering is by relative
    /// `position ASC`, and the next reorder there renumbers it. Returns
    /// [`Error::TaskNotFound`] when no todo has `id`.
    pub fn set_task_position(&self, id: i64, status: &str, index: i64, now_ms: i64) -> Result<()> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let tx = self.conn.unchecked_transaction()?;
        // The target column's todos in board order, excluding the mover.
        let others: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM tasks WHERE status = ?1 AND id <> ?2
                 ORDER BY position ASC, created_at ASC",
            )?;
            let rows = stmt.query_map(params![status, id], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()?
        };
        let pos = index.clamp(0, others.len() as i64) as usize;
        let mut order = others;
        order.insert(pos, id);
        {
            let mut up = tx.prepare("UPDATE tasks SET position = ?1 WHERE id = ?2")?;
            for (pos, tid) in order.iter().enumerate() {
                up.execute(params![pos as i64, tid])?;
            }
        }
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        let affected = tx.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2,
                    outcome = CASE WHEN ?1 = 'done' THEN outcome ELSE NULL END,
                    archived_at = CASE WHEN ?1 = 'done' THEN archived_at ELSE NULL END
             WHERE id = ?3",
            params![status, completed_at, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.commit()?;
        Ok(())
    }

    /// Edit a todo's free-form fields: its `text` and optional `notes`. This
    /// is a full replace of both fields — passing `None` for `notes` clears it
    /// (there is no "leave unchanged" sentinel). Status, position, and any
    /// issue link are left untouched. Returns the updated todo, or
    /// [`Error::TaskNotFound`] when no todo has `id`.
    pub fn update_task(&self, id: i64, text: &str, notes: Option<&str>) -> Result<TaskItem> {
        let affected = self.conn.execute(
            "UPDATE tasks SET text = ?1, notes = ?2 WHERE id = ?3",
            params![text, notes, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        self.task_by_id(id)
    }

    /// Delete a task permanently, cascading its issue/PR link rows. Returns
    /// [`Error::TaskNotFound`] when no task has `id`.
    pub fn delete_task(&self, id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.execute("DELETE FROM task_issues WHERE task_id = ?1", params![id])?;
        tx.execute("DELETE FROM task_prs WHERE task_id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Close a task: record how it ended and detach it from its worktree
    /// directory — the row survives as the record, this is what replaced
    /// deleting it. Closing as [`TaskOutcome::Done`] also lands the card at
    /// the end of the `done` column (matching [`Store::set_task_status`]);
    /// closing as [`TaskOutcome::Abandoned`] freezes `status` where the work
    /// stopped. Either way `completed_at` is stamped if not already set,
    /// which is what later ages the row into the archive
    /// ([`Store::archive_closed_tasks`]). Returns the updated task, or
    /// [`Error::TaskNotFound`] when no task has `id`.
    pub fn close_task(&self, id: i64, outcome: TaskOutcome, now_ms: i64) -> Result<TaskItem> {
        let outcome = outcome.as_str();
        let tx = self.conn.unchecked_transaction()?;
        let affected = if outcome == "done" {
            let position: i64 = tx.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks
                 WHERE status = 'done' AND id <> ?1",
                params![id],
                |r| r.get(0),
            )?;
            tx.execute(
                "UPDATE tasks SET status = 'done', position = ?2,
                        completed_at = COALESCE(completed_at, ?3),
                        outcome = ?4, worktree_dir = NULL
                 WHERE id = ?1",
                params![id, position, now_ms, outcome],
            )?
        } else {
            tx.execute(
                "UPDATE tasks SET completed_at = COALESCE(completed_at, ?2),
                        outcome = ?3, worktree_dir = NULL
                 WHERE id = ?1",
                params![id, now_ms, outcome],
            )?
        };
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.commit()?;
        self.task_by_id(id)
    }

    /// Archive one task off the active board now. Archiving twice keeps the
    /// original timestamp. Returns [`Error::TaskNotFound`] when no task has
    /// `id`.
    pub fn archive_task(&self, id: i64, now_ms: i64) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE tasks SET archived_at = COALESCE(archived_at, ?2) WHERE id = ?1",
            params![id, now_ms],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Bring an archived task back onto the board. Its `status` and `outcome`
    /// are left as they were — it reappears in the terminal column, and a
    /// status move out of there reopens it fully. Returns
    /// [`Error::TaskNotFound`] when no task has `id`.
    pub fn unarchive_task(&self, id: i64) -> Result<()> {
        let affected =
            self.conn.execute("UPDATE tasks SET archived_at = NULL WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Archive closed tasks (an `outcome` on record, or sitting in `done`)
    /// that finished before `before_ms`, returning how many were archived.
    /// This replaced the old hard-delete sweep — history is hidden, never
    /// destroyed. Open tasks and recently-finished ones are left untouched; a
    /// closed row with a NULL `completed_at` (legacy data) is never swept,
    /// since its completion time is unknown. Both instants are injected — the
    /// clock read happens at the call boundary, not here.
    pub fn archive_closed_tasks(&self, before_ms: i64, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE tasks SET archived_at = ?2
             WHERE archived_at IS NULL
               AND (outcome IS NOT NULL OR status = 'done')
               AND completed_at IS NOT NULL AND completed_at < ?1",
            params![before_ms, now_ms],
        )?)
    }

    /// Attach a GitHub issue to a task. Re-attaching an existing link only
    /// refreshes the `url` — the cached `state` is preserved (the collector
    /// owns it). Returns [`Error::TaskNotFound`] when no task has `id`.
    pub fn attach_task_issue(&self, id: i64, repo: &str, number: i64, url: &str) -> Result<()> {
        self.require_task(id)?;
        self.conn.execute(
            "INSERT INTO task_issues (task_id, repo, number, url) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id, repo, number) DO UPDATE SET url = excluded.url",
            params![id, repo, number, url],
        )?;
        Ok(())
    }

    /// Detach a GitHub issue from a task. Detaching a link that doesn't exist
    /// is a no-op.
    pub fn detach_task_issue(&self, id: i64, repo: &str, number: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM task_issues WHERE task_id = ?1 AND repo = ?2 AND number = ?3",
            params![id, repo, number],
        )?;
        Ok(())
    }

    /// Attach a GitHub PR to a task. Re-attaching refreshes only the `url`
    /// (state/checks stay collector-owned). Returns [`Error::TaskNotFound`]
    /// when no task has `id`.
    pub fn attach_task_pr(&self, id: i64, repo: &str, number: i64, url: &str) -> Result<()> {
        self.require_task(id)?;
        self.conn.execute(
            "INSERT INTO task_prs (task_id, repo, number, url) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id, repo, number) DO UPDATE SET url = excluded.url",
            params![id, repo, number, url],
        )?;
        Ok(())
    }

    /// Detach a GitHub PR from a task. Detaching a link that doesn't exist is
    /// a no-op.
    pub fn detach_task_pr(&self, id: i64, repo: &str, number: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM task_prs WHERE task_id = ?1 AND repo = ?2 AND number = ?3",
            params![id, repo, number],
        )?;
        Ok(())
    }

    /// Bind a task to its repo, and to the worktree its work happens in
    /// once one exists. Called twice in the Agentboard's new-task flow: at
    /// submit with the repo alone (`branch`/`dir` `None`), then again once
    /// `task_create` resolves. A "task only" submit stops after the first.
    ///
    /// The optional columns are upserts, never clears: a `None` means "leave
    /// as is" (`COALESCE`), so a repo-only rebind — e.g. retrying a failed
    /// `task_create` on a task whose worktree already exists — can't erase an
    /// established branch/dir. Nothing here un-sets a branch or a dir: the
    /// one legitimate detach is [`Store::close_task`], which clears `dir` (the
    /// worktree is off disk) while `repo_root`/`branch` survive as historical
    /// fact. Returns [`Error::TaskNotFound`] when no task has `id`.
    pub fn set_task_worktree(
        &self,
        id: i64,
        repo_root: &str,
        repo: Option<&str>,
        branch: Option<&str>,
        dir: Option<&str>,
    ) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE tasks SET worktree_repo_root = ?1,
                              worktree_repo = COALESCE(?2, worktree_repo),
                              worktree_branch = COALESCE(?3, worktree_branch),
                              worktree_dir = COALESCE(?4, worktree_dir)
             WHERE id = ?5",
            params![repo_root, repo, branch, dir, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Reconcile the tracked-repo identity cache to exactly `repos`
    /// (`repo_root` -> `owner_repo` pairs): upsert each pair, then delete any
    /// existing row whose `repo_root` isn't in the set. The Agentboard poll
    /// loop calls this every cycle with the currently tracked repos and their
    /// freshly-derived git origin, so untracking a repo (or its origin
    /// becoming unparseable) drops its row on the next poll with no separate
    /// untrack step — `repos.json` stays the one source of truth for which
    /// repos exist, and this table can never drift into holding a stale one.
    pub fn reconcile_repos(&self, repos: &[(String, String)], now_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut upsert = tx.prepare(
                "INSERT INTO repos (repo_root, owner_repo, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(repo_root) DO UPDATE SET owner_repo = excluded.owner_repo,
                                                       updated_at = excluded.updated_at",
            )?;
            for (repo_root, owner_repo) in repos {
                upsert.execute(params![repo_root, owner_repo, now_ms])?;
            }
            if repos.is_empty() {
                tx.execute("DELETE FROM repos", [])?;
            } else {
                let placeholders = repos.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                let mut del = tx.prepare(&format!(
                    "DELETE FROM repos WHERE repo_root NOT IN ({placeholders})"
                ))?;
                let roots: Vec<&String> = repos.iter().map(|(root, _)| root).collect();
                del.execute(rusqlite::params_from_iter(roots))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The tracked repo root for a given `owner/repo` slug, if the identity
    /// cache currently knows it. `task_create` validates its `repo` argument
    /// against this instead of matching a dir/basename.
    pub fn repo_root_for_owner_repo(&self, owner_repo: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT repo_root FROM repos WHERE owner_repo = ?1",
                params![owner_repo],
                |r| r.get(0),
            )
            .optional()
            .map_err(Error::from)
    }

    /// Every tracked repo's `owner/repo` slug, sorted for a stable error
    /// message when `task_create` rejects an unknown `repo` argument.
    pub fn repo_slugs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT owner_repo FROM repos ORDER BY owner_repo")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    /// Replace only the named repos' PR rows, leaving other repos' rows intact.
    /// See [`Store::replace_issues_for_repos`] for the failure-containment
    /// rationale.
    pub fn replace_prs_for_repos(&self, repos: &[String], prs: &[PrInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM pr_status WHERE repo = ?1")?;
            for repo in repos {
                del.execute(params![repo])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    /// Full-snapshot replace of PR status rows.
    pub fn replace_prs(&self, prs: &[PrInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM pr_status", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    /// Replace only the non-merged PR rows for `repos`, leaving each repo's
    /// merged rows and every other repo's rows intact. Used by the fast,
    /// frequent open-PR sweep so it never has to re-fetch (and thus never
    /// clobbers) the separately-cadenced merged-PR rows — see
    /// [`Store::replace_merged_prs_for_repos`].
    pub fn replace_open_prs_for_repos(&self, repos: &[String], prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_for_repos_where(repos, prs, "state != 'merged'")
    }

    /// Full-snapshot replace of the non-merged PR rows, preserving merged rows.
    pub fn replace_open_prs(&self, prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_where(prs, "state != 'merged'")
    }

    /// Replace only the merged PR rows for `repos`, leaving each repo's open
    /// rows intact. See [`Store::replace_open_prs_for_repos`].
    pub fn replace_merged_prs_for_repos(&self, repos: &[String], prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_for_repos_where(repos, prs, "state = 'merged'")
    }

    /// Full-snapshot replace of the merged PR rows, preserving open rows.
    pub fn replace_merged_prs(&self, prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_where(prs, "state = 'merged'")
    }

    fn replace_prs_for_repos_where(
        &self,
        repos: &[String],
        prs: &[PrInput],
        state_predicate: &str,
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx
                .prepare(&format!("DELETE FROM pr_status WHERE repo = ?1 AND {state_predicate}"))?;
            for repo in repos {
                del.execute(params![repo])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    fn replace_prs_where(&self, prs: &[PrInput], state_predicate: &str) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(&format!("DELETE FROM pr_status WHERE {state_predicate}"), [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    /// Upsert the latest state of a watched DM conversation. `dismissed_ts` is
    /// preserved across upserts — dismissal is user state, not collector state.
    pub fn upsert_dm(&self, dm: &DmInput, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO dm_status (channel, from_name, text, ts, from_me, url, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(channel) DO UPDATE SET
               from_name = excluded.from_name, text = excluded.text, ts = excluded.ts,
               from_me = excluded.from_me, url = excluded.url, fetched_at = excluded.fetched_at",
            params![
                dm.channel,
                dm.from_name,
                dm.text,
                dm.ts,
                dm.from_me,
                dm.url,
                now_ms
            ],
        )?;
        Ok(())
    }

    /// Mark the message at `ts` in `channel` handled: the UI stops showing it.
    /// A newer message (larger `ts`) re-raises the banner.
    pub fn dismiss_dm(&self, channel: &str, ts: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE dm_status SET dismissed_ts = ?2 WHERE channel = ?1",
            params![channel, ts],
        )?;
        Ok(())
    }

    /// Dismiss one GitHub item (`kind` is `"issue"` or `"pr"`) at `(repo,
    /// number)`, recording the `updated_ts` it had at dismissal time — the UI
    /// re-shows it once the collector observes a newer `updated_ts` (see
    /// [`IssueItem::dismissed_ts`]).
    pub fn dismiss_item(&self, kind: &str, repo: &str, number: i64, updated_ts: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO item_dismissals (kind, repo, number, dismissed_ts) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, repo, number) DO UPDATE SET dismissed_ts = excluded.dismissed_ts",
            params![kind, repo, number, updated_ts],
        )?;
        Ok(())
    }

    /// Clear every stored dismissal — every previously dismissed issue/PR
    /// reappears. Returns how many were cleared.
    pub fn clear_dismissals(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM item_dismissals", [])?)
    }

    /// Record the outcome of a collector run (one row per collector, upserted).
    pub fn record_run(
        &self,
        collector: &str,
        ok: bool,
        message: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO collect_runs (collector, ran_at, ok, message) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(collector) DO UPDATE SET
               ran_at = excluded.ran_at, ok = excluded.ok, message = excluded.message",
            params![collector, now_ms, ok, message],
        )?;
        Ok(())
    }

    /// Append one handled MCP request to the call log, pruning rows beyond the
    /// newest [`MCP_CALL_RETAIN`] so the log never grows unbounded.
    pub fn record_mcp_call(&self, call: &McpCallInput, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mcp_calls (ts, method, tool, args, ok, error, duration_ms, client)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now_ms,
                call.method,
                call.tool,
                call.args,
                call.ok,
                call.error,
                call.duration_ms,
                call.client,
            ],
        )?;
        self.conn.execute(
            "DELETE FROM mcp_calls WHERE id NOT IN
               (SELECT id FROM mcp_calls ORDER BY id DESC LIMIT ?1)",
            params![MCP_CALL_RETAIN],
        )?;
        Ok(())
    }

    /// The newest `limit` MCP call-log rows, newest first.
    pub fn mcp_calls(&self, limit: usize) -> Result<Vec<McpCall>> {
        self.query_mcp_calls(
            &format!("SELECT {MCP_CALL_COLS} FROM mcp_calls ORDER BY id DESC LIMIT ?1"),
            [limit as i64],
        )
    }

    // --- Queries ----------------------------------------------------------

    /// Events starting within `[start_ms, end_ms)`, ordered by start time.
    pub fn events_between(&self, start_ms: i64, end_ms: i64) -> Result<Vec<CalEvent>> {
        self.query_events(
            &format!(
                "SELECT {EVENT_COLS} FROM events
                 WHERE starts_at_utc >= ?1 AND starts_at_utc < ?2 ORDER BY starts_at_utc ASC"
            ),
            params![utc_key(start_ms), utc_key(end_ms)],
        )
    }

    /// The meeting to surface at `now_ms`: the one in progress right now, or
    /// the soonest still to start — whichever begins first.
    ///
    /// An event counts as in progress while `start_ts <= now_ms < end_ts`, so
    /// a meeting stays selected until it actually ends rather than vanishing
    /// the instant it starts. An event with no `end_ts` is treated as a point
    /// in time and is only returned while still in the future
    /// (`start_ts >= now_ms`). Returns `None` once the last event has ended.
    pub fn current_or_next_event(&self, now_ms: i64) -> Result<Option<CalEvent>> {
        Ok(self
            .query_events(
                &format!(
                    "SELECT {EVENT_COLS} FROM events
                     WHERE (ends_at_utc IS NOT NULL AND ends_at_utc > ?1)
                        OR (ends_at_utc IS NULL AND starts_at_utc >= ?1)
                     ORDER BY starts_at_utc ASC LIMIT 1"
                ),
                [utc_key(now_ms)],
            )?
            .into_iter()
            .next())
    }

    /// Open todos in kanban order: not in `done`, not closed with an
    /// `outcome`, not archived.
    pub fn open_tasks(&self) -> Result<Vec<TaskItem>> {
        self.query_tasks(
            &format!(
                "SELECT {TASK_COLS} FROM tasks
                 WHERE status != 'done' AND outcome IS NULL AND archived_at IS NULL {TASK_ORDER}"
            ),
            [],
        )
    }

    /// A single todo by id, if it exists.
    pub fn get_task(&self, id: i64) -> Result<Option<TaskItem>> {
        Ok(self
            .query_tasks(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [id])?
            .into_iter()
            .next())
    }

    /// All tasks in kanban order, links and worktree included. The collectors'
    /// rollup walks this; the board gets it via [`Store::snapshot`].
    pub fn all_tasks(&self) -> Result<Vec<TaskItem>> {
        self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks {TASK_ORDER}"), [])
    }

    /// Distinct `(repo, number)` PR refs linked to any task.
    pub fn linked_pr_refs(&self) -> Result<Vec<(String, i64)>> {
        self.query_refs("SELECT DISTINCT repo, number FROM task_prs ORDER BY repo, number")
    }

    /// Issue refs whose cached link state is still `open` but which are
    /// missing from the collector's `issues` snapshot — the ambiguous set
    /// (closed? reassigned away?) that needs a targeted `gh issue view`.
    /// Terminal-state links absent from the snapshot are *not* returned:
    /// their cached state stands until the ref reappears in the snapshot.
    pub fn open_issue_refs_missing_from_cache(&self) -> Result<Vec<(String, i64)>> {
        self.query_refs(
            "SELECT DISTINCT ti.repo, ti.number FROM task_issues ti
             WHERE ti.state = 'open'
               AND NOT EXISTS (SELECT 1 FROM issues i
                               WHERE i.repo = ti.repo AND i.number = ti.number)
             ORDER BY ti.repo, ti.number",
        )
    }

    /// PR refs whose cached link state is still `open` but which are missing
    /// from the `pr_status` snapshot. See
    /// [`Store::open_issue_refs_missing_from_cache`].
    pub fn open_pr_refs_missing_from_cache(&self) -> Result<Vec<(String, i64)>> {
        self.query_refs(
            "SELECT DISTINCT tp.repo, tp.number FROM task_prs tp
             WHERE tp.state = 'open'
               AND NOT EXISTS (SELECT 1 FROM pr_status p
                               WHERE p.repo = tp.repo AND p.number = tp.number)
             ORDER BY tp.repo, tp.number",
        )
    }

    /// Stamp the observed state onto every link row for one issue ref.
    pub fn set_issue_link_state(
        &self,
        repo: &str,
        number: i64,
        state: &str,
        now_ms: i64,
    ) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE task_issues SET state = ?3, state_ts = ?4
             WHERE repo = ?1 AND number = ?2",
            params![repo, number, state, now_ms],
        )?)
    }

    /// Stamp the observed state onto every link row for one PR ref. `checks`
    /// updates when given; `None` keeps the cached value (the targeted fetch
    /// only learns the state).
    pub fn set_pr_link_state(
        &self,
        repo: &str,
        number: i64,
        state: &str,
        checks: Option<&str>,
        now_ms: i64,
    ) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE task_prs SET state = ?3, checks = COALESCE(?4, checks), state_ts = ?5
             WHERE repo = ?1 AND number = ?2",
            params![repo, number, state, checks, now_ms],
        )?)
    }

    /// Refresh every issue/PR link row whose ref is present in the collector
    /// snapshot (`issues` / `pr_status`), copying state (and checks) across.
    /// Refs absent from the snapshot are left untouched — see the targeted
    /// fetch in `tt-collect` for those.
    pub fn refresh_link_states_from_cache(&self, now_ms: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let issues = tx.execute(
            "UPDATE task_issues SET
               state = (SELECT i.state FROM issues i
                        WHERE i.repo = task_issues.repo AND i.number = task_issues.number),
               state_ts = ?1
             WHERE EXISTS (SELECT 1 FROM issues i
                           WHERE i.repo = task_issues.repo AND i.number = task_issues.number)",
            params![now_ms],
        )?;
        let prs = tx.execute(
            "UPDATE task_prs SET
               state = (SELECT p.state FROM pr_status p
                        WHERE p.repo = task_prs.repo AND p.number = task_prs.number),
               checks = (SELECT p.checks FROM pr_status p
                         WHERE p.repo = task_prs.repo AND p.number = task_prs.number),
               state_ts = ?1
             WHERE EXISTS (SELECT 1 FROM pr_status p
                           WHERE p.repo = task_prs.repo AND p.number = task_prs.number)",
            params![now_ms],
        )?;
        tx.commit()?;
        Ok(issues + prs)
    }

    /// Auto-attach collected PRs to worktree-bound tasks: any `pr_status` row
    /// whose `(repo, branch)` matches a task's `(worktree_repo, worktree_branch)`
    /// becomes a `task_prs` link — "PRs open in the worktree, linked to the task"
    /// without a manual step. Existing links are left untouched. Archived
    /// tasks are excluded: their kept `branch` is historical fact, and a
    /// reused branch name must not link a future PR to a long-dead task. A
    /// merely *closed* task still attaches — a PR that merges right as the
    /// worktree is deleted completes the record. Returns how many links were
    /// created.
    pub fn auto_attach_worktree_prs(&self, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO task_prs (task_id, repo, number, url, state, checks, state_ts)
             SELECT t.id, p.repo, p.number, p.url, p.state, p.checks, ?1
             FROM tasks t
             JOIN pr_status p ON p.repo = t.worktree_repo AND p.branch = t.worktree_branch
             WHERE t.worktree_repo IS NOT NULL AND t.worktree_branch IS NOT NULL
               AND t.archived_at IS NULL",
            params![now_ms],
        )?)
    }

    /// The task bound to the worktree at `dir`, if any (a worktree belongs to at
    /// most one task; if data ever disagrees, the oldest task wins).
    pub fn task_for_worktree_dir(&self, dir: &str) -> Result<Option<TaskItem>> {
        Ok(self
            .query_tasks(
                &format!(
                    "SELECT {TASK_COLS} FROM tasks WHERE worktree_dir = ?1
                     ORDER BY created_at ASC LIMIT 1"
                ),
                params![dir],
            )?
            .into_iter()
            .next())
    }

    /// All issue rows, newest update first.
    pub fn issues(&self) -> Result<Vec<IssueItem>> {
        self.query_issues(
            &format!(
                "SELECT {ISSUE_COLS} FROM issues i \
                 LEFT JOIN item_dismissals d \
                   ON d.kind = 'issue' AND d.repo = i.repo AND d.number = i.number \
                 ORDER BY i.updated_ts DESC"
            ),
            [],
        )
    }

    /// A single cached issue row by `(repo, number)`, if the collector has seen it.
    pub fn get_issue(&self, repo: &str, number: i64) -> Result<Option<IssueItem>> {
        Ok(self
            .query_issues(
                &format!(
                    "SELECT {ISSUE_COLS} FROM issues i \
                     LEFT JOIN item_dismissals d \
                       ON d.kind = 'issue' AND d.repo = i.repo AND d.number = i.number \
                     WHERE i.repo = ?1 AND i.number = ?2"
                ),
                params![repo, number],
            )?
            .into_iter()
            .next())
    }

    /// All PR status rows, newest update first.
    pub fn prs(&self) -> Result<Vec<PrItem>> {
        self.query_prs(
            &format!(
                "SELECT {PR_COLS} FROM pr_status p \
                 LEFT JOIN item_dismissals d \
                   ON d.kind = 'pr' AND d.repo = p.repo AND d.number = p.number \
                 ORDER BY p.updated_ts DESC"
            ),
            [],
        )
    }

    /// All collector run records, ordered by collector name.
    pub fn runs(&self) -> Result<Vec<CollectRun>> {
        self.query_runs(&format!("SELECT {RUN_COLS} FROM collect_runs ORDER BY collector ASC"), [])
    }

    /// All watched DM conversations, newest message first.
    pub fn dms(&self) -> Result<Vec<DmItem>> {
        self.query_dms(&format!("SELECT {DM_COLS} FROM dm_status ORDER BY ts DESC"), [])
    }

    /// A single full snapshot of the store for the dashboard. The reads share
    /// one transaction so a concurrent writer (CLI collector, another window)
    /// can't produce a torn cross-table view.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let tx = self.conn.unchecked_transaction()?;
        let events = self.query_events(
            &format!("SELECT {EVENT_COLS} FROM events ORDER BY starts_at_utc ASC"),
            [],
        )?;
        let tasks = self.all_tasks()?;
        let issues = self.issues()?;
        let prs = self.prs()?;
        let runs = self.runs()?;
        let dms = self.dms()?;
        let mcp_calls = self.mcp_calls(MCP_CALL_SNAPSHOT_LIMIT)?;
        tx.commit()?;
        Ok(Snapshot { events, tasks, issues, prs, runs, dms, mcp_calls })
    }

    // --- Row-mapping helpers ---------------------------------------------

    /// One task by id, with its links and worktree binding (the same row shape
    /// [`Store::open_tasks`] returns).
    pub fn task_by_id(&self, id: i64) -> Result<TaskItem> {
        self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [id])?
            // `TaskNotFound`, like every other id lookup in this module — not a
            // fabricated `Sqlite(QueryReturnedNoRows)`. A caller has to be able
            // to tell "this row does not exist" from "the database could not
            // answer", and the `?` above already carries the genuine failures.
            .into_iter()
            .next()
            .ok_or(Error::TaskNotFound(id))
    }

    fn query_events(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<CalEvent>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                source,
                external_id,
                title,
                starts_at,
                ends_at,
                attendees_json,
                location,
                join_url,
            ) = row?;
            let attendees: Vec<String> = serde_json::from_str(&attendees_json)?;
            // Rows are written from a `DateTime`, so a value that no longer
            // parses means the column was edited by hand or by another tool.
            // Skip that row rather than failing the whole query: one bad row
            // must not blank the countdown, and it ages out with retention.
            let Some(start) = parse_rfc3339(&starts_at) else {
                log_unparseable_event(&external_id, &starts_at);
                continue;
            };
            out.push(CalEvent {
                id,
                source,
                external_id,
                title,
                start,
                end: ends_at.as_deref().and_then(parse_rfc3339),
                attendees,
                location,
                join_url,
            });
        }
        Ok(out)
    }

    fn query_tasks(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<TaskItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            let worktree_repo_root: Option<String> = r.get(7)?;
            let worktree_repo: Option<String> = r.get(8)?;
            let worktree_branch: Option<String> = r.get(9)?;
            let worktree_dir: Option<String> = r.get(10)?;
            let outcome: Option<String> = r.get(11)?;
            let archived_at: Option<i64> = r.get(12)?;
            // Keyed on `repo_root` alone: a repo-bound task with no worktree
            // yet still has a worktree binding, and dropping it here would hide
            // the task's repo from the Board's swimlanes.
            let worktree = worktree_repo_root.map(|repo_root| TaskWorktree {
                repo_root,
                repo: worktree_repo,
                branch: worktree_branch,
                dir: worktree_dir,
            });
            Ok(TaskItem {
                id: r.get(0)?,
                text: r.get(1)?,
                status: r.get(2)?,
                position: r.get(3)?,
                created_at: r.get(4)?,
                completed_at: r.get(5)?,
                notes: r.get(6)?,
                outcome,
                archived_at,
                worktree,
                issues: Vec::new(),
                prs: Vec::new(),
                closed: false,
                display_outcome: None,
                has_worktree: false,
            }
            .with_derived_fields())
        })?;
        let mut tasks = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_task_links(&mut tasks)?;
        Ok(tasks)
    }

    /// Fill `issues`/`prs` on already-mapped tasks. Loads both link tables
    /// whole (they are small — one row per attached ref) and distributes by
    /// `task_id`, keeping `(repo, number)` order deterministic.
    fn load_task_links(&self, tasks: &mut [TaskItem]) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        use std::collections::HashMap;
        let mut issues: HashMap<i64, Vec<TaskIssueLink>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT task_id, repo, number, url, state FROM task_issues
                 ORDER BY task_id, repo, number",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    TaskIssueLink {
                        repo: r.get(1)?,
                        number: r.get(2)?,
                        url: r.get(3)?,
                        state: r.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (task_id, link) = row?;
                issues.entry(task_id).or_default().push(link);
            }
        }
        let mut prs: HashMap<i64, Vec<TaskPrLink>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT task_id, repo, number, url, state, checks FROM task_prs
                 ORDER BY task_id, repo, number",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    TaskPrLink {
                        repo: r.get(1)?,
                        number: r.get(2)?,
                        url: r.get(3)?,
                        state: r.get(4)?,
                        checks: r.get(5)?,
                    },
                ))
            })?;
            for row in rows {
                let (task_id, link) = row?;
                prs.entry(task_id).or_default().push(link);
            }
        }
        for task in tasks.iter_mut() {
            if let Some(links) = issues.remove(&task.id) {
                task.issues = links;
            }
            if let Some(links) = prs.remove(&task.id) {
                task.prs = links;
            }
        }
        Ok(())
    }

    fn query_refs(&self, sql: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Error with [`Error::TaskNotFound`] unless a task with `id` exists.
    fn require_task(&self, id: i64) -> Result<()> {
        let exists = self.conn.prepare("SELECT 1 FROM tasks WHERE id = ?1")?.exists(params![id])?;
        if exists { Ok(()) } else { Err(Error::TaskNotFound(id)) }
    }

    fn query_issues(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<IssueItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (repo, number, title, labels_json, state, url, updated_ts, dismissed_ts) = row?;
            let labels: Vec<String> = serde_json::from_str(&labels_json)?;
            out.push(IssueItem {
                repo,
                number,
                title,
                labels,
                state,
                url,
                updated_ts,
                dismissed_ts,
            });
        }
        Ok(out)
    }

    fn query_prs(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<PrItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(PrItem {
                repo: r.get(0)?,
                number: r.get(1)?,
                title: r.get(2)?,
                branch: r.get(3)?,
                state: r.get(4)?,
                checks: r.get(5)?,
                review_state: r.get(6)?,
                url: r.get(7)?,
                updated_ts: r.get(8)?,
                dismissed_ts: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn query_dms(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<DmItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(DmItem {
                channel: r.get(0)?,
                from_name: r.get(1)?,
                text: r.get(2)?,
                ts: r.get(3)?,
                from_me: r.get(4)?,
                url: r.get(5)?,
                fetched_at: r.get(6)?,
                dismissed_ts: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn query_runs(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<CollectRun>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(CollectRun {
                collector: r.get(0)?,
                ran_at: r.get(1)?,
                ok: r.get(2)?,
                message: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn query_mcp_calls(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<McpCall>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(McpCall {
                id: r.get(0)?,
                ts: r.get(1)?,
                method: r.get(2)?,
                tool: r.get(3)?,
                args: r.get(4)?,
                ok: r.get(5)?,
                error: r.get(6)?,
                duration_ms: r.get(7)?,
                client: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Raw `task_issues` rows — the observable for delete-cascade tests, now
    /// that no production API exposes the table's contents directly.
    fn issue_link_rows(s: &Store) -> Vec<(i64, String, i64)> {
        let mut stmt = s
            .conn
            .prepare("SELECT task_id, repo, number FROM task_issues ORDER BY repo, number")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    /// Epoch ms -> the `DateTime<FixedOffset>` the event types now hold. UTC,
    /// since these tests assert on instants, not on presentation.
    fn at(ms: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp_millis(ms).unwrap().fixed_offset()
    }

    fn event(ext: &str, start: i64) -> EventInput {
        EventInput {
            external_id: ext.to_string(),
            title: format!("Event {ext}"),
            start: at(start),
            end: Some(at(start + 1000)),
            attendees: vec!["a@example.com".to_string()],
            location: None,
            join_url: None,
        }
    }

    /// Write events for tests that don't care about source/day scoping: one
    /// source, a window wide enough to sweep everything. Tests that DO care
    /// about the scoping call [`Store::replace_events_for_source`] directly.
    fn put_events(s: &Store, events: &[EventInput], now_ms: i64) -> Result<usize> {
        s.replace_events_for_source("test", i64::MIN, i64::MAX, events, now_ms)
    }

    fn issue(repo: &str, number: i64, updated: i64) -> IssueInput {
        IssueInput {
            repo: repo.to_string(),
            number,
            title: format!("Issue {number}"),
            labels: vec!["bug".to_string()],
            state: "open".to_string(),
            url: format!("https://github.com/{repo}/issues/{number}"),
            updated_ts: updated,
        }
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let s = Store::open(&path).unwrap();
            s.add_task("survives", "backlog", None, 1).unwrap();
        }
        // Re-open: migrate runs again without error, data intact.
        let s = Store::open(&path).unwrap();
        assert_eq!(s.open_tasks().unwrap().len(), 1);
        let version: String = s
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn migrate_brings_pre_kanban_tasks_table_forward() {
        // Reproduces a db created before the day-screens pivot: `tasks` has the
        // old source/source_ref/done columns and no status/position/repo/
        // issue_number/issue_url, plus the since-removed `emails` table.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    source_ref TEXT,
                    text TEXT NOT NULL,
                    due_ts INTEGER,
                    done INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER
                );
                CREATE TABLE emails (id INTEGER PRIMARY KEY);
                INSERT INTO tasks (source, text, done, created_at)
                    VALUES ('manual', 'old todo', 0, 1),
                           ('manual', 'finished todo', 1, 2);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let snapshot = s.snapshot().unwrap();
        assert_eq!(snapshot.tasks.len(), 2);
        assert!(snapshot.tasks.iter().any(|t| t.text == "old todo" && t.status == "backlog"));
        assert!(snapshot.tasks.iter().any(|t| t.text == "finished todo" && t.status == "done"));

        // Writes must work too: the legacy NOT-NULL `source` column has to be
        // gone, or every INSERT that omits it fails.
        let added = s.add_task("new todo", "backlog", None, 3).unwrap();
        assert_eq!(added.status, "backlog");
        assert!(!task_columns(&s).contains(&"source".to_string()));

        let has_emails: bool = s
            .conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'emails'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(!has_emails, "dead `emails` table should be dropped");
    }

    fn task_columns(s: &Store) -> Vec<String> {
        let mut stmt = s.conn.prepare("PRAGMA table_info(tasks)").unwrap();
        let cols = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
        cols.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    #[test]
    fn migrate_repairs_half_migrated_tasks_table() {
        // A db the old ALTER-based migration already touched: v2 columns exist,
        // but the legacy NOT-NULL `source` column is still present, so inserts
        // that omit it fail. The rebuild must keep the v2 values it finds.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    source_ref TEXT,
                    text TEXT NOT NULL,
                    due_ts INTEGER,
                    done INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT
                );
                INSERT INTO tasks (source, text, done, created_at, status, position, repo,
                                   issue_number, issue_url)
                    VALUES ('manual', 'linked todo', 0, 1, 'doing', 2, 'o/r', 7,
                            'https://github.com/o/r/issues/7');",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let t = s.snapshot().unwrap().tasks.into_iter().find(|t| t.text == "linked todo").unwrap();
        assert_eq!(t.status, "doing");
        assert_eq!(t.position, 2);
        // The old single link came through the v2 rebuild AND the v7 port
        // into the task_issues link table.
        assert_eq!(t.issues.len(), 1);
        assert_eq!(t.issues[0].repo, "o/r");
        assert_eq!(t.issues[0].number, 7);
        assert_eq!(t.issues[0].state, "open");
        s.add_task("post-repair todo", "backlog", None, 9).unwrap();
        assert!(!task_columns(&s).contains(&"source".to_string()));
        assert!(!task_columns(&s).contains(&"repo".to_string()));
    }

    /// `local_day_bounds` exists in one place because it scopes a DELETE, and
    /// two implementations had already drifted apart on DST handling — one
    /// widening to a ±1-day window, the other collapsing to an empty one, both
    /// feeding the same destructive call. These pin the properties that make
    /// the shared version safe to hand to a delete.
    #[test]
    fn local_day_bounds_is_a_single_day_that_contains_its_reference() {
        // A few instants spread across the year, including both DST changeover
        // weekends in the northern hemisphere.
        for reference in [
            1_700_000_000_000_i64, // Nov 2023
            1_678_600_000_000,     // Mar 2023 (spring forward)
            1_699_164_000_000,     // Nov 2023 (fall back)
            1_719_000_000_000,     // Jun 2024
            0,                     // epoch
        ] {
            let (start, end) = Store::local_day_bounds(reference);
            assert!(start <= reference, "window starts at or before its reference ({reference})");
            assert!(reference < end, "window contains its reference ({reference})");
            let span = end - start;
            // A civil day is 23, 24 or 25 hours long depending on DST. Never more.
            assert!(
                (23 * 3_600_000..=25 * 3_600_000).contains(&span),
                "span {span}ms for {reference} is not one civil day"
            );
        }
    }

    /// The fallback direction is the safety property: if the boundary can't be
    /// resolved, delete nothing rather than delete more. Stale rows are fixed by
    /// the next pull; over-deleted rows are gone.
    #[test]
    fn local_day_bounds_never_widens_past_a_day() {
        let (start, end) = Store::local_day_bounds(i64::MAX);
        assert!(end - start <= 25 * 3_600_000, "degenerate input must not widen the delete");
    }

    #[test]
    fn replace_events_swaps_within_one_source_and_day() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("a", 100), event("b", 200)], 1)
            .unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2);
        let n = s.replace_events_for_source("google", 0, 1000, &[event("c", 300)], 2).unwrap();
        assert_eq!(n, 1);
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1, "the earlier pull for this source+day is swept");
        assert_eq!(events[0].external_id, "c");
        assert_eq!(events[0].source, "google", "source is recorded for provenance");
    }

    /// The reason this method exists: two calendars are pulled independently and
    /// merged into one timeline. Under the old full-table swap, whichever pulled
    /// second erased the first.
    #[test]
    fn one_sources_pull_never_disturbs_another() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("personal", 100)], 1).unwrap();
        s.replace_events_for_source("outlook", 0, 1000, &[event("work", 200)], 1).unwrap();

        let merged = s.snapshot().unwrap().events;
        let ids: Vec<&str> = merged.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["personal", "work"], "both calendars coexist, merged by start_ts");

        // Re-pulling one source replaces only its own lane.
        s.replace_events_for_source("google", 0, 1000, &[event("personal-v2", 100)], 2).unwrap();
        let events = s.snapshot().unwrap().events;
        let ids: Vec<&str> = events.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["personal-v2", "work"], "the work calendar survives untouched");
    }

    /// The delete is scoped to the day window too, so pulling today never drops
    /// a tomorrow's-events row that some other call stored.
    #[test]
    fn replace_events_leaves_other_days_alone() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, i64::MAX, &[event("tomorrow", 5_000)], 1).unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("today", 100)], 2).unwrap();

        let events = s.snapshot().unwrap().events;
        let ids: Vec<&str> = events.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["today", "tomorrow"], "out-of-window row untouched");
    }

    /// The scoped delete bounds one lane and one day, so something else has to
    /// bound the table over time — otherwise yesterday's meetings, and every row
    /// from a calendar the user renamed or removed, accumulate forever. The old
    /// full-table swap did that implicitly; this pins the replacement.
    #[test]
    fn old_events_are_swept_including_orphaned_lanes() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        let stale = now - EVENT_RETAIN_MS - 1;

        // A row from a lane that will never be written again (its source was
        // removed from settings), old enough to be past retention.
        s.replace_events_for_source(
            "retired-calendar",
            0,
            i64::MAX,
            &[event("ancient", stale)],
            now,
        )
        .unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 1);

        // A write to a *different* lane sweeps it — age, not source, is what
        // catches an orphan, since no per-source write will ever visit it again.
        s.replace_events_for_source("google", now, now + 86_400_000, &[event("today", now)], now)
            .unwrap();
        let ids: Vec<String> =
            s.snapshot().unwrap().events.iter().map(|e| e.external_id.clone()).collect();
        assert_eq!(ids, vec!["today"], "the orphaned lane's stale row is gone");
    }

    /// A repeated `externalId` inside one payload used to have the upsert
    /// overwrite its own earlier row mid-loop: one row landed, the other
    /// vanished, and the returned count still claimed both were written.
    #[test]
    fn duplicate_external_ids_in_one_payload_are_collapsed_and_counted_honestly() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        let mut first = event("abc", now + 1000);
        first.title = "Standup (9am)".to_string();
        let mut second = event("abc", now + 5000);
        second.title = "Standup (2pm)".to_string();

        let written = s
            .replace_events_for_source("google", now, now + 86_400_000, &[first, second], now)
            .unwrap();
        assert_eq!(written, 1, "count reflects rows that landed, not payload length");
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Standup (2pm)", "last occurrence wins, matching the upsert");
    }

    /// Retention can't be hostage to a write happening: switching the last
    /// calendar off means no write ever runs again, so the sweep has to be
    /// callable on its own or stale rows live forever.
    #[test]
    fn sweep_old_events_works_without_a_write() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        s.replace_events_for_source(
            "google",
            0,
            i64::MAX,
            &[
                event("stale", now - EVENT_RETAIN_MS - 1),
                event("fresh", now),
            ],
            now,
        )
        .unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "both written inside the window");

        // Time passes; nothing writes. The sweep alone must still age it out.
        let later = now + EVENT_RETAIN_MS;
        let removed = s.sweep_old_events(later).unwrap();
        assert_eq!(removed, 1);
        let ids: Vec<String> =
            s.snapshot().unwrap().events.iter().map(|e| e.external_id.clone()).collect();
        assert_eq!(ids, vec!["fresh"]);
    }

    /// Retention must not eat a meeting that hasn't happened yet, nor the recent
    /// past a countdown might still be reasoning about.
    #[test]
    fn retention_keeps_recent_and_future_events() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        s.replace_events_for_source(
            "google",
            0,
            i64::MAX,
            &[
                event("yesterday", now - 86_400_000),
                event("next-week", now + 7 * 86_400_000),
            ],
            now,
        )
        .unwrap();
        s.replace_events_for_source("outlook", now, now + 86_400_000, &[], now).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "both are inside the retention window");
    }

    /// Two providers can legitimately mint the same event id — that's why the
    /// uniqueness rule is `(source, external_id)` and not `external_id` alone.
    #[test]
    fn the_same_external_id_can_exist_in_two_sources() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("shared-id", 100)], 1).unwrap();
        s.replace_events_for_source("outlook", 0, 1000, &[event("shared-id", 200)], 1).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "no unique-constraint collision");
    }

    /// A row outside the swept window with a colliding id must upsert rather
    /// than blow up the whole pull on a constraint violation.
    #[test]
    fn re_pushing_an_out_of_window_id_updates_it_instead_of_failing() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, i64::MAX, &[event("e", 9_000)], 1).unwrap();
        // Window doesn't cover 9_000, so the delete misses it; the insert collides.
        s.replace_events_for_source("google", 0, 1000, &[event("e", 9_000)], 2).unwrap();
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1, "upserted, not duplicated");
    }

    #[test]
    fn replace_issues_is_full_swap_and_decodes_labels() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100), issue("o/r", 2, 200)]).unwrap();
        assert_eq!(s.issues().unwrap().len(), 2);
        // Newest update first.
        assert_eq!(s.issues().unwrap()[0].number, 2);
        assert_eq!(s.issues().unwrap()[0].labels, vec!["bug".to_string()]);
        let n = s.replace_issues(&[issue("o/r", 3, 300)]).unwrap();
        assert_eq!(n, 1);
        let issues = s.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 3);
    }

    #[test]
    fn replace_issues_for_repos_preserves_other_repos_rows() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/a", 1, 100), issue("o/b", 2, 200)]).unwrap();

        // Repo o/a re-collected (now empty); o/b's gh call failed → untouched.
        s.replace_issues_for_repos(&["o/a".to_string()], &[]).unwrap();
        let issues = s.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].repo, "o/b");

        // Scoped write inserts the named repo's fresh rows.
        s.replace_issues_for_repos(&["o/a".to_string()], &[issue("o/a", 9, 900)]).unwrap();
        let issues = s.issues().unwrap();
        let repos: Vec<&str> = issues.iter().map(|i| i.repo.as_str()).collect();
        assert!(repos.contains(&"o/a") && repos.contains(&"o/b"));
    }

    #[test]
    fn reconcile_repos_upserts_and_drops_untracked() {
        let s = Store::open_in_memory().unwrap();
        s.reconcile_repos(
            &[
                ("/repo/a".to_string(), "o/a".to_string()),
                ("/repo/b".to_string(), "o/b".to_string()),
            ],
            100,
        )
        .unwrap();
        assert_eq!(s.repo_root_for_owner_repo("o/a").unwrap().as_deref(), Some("/repo/a"));
        assert_eq!(s.repo_root_for_owner_repo("o/b").unwrap().as_deref(), Some("/repo/b"));

        // /repo/a's origin was renamed and /repo/b fell out of tracking.
        s.reconcile_repos(&[("/repo/a".to_string(), "o/a-renamed".to_string())], 200).unwrap();
        assert_eq!(s.repo_root_for_owner_repo("o/a").unwrap(), None);
        assert_eq!(s.repo_root_for_owner_repo("o/a-renamed").unwrap().as_deref(), Some("/repo/a"));
        assert_eq!(s.repo_root_for_owner_repo("o/b").unwrap(), None);
    }

    #[test]
    fn reconcile_repos_empty_clears_the_cache() {
        let s = Store::open_in_memory().unwrap();
        s.reconcile_repos(&[("/repo/a".to_string(), "o/a".to_string())], 100).unwrap();
        s.reconcile_repos(&[], 200).unwrap();
        assert!(s.repo_slugs().unwrap().is_empty());
    }

    #[test]
    fn attach_detach_issue_links_and_get_issue() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        let plain = s.add_task("plain task", "backlog", None, 1).unwrap();
        let linked = s.add_task("linked task", "backlog", None, 2).unwrap();
        s.attach_task_issue(linked.id, "o/r", 1, "https://github.com/o/r/issues/1").unwrap();
        s.attach_task_issue(linked.id, "o/r", 2, "https://github.com/o/r/issues/2").unwrap();

        let got = s.get_task(linked.id).unwrap().unwrap();
        assert_eq!(got.issues.len(), 2);
        assert_eq!(got.issues[0].state, "open");
        assert!(s.get_task(plain.id).unwrap().unwrap().issues.is_empty());

        // Re-attach refreshes the url but never resets collector-owned state.
        s.set_issue_link_state("o/r", 1, "closed", 5).unwrap();
        s.attach_task_issue(linked.id, "o/r", 1, "https://new.example/1").unwrap();
        let got = s.get_task(linked.id).unwrap().unwrap();
        let one = got.issues.iter().find(|l| l.number == 1).unwrap();
        assert_eq!(one.state, "closed");
        assert_eq!(one.url, "https://new.example/1");

        s.detach_task_issue(linked.id, "o/r", 2).unwrap();
        assert_eq!(s.get_task(linked.id).unwrap().unwrap().issues.len(), 1);
        // Detaching a non-existent link is a no-op, attaching to a missing task errors.
        s.detach_task_issue(linked.id, "o/r", 99).unwrap();
        assert!(matches!(s.attach_task_issue(9999, "o/r", 1, "u"), Err(Error::TaskNotFound(9999))));

        let found = s.get_issue("o/r", 1).unwrap().unwrap();
        assert_eq!(found.number, 1);
        assert!(s.get_issue("o/r", 999).unwrap().is_none());
    }

    #[test]
    fn worktree_binding_set_lookup_and_detach() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("worktree-backed", "doing", None, 1).unwrap();
        assert!(t.worktree.is_none());
        s.set_task_worktree(
            t.id,
            "/repos/x",
            Some("o/x"),
            Some("feat/y"),
            Some("/repos/x/.claude/worktrees/feat-y"),
        )
        .unwrap();

        let bound = s.task_for_worktree_dir("/repos/x/.claude/worktrees/feat-y").unwrap().unwrap();
        assert_eq!(bound.id, t.id);
        let worktree = bound.worktree.unwrap();
        assert_eq!(worktree.repo_root, "/repos/x");
        assert_eq!(worktree.repo.as_deref(), Some("o/x"));
        assert_eq!(worktree.branch.as_deref(), Some("feat/y"));

        // A repo-only rebind (the retry path re-sends the submit-time bind)
        // upserts: `None` means "leave as is", never "clear".
        s.set_task_worktree(t.id, "/repos/x", None, None, None).unwrap();
        let rebound = s.get_task(t.id).unwrap().unwrap().worktree.unwrap();
        assert_eq!(rebound.repo.as_deref(), Some("o/x"));
        assert_eq!(rebound.branch.as_deref(), Some("feat/y"));
        assert_eq!(rebound.dir.as_deref(), Some("/repos/x/.claude/worktrees/feat-y"));

        // Removing the worktree takes the whole task with it — there is no
        // detached-task state to land in (see `set_task_worktree`'s doc).
        s.delete_task(t.id).unwrap();
        assert!(s.task_for_worktree_dir("/repos/x/.claude/worktrees/feat-y").unwrap().is_none());
        assert!(s.get_task(t.id).unwrap().is_none());
        assert!(matches!(
            s.set_task_worktree(777, "/r", None, Some("b"), None),
            Err(Error::TaskNotFound(777))
        ));
    }

    #[test]
    fn refresh_link_states_from_cache_and_missing_refs() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("t", "doing", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 1, "u1").unwrap();
        s.attach_task_issue(t.id, "o/r", 2, "u2").unwrap();
        s.attach_task_pr(t.id, "o/r", 10, "p10").unwrap();

        // Issue 1 is in the snapshot (still open); issue 2 and PR 10 are not.
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        s.refresh_link_states_from_cache(50).unwrap();

        assert_eq!(s.open_issue_refs_missing_from_cache().unwrap(), vec![("o/r".to_string(), 2)]);
        assert_eq!(s.open_pr_refs_missing_from_cache().unwrap(), vec![("o/r".to_string(), 10)]);

        // A targeted fetch resolves the misses; terminal states stop being
        // reported even though they remain absent from the snapshot.
        s.set_issue_link_state("o/r", 2, "closed", 60).unwrap();
        s.set_pr_link_state("o/r", 10, "merged", None, 60).unwrap();
        assert!(s.open_issue_refs_missing_from_cache().unwrap().is_empty());
        assert!(s.open_pr_refs_missing_from_cache().unwrap().is_empty());
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(got.issues.iter().find(|l| l.number == 2).unwrap().state, "closed");
        assert_eq!(got.prs[0].state, "merged");
    }

    #[test]
    fn auto_attach_worktree_prs_links_by_repo_and_branch() {
        let pr = |branch: &str, number: i64| PrInput {
            repo: "o/x".to_string(),
            number,
            title: "t".to_string(),
            branch: branch.to_string(),
            state: "open".to_string(),
            checks: "pending".to_string(),
            review_state: String::new(),
            url: format!("https://github.com/o/x/pull/{number}"),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("worktree task", "doing", None, 1).unwrap();
        s.set_task_worktree(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/w")).unwrap();
        let other = s.add_task("no worktree", "backlog", None, 2).unwrap();

        s.replace_prs(&[pr("feat/y", 7), pr("other-branch", 8)]).unwrap();
        let n = s.auto_attach_worktree_prs(9).unwrap();
        assert_eq!(n, 1);
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(got.prs.len(), 1);
        assert_eq!(got.prs[0].number, 7);
        assert!(s.get_task(other.id).unwrap().unwrap().prs.is_empty());

        // Idempotent: a second pass creates nothing new.
        assert_eq!(s.auto_attach_worktree_prs(10).unwrap(), 0);
    }

    #[test]
    fn replace_prs_for_repos_preserves_other_repos_rows() {
        let pr = |repo: &str, number: i64| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1), pr("o/b", 2)]).unwrap();
        s.replace_prs_for_repos(&["o/a".to_string()], &[pr("o/a", 3)]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.repo == "o/b" && p.number == 2));
        assert!(prs.iter().any(|p| p.repo == "o/a" && p.number == 3));
    }

    #[test]
    fn replace_open_prs_for_repos_preserves_merged_rows() {
        let pr = |repo: &str, number: i64, state: &str| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: state.to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1, "open"), pr("o/a", 2, "merged")]).unwrap();
        // A fresh open-only sweep must not delete the merged row it never fetched.
        s.replace_open_prs_for_repos(&["o/a".to_string()], &[pr("o/a", 3, "open")]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 2 && p.state == "merged"));
        assert!(prs.iter().any(|p| p.number == 3 && p.state == "open"));
        assert!(!prs.iter().any(|p| p.number == 1));
    }

    #[test]
    fn replace_merged_prs_for_repos_preserves_open_rows() {
        let pr = |repo: &str, number: i64, state: &str| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: state.to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1, "open"), pr("o/a", 2, "merged")]).unwrap();
        // A merged-only sweep must not delete the open row it never fetched.
        s.replace_merged_prs_for_repos(&["o/a".to_string()], &[pr("o/a", 4, "merged")]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 1 && p.state == "open"));
        assert!(prs.iter().any(|p| p.number == 4 && p.state == "merged"));
        assert!(!prs.iter().any(|p| p.number == 2));
    }

    #[test]
    fn replace_open_prs_and_replace_merged_prs_are_full_snapshots_scoped_by_state() {
        let pr = |repo: &str, number: i64, state: &str| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: state.to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1, "open"), pr("o/b", 2, "merged")]).unwrap();
        s.replace_open_prs(&[pr("o/a", 3, "open")]).unwrap();
        let prs = s.prs().unwrap();
        // The other repo's open row (none here) would be purged, but its merged
        // row survives; repo o/a's stale open row 1 is gone, replaced by 3.
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 3 && p.state == "open"));
        assert!(prs.iter().any(|p| p.number == 2 && p.state == "merged"));

        s.replace_merged_prs(&[pr("o/b", 5, "merged")]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 3 && p.state == "open"));
        assert!(prs.iter().any(|p| p.number == 5 && p.state == "merged"));
    }

    #[test]
    fn replace_prs_round_trips_every_checks_state() {
        let pr = |number: i64, checks: &str| PrInput {
            repo: "o/r".to_string(),
            number,
            title: format!("pr {number}"),
            branch: format!("b{number}"),
            state: "open".to_string(),
            checks: checks.to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: number,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[
            pr(1, "passing"),
            pr(2, "failing"),
            pr(3, "pending"),
            pr(4, "none"),
        ])
        .unwrap();
        let mut got: Vec<(i64, String)> =
            s.prs().unwrap().into_iter().map(|p| (p.number, p.checks)).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (1, "passing".to_string()),
                (2, "failing".to_string()),
                (3, "pending".to_string()),
                (4, "none".to_string()),
            ]
        );
    }

    #[test]
    fn add_task_lands_in_backlog_and_orders_by_position() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("first", "backlog", None, 100).unwrap();
        let b = s.add_task("second", "backlog", None, 200).unwrap();
        assert_eq!(a.status, "backlog");
        assert_eq!(a.position, 0);
        assert_eq!(b.position, 1);
        let open = s.open_tasks().unwrap();
        let texts: Vec<&str> = open.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "second"]);
    }

    #[test]
    fn set_task_status_moves_columns_and_stamps_done() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship it", "backlog", None, 1).unwrap();
        s.set_task_status(t.id, "doing", 5).unwrap();
        let doing = s.open_tasks().unwrap();
        assert_eq!(doing[0].status, "doing");
        assert_eq!(doing[0].completed_at, None);

        s.set_task_status(t.id, "done", 20).unwrap();
        assert!(s.open_tasks().unwrap().is_empty());
        let done = s.snapshot().unwrap().tasks.into_iter().find(|x| x.id == t.id).unwrap();
        assert_eq!(done.status, "done");
        assert_eq!(done.completed_at, Some(20));

        // Re-opening clears completed_at.
        s.set_task_status(t.id, "backlog", 30).unwrap();
        let reopened = s.open_tasks().unwrap();
        assert_eq!(reopened[0].status, "backlog");
        assert_eq!(reopened[0].completed_at, None);
    }

    #[test]
    fn archive_closed_tasks_sweeps_only_old_finished() {
        let s = Store::open_in_memory().unwrap();
        let old = s.add_task("old done", "backlog", None, 1).unwrap();
        let abandoned = s.add_task("old abandoned", "doing", None, 1).unwrap();
        let recent = s.add_task("recent done", "backlog", None, 2).unwrap();
        let open = s.add_task("still open", "backlog", None, 3).unwrap();
        s.set_task_status(old.id, "done", 100).unwrap();
        s.close_task(abandoned.id, TaskOutcome::Abandoned, 200).unwrap();
        s.set_task_status(recent.id, "done", 5_000).unwrap();
        s.set_task_status(open.id, "doing", 4).unwrap();

        // Cutoff between the finished todos: the old done *and* the old
        // abandoned are archived — the rows survive, hidden, not deleted.
        let archived = s.archive_closed_tasks(1_000, 9_000).unwrap();
        assert_eq!(archived, 2);

        let tasks = s.snapshot().unwrap().tasks;
        let archived_ids: Vec<i64> =
            tasks.iter().filter(|t| t.archived_at.is_some()).map(|t| t.id).collect();
        assert!(archived_ids.contains(&old.id));
        assert!(archived_ids.contains(&abandoned.id));
        assert!(tasks.iter().any(|t| t.id == recent.id && t.archived_at.is_none()));
        assert!(tasks.iter().any(|t| t.id == open.id && t.archived_at.is_none()));

        // Nothing else old enough on a second sweep.
        assert_eq!(s.archive_closed_tasks(1_000, 9_001).unwrap(), 0);
    }

    #[test]
    fn close_task_as_done_lands_in_done_and_detaches_the_dir() {
        let s = Store::open_in_memory().unwrap();
        let done_first = s.add_task("already done", "done", None, 1).unwrap();
        let t = s.add_task("ship it", "doing", None, 2).unwrap();
        s.set_task_worktree(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/repos/x/wt"))
            .unwrap();

        let closed = s.close_task(t.id, TaskOutcome::Done, 500).unwrap();
        assert_eq!(closed.status, "done");
        assert_eq!(closed.outcome.as_deref(), Some("done"));
        assert_eq!(closed.completed_at, Some(500));
        assert!(closed.position > done_first.position, "appended to the done column");
        let wt = closed.worktree.expect("repo binding survives");
        assert_eq!(wt.branch.as_deref(), Some("feat/y"), "branch kept as historical fact");
        assert_eq!(wt.dir, None, "dir cleared — the worktree is gone");
        assert!(s.task_for_worktree_dir("/repos/x/wt").unwrap().is_none());
    }

    #[test]
    fn close_task_as_abandoned_freezes_status() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("didn't pan out", "doing", None, 1).unwrap();

        let closed = s.close_task(t.id, TaskOutcome::Abandoned, 500).unwrap();
        assert_eq!(closed.status, "doing", "status stays where the work stopped");
        assert_eq!(closed.outcome.as_deref(), Some("abandoned"));
        assert_eq!(closed.completed_at, Some(500), "stamped so the archive sweep can age it");
        assert!(!s.open_tasks().unwrap().iter().any(|x| x.id == t.id), "closed = not open");

        // Unknown outcomes never parse; a bad id is TaskNotFound.
        assert_eq!(TaskOutcome::parse("exploded"), None);
        assert_eq!(TaskOutcome::parse("abandoned"), Some(TaskOutcome::Abandoned));
        assert!(matches!(
            s.close_task(9999, TaskOutcome::Done, 501),
            Err(Error::TaskNotFound(9999))
        ));
    }

    #[test]
    fn status_move_out_of_done_reopens_a_closed_task() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("round two", "doing", None, 1).unwrap();
        s.close_task(t.id, TaskOutcome::Done, 100).unwrap();
        s.archive_task(t.id, 200).unwrap();

        // Dragging the card back to an active column clears the whole
        // terminal record: outcome, archive, completed_at.
        s.set_task_status(t.id, "doing", 300).unwrap();
        let back = s.task_by_id(t.id).unwrap();
        assert_eq!(back.outcome, None);
        assert_eq!(back.archived_at, None);
        assert_eq!(back.completed_at, None);

        // A move *within* done (re-close) keeps the record.
        s.close_task(t.id, TaskOutcome::Done, 400).unwrap();
        s.set_task_status(t.id, "done", 500).unwrap();
        assert_eq!(s.task_by_id(t.id).unwrap().outcome.as_deref(), Some("done"));
    }

    #[test]
    fn archive_and_unarchive_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("history", "doing", None, 1).unwrap();
        s.close_task(t.id, TaskOutcome::Abandoned, 100).unwrap();

        s.archive_task(t.id, 200).unwrap();
        assert_eq!(s.task_by_id(t.id).unwrap().archived_at, Some(200));
        // Idempotent: the original archive instant survives a re-archive.
        s.archive_task(t.id, 300).unwrap();
        assert_eq!(s.task_by_id(t.id).unwrap().archived_at, Some(200));

        s.unarchive_task(t.id).unwrap();
        let back = s.task_by_id(t.id).unwrap();
        assert_eq!(back.archived_at, None);
        assert_eq!(back.outcome.as_deref(), Some("abandoned"), "outcome survives unarchive");
        assert!(matches!(s.unarchive_task(9999), Err(Error::TaskNotFound(9999))));
    }

    #[test]
    fn auto_attach_skips_archived_tasks_but_not_closed_ones() {
        let s = Store::open_in_memory().unwrap();
        let closed = s.add_task("closed", "doing", None, 1).unwrap();
        s.set_task_worktree(closed.id, "/r/a", Some("o/r"), Some("feat/a"), None).unwrap();
        s.close_task(closed.id, TaskOutcome::Done, 10).unwrap();
        let archived = s.add_task("archived", "doing", None, 2).unwrap();
        s.set_task_worktree(archived.id, "/r/b", Some("o/r"), Some("feat/b"), None).unwrap();
        s.close_task(archived.id, TaskOutcome::Done, 10).unwrap();
        s.archive_task(archived.id, 20).unwrap();

        let pr = |number: i64, branch: &str| PrInput {
            repo: "o/r".to_string(),
            number,
            title: "t".to_string(),
            branch: branch.to_string(),
            state: "merged".to_string(),
            checks: "none".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: number,
        };
        s.replace_prs(&[pr(1, "feat/a"), pr(2, "feat/b")]).unwrap();

        assert_eq!(s.auto_attach_worktree_prs(30).unwrap(), 1);
        assert_eq!(s.task_by_id(closed.id).unwrap().prs.len(), 1, "closed still attaches");
        assert!(s.task_by_id(archived.id).unwrap().prs.is_empty(), "archived never attaches");
    }

    #[test]
    fn migrate_v13_adds_outcome_columns_to_a_v12_tasks_table() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            // A v12-era tasks table: full worktree columns, no outcome/archive.
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    worktree_repo_root TEXT,
                    worktree_repo TEXT,
                    worktree_branch TEXT,
                    worktree_dir TEXT
                );
                INSERT INTO tasks (text, status, position, created_at)
                    VALUES ('carried forward', 'doing', 0, 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].outcome, None, "pre-existing rows stay open");
        assert_eq!(tasks[0].archived_at, None);
        s.close_task(tasks[0].id, TaskOutcome::Abandoned, 10).unwrap();

        // Idempotent: reopening doesn't re-alter or lose the close.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.task_by_id(1).unwrap().outcome.as_deref(), Some("abandoned"));
    }

    #[test]
    fn migrate_v14_remaps_next_and_review_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    worktree_repo_root TEXT,
                    worktree_repo TEXT,
                    worktree_branch TEXT,
                    worktree_dir TEXT,
                    outcome TEXT,
                    archived_at INTEGER
                );
                INSERT INTO tasks (text, status, position, created_at)
                    VALUES ('was up next', 'next', 0, 1);
                INSERT INTO tasks (text, status, position, created_at)
                    VALUES ('was in review', 'review', 0, 2);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        let next = tasks.iter().find(|t| t.text == "was up next").unwrap();
        let review = tasks.iter().find(|t| t.text == "was in review").unwrap();
        assert_eq!(next.status, "backlog", "next folds back to not-started");
        assert_eq!(review.status, "doing", "review folds forward to in-progress");

        // Idempotent: reopening a db that already went through v14 is a no-op.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.task_by_id(next.id).unwrap().status, "backlog");
        assert_eq!(s.task_by_id(review.id).unwrap().status, "doing");
    }

    #[test]
    fn add_task_stores_notes_and_lands_in_requested_status() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("port the CLI", "backlog", Some("start with doctor"), 1).unwrap();
        assert_eq!(t.notes.as_deref(), Some("start with doctor"));
        assert!(t.issues.is_empty() && t.prs.is_empty() && t.worktree.is_none());
        // A worktree-backed task is born straight into `doing`.
        let d = s.add_task("agent already running", "doing", None, 2).unwrap();
        assert_eq!(d.status, "doing");
        assert_eq!(d.completed_at, None);
        // Unknown statuses are rejected.
        assert!(s.add_task("nope", "bogus", None, 3).is_err());
        let bare = s.add_task("no context", "backlog", None, 4).unwrap();
        assert_eq!(bare.notes, None);
    }

    #[test]
    fn migrate_adds_notes_column_to_pre_v4_tasks_table() {
        // A v2/v3-era db: kanban-shaped tasks table, but no `notes` column.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER
                );
                INSERT INTO tasks (text, created_at) VALUES ('pre-v4 todo', 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        assert!(task_columns(&s).contains(&"notes".to_string()));
        let existing = s.open_tasks().unwrap();
        assert_eq!(existing[0].text, "pre-v4 todo");
        assert_eq!(existing[0].notes, None);
        let t = s.add_task("with notes", "backlog", Some("context"), 2).unwrap();
        assert_eq!(t.notes.as_deref(), Some("context"));
    }

    #[test]
    fn migrate_drops_retired_collector_rows_v6() {
        // A db carrying freshness rows from collectors removed in the
        // day-screens pivot alongside live ones.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let s = Store::open(&path).unwrap();
            for key in ["claude:email", "claude:tasks", "prs", "slack:dm"] {
                s.conn
                    .execute(
                        "INSERT INTO collect_runs (collector, ran_at, ok) VALUES (?1, 1, 1)",
                        params![key],
                    )
                    .unwrap();
            }
        }

        let s = Store::open(&path).unwrap();
        let keys: Vec<String> = s.runs().unwrap().into_iter().map(|r| r.collector).collect();
        assert_eq!(keys, ["prs", "slack:dm"], "retired collector keys are swept");
    }

    #[test]
    fn set_task_status_appends_to_end_of_target_column() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, 3).unwrap();

        // Moving into an empty column starts at 0; the next arrival lands after it.
        s.set_task_status(a.id, "doing", 10).unwrap();
        s.set_task_status(b.id, "doing", 11).unwrap();
        let pos = |id: i64, tasks: &[TaskItem]| tasks.iter().find(|t| t.id == id).unwrap().position;
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(a.id, &tasks), 0);
        assert_eq!(pos(b.id, &tasks), 1);

        // A later drop into the same column lands at the end, not at its old position.
        s.set_task_status(c.id, "doing", 12).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(c.id, &tasks), 2);

        // Bouncing a card out and back re-appends it after the survivors.
        s.set_task_status(a.id, "backlog", 13).unwrap();
        s.set_task_status(a.id, "doing", 14).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(a.id, &tasks), 3);
    }

    #[test]
    fn set_task_status_rejects_unknown() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", "backlog", None, 1).unwrap();
        assert!(s.set_task_status(t.id, "bogus", 2).is_err());
    }

    /// The ids in `status`'s column, in board (displayed) order.
    #[cfg(test)]
    fn column_ids(s: &Store, status: &str) -> Vec<i64> {
        s.snapshot()
            .unwrap()
            .tasks
            .into_iter()
            .filter(|t| t.status == status)
            .map(|t| t.id)
            .collect()
    }

    #[test]
    fn set_task_position_reorders_within_a_column() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, 3).unwrap();
        // Column starts [a, b, c] at positions 0,1,2.
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);

        // Move c to the top.
        s.set_task_position(c.id, "backlog", 0, 10).unwrap();
        assert_eq!(column_ids(&s, "backlog"), vec![c.id, a.id, b.id]);

        // Move c to the bottom (index past the end clamps to last).
        s.set_task_position(c.id, "backlog", 99, 11).unwrap();
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);

        // Move a into the middle.
        s.set_task_position(a.id, "backlog", 1, 12).unwrap();
        assert_eq!(column_ids(&s, "backlog"), vec![b.id, a.id, c.id]);

        // Positions are contiguous 0..n after each move.
        let positions: Vec<i64> = {
            let mut ts = s.snapshot().unwrap().tasks;
            ts.sort_by_key(|t| t.position);
            ts.into_iter().map(|t| t.position).collect()
        };
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn set_task_position_moves_across_columns_preserving_order() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        s.set_task_status(a.id, "doing", 3).unwrap();
        s.set_task_status(b.id, "doing", 4).unwrap();
        // doing = [a, b].
        let c = s.add_task("c", "backlog", None, 5).unwrap();

        // Drop c between a and b.
        s.set_task_position(c.id, "doing", 1, 6).unwrap();
        assert_eq!(column_ids(&s, "doing"), vec![a.id, c.id, b.id]);
        assert!(column_ids(&s, "backlog").is_empty());
    }

    #[test]
    fn set_task_position_stamps_and_clears_done() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship", "backlog", None, 1).unwrap();
        s.set_task_position(t.id, "done", 0, 20).unwrap();
        let done = s.snapshot().unwrap().tasks.into_iter().find(|x| x.id == t.id).unwrap();
        assert_eq!(done.status, "done");
        assert_eq!(done.completed_at, Some(20));

        s.set_task_position(t.id, "backlog", 0, 30).unwrap();
        let reopened = s.open_tasks().unwrap();
        assert_eq!(reopened[0].status, "backlog");
        assert_eq!(reopened[0].completed_at, None);
    }

    #[test]
    fn set_task_position_is_stable_under_repeated_moves() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, 3).unwrap();
        // Dropping a card onto its own position leaves the order unchanged.
        for _ in 0..5 {
            s.set_task_position(b.id, "backlog", 1, 10).unwrap();
        }
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);
    }

    #[test]
    fn set_task_position_rejects_unknown_status_and_missing_id() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", "backlog", None, 1).unwrap();
        assert!(s.set_task_position(t.id, "bogus", 0, 2).is_err());
        assert!(matches!(
            s.set_task_position(9999, "backlog", 0, 2),
            Err(Error::TaskNotFound(9999))
        ));
    }

    #[test]
    fn attach_task_issue_stores_reference() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire up board", "backlog", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 42, "https://github.com/o/r/issues/42").unwrap();
        let linked = s.open_tasks().unwrap()[0].clone();
        assert_eq!(linked.issues.len(), 1);
        assert_eq!(linked.issues[0].repo, "o/r");
        assert_eq!(linked.issues[0].number, 42);
        assert_eq!(linked.issues[0].url, "https://github.com/o/r/issues/42");
    }

    #[test]
    fn update_task_edits_text_and_notes() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("rough draft", "backlog", None, 1).unwrap();
        let updated = s.update_task(t.id, "polished", Some("ship friday")).unwrap();
        assert_eq!(updated.text, "polished");
        assert_eq!(updated.notes.as_deref(), Some("ship friday"));
        // Status/position are untouched by an edit.
        assert_eq!(updated.status, "backlog");
        assert_eq!(updated.position, t.position);
        // And it persists.
        assert_eq!(s.get_task(t.id).unwrap().unwrap().text, "polished");
    }

    #[test]
    fn update_task_none_notes_clears_them() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("call dentist", "backlog", Some("weds am"), 1).unwrap();
        assert_eq!(t.notes.as_deref(), Some("weds am"));
        // Passing None clears notes back out — a full replace, no sentinel.
        let cleared = s.update_task(t.id, "call dentist", None).unwrap();
        assert_eq!(cleared.notes, None);
    }

    #[test]
    fn update_task_nonexistent_errors() {
        let s = Store::open_in_memory().unwrap();
        let err = s.update_task(999, "ghost", None).unwrap_err();
        assert!(matches!(err, Error::TaskNotFound(999)));
    }

    #[test]
    fn delete_task_removes_row() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("keep", "backlog", None, 1).unwrap();
        let b = s.add_task("toss", "backlog", None, 2).unwrap();
        s.delete_task(b.id).unwrap();
        let open = s.open_tasks().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, a.id);
        assert!(s.get_task(b.id).unwrap().is_none());
    }

    #[test]
    fn delete_task_nonexistent_errors() {
        let s = Store::open_in_memory().unwrap();
        let err = s.delete_task(999).unwrap_err();
        assert!(matches!(err, Error::TaskNotFound(999)));
    }

    #[test]
    fn events_between_windows_by_start() {
        let s = Store::open_in_memory().unwrap();
        put_events(&s, &[event("a", 100), event("b", 300), event("c", 500)], 1).unwrap();
        let win = s.events_between(150, 500).unwrap();
        assert_eq!(win.iter().map(|e| e.external_id.as_str()).collect::<Vec<_>>(), vec!["b"]);
    }

    #[test]
    fn current_or_next_event_across_the_meeting_lifecycle() {
        // The `event` helper spans [start, start + 1000). Two non-overlapping
        // meetings: "b" runs [300, 1300), "c" runs [1500, 2500).
        let s = Store::open_in_memory().unwrap();
        put_events(&s, &[event("b", 300), event("c", 1500)], 1).unwrap();

        // Future: before it starts, "b" is the next meeting.
        assert_eq!(s.current_or_next_event(200).unwrap().unwrap().external_id, "b");
        // At the exact start it is already live.
        assert_eq!(s.current_or_next_event(300).unwrap().unwrap().external_id, "b");
        // In progress (start <= now < end): "b" stays selected, not skipped.
        assert_eq!(s.current_or_next_event(800).unwrap().unwrap().external_id, "b");
        // Ended (now >= end_ts): "b" drops out and the next meeting "c" takes over.
        assert_eq!(s.current_or_next_event(1300).unwrap().unwrap().external_id, "c");
        // After the last meeting ends there is nothing left.
        assert!(s.current_or_next_event(3000).unwrap().is_none());
    }

    #[test]
    fn current_or_next_event_without_end_is_a_point_in_time() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[EventInput {
                external_id: "no-end".to_string(),
                title: "Open-ended".to_string(),
                start: at(500),
                end: None,
                attendees: vec![],
                location: None,
                join_url: None,
            }],
            1,
        )
        .unwrap();
        // With no duration there is no live window: shown up to its start, then gone.
        assert_eq!(s.current_or_next_event(400).unwrap().unwrap().external_id, "no-end");
        assert_eq!(s.current_or_next_event(500).unwrap().unwrap().external_id, "no-end");
        assert!(s.current_or_next_event(600).unwrap().is_none());
    }

    #[test]
    fn record_run_upserts_per_collector() {
        let s = Store::open_in_memory().unwrap();
        s.record_run("gcal", true, None, 10).unwrap();
        s.record_run("gcal", false, Some("boom"), 20).unwrap();
        let runs = s.runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].ran_at, 20);
        assert!(!runs[0].ok);
        assert_eq!(runs[0].message.as_deref(), Some("boom"));
    }

    #[test]
    fn upsert_dm_preserves_dismissal_until_a_newer_message() {
        let s = Store::open_in_memory().unwrap();
        let msg = |ts: i64, from_me: bool| DmInput {
            channel: "D123".to_string(),
            from_name: "Sarah".to_string(),
            text: format!("msg at {ts}"),
            ts,
            from_me,
            url: Some("slack://channel?team=T1&id=D123".to_string()),
        };

        s.upsert_dm(&msg(100, false), 1).unwrap();
        let dm = &s.dms().unwrap()[0];
        assert!(!dm.from_me);
        assert_eq!(dm.dismissed_ts, 0, "fresh message starts undismissed");

        // Mark handled: dismissed_ts catches up to ts.
        s.dismiss_dm("D123", 100).unwrap();
        assert_eq!(s.dms().unwrap()[0].dismissed_ts, 100);

        // Re-collecting the same message keeps the dismissal.
        s.upsert_dm(&msg(100, false), 2).unwrap();
        let dm = s.dms().unwrap()[0].clone();
        assert_eq!(dm.dismissed_ts, 100);
        assert_eq!(dm.fetched_at, 2);

        // A newer message outruns the dismissal (dismissed_ts < ts again).
        s.upsert_dm(&msg(200, false), 3).unwrap();
        let dm = s.dms().unwrap()[0].clone();
        assert_eq!(dm.ts, 200);
        assert!(dm.dismissed_ts < dm.ts);

        // Replying clears it collector-side: latest message is mine.
        s.upsert_dm(&msg(300, true), 4).unwrap();
        assert!(s.dms().unwrap()[0].from_me);
    }

    #[test]
    fn dismiss_item_survives_replace_until_the_item_updates() {
        let s = Store::open_in_memory().unwrap();
        let pr = |updated_ts: i64| PrInput {
            repo: "octo/widgets".to_string(),
            number: 42,
            title: "feat: treemap".to_string(),
            branch: "feat/treemap".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: "review_requested".to_string(),
            url: "https://github.com/octo/widgets/pull/42".to_string(),
            updated_ts,
        };

        s.replace_prs(&[pr(100)]).unwrap();
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 0, "fresh PR starts undismissed");

        s.dismiss_item("pr", "octo/widgets", 42, 100).unwrap();
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 100);

        // A collector re-run with no real change (same updated_ts) keeps the
        // dismissal, exactly like a re-sent DM at the same ts.
        s.replace_prs(&[pr(100)]).unwrap();
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 100);

        // The PR actually changing (a newer review) outruns the dismissal.
        s.replace_prs(&[pr(200)]).unwrap();
        let pr_row = &s.prs().unwrap()[0];
        assert_eq!(pr_row.updated_ts, 200);
        assert!(pr_row.dismissed_ts < pr_row.updated_ts);
    }

    #[test]
    fn clear_dismissals_removes_every_kind() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[IssueInput {
            repo: "octo/widgets".to_string(),
            number: 118,
            title: "Flaky resize".to_string(),
            labels: vec![],
            state: "open".to_string(),
            url: "https://github.com/octo/widgets/issues/118".to_string(),
            updated_ts: 50,
        }])
        .unwrap();
        s.replace_prs(&[PrInput {
            repo: "octo/widgets".to_string(),
            number: 42,
            title: "feat: treemap".to_string(),
            branch: "feat/treemap".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: "review_requested".to_string(),
            url: "https://github.com/octo/widgets/pull/42".to_string(),
            updated_ts: 100,
        }])
        .unwrap();

        s.dismiss_item("issue", "octo/widgets", 118, 50).unwrap();
        s.dismiss_item("pr", "octo/widgets", 42, 100).unwrap();
        assert_eq!(s.issues().unwrap()[0].dismissed_ts, 50);
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 100);

        let cleared = s.clear_dismissals().unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(s.issues().unwrap()[0].dismissed_ts, 0);
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 0);
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[EventInput {
                external_id: "x".to_string(),
                title: "T".to_string(),
                start: at(1),
                end: Some(at(2)),
                attendees: vec!["a@b.com".to_string()],
                location: Some("room".to_string()),
                join_url: Some("https://meet".to_string()),
            }],
            1,
        )
        .unwrap();
        s.add_task("do thing", "backlog", None, 1).unwrap();
        s.replace_issues(&[issue("o/r", 5, 6)]).unwrap();
        s.replace_prs(&[PrInput {
            repo: "o/r".to_string(),
            number: 7,
            title: "Fix".to_string(),
            branch: "feat".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: "approved".to_string(),
            url: "https://x".to_string(),
            updated_ts: 3,
        }])
        .unwrap();
        s.record_run("gcal", true, None, 4).unwrap();
        s.upsert_dm(
            &DmInput {
                channel: "D1".to_string(),
                from_name: "Sarah".to_string(),
                text: "hi".to_string(),
                ts: 5,
                from_me: false,
                url: None,
            },
            6,
        )
        .unwrap();

        let json = serde_json::to_string(&s.snapshot().unwrap()).unwrap();
        for key in [
            "\"start\"",
            "\"externalId\"",
            "\"joinUrl\"",
            "\"createdAt\"",
            "\"updatedTs\"",
            "\"reviewState\"",
            "\"ranAt\"",
            "\"fromName\"",
            "\"fromMe\"",
            "\"dismissedTs\"",
        ] {
            assert!(json.contains(key), "expected {key} in snapshot JSON: {json}");
        }
        // snake_case must not leak through.
        assert!(!json.contains("start_ts"));
        assert!(!json.contains("review_state"));
        // Event times are RFC 3339 on the wire, not epoch integers — this is
        // the readability the format exists for, so pin the rendered shape.
        // Note chrono's serde renders a zero offset as `Z` where `to_rfc3339`
        // writes `+00:00`. Both are valid RFC 3339 and parse identically, and
        // the generated sort column normalizes either — so this pins the shape
        // without pretending the two spellings must match.
        assert!(
            json.contains("\"start\":\"1970-01-01T00:00:00.001Z\""),
            "event start should be RFC 3339: {json}"
        );
    }

    /// `utc_key` and the `starts_at_utc` generated column must produce byte-
    /// identical strings. They are compared lexically, so a divergence in
    /// width or precision does not error — it silently returns the wrong rows
    /// from every range query. Pinned through SQLite itself rather than by
    /// eyeballing two format strings that live in different languages.
    #[test]
    fn utc_key_matches_the_generated_column() {
        let s = Store::open_in_memory().unwrap();
        for ms in [0i64, 1, 999, 1_700_000_000_000, -86_400_000] {
            s.replace_events_for_source("k", i64::MIN, i64::MAX, &[event("x", ms)], ms + 1)
                .unwrap();
            let stored: String = s
                .conn
                .query_row("SELECT starts_at_utc FROM events WHERE source = 'k'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(stored, utc_key(ms), "format drift at {ms}");
        }
    }

    /// The offset the calendar reported survives a write/read round trip —
    /// the whole reason these columns are text and not integers.
    #[test]
    fn a_non_utc_offset_round_trips_and_still_sorts_by_instant() {
        let s = Store::open_in_memory().unwrap();
        let london = DateTime::parse_from_rfc3339("2026-07-20T15:00:00+01:00").unwrap();
        let chicago = DateTime::parse_from_rfc3339("2026-07-20T09:30:00-05:00").unwrap();
        // Chicago 09:30-05:00 is 14:30Z — half an hour *before* London 15:00+01:00
        // (14:00Z)... no: 14:30Z is after 14:00Z. Instant order is london, chicago.
        s.replace_events_for_source(
            "tz",
            i64::MIN,
            i64::MAX,
            &[
                EventInput {
                    external_id: "chicago".to_string(),
                    title: "Standup".to_string(),
                    start: chicago,
                    end: None,
                    attendees: vec![],
                    location: None,
                    join_url: None,
                },
                EventInput {
                    external_id: "london".to_string(),
                    title: "Review".to_string(),
                    start: london,
                    end: None,
                    attendees: vec![],
                    location: None,
                    join_url: None,
                },
            ],
            london.timestamp_millis(),
        )
        .unwrap();

        let all = s.events_between(i64::MIN, i64::MAX).unwrap();
        // Sorted by instant (14:00Z then 14:30Z), NOT by the authored strings —
        // lexically "09:30-05:00" would come first and be wrong.
        assert_eq!(
            all.iter().map(|e| e.external_id.as_str()).collect::<Vec<_>>(),
            vec!["london", "chicago"]
        );
        // ...and each keeps the offset it was written with.
        let stored_london = all.iter().find(|e| e.external_id == "london").unwrap();
        assert_eq!(stored_london.start.to_rfc3339(), "2026-07-20T15:00:00+01:00");
        assert_eq!(stored_london.start.offset().local_minus_utc(), 3600);
    }

    fn mcp_call(method: &str, tool: Option<&str>, ok: bool) -> McpCallInput {
        McpCallInput {
            method: method.to_string(),
            tool: tool.map(str::to_string),
            args: tool.map(|_| "{\"title\":\"x\"}".to_string()),
            ok,
            error: (!ok).then(|| "boom".to_string()),
            duration_ms: Some(3),
            client: Some("claude-code 2.0".to_string()),
        }
    }

    #[test]
    fn record_mcp_call_reads_back_newest_first() {
        let s = Store::open_in_memory().unwrap();
        s.record_mcp_call(&mcp_call("tools/list", None, true), 10).unwrap();
        s.record_mcp_call(&mcp_call("tools/call", Some("todo_create"), true), 20).unwrap();
        s.record_mcp_call(&mcp_call("tools/call", Some("nope"), false), 30).unwrap();

        let calls = s.mcp_calls(10).unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].ts, 30);
        assert_eq!(calls[0].tool.as_deref(), Some("nope"));
        assert!(!calls[0].ok);
        assert_eq!(calls[0].error.as_deref(), Some("boom"));
        assert_eq!(calls[2].method, "tools/list");
        assert_eq!(calls[2].tool, None);
        assert_eq!(calls[1].args.as_deref(), Some("{\"title\":\"x\"}"));
        assert_eq!(calls[1].client.as_deref(), Some("claude-code 2.0"));

        // The limit caps the read.
        assert_eq!(s.mcp_calls(2).unwrap().len(), 2);
    }

    #[test]
    fn record_mcp_call_prunes_beyond_retention() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..(MCP_CALL_RETAIN + 25) {
            s.record_mcp_call(&mcp_call("ping", None, true), i).unwrap();
        }
        let calls = s.mcp_calls(MCP_CALL_RETAIN as usize * 2).unwrap();
        assert_eq!(calls.len(), MCP_CALL_RETAIN as usize);
        // The survivors are the newest rows.
        assert_eq!(calls[0].ts, MCP_CALL_RETAIN + 24);
        assert_eq!(calls.last().unwrap().ts, 25);
    }

    #[test]
    fn snapshot_carries_mcp_calls_camel_cased() {
        let s = Store::open_in_memory().unwrap();
        s.record_mcp_call(&mcp_call("tools/call", Some("day_brief"), true), 7).unwrap();
        let snapshot = s.snapshot().unwrap();
        assert_eq!(snapshot.mcp_calls.len(), 1);
        assert_eq!(snapshot.mcp_calls[0].tool.as_deref(), Some("day_brief"));

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"mcpCalls\""), "expected mcpCalls in {json}");
        assert!(json.contains("\"durationMs\""), "expected durationMs in {json}");
        assert!(!json.contains("mcp_calls"));
    }

    #[test]
    fn archive_closed_tasks_never_sweeps_legacy_null_completed_at() {
        // A `done` row whose `completed_at` is NULL (data from before the column
        // was stamped) has no known completion time, so the sweep must skip it —
        // even with a cutoff far in the future. There is no public API to make
        // such a row, so insert it directly.
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute(
                "INSERT INTO tasks (text, status, position, created_at, completed_at)
                 VALUES ('legacy done', 'done', 0, 1, NULL)",
                [],
            )
            .unwrap();
        let normal = s.add_task("normal done", "backlog", None, 2).unwrap();
        s.set_task_status(normal.id, "done", 10).unwrap();

        let archived = s.archive_closed_tasks(1_000_000, 1_000_001).unwrap();
        assert_eq!(archived, 1, "only the stamped done row is swept");
        let visible: Vec<String> = s
            .snapshot()
            .unwrap()
            .tasks
            .into_iter()
            .filter(|t| t.archived_at.is_none())
            .map(|t| t.text)
            .collect();
        assert_eq!(visible, vec!["legacy done".to_string()]);
    }

    #[test]
    fn events_between_is_start_inclusive_end_exclusive() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[
                event("at-start", 100),
                event("mid", 150),
                event("at-end", 200),
            ],
            1,
        )
        .unwrap();
        // Window [100, 200): the event exactly at start is in, the one at end is out.
        let ids: Vec<String> =
            s.events_between(100, 200).unwrap().into_iter().map(|e| e.external_id).collect();
        assert_eq!(ids, vec!["at-start".to_string(), "mid".to_string()]);
    }

    #[test]
    fn current_or_next_event_on_empty_store_is_none() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.current_or_next_event(0).unwrap().is_none());
    }

    #[test]
    fn replace_events_round_trips_attendees_json() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[
                EventInput {
                    external_id: "many".to_string(),
                    title: "Sync".to_string(),
                    start: at(100),
                    end: Some(at(200)),
                    attendees: vec!["a@x.com".to_string(), "b@x.com".to_string()],
                    location: Some("Room 1".to_string()),
                    join_url: Some("https://meet/x".to_string()),
                },
                EventInput {
                    external_id: "none".to_string(),
                    title: "Solo".to_string(),
                    start: at(300),
                    end: None,
                    attendees: vec![],
                    location: None,
                    join_url: None,
                },
            ],
            1,
        )
        .unwrap();
        let events = s.snapshot().unwrap().events;
        let many = events.iter().find(|e| e.external_id == "many").unwrap();
        assert_eq!(many.attendees, vec!["a@x.com".to_string(), "b@x.com".to_string()]);
        assert_eq!(many.location.as_deref(), Some("Room 1"));
        let none = events.iter().find(|e| e.external_id == "none").unwrap();
        assert!(none.attendees.is_empty());
        assert_eq!(none.end, None);
    }

    #[test]
    fn open_tasks_orders_across_columns_by_board_order() {
        let s = Store::open_in_memory().unwrap();
        s.add_task("backlog item", "backlog", None, 1).unwrap();
        let doing = s.add_task("doing item", "backlog", None, 2).unwrap();
        let done = s.add_task("done item", "backlog", None, 3).unwrap();
        s.set_task_status(doing.id, "doing", 11).unwrap();
        s.set_task_status(done.id, "done", 13).unwrap();

        // open_tasks excludes done and returns backlog → doing.
        let statuses: Vec<String> = s.open_tasks().unwrap().into_iter().map(|t| t.status).collect();
        assert_eq!(statuses, vec!["backlog".to_string(), "doing".to_string()]);
    }

    #[test]
    fn task_derived_fields_track_closed_worktree_and_outcome() {
        let s = Store::open_in_memory().unwrap();
        // Backlog, no worktree: open, no badge, nothing to jump to.
        let bare = s.add_task("bare", "backlog", None, 1).unwrap();
        assert!(!bare.closed);
        assert_eq!(bare.display_outcome, None);
        assert!(!bare.has_worktree);

        // A bound repo with no worktree dir yet — still not "has_worktree".
        s.set_task_worktree(bare.id, "/repo", None, None, None).unwrap();
        let repo_bound = s.get_task(bare.id).unwrap().unwrap();
        assert!(!repo_bound.closed);
        assert!(!repo_bound.has_worktree);

        // A worktree dir makes it `has_worktree`.
        s.set_task_worktree(bare.id, "/repo", None, Some("br"), Some("/repo/wt")).unwrap();
        let with_dir = s.get_task(bare.id).unwrap().unwrap();
        assert!(with_dir.has_worktree);

        // Dragged straight into `done` with no explicit outcome: closed, and
        // the badge falls back to `done` even though `outcome` itself is unset.
        let dragged = s.add_task("dragged", "backlog", None, 2).unwrap();
        s.set_task_status(dragged.id, "done", 10).unwrap();
        let dragged = s.get_task(dragged.id).unwrap().unwrap();
        assert!(dragged.closed);
        assert_eq!(dragged.outcome, None);
        assert_eq!(dragged.display_outcome, Some("done".to_string()));

        // Explicitly closed as abandoned: closed, badge mirrors the recorded outcome.
        let abandoned = s.add_task("abandoned", "backlog", None, 3).unwrap();
        let abandoned = s.close_task(abandoned.id, TaskOutcome::Abandoned, 20).unwrap();
        assert!(abandoned.closed);
        assert_eq!(abandoned.display_outcome, Some("abandoned".to_string()));
    }

    #[test]
    fn snapshot_tasks_place_done_column_last() {
        let s = Store::open_in_memory().unwrap();
        let d = s.add_task("finish", "backlog", None, 1).unwrap();
        s.add_task("start", "backlog", None, 2).unwrap();
        s.set_task_status(d.id, "done", 10).unwrap();
        // Snapshot keeps done rows but orders them after open columns regardless
        // of insertion/completion order.
        let statuses: Vec<String> =
            s.snapshot().unwrap().tasks.into_iter().map(|t| t.status).collect();
        assert_eq!(statuses, vec!["backlog".to_string(), "done".to_string()]);
    }

    #[test]
    fn snapshot_caps_mcp_calls_at_the_snapshot_limit() {
        let s = Store::open_in_memory().unwrap();
        // More rows than the snapshot carries, but within retention.
        let total = MCP_CALL_SNAPSHOT_LIMIT + 20;
        for i in 0..total {
            s.record_mcp_call(&mcp_call("ping", None, true), i as i64).unwrap();
        }
        let snapshot = s.snapshot().unwrap();
        assert_eq!(snapshot.mcp_calls.len(), MCP_CALL_SNAPSHOT_LIMIT);
        // Newest first: the last recorded call heads the list.
        assert_eq!(snapshot.mcp_calls[0].ts, (total - 1) as i64);
    }

    #[test]
    fn replace_issues_and_prs_with_empty_clears_all_rows() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        s.replace_prs(&[PrInput {
            repo: "o/r".to_string(),
            number: 2,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        }])
        .unwrap();
        assert_eq!(s.replace_issues(&[]).unwrap(), 0);
        assert_eq!(s.replace_prs(&[]).unwrap(), 0);
        assert!(s.issues().unwrap().is_empty());
        assert!(s.prs().unwrap().is_empty());
    }

    /// v10 converts epoch-ms event rows to RFC 3339 text **without losing
    /// them**, unlike v9's deliberate drop. The instant is known exactly here;
    /// only the authored offset isn't, and `Z` says that honestly. Dropping
    /// instead would blank the next-meeting countdown until something writes —
    /// and with the pull collector off by default, that may be a long time.
    #[test]
    fn migrate_v10_converts_epoch_rows_to_rfc3339_keeping_them() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        // A v9-shaped db: has `source`, still epoch integers.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE events (
                    id INTEGER PRIMARY KEY,
                    source TEXT NOT NULL,
                    external_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    start_ts INTEGER NOT NULL,
                    end_ts INTEGER,
                    attendees TEXT NOT NULL DEFAULT '[]',
                    location TEXT,
                    join_url TEXT,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(source, external_id)
                );
                INSERT INTO events
                    (source, external_id, title, start_ts, end_ts, updated_at)
                    VALUES
                    ('google', 'kept', 'Standup', 1700000000000, 1700001800000, 1),
                    ('google', 'no-end', 'Reminder', 1700003600000, NULL, 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 2, "rows are converted, not dropped");

        let kept = events.iter().find(|e| e.external_id == "kept").unwrap();
        assert_eq!(kept.start_ms(), 1_700_000_000_000, "the instant survives exactly");
        assert_eq!(kept.end_ms(), Some(1_700_001_800_000));
        // Unknown authored zone becomes UTC, stated as such rather than guessed.
        assert_eq!(kept.start.offset().local_minus_utc(), 0);
        assert_eq!(kept.start.to_rfc3339(), "2023-11-14T22:13:20+00:00");

        let no_end = events.iter().find(|e| e.external_id == "no-end").unwrap();
        assert_eq!(no_end.end, None, "a NULL end stays NULL, not epoch 0");

        // The rebuilt table still writes, sorts and enforces its unique key.
        s.replace_events_for_source("outlook", i64::MIN, i64::MAX, &[event("kept", 1)], 2).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 3, "same id in another lane is fine");

        // Idempotent: reopening must not rebuild again and lose the rows.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 3, "no-op on a v10 db");
    }

    /// v9 rebuilds `events` for the `source` column and the composite unique
    /// key. Pre-v9 rows are **intentionally dropped** — the old schema recorded
    /// no source, and a row tagged with a guessed source would never be swept by
    /// any real pull, lingering in the countdown forever. Pinned as a test so
    /// the data loss stays a decision rather than a surprise.
    #[test]
    fn migrate_v9_rebuilds_events_and_drops_sourceless_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE events (
                    id INTEGER PRIMARY KEY,
                    external_id TEXT NOT NULL UNIQUE,
                    title TEXT NOT NULL,
                    start_ts INTEGER NOT NULL,
                    end_ts INTEGER,
                    attendees TEXT NOT NULL DEFAULT '[]',
                    location TEXT,
                    join_url TEXT,
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO events (external_id, title, start_ts, updated_at)
                    VALUES ('legacy', 'Old meeting', 100, 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let cols: Vec<String> = {
            let mut stmt = s.conn.prepare("PRAGMA table_info(events)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert!(cols.contains(&"source".to_string()), "source column added");
        assert!(s.snapshot().unwrap().events.is_empty(), "sourceless rows dropped, not guessed");

        // The rebuilt table takes writes and enforces the new composite key.
        s.replace_events_for_source("google", 0, 1000, &[event("a", 100)], 2).unwrap();
        s.replace_events_for_source("outlook", 0, 1000, &[event("a", 200)], 2).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2);

        // Idempotent: reopening doesn't rebuild again and lose the new rows.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "migration is a no-op on a v9 db");
    }

    #[test]
    fn update_task_leaves_links_and_worktree_intact() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire board", "backlog", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 7, "https://github.com/o/r/issues/7").unwrap();
        s.set_task_worktree(t.id, "/repos/r", Some("o/r"), Some("feat/wire"), Some("/w")).unwrap();
        let updated = s.update_task(t.id, "wire board v2", Some("note")).unwrap();
        assert_eq!(updated.text, "wire board v2");
        // Editing free-form fields must not disturb links or the worktree binding.
        assert_eq!(updated.issues.len(), 1);
        assert_eq!(updated.issues[0].number, 7);
        assert_eq!(updated.worktree.unwrap().branch.as_deref(), Some("feat/wire"));
    }

    #[test]
    fn attach_task_issue_accumulates_links() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("multi-issue", "backlog", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/a", 1, "https://github.com/o/a/issues/1").unwrap();
        s.attach_task_issue(t.id, "o/b", 2, "https://github.com/o/b/issues/2").unwrap();
        let got = s.get_task(t.id).unwrap().unwrap();
        // Attaching a second issue adds a link — it no longer overwrites.
        assert_eq!(got.issues.len(), 2);
        let repos: Vec<&str> = got.issues.iter().map(|l| l.repo.as_str()).collect();
        assert_eq!(repos, vec!["o/a", "o/b"]);
    }

    #[test]
    fn delete_task_cascades_link_rows_but_archiving_keeps_them() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        s.attach_task_issue(a.id, "o/r", 1, "u").unwrap();
        s.attach_task_pr(a.id, "o/r", 2, "u").unwrap();
        s.attach_task_issue(b.id, "o/r", 3, "u").unwrap();

        s.delete_task(a.id).unwrap();
        assert_eq!(issue_link_rows(&s), vec![(b.id, "o/r".to_string(), 3)]);
        assert!(s.linked_pr_refs().unwrap().is_empty());

        // The archive sweep keeps the row, so its links survive too.
        s.set_task_status(b.id, "done", 10).unwrap();
        assert_eq!(s.archive_closed_tasks(100, 101).unwrap(), 1);
        assert_eq!(issue_link_rows(&s), vec![(b.id, "o/r".to_string(), 3)]);
    }

    #[test]
    fn migrate_v7_ports_single_link_and_drops_link_columns() {
        // A v5-era db: kanban tasks table with the single-issue link columns
        // and one linked + one bare todo.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT
                );
                INSERT INTO tasks (text, status, position, repo, issue_number, issue_url,
                                   created_at, notes)
                    VALUES ('linked', 'doing', 1, 'o/r', 7,
                            'https://github.com/o/r/issues/7', 1, 'ctx'),
                           ('bare', 'backlog', 0, NULL, NULL, NULL, 2, NULL);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let cols = task_columns(&s);
        for gone in ["repo", "issue_number", "issue_url"] {
            assert!(!cols.contains(&gone.to_string()), "column {gone} should be dropped");
        }
        for added in [
            "worktree_repo_root",
            "worktree_repo",
            "worktree_branch",
            "worktree_dir",
        ] {
            assert!(cols.contains(&added.to_string()), "column {added} should exist");
        }

        let tasks = s.all_tasks().unwrap();
        let linked = tasks.iter().find(|t| t.text == "linked").unwrap();
        assert_eq!(linked.status, "doing");
        assert_eq!(linked.notes.as_deref(), Some("ctx"));
        assert_eq!(linked.issues.len(), 1);
        assert_eq!(linked.issues[0].repo, "o/r");
        assert_eq!(linked.issues[0].number, 7);
        assert_eq!(linked.issues[0].url, "https://github.com/o/r/issues/7");
        assert_eq!(linked.issues[0].state, "open");
        let bare = tasks.iter().find(|t| t.text == "bare").unwrap();
        assert!(bare.issues.is_empty());

        // Idempotent: re-open runs migrate again without duplicating links.
        drop(s);
        let s = Store::open(&path).unwrap();
        let linked = s.all_tasks().unwrap().into_iter().find(|t| t.text == "linked").unwrap();
        assert_eq!(linked.issues.len(), 1);
    }

    #[test]
    fn migrate_v8_drops_due_column_keeping_rows() {
        // A v7-era db: current shape plus the retired due_ts column.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    worktree_repo_root TEXT,
                    worktree_repo TEXT,
                    worktree_branch TEXT,
                    worktree_dir TEXT
                );
                INSERT INTO tasks (text, status, position, due_ts, created_at, notes)
                    VALUES ('was due', 'doing', 1, 1752200000000, 1, 'ctx'),
                           ('never due', 'backlog', 0, NULL, 2, NULL);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        assert!(!task_columns(&s).contains(&"due_ts".to_string()), "due_ts should be dropped");
        let tasks = s.all_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        let kept = tasks.iter().find(|t| t.text == "was due").unwrap();
        assert_eq!(kept.status, "doing");
        assert_eq!(kept.notes.as_deref(), Some("ctx"));

        // Idempotent: a second open finds no due_ts column and is a no-op.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.all_tasks().unwrap().len(), 2);
    }

    #[test]
    fn migrate_v11_renames_slot_columns_to_worktree_keeping_bindings() {
        // A v7-era db with the pre-rename slot_* columns and a bound task.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    slot_repo_root TEXT,
                    slot_repo TEXT,
                    slot_branch TEXT,
                    slot_dir TEXT
                );
                INSERT INTO tasks (text, status, position, created_at,
                                   slot_repo_root, slot_repo, slot_branch, slot_dir)
                    VALUES ('bound', 'doing', 0, 1,
                            '/repos/x', 'o/x', 'feat/y', '/repos/x/wt');",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let cols = task_columns(&s);
        assert!(cols.contains(&"worktree_repo_root".to_string()), "columns renamed");
        assert!(!cols.iter().any(|c| c.starts_with("slot_")), "no slot_* columns remain");
        let task = s.all_tasks().unwrap().into_iter().find(|t| t.text == "bound").unwrap();
        let wt = task.worktree.expect("binding survives the rename");
        assert_eq!(wt.repo_root, "/repos/x");
        assert_eq!(wt.branch.as_deref(), Some("feat/y"));
        assert_eq!(wt.dir.as_deref(), Some("/repos/x/wt"));

        // Idempotent: a second open finds no slot_* columns and is a no-op.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.all_tasks().unwrap().len(), 1);
    }
}
