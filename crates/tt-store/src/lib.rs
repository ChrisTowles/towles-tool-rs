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
}

pub type Result<T> = std::result::Result<T, Error>;

/// Current on-disk schema version, stored in the `meta` table.
const SCHEMA_VERSION: i64 = 1;

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
    completed_at INTEGER
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
";

// Column lists, kept in sync with the row-mapping closures below.
const EVENT_COLS: &str = "id, external_id, title, start_ts, end_ts, attendees, location, join_url";
const TASK_COLS: &str =
    "id, text, status, position, due_ts, repo, issue_number, issue_url, created_at, completed_at";
const ISSUE_COLS: &str = "repo, number, title, labels, state, url, updated_ts";
const PR_COLS: &str = "repo, number, title, branch, state, checks, review_state, url, updated_ts";
const RUN_COLS: &str = "collector, ran_at, ok, message";

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

/// Full-store snapshot for the dashboard UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub events: Vec<CalEvent>,
    pub tasks: Vec<TaskItem>,
    pub issues: Vec<IssueItem>,
    pub prs: Vec<PrItem>,
    pub runs: Vec<CollectRun>,
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

    /// Open the store at the default location:
    /// `<data_dir>/towles-tool/tt.db` (e.g. `~/.local/share/towles-tool/tt.db`).
    pub fn open_default() -> Result<Store> {
        let path = dirs::data_dir().ok_or(Error::NoDataDir)?.join("towles-tool").join("tt.db");
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
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
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
    pub fn add_task(&self, text: &str, due_ts: Option<i64>, now_ms: i64) -> Result<TaskItem> {
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE status = 'backlog'",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO tasks (text, status, position, due_ts, created_at, completed_at)
             VALUES (?1, 'backlog', ?2, ?3, ?4, NULL)",
            params![text, position, due_ts, now_ms],
        )?;
        self.task_by_id(self.conn.last_insert_rowid())
    }

    /// Move a todo to a kanban column. Sets `completed_at` when entering `done`,
    /// clears it otherwise. Unknown statuses are rejected.
    pub fn set_task_status(&self, id: i64, status: &str, now_ms: i64) -> Result<()> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        self.conn.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2 WHERE id = ?3",
            params![status, completed_at, id],
        )?;
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

    /// The earliest event starting at/after `after_ms`, if any.
    pub fn next_event(&self, after_ms: i64) -> Result<Option<CalEvent>> {
        Ok(self
            .query_events(
                &format!(
                    "SELECT {EVENT_COLS} FROM events
                     WHERE start_ts >= ?1 ORDER BY start_ts ASC LIMIT 1"
                ),
                [after_ms],
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

    /// A single full snapshot of the store for the dashboard.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let events = self
            .query_events(&format!("SELECT {EVENT_COLS} FROM events ORDER BY start_ts ASC"), [])?;
        let tasks = self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks {TASK_ORDER}"), [])?;
        let issues = self.issues()?;
        let prs = self.prs()?;
        let runs = self.runs()?;
        Ok(Snapshot { events, tasks, issues, prs, runs })
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
            s.add_task("survives", None, 1).unwrap();
        }
        // Re-open: migrate runs again without error, data intact.
        let s = Store::open(&path).unwrap();
        assert_eq!(s.open_tasks().unwrap().len(), 1);
        let version: String = s
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, "1");
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
    fn add_task_lands_in_backlog_and_orders_by_position() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("first", None, 100).unwrap();
        let b = s.add_task("second", None, 200).unwrap();
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
        let t = s.add_task("ship it", None, 1).unwrap();
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
    fn set_task_status_rejects_unknown() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", None, 1).unwrap();
        assert!(s.set_task_status(t.id, "bogus", 2).is_err());
    }

    #[test]
    fn link_task_issue_stores_reference() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire up board", None, 1).unwrap();
        s.link_task_issue(t.id, "o/r", 42, "https://github.com/o/r/issues/42").unwrap();
        let linked = s.open_tasks().unwrap()[0].clone();
        assert_eq!(linked.repo.as_deref(), Some("o/r"));
        assert_eq!(linked.issue_number, Some(42));
        assert_eq!(linked.issue_url.as_deref(), Some("https://github.com/o/r/issues/42"));
    }

    #[test]
    fn events_between_and_next_event() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events(&[event("a", 100), event("b", 300), event("c", 500)], 1).unwrap();
        let win = s.events_between(150, 500).unwrap();
        assert_eq!(win.iter().map(|e| e.external_id.as_str()).collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(s.next_event(200).unwrap().unwrap().external_id, "b");
        assert!(s.next_event(600).unwrap().is_none());
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
        s.add_task("do thing", Some(9), 1).unwrap();
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
        ] {
            assert!(json.contains(key), "expected {key} in snapshot JSON: {json}");
        }
        // snake_case must not leak through.
        assert!(!json.contains("start_ts"));
        assert!(!json.contains("review_state"));
    }
}
