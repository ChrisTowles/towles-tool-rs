//! SQLite-backed store for the towles-tool "personal dashboard" data: calendar
//! events, kanban todos, issues, PR status, and collector run bookkeeping.
//!
//! This crate is deliberately Tauri-free (the shared-crate rule): both the CLI and
//! the Tauri app depend on it. All timestamps are epoch milliseconds (`i64`); clocks
//! are injected as `now_ms` parameters so logic stays deterministic under test.
//!
//! The public output structs serialize with `camelCase` keys to match the TypeScript
//! contract consumed by the frontend / Tauri commands.

use std::path::Path;

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
const SCHEMA_VERSION: i64 = 7;

/// How many MCP call-log rows are retained; older rows are pruned on insert.
const MCP_CALL_RETAIN: i64 = 500;

/// How many MCP call-log rows ride along in a [`Snapshot`] (newest first).
const MCP_CALL_SNAPSHOT_LIMIT: usize = 100;

/// Kanban columns a todo can live in, in board order.
pub const TASK_STATUSES: [&str; 5] = ["backlog", "next", "doing", "review", "done"];

/// Schema v1. Every statement is `IF NOT EXISTS` so `migrate` is idempotent.
const SCHEMA_V1: &str = "\
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
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
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'backlog',
    position INTEGER NOT NULL DEFAULT 0,
    due_ts INTEGER,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    notes TEXT,
    slot_repo_root TEXT,
    slot_repo TEXT,
    slot_branch TEXT,
    slot_dir TEXT
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
/// `tt mcp serve` dispatcher handled). `IF NOT EXISTS`, so `migrate` stays
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

// Column lists, kept in sync with the row-mapping closures below.
const EVENT_COLS: &str = "id, external_id, title, start_ts, end_ts, attendees, location, join_url";
const TASK_COLS: &str = "id, text, status, position, due_ts, created_at, completed_at, notes, \
     slot_repo_root, slot_repo, slot_branch, slot_dir";
const ISSUE_COLS: &str = "repo, number, title, labels, state, url, updated_ts";
const PR_COLS: &str = "repo, number, title, branch, state, checks, review_state, url, updated_ts";
const RUN_COLS: &str = "collector, ran_at, ok, message";
const DM_COLS: &str = "channel, from_name, text, ts, from_me, url, fetched_at, dismissed_ts";
const MCP_CALL_COLS: &str = "id, ts, method, tool, args, ok, error, duration_ms, client";

/// Kanban ordering used across queries: board column, then manual position, then age.
const TASK_ORDER: &str = "\
ORDER BY CASE status
    WHEN 'backlog' THEN 0 WHEN 'next' THEN 1 WHEN 'doing' THEN 2
    WHEN 'review' THEN 3 WHEN 'done' THEN 4 ELSE 5 END,
  position ASC, created_at ASC";

// ---------------------------------------------------------------------------
// Output structs (camelCase, matching the TypeScript contract).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalEvent {
    pub id: i64,
    pub external_id: String,
    pub title: String,
    pub start_ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ts: Option<i64>,
    pub attendees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_url: Option<String>,
}

/// A task on the board (#339): the unit of work. Local by default; it can
/// link any number of GitHub issues and PRs, and usually gets a worktree
/// slot (its [`TaskSlot`] binding).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: i64,
    pub text: String,
    pub status: String,
    pub position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_ts: Option<i64>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<TaskSlot>,
    #[serde(default)]
    pub issues: Vec<TaskIssueLink>,
    #[serde(default)]
    pub prs: Vec<TaskPrLink>,
}

