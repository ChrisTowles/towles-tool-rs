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
    MeetingStartEdge, MeetingStartWatch, ReviewRequestedEdge, ReviewRequestedWatch,
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
const SCHEMA_VERSION: i64 = 4;

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
    repo TEXT,
    issue_number INTEGER,
    issue_url TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    notes TEXT
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

// Column lists, kept in sync with the row-mapping closures below.
const EVENT_COLS: &str = "id, external_id, title, start_ts, end_ts, attendees, location, join_url";
const TASK_COLS: &str = "id, text, status, position, due_ts, repo, issue_number, issue_url, \
     created_at, completed_at, notes";
const ISSUE_COLS: &str = "repo, number, title, labels, state, url, updated_ts";
const PR_COLS: &str = "repo, number, title, branch, state, checks, review_state, url, updated_ts";
const RUN_COLS: &str = "collector, ran_at, ok, message";
const DM_COLS: &str = "channel, from_name, text, ts, from_me, url, fetched_at, dismissed_ts";

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

/// A kanban todo. Local by default; `repo`/`issue_number`/`issue_url` are set once a
/// todo is promoted to (or linked with) a GitHub issue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: i64,
    pub text: String,
    pub status: String,
    pub position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_ts: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
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

    /// Add a manually-entered todo. Lands in the `backlog` column at the end.
    /// `repo` associates it with a repository without linking an issue; `notes`
    /// is free-form context.
    pub fn add_task(
        &self,
        text: &str,
        due_ts: Option<i64>,
        repo: Option<&str>,
        notes: Option<&str>,
        now_ms: i64,
    ) -> Result<TaskItem> {
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE status = 'backlog'",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO tasks (text, status, position, due_ts, repo, notes, created_at,
                                completed_at)
             VALUES (?1, 'backlog', ?2, ?3, ?4, ?5, ?6, NULL)",
            params![text, position, due_ts, repo, notes, now_ms],
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

    /// Delete a todo permanently. Returns [`Error::TaskNotFound`] when no todo
    /// has `id`.
    pub fn delete_task(&self, id: i64) -> Result<()> {
        let affected = self.conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Link a todo to a GitHub issue (after promoting it via `gh issue create`).
    pub fn link_task_issue(&self, id: i64, repo: &str, number: i64, url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET repo = ?1, issue_number = ?2, issue_url = ?3 WHERE id = ?4",
            params![repo, number, url, id],
        )?;
        Ok(())
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

    /// All issue rows, newest update first.
    pub fn issues(&self) -> Result<Vec<IssueItem>> {
        self.query_issues(&format!("SELECT {ISSUE_COLS} FROM issues ORDER BY updated_ts DESC"), [])
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

    /// A single full snapshot of the store for the dashboard. The five reads
    /// share one transaction so a concurrent writer (CLI collector, another
    /// window) can't produce a torn cross-table view.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let tx = self.conn.unchecked_transaction()?;
        let events = self
            .query_events(&format!("SELECT {EVENT_COLS} FROM events ORDER BY start_ts ASC"), [])?;
        let tasks = self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks {TASK_ORDER}"), [])?;
        let issues = self.issues()?;
        let prs = self.prs()?;
        let runs = self.runs()?;
        let dms = self.dms()?;
        tx.commit()?;
        Ok(Snapshot { events, tasks, issues, prs, runs, dms })
    }

    // --- Row-mapping helpers ---------------------------------------------

    fn task_by_id(&self, id: i64) -> Result<TaskItem> {
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
            Ok(TaskItem {
                id: r.get(0)?,
                text: r.get(1)?,
                status: r.get(2)?,
                position: r.get(3)?,
                due_ts: r.get(4)?,
                repo: r.get(5)?,
                issue_number: r.get(6)?,
                issue_url: r.get(7)?,
                created_at: r.get(8)?,
                completed_at: r.get(9)?,
                notes: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
            s.add_task("survives", None, None, None, 1).unwrap();
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
        let added = s.add_task("new todo", None, None, None, 3).unwrap();
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
        assert_eq!(t.repo.as_deref(), Some("o/r"));
        assert_eq!(t.issue_number, Some(7));
        s.add_task("post-repair todo", None, None, None, 9).unwrap();
        assert!(!task_columns(&s).contains(&"source".to_string()));
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
        let a = s.add_task("first", None, None, None, 100).unwrap();
        let b = s.add_task("second", None, None, None, 200).unwrap();
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
        let t = s.add_task("ship it", None, None, None, 1).unwrap();
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
    fn add_task_stores_repo_and_notes() {
        let s = Store::open_in_memory().unwrap();
        let t =
            s.add_task("port the CLI", None, Some("o/r"), Some("start with doctor"), 1).unwrap();
        assert_eq!(t.repo.as_deref(), Some("o/r"));
        assert_eq!(t.notes.as_deref(), Some("start with doctor"));
        // No issue link yet: repo alone does not make it issue-linked.
        assert_eq!(t.issue_number, None);
        let bare = s.add_task("no context", None, None, None, 2).unwrap();
        assert_eq!(bare.repo, None);
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
        let t = s.add_task("with notes", None, None, Some("context"), 2).unwrap();
        assert_eq!(t.notes.as_deref(), Some("context"));
    }

    #[test]
    fn set_task_status_appends_to_end_of_target_column() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", None, None, None, 1).unwrap();
        let b = s.add_task("b", None, None, None, 2).unwrap();
        let c = s.add_task("c", None, None, None, 3).unwrap();

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
        let t = s.add_task("x", None, None, None, 1).unwrap();
        assert!(s.set_task_status(t.id, "bogus", 2).is_err());
    }

    #[test]
    fn link_task_issue_stores_reference() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire up board", None, None, None, 1).unwrap();
        s.link_task_issue(t.id, "o/r", 42, "https://github.com/o/r/issues/42").unwrap();
        let linked = s.open_tasks().unwrap()[0].clone();
        assert_eq!(linked.repo.as_deref(), Some("o/r"));
        assert_eq!(linked.issue_number, Some(42));
        assert_eq!(linked.issue_url.as_deref(), Some("https://github.com/o/r/issues/42"));
    }

    #[test]
    fn update_task_edits_text_notes_and_due() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("rough draft", None, None, None, 1).unwrap();
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
        let t = s.add_task("call dentist", None, None, None, 1).unwrap();
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
        let a = s.add_task("keep", None, None, None, 1).unwrap();
        let b = s.add_task("toss", None, None, None, 2).unwrap();
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
        s.add_task("do thing", Some(9), None, None, 1).unwrap();
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
}
