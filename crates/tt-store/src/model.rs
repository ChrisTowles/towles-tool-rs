//! Data types and pure helpers for the store: the output/input structs that
//! serialize to the frontend's `camelCase` contract, the task-status/outcome
//! vocabulary, and the small pure decisions (event-time parsing, gh
//! close/reopen targeting) that need no database handle.

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

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
pub(crate) const UTC_KEY_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

/// An instant as its `starts_at_utc` sort key — the bridge between the injected
/// `now_ms` clock and the events table's text columns.
pub(crate) fn utc_key(ms: i64) -> String {
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
pub(crate) fn parse_rfc3339(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text).ok()
}

/// One unparseable event row, logged so a hand-edit is discoverable instead of
/// silently shrinking the calendar.
pub(crate) fn log_unparseable_event(external_id: &str, value: &str) {
    tracing::warn!(%external_id, %value, "tt-store: unparseable event time; row skipped");
}

/// How many MCP call-log rows are retained; older rows are pruned on insert.
pub(crate) const MCP_CALL_RETAIN: i64 = 500;

/// How far back calendar events are kept, swept on each calendar write.
/// Public so writers can refuse a backfill their own sweep would reclaim.
///
/// Events are a cache in service of "when is my next meeting", so history has
/// no value here — but a few days of slack means a clock skew or a late-running
/// pull can't discard a meeting that hasn't happened yet.
pub const EVENT_RETAIN_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How many MCP call-log rows ride along in a [`Snapshot`] (newest first).
pub(crate) const MCP_CALL_SNAPSHOT_LIMIT: usize = 100;

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

// Column lists, kept in sync with the row-mapping closures in the domain modules.
// Column lists, kept in sync with the row-mapping closures below.
pub(crate) const EVENT_COLS: &str =
    "id, source, external_id, title, starts_at, ends_at, attendees, location, join_url";
pub(crate) const TASK_COLS: &str = "id, text, status, position, created_at, completed_at, notes, \
     worktree_repo_root, worktree_repo, worktree_branch, worktree_dir, outcome, archived_at";
// Aliased to `i`/`p` and joined against `item_dismissals` in the read paths
// below, so each column list carries its own dismissed_ts.
pub(crate) const ISSUE_COLS: &str = "i.repo, i.number, i.title, i.labels, i.state, i.url, i.updated_ts, COALESCE(d.dismissed_ts, 0)";
pub(crate) const PR_COLS: &str = "p.repo, p.number, p.title, p.branch, p.state, p.checks, p.review_state, \
     p.url, p.updated_ts, COALESCE(d.dismissed_ts, 0)";
pub(crate) const RUN_COLS: &str = "collector, ran_at, ok, message";
pub(crate) const DM_COLS: &str =
    "channel, from_name, text, ts, from_me, url, fetched_at, dismissed_ts";
pub(crate) const MCP_CALL_COLS: &str = "id, ts, method, tool, args, ok, error, duration_ms, client";

/// Kanban ordering used across queries: board column, then manual position, then age.
pub(crate) const TASK_ORDER: &str = "\
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

/// Which gh actions a task's status change should trigger: entering `done`
/// closes every linked issue still cached `open`; leaving `done` reopens the
/// ones cached `closed`. Returns `(repo, number, close)` tuples. Empty for
/// link-less tasks, moves that don't touch `done`, and links already in the
/// target state (so re-running is a no-op and a half-failed batch converges on
/// retry).
///
/// A pure decision — the caller (`tt-app`'s `spawn_gh_status_sync`) turns the
/// tuples into `gh issue close`/`reopen` spawns. Lives here, next to
/// [`TaskIssueLink`], so it's unit-testable without the Tauri shell.
pub fn gh_close_reopen_targets(
    old_status: &str,
    new_status: &str,
    issues: &[TaskIssueLink],
) -> Vec<(String, i64, bool)> {
    if old_status == new_status {
        return Vec::new();
    }
    let close = if new_status == "done" {
        true
    } else if old_status == "done" {
        false
    } else {
        return Vec::new();
    };
    issues
        .iter()
        .filter(|link| if close { link.state != "closed" } else { link.state == "closed" })
        .map(|link| (link.repo.clone(), link.number, close))
        .collect()
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