/// A task's repo binding, and the slot its work happens in once one exists.
///
/// `repo_root` is the only required part: a task created from the Agentboard
/// knows its repo from the moment of submit, including a "task only" submit
/// that never creates a worktree. `branch` is therefore `None` until the slot
/// is created — which is what lets every task land in a repo swimlane on the
/// Board rather than an "unassigned" bucket.
///
/// `repo_root` and `branch` survive slot removal as historical fact; `dir` is
/// cleared when the worktree is removed (a "detached" task). `repo` is the
/// GitHub `owner/name`, used to auto-attach collected PRs whose head branch
/// matches `branch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSlot {
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
/// (from the session's `initialize`). Written by the `tt mcp serve` dispatcher,
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
    pub start_ts: i64,
    #[serde(default)]
    pub end_ts: Option<i64>,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub join_url: Option<String>,
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
    /// in a slot checkout it nests under `…/slots/<scope>/` (see [`tt_config`]).
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
        self.conn.execute_batch(SCHEMA_MCP_CALLS_V5)?;
        self.migrate_collect_runs_v6()?;
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
    /// `task_issues` link table, and the task gains its slot binding
    /// (`slot_repo_root`/`slot_repo`/`slot_branch`/`slot_dir`). A rebuild —
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
                slot_repo_root TEXT,
                slot_repo TEXT,
                slot_branch TEXT,
                slot_dir TEXT
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

    // --- Writes -----------------------------------------------------------

    /// Full-snapshot replace of all events.
    pub fn replace_events(&self, events: &[EventInput], now_ms: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM events", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO events
                   (external_id, title, start_ts, end_ts, attendees, location, join_url, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for e in events {
                stmt.execute(params![
                    e.external_id,
                    e.title,
                    e.start_ts,
                    e.end_ts,
                    serde_json::to_string(&e.attendees)?,
                    e.location,
                    e.join_url,
                    now_ms,
                ])?;
            }
        }
        tx.commit()?;
        Ok(events.len())
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
    /// [`Store::attach_task_pr`]), the slot via [`Store::set_task_slot`].
    pub fn add_task(
        &self,
        text: &str,
        status: &str,
        due_ts: Option<i64>,
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
            "INSERT INTO tasks (text, status, position, due_ts, notes, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![text, status, position, due_ts, notes, now_ms, completed_at],
        )?;
        self.task_by_id(self.conn.last_insert_rowid())
    }

    /// Move a todo to a kanban column, appending it at the end of the target
    /// column (position = max there + 1, ignoring the task itself). Sets
    /// `completed_at` when entering `done`, clears it otherwise. Unknown
    /// statuses are rejected.
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
            "UPDATE tasks SET status = ?1, completed_at = ?2, position = ?3 WHERE id = ?4",
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
    /// arbitrary slot — it powers drag-to-reorder within a column and
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
        let slot = index.clamp(0, others.len() as i64) as usize;
        let mut order = others;
        order.insert(slot, id);
        {
            let mut up = tx.prepare("UPDATE tasks SET position = ?1 WHERE id = ?2")?;
            for (pos, tid) in order.iter().enumerate() {
                up.execute(params![pos as i64, tid])?;
            }
        }
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        let affected = tx.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2 WHERE id = ?3",
            params![status, completed_at, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.commit()?;
        Ok(())
    }

    /// Edit a todo's free-form fields: its `text`, optional `notes`, and optional
    /// `due_ts`. This is a full replace of those three fields — passing `None`
    /// for `notes` or `due_ts` clears them (there is no "leave unchanged"
    /// sentinel). Status, position, and any issue link are left untouched.
    /// Returns the updated todo, or [`Error::TaskNotFound`] when no todo has `id`.
    pub fn update_task(
        &self,
        id: i64,
        text: &str,
        notes: Option<&str>,
        due_ts: Option<i64>,
    ) -> Result<TaskItem> {
        let affected = self.conn.execute(
            "UPDATE tasks SET text = ?1, notes = ?2, due_ts = ?3 WHERE id = ?4",
            params![text, notes, due_ts, id],
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

    /// Delete `done` tasks completed before `before_ms` (cascading their link
    /// rows), returning how many tasks were removed. Open tasks and
    /// recently-completed `done` tasks are left untouched. A `done` row with a
    /// NULL `completed_at` (legacy data) is never swept, since its completion
    /// time is unknown. The cutoff is injected — the clock read happens at the
    /// call boundary, not here.
    pub fn clear_done_tasks(&self, before_ms: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let deleted = tx.execute(
            "DELETE FROM tasks
             WHERE status = 'done' AND completed_at IS NOT NULL AND completed_at < ?1",
            params![before_ms],
        )?;
        tx.execute("DELETE FROM task_issues WHERE task_id NOT IN (SELECT id FROM tasks)", [])?;
        tx.execute("DELETE FROM task_prs WHERE task_id NOT IN (SELECT id FROM tasks)", [])?;
        tx.commit()?;
        Ok(deleted)
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

    /// Bind a task to its repo, and to the worktree slot its work happens in
    /// once one exists. Called twice in the Agentboard's new-task flow: at
    /// submit with the repo alone (`branch`/`dir` `None`), then again once
    /// `slot_create` resolves. A "task only" submit stops after the first.
    ///
    /// The optional columns are upserts, never clears: a `None` means "leave
    /// as is" (`COALESCE`), so a repo-only rebind — e.g. retrying a failed
    /// `slot_create` on a task whose worktree already exists — can't erase an
    /// established branch/dir. Clearing `dir` has its own dedicated path
    /// ([`Store::clear_task_slot_dir`]); nothing legitimately un-sets a
    /// branch. Returns [`Error::TaskNotFound`] when no task has `id`.
    pub fn set_task_slot(
        &self,
        id: i64,
        repo_root: &str,
        repo: Option<&str>,
        branch: Option<&str>,
        dir: Option<&str>,
    ) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE tasks SET slot_repo_root = ?1,
                              slot_repo = COALESCE(?2, slot_repo),
                              slot_branch = COALESCE(?3, slot_branch),
                              slot_dir = COALESCE(?4, slot_dir)
             WHERE id = ?5",
            params![repo_root, repo, branch, dir, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Detach the worktree from whatever task holds it: clears `slot_dir` for
    /// tasks bound to `dir`, keeping `slot_repo_root`/`slot_branch` as
    /// historical fact. Called from the slot-removal seam. Returns how many
    /// tasks were detached (0 when the slot had no task — not an error).
    pub fn clear_task_slot_dir(&self, dir: &str) -> Result<usize> {
        let affected = self
            .conn
            .execute("UPDATE tasks SET slot_dir = NULL WHERE slot_dir = ?1", params![dir])?;
        Ok(affected)
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
                 WHERE start_ts >= ?1 AND start_ts < ?2 ORDER BY start_ts ASC"
            ),
            params![start_ms, end_ms],
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
                     WHERE (end_ts IS NOT NULL AND end_ts > ?1)
                        OR (end_ts IS NULL AND start_ts >= ?1)
                     ORDER BY start_ts ASC LIMIT 1"
                ),
                [now_ms],
            )?
            .into_iter()
            .next())
    }

    /// Open (not-done) todos in kanban order.
    pub fn open_tasks(&self) -> Result<Vec<TaskItem>> {
        self.query_tasks(
            &format!("SELECT {TASK_COLS} FROM tasks WHERE status != 'done' {TASK_ORDER}"),
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

    /// All tasks in kanban order, links and slot included. The collectors'
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

    /// Auto-attach collected PRs to slot-bound tasks: any `pr_status` row
    /// whose `(repo, branch)` matches a task's `(slot_repo, slot_branch)`
    /// becomes a `task_prs` link — "PRs open in the slot, linked to the task"
    /// without a manual step. Existing links are left untouched. Returns how
    /// many links were created.
    pub fn auto_attach_slot_prs(&self, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO task_prs (task_id, repo, number, url, state, checks, state_ts)
             SELECT t.id, p.repo, p.number, p.url, p.state, p.checks, ?1
             FROM tasks t
             JOIN pr_status p ON p.repo = t.slot_repo AND p.branch = t.slot_branch
             WHERE t.slot_repo IS NOT NULL AND t.slot_branch IS NOT NULL",
            params![now_ms],
        )?)
    }

    /// The task bound to the worktree at `dir`, if any (a slot belongs to at
    /// most one task; if data ever disagrees, the oldest task wins).
    pub fn task_for_slot_dir(&self, dir: &str) -> Result<Option<TaskItem>> {
        Ok(self
            .query_tasks(
                &format!(
                    "SELECT {TASK_COLS} FROM tasks WHERE slot_dir = ?1
                     ORDER BY created_at ASC LIMIT 1"
                ),
                params![dir],
            )?
            .into_iter()
            .next())
    }

    /// All issue rows, newest update first.
    pub fn issues(&self) -> Result<Vec<IssueItem>> {
        self.query_issues(&format!("SELECT {ISSUE_COLS} FROM issues ORDER BY updated_ts DESC"), [])
    }

    /// A single cached issue row by `(repo, number)`, if the collector has seen it.
    pub fn get_issue(&self, repo: &str, number: i64) -> Result<Option<IssueItem>> {
        Ok(self
            .query_issues(
                &format!("SELECT {ISSUE_COLS} FROM issues WHERE repo = ?1 AND number = ?2"),
                params![repo, number],
            )?
            .into_iter()
            .next())
    }

    /// All PR status rows, newest update first.
    pub fn prs(&self) -> Result<Vec<PrItem>> {
        self.query_prs(&format!("SELECT {PR_COLS} FROM pr_status ORDER BY updated_ts DESC"), [])
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
        let events = self
            .query_events(&format!("SELECT {EVENT_COLS} FROM events ORDER BY start_ts ASC"), [])?;
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

    /// One task by id, with its links and slot binding (the same row shape
    /// [`Store::open_tasks`] returns).
    pub fn task_by_id(&self, id: i64) -> Result<TaskItem> {
        self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [id])?
            .into_iter()
            .next()
            .ok_or(Error::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    fn query_events(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<CalEvent>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, external_id, title, start_ts, end_ts, attendees_json, location, join_url) =
                row?;
            let attendees: Vec<String> = serde_json::from_str(&attendees_json)?;
            out.push(CalEvent {
                id,
                external_id,
                title,
                start_ts,
                end_ts,
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
            let slot_repo_root: Option<String> = r.get(8)?;
            let slot_repo: Option<String> = r.get(9)?;
            let slot_branch: Option<String> = r.get(10)?;
            let slot_dir: Option<String> = r.get(11)?;
            // Keyed on `repo_root` alone: a repo-bound task with no worktree
            // yet still has a slot binding, and dropping it here would hide
            // the task's repo from the Board's swimlanes.
            let slot = slot_repo_root.map(|repo_root| TaskSlot {
                repo_root,
                repo: slot_repo,
                branch: slot_branch,
                dir: slot_dir,
            });
            Ok(TaskItem {
                id: r.get(0)?,
                text: r.get(1)?,
                status: r.get(2)?,
                position: r.get(3)?,
                due_ts: r.get(4)?,
                created_at: r.get(5)?,
                completed_at: r.get(6)?,
                notes: r.get(7)?,
                slot,
                issues: Vec::new(),
                prs: Vec::new(),
            })
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
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (repo, number, title, labels_json, state, url, updated_ts) = row?;
            let labels: Vec<String> = serde_json::from_str(&labels_json)?;
            out.push(IssueItem { repo, number, title, labels, state, url, updated_ts });
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

    fn event(ext: &str, start: i64) -> EventInput {
        EventInput {
            external_id: ext.to_string(),
            title: format!("Event {ext}"),
            start_ts: start,
            end_ts: Some(start + 1000),
            attendees: vec!["a@example.com".to_string()],
            location: None,
            join_url: None,
        }
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
            s.add_task("survives", "backlog", None, None, 1).unwrap();
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
        let added = s.add_task("new todo", "backlog", None, None, 3).unwrap();
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
        s.add_task("post-repair todo", "backlog", None, None, 9).unwrap();
        assert!(!task_columns(&s).contains(&"source".to_string()));
        assert!(!task_columns(&s).contains(&"repo".to_string()));
    }

    #[test]
    fn replace_events_is_full_swap() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events(&[event("a", 100), event("b", 200)], 1).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2);
        let n = s.replace_events(&[event("c", 300)], 2).unwrap();
        assert_eq!(n, 1);
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].external_id, "c");
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
    fn attach_detach_issue_links_and_get_issue() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        let plain = s.add_task("plain task", "backlog", None, None, 1).unwrap();
        let linked = s.add_task("linked task", "backlog", None, None, 2).unwrap();
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
    fn slot_binding_set_lookup_and_detach() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("slot-backed", "doing", None, None, 1).unwrap();
        assert!(t.slot.is_none());
        s.set_task_slot(
            t.id,
            "/repos/x",
            Some("o/x"),
            Some("feat/y"),
            Some("/repos/x/.claude/worktrees/feat-y"),
        )
        .unwrap();

        let bound = s.task_for_slot_dir("/repos/x/.claude/worktrees/feat-y").unwrap().unwrap();
        assert_eq!(bound.id, t.id);
        let slot = bound.slot.unwrap();
        assert_eq!(slot.repo_root, "/repos/x");
        assert_eq!(slot.repo.as_deref(), Some("o/x"));
        assert_eq!(slot.branch.as_deref(), Some("feat/y"));

        // A repo-only rebind (the retry path re-sends the submit-time bind)
        // upserts: `None` means "leave as is", never "clear".
        s.set_task_slot(t.id, "/repos/x", None, None, None).unwrap();
        let rebound = s.get_task(t.id).unwrap().unwrap().slot.unwrap();
        assert_eq!(rebound.repo.as_deref(), Some("o/x"));
        assert_eq!(rebound.branch.as_deref(), Some("feat/y"));
        assert_eq!(rebound.dir.as_deref(), Some("/repos/x/.claude/worktrees/feat-y"));

        // Removing the worktree detaches the dir but keeps branch + root.
        let n = s.clear_task_slot_dir("/repos/x/.claude/worktrees/feat-y").unwrap();
        assert_eq!(n, 1);
        assert!(s.task_for_slot_dir("/repos/x/.claude/worktrees/feat-y").unwrap().is_none());
        let after = s.get_task(t.id).unwrap().unwrap().slot.unwrap();
        assert_eq!(after.branch.as_deref(), Some("feat/y"));
        assert_eq!(after.dir, None);
        // Clearing an unknown dir is a 0-count no-op; unknown task errors.
        assert_eq!(s.clear_task_slot_dir("/nope").unwrap(), 0);
        assert!(matches!(
            s.set_task_slot(777, "/r", None, Some("b"), None),
            Err(Error::TaskNotFound(777))
        ));
    }

    #[test]
    fn refresh_link_states_from_cache_and_missing_refs() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("t", "doing", None, None, 1).unwrap();
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
    fn auto_attach_slot_prs_links_by_repo_and_branch() {
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
        let t = s.add_task("slot task", "doing", None, None, 1).unwrap();
        s.set_task_slot(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/w")).unwrap();
        let other = s.add_task("no slot", "backlog", None, None, 2).unwrap();

        s.replace_prs(&[pr("feat/y", 7), pr("other-branch", 8)]).unwrap();
        let n = s.auto_attach_slot_prs(9).unwrap();
        assert_eq!(n, 1);
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(got.prs.len(), 1);
        assert_eq!(got.prs[0].number, 7);
        assert!(s.get_task(other.id).unwrap().unwrap().prs.is_empty());

        // Idempotent: a second pass creates nothing new.
        assert_eq!(s.auto_attach_slot_prs(10).unwrap(), 0);
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
        let a = s.add_task("first", "backlog", None, None, 100).unwrap();
        let b = s.add_task("second", "backlog", None, None, 200).unwrap();
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
        let t = s.add_task("ship it", "backlog", None, None, 1).unwrap();
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
        s.set_task_status(t.id, "next", 30).unwrap();
        let reopened = s.open_tasks().unwrap();
        assert_eq!(reopened[0].status, "next");
        assert_eq!(reopened[0].completed_at, None);
    }

    #[test]
    fn clear_done_tasks_sweeps_only_old_done() {
        let s = Store::open_in_memory().unwrap();
        let old = s.add_task("old done", "backlog", None, None, 1).unwrap();
        let recent = s.add_task("recent done", "backlog", None, None, 2).unwrap();
        let open = s.add_task("still open", "backlog", None, None, 3).unwrap();
        s.set_task_status(old.id, "done", 100).unwrap();
        s.set_task_status(recent.id, "done", 5_000).unwrap();
        s.set_task_status(open.id, "doing", 4).unwrap();

        // Cutoff between the two done todos: only the old one is swept.
        let deleted = s.clear_done_tasks(1_000).unwrap();
        assert_eq!(deleted, 1);

        let remaining: Vec<i64> = s.snapshot().unwrap().tasks.iter().map(|t| t.id).collect();
        assert!(!remaining.contains(&old.id));
        assert!(remaining.contains(&recent.id));
        assert!(remaining.contains(&open.id));

        // Nothing else old enough on a second sweep.
        assert_eq!(s.clear_done_tasks(1_000).unwrap(), 0);
    }

    #[test]
    fn add_task_stores_notes_and_lands_in_requested_status() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("port the CLI", "backlog", None, Some("start with doctor"), 1).unwrap();
        assert_eq!(t.notes.as_deref(), Some("start with doctor"));
        assert!(t.issues.is_empty() && t.prs.is_empty() && t.slot.is_none());
        // A slot-backed task is born straight into `doing`.
        let d = s.add_task("agent already running", "doing", None, None, 2).unwrap();
        assert_eq!(d.status, "doing");
        assert_eq!(d.completed_at, None);
        // Unknown statuses are rejected.
        assert!(s.add_task("nope", "bogus", None, None, 3).is_err());
        let bare = s.add_task("no context", "backlog", None, None, 4).unwrap();
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
        let t = s.add_task("with notes", "backlog", None, Some("context"), 2).unwrap();
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
        let a = s.add_task("a", "backlog", None, None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, None, 3).unwrap();

        // Moving into an empty column starts at 0; the next arrival lands after it.
        s.set_task_status(a.id, "doing", 10).unwrap();
        s.set_task_status(b.id, "doing", 11).unwrap();
        let pos = |id: i64, tasks: &[TaskItem]| tasks.iter().find(|t| t.id == id).unwrap().position;
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(a.id, &tasks), 0);
        assert_eq!(pos(b.id, &tasks), 1);

        // A later drop into the same column lands at the end, not at its old slot.
        s.set_task_status(c.id, "doing", 12).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(c.id, &tasks), 2);

        // Bouncing a card out and back re-appends it after the survivors.
        s.set_task_status(a.id, "review", 13).unwrap();
        s.set_task_status(a.id, "doing", 14).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(a.id, &tasks), 3);
    }

    #[test]
    fn set_task_status_rejects_unknown() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", "backlog", None, None, 1).unwrap();
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
        let a = s.add_task("a", "backlog", None, None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, None, 3).unwrap();
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
        let a = s.add_task("a", "backlog", None, None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, None, 2).unwrap();
        s.set_task_status(a.id, "doing", 3).unwrap();
        s.set_task_status(b.id, "doing", 4).unwrap();
        // doing = [a, b].
        let c = s.add_task("c", "backlog", None, None, 5).unwrap();

        // Drop c between a and b.
        s.set_task_position(c.id, "doing", 1, 6).unwrap();
        assert_eq!(column_ids(&s, "doing"), vec![a.id, c.id, b.id]);
        assert!(column_ids(&s, "backlog").is_empty());
    }

    #[test]
    fn set_task_position_stamps_and_clears_done() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship", "backlog", None, None, 1).unwrap();
        s.set_task_position(t.id, "done", 0, 20).unwrap();
        let done = s.snapshot().unwrap().tasks.into_iter().find(|x| x.id == t.id).unwrap();
        assert_eq!(done.status, "done");
        assert_eq!(done.completed_at, Some(20));

        s.set_task_position(t.id, "next", 0, 30).unwrap();
        let reopened = s.open_tasks().unwrap();
        assert_eq!(reopened[0].status, "next");
        assert_eq!(reopened[0].completed_at, None);
    }

    #[test]
    fn set_task_position_is_stable_under_repeated_moves() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, None, 3).unwrap();
        // Dropping a card onto its own slot leaves the order unchanged.
        for _ in 0..5 {
            s.set_task_position(b.id, "backlog", 1, 10).unwrap();
        }
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);
    }

    #[test]
    fn set_task_position_rejects_unknown_status_and_missing_id() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", "backlog", None, None, 1).unwrap();
        assert!(s.set_task_position(t.id, "bogus", 0, 2).is_err());
        assert!(matches!(
            s.set_task_position(9999, "backlog", 0, 2),
            Err(Error::TaskNotFound(9999))
        ));
    }

    #[test]
    fn attach_task_issue_stores_reference() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire up board", "backlog", None, None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 42, "https://github.com/o/r/issues/42").unwrap();
        let linked = s.open_tasks().unwrap()[0].clone();
        assert_eq!(linked.issues.len(), 1);
        assert_eq!(linked.issues[0].repo, "o/r");
        assert_eq!(linked.issues[0].number, 42);
        assert_eq!(linked.issues[0].url, "https://github.com/o/r/issues/42");
    }

    #[test]
    fn update_task_edits_text_notes_and_due() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("rough draft", "backlog", None, None, 1).unwrap();
        let updated = s.update_task(t.id, "polished", Some("ship friday"), Some(500)).unwrap();
        assert_eq!(updated.text, "polished");
        assert_eq!(updated.notes.as_deref(), Some("ship friday"));
        assert_eq!(updated.due_ts, Some(500));
        // Status/position are untouched by an edit.
        assert_eq!(updated.status, "backlog");
        assert_eq!(updated.position, t.position);
        // And it persists.
        assert_eq!(s.get_task(t.id).unwrap().unwrap().text, "polished");
    }

    #[test]
    fn update_task_can_set_and_clear_due_date() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("call dentist", "backlog", None, None, 1).unwrap();
        assert_eq!(t.due_ts, None);
        let with_due = s.update_task(t.id, "call dentist", None, Some(900)).unwrap();
        assert_eq!(with_due.due_ts, Some(900));
        // Passing None clears it back out.
        let cleared = s.update_task(t.id, "call dentist", None, None).unwrap();
        assert_eq!(cleared.due_ts, None);
    }

    #[test]
    fn update_task_nonexistent_errors() {
        let s = Store::open_in_memory().unwrap();
        let err = s.update_task(999, "ghost", None, None).unwrap_err();
        assert!(matches!(err, Error::TaskNotFound(999)));
    }

    #[test]
    fn delete_task_removes_row() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("keep", "backlog", None, None, 1).unwrap();
        let b = s.add_task("toss", "backlog", None, None, 2).unwrap();
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
        s.replace_events(&[event("a", 100), event("b", 300), event("c", 500)], 1).unwrap();
        let win = s.events_between(150, 500).unwrap();
        assert_eq!(win.iter().map(|e| e.external_id.as_str()).collect::<Vec<_>>(), vec!["b"]);
    }

    #[test]
    fn current_or_next_event_across_the_meeting_lifecycle() {
        // The `event` helper spans [start, start + 1000). Two non-overlapping
        // meetings: "b" runs [300, 1300), "c" runs [1500, 2500).
        let s = Store::open_in_memory().unwrap();
        s.replace_events(&[event("b", 300), event("c", 1500)], 1).unwrap();

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
        s.replace_events(
            &[EventInput {
                external_id: "no-end".to_string(),
                title: "Open-ended".to_string(),
                start_ts: 500,
                end_ts: None,
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
    fn snapshot_serializes_camel_case() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events(
            &[EventInput {
                external_id: "x".to_string(),
                title: "T".to_string(),
                start_ts: 1,
                end_ts: Some(2),
                attendees: vec!["a@b.com".to_string()],
                location: Some("room".to_string()),
                join_url: Some("https://meet".to_string()),
            }],
            1,
        )
        .unwrap();
        s.add_task("do thing", "backlog", Some(9), None, 1).unwrap();
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
            "\"startTs\"",
            "\"externalId\"",
            "\"joinUrl\"",
            "\"dueTs\"",
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
    fn clear_done_tasks_never_sweeps_legacy_null_completed_at() {
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
        let normal = s.add_task("normal done", "backlog", None, None, 2).unwrap();
        s.set_task_status(normal.id, "done", 10).unwrap();

        let deleted = s.clear_done_tasks(1_000_000).unwrap();
        assert_eq!(deleted, 1, "only the stamped done row is swept");
        let texts: Vec<String> = s.snapshot().unwrap().tasks.into_iter().map(|t| t.text).collect();
        assert_eq!(texts, vec!["legacy done".to_string()]);
    }

    #[test]
    fn events_between_is_start_inclusive_end_exclusive() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events(
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
        s.replace_events(
            &[
                EventInput {
                    external_id: "many".to_string(),
                    title: "Sync".to_string(),
                    start_ts: 100,
                    end_ts: Some(200),
                    attendees: vec!["a@x.com".to_string(), "b@x.com".to_string()],
                    location: Some("Room 1".to_string()),
                    join_url: Some("https://meet/x".to_string()),
                },
                EventInput {
                    external_id: "none".to_string(),
                    title: "Solo".to_string(),
                    start_ts: 300,
                    end_ts: None,
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
        assert_eq!(none.end_ts, None);
    }

    #[test]
    fn open_tasks_orders_across_columns_by_board_order() {
        let s = Store::open_in_memory().unwrap();
        s.add_task("backlog item", "backlog", None, None, 1).unwrap();
        let review = s.add_task("review item", "backlog", None, None, 2).unwrap();
        let doing = s.add_task("doing item", "backlog", None, None, 3).unwrap();
        let next = s.add_task("next item", "backlog", None, None, 4).unwrap();
        let done = s.add_task("done item", "backlog", None, None, 5).unwrap();
        s.set_task_status(review.id, "review", 10).unwrap();
        s.set_task_status(doing.id, "doing", 11).unwrap();
        s.set_task_status(next.id, "next", 12).unwrap();
        s.set_task_status(done.id, "done", 13).unwrap();

        // open_tasks excludes done and returns backlog → next → doing → review.
        let statuses: Vec<String> = s.open_tasks().unwrap().into_iter().map(|t| t.status).collect();
        assert_eq!(
            statuses,
            vec![
                "backlog".to_string(),
                "next".to_string(),
                "doing".to_string(),
                "review".to_string(),
            ]
        );
    }

    #[test]
    fn snapshot_tasks_place_done_column_last() {
        let s = Store::open_in_memory().unwrap();
        let d = s.add_task("finish", "backlog", None, None, 1).unwrap();
        s.add_task("start", "backlog", None, None, 2).unwrap();
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

    #[test]
    fn update_task_leaves_links_and_slot_intact() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire board", "backlog", None, None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 7, "https://github.com/o/r/issues/7").unwrap();
        s.set_task_slot(t.id, "/repos/r", Some("o/r"), Some("feat/wire"), Some("/w")).unwrap();
        let updated = s.update_task(t.id, "wire board v2", Some("note"), None).unwrap();
        assert_eq!(updated.text, "wire board v2");
        // Editing free-form fields must not disturb links or the slot binding.
        assert_eq!(updated.issues.len(), 1);
        assert_eq!(updated.issues[0].number, 7);
        assert_eq!(updated.slot.unwrap().branch.as_deref(), Some("feat/wire"));
    }

    #[test]
    fn attach_task_issue_accumulates_links() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("multi-issue", "backlog", None, None, 1).unwrap();
        s.attach_task_issue(t.id, "o/a", 1, "https://github.com/o/a/issues/1").unwrap();
        s.attach_task_issue(t.id, "o/b", 2, "https://github.com/o/b/issues/2").unwrap();
        let got = s.get_task(t.id).unwrap().unwrap();
        // Attaching a second issue adds a link — it no longer overwrites.
        assert_eq!(got.issues.len(), 2);
        let repos: Vec<&str> = got.issues.iter().map(|l| l.repo.as_str()).collect();
        assert_eq!(repos, vec!["o/a", "o/b"]);
    }

    #[test]
    fn delete_and_clear_done_cascade_link_rows() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, None, 2).unwrap();
        s.attach_task_issue(a.id, "o/r", 1, "u").unwrap();
        s.attach_task_pr(a.id, "o/r", 2, "u").unwrap();
        s.attach_task_issue(b.id, "o/r", 3, "u").unwrap();

        s.delete_task(a.id).unwrap();
        assert_eq!(issue_link_rows(&s), vec![(b.id, "o/r".to_string(), 3)]);
        assert!(s.linked_pr_refs().unwrap().is_empty());

        s.set_task_status(b.id, "done", 10).unwrap();
        assert_eq!(s.clear_done_tasks(100).unwrap(), 1);
        assert!(issue_link_rows(&s).is_empty());
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
        for added in ["slot_repo_root", "slot_repo", "slot_branch", "slot_dir"] {
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
}
