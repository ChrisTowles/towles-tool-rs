//! SQLite-backed store for the towles-tool "personal dashboard" data: calendar
//! events, tasks, emails, PR status, and collector run bookkeeping.
//!
//! This crate is deliberately Tauri-free (the shared-crate rule): both the CLI and
//! the Tauri app depend on it. All timestamps are epoch milliseconds (`i64`); clocks
//! are injected as `now_ms` parameters so logic stays deterministic under test.
//!
//! The public output structs serialize with `camelCase` keys to match the TypeScript
//! contract consumed by the frontend / Tauri commands.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
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
    source TEXT NOT NULL,
    source_ref TEXT,
    text TEXT NOT NULL,
    due_ts INTEGER,
    done INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);
CREATE TABLE IF NOT EXISTS emails (
    id INTEGER PRIMARY KEY,
    external_id TEXT NOT NULL UNIQUE,
    from_name TEXT NOT NULL,
    from_addr TEXT NOT NULL,
    subject TEXT NOT NULL,
    summary TEXT NOT NULL,
    tag TEXT NOT NULL,
    received_ts INTEGER NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0
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
const TASK_COLS: &str = "id, source, source_ref, text, due_ts, done, created_at, completed_at";
const EMAIL_COLS: &str =
    "id, external_id, from_name, from_addr, subject, summary, tag, received_ts, archived";
const PR_COLS: &str = "repo, number, title, branch, state, checks, review_state, url, updated_ts";
const RUN_COLS: &str = "collector, ran_at, ok, message";

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: i64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_ts: Option<i64>,
    pub done: bool,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailItem {
    pub id: i64,
    pub external_id: String,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub summary: String,
    pub tag: String,
    pub received_ts: i64,
    pub archived: bool,
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
    pub emails: Vec<EmailItem>,
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
pub struct TaskInput {
    pub source: String,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub text: String,
    #[serde(default)]
    pub due_ts: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailInput {
    pub external_id: String,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub summary: String,
    pub tag: String,
    pub received_ts: i64,
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

    /// Reconcile emails: delete rows whose `external_id` is not in the new set, and
    /// upsert the rest. The `archived` flag of an existing matching row is preserved.
    pub fn replace_emails(&self, emails: &[EmailInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let keep: Vec<&str> = emails.iter().map(|e| e.external_id.as_str()).collect();
        {
            let mut existing: Vec<String> = Vec::new();
            let mut stmt = tx.prepare("SELECT external_id FROM emails")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                existing.push(r?);
            }
            for id in &existing {
                if !keep.contains(&id.as_str()) {
                    tx.execute("DELETE FROM emails WHERE external_id = ?1", [id])?;
                }
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO emails
                   (external_id, from_name, from_addr, subject, summary, tag, received_ts, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
                 ON CONFLICT(external_id) DO UPDATE SET
                   from_name = excluded.from_name,
                   from_addr = excluded.from_addr,
                   subject = excluded.subject,
                   summary = excluded.summary,
                   tag = excluded.tag,
                   received_ts = excluded.received_ts",
            )?;
            for e in emails {
                stmt.execute(params![
                    e.external_id,
                    e.from_name,
                    e.from_addr,
                    e.subject,
                    e.summary,
                    e.tag,
                    e.received_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(emails.len())
    }

    /// Merge tasks by `(source, source_ref)`: matching rows have their `text`/`due_ts`
    /// refreshed (keeping `done`/`completed_at`); non-matching rows are inserted.
    pub fn upsert_tasks(&self, tasks: &[TaskInput], now_ms: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        for t in tasks {
            let existing: Option<i64> = match &t.source_ref {
                Some(sr) => tx
                    .query_row(
                        "SELECT id FROM tasks WHERE source = ?1 AND source_ref = ?2",
                        params![t.source, sr],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => tx
                    .query_row(
                        "SELECT id FROM tasks WHERE source = ?1 AND source_ref IS NULL",
                        params![t.source],
                        |r| r.get(0),
                    )
                    .optional()?,
            };
            match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE tasks SET text = ?1, due_ts = ?2 WHERE id = ?3",
                        params![t.text, t.due_ts, id],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO tasks
                           (source, source_ref, text, due_ts, done, created_at, completed_at)
                         VALUES (?1, ?2, ?3, ?4, 0, ?5, NULL)",
                        params![t.source, t.source_ref, t.text, t.due_ts, now_ms],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(tasks.len())
    }

    /// Add a manually-entered task (`source = "manual"`, no `source_ref`).
    pub fn add_task(&self, text: &str, due_ts: Option<i64>, now_ms: i64) -> Result<TaskItem> {
        self.conn.execute(
            "INSERT INTO tasks
               (source, source_ref, text, due_ts, done, created_at, completed_at)
             VALUES ('manual', NULL, ?1, ?2, 0, ?3, NULL)",
            params![text, due_ts, now_ms],
        )?;
        self.task_by_id(self.conn.last_insert_rowid())
    }

    /// Mark a task done/undone; sets `completed_at` to `now_ms` when done, clears it otherwise.
    pub fn set_task_done(&self, id: i64, done: bool, now_ms: i64) -> Result<()> {
        let completed_at: Option<i64> = if done { Some(now_ms) } else { None };
        self.conn.execute(
            "UPDATE tasks SET done = ?1, completed_at = ?2 WHERE id = ?3",
            params![done, completed_at, id],
        )?;
        Ok(())
    }

    /// Archive an email by row id.
    pub fn archive_email(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE emails SET archived = 1 WHERE id = ?1", [id])?;
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

    /// Open (not-done) tasks, due-soonest first (null due dates last), then oldest first.
    pub fn open_tasks(&self) -> Result<Vec<TaskItem>> {
        self.query_tasks(
            &format!(
                "SELECT {TASK_COLS} FROM tasks WHERE done = 0
                 ORDER BY due_ts IS NULL, due_ts ASC, created_at ASC"
            ),
            [],
        )
    }

    /// Active (non-archived) emails, tag-priority first (needs_reply, invite, fyi),
    /// then newest first.
    pub fn emails_active(&self) -> Result<Vec<EmailItem>> {
        self.query_emails(
            &format!(
                "SELECT {EMAIL_COLS} FROM emails WHERE archived = 0
                 ORDER BY CASE tag
                   WHEN 'needs_reply' THEN 0 WHEN 'invite' THEN 1 WHEN 'fyi' THEN 2 ELSE 3 END,
                   received_ts DESC"
            ),
            [],
        )
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
        // All tasks (incl. done): open first, due-soonest, oldest first.
        let tasks = self.query_tasks(
            &format!(
                "SELECT {TASK_COLS} FROM tasks
                 ORDER BY done ASC, due_ts IS NULL, due_ts ASC, created_at ASC"
            ),
            [],
        )?;
        let emails = self.emails_active()?;
        let prs = self.prs()?;
        let runs = self.runs()?;
        Ok(Snapshot { events, tasks, emails, prs, runs })
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
                source: r.get(1)?,
                source_ref: r.get(2)?,
                text: r.get(3)?,
                due_ts: r.get(4)?,
                done: r.get(5)?,
                created_at: r.get(6)?,
                completed_at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn query_emails(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<EmailItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(EmailItem {
                id: r.get(0)?,
                external_id: r.get(1)?,
                from_name: r.get(2)?,
                from_addr: r.get(3)?,
                subject: r.get(4)?,
                summary: r.get(5)?,
                tag: r.get(6)?,
                received_ts: r.get(7)?,
                archived: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

    fn email(ext: &str, tag: &str, received: i64) -> EmailInput {
        EmailInput {
            external_id: ext.to_string(),
            from_name: "Sender".to_string(),
            from_addr: "sender@example.com".to_string(),
            subject: format!("Subject {ext}"),
            summary: "summary".to_string(),
            tag: tag.to_string(),
            received_ts: received,
        }
    }

    fn task(source: &str, source_ref: Option<&str>, text: &str, due: Option<i64>) -> TaskInput {
        TaskInput {
            source: source.to_string(),
            source_ref: source_ref.map(str::to_string),
            text: text.to_string(),
            due_ts: due,
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
    fn replace_emails_preserves_archived_and_deletes_missing() {
        let s = Store::open_in_memory().unwrap();
        s.replace_emails(&[email("e1", "needs_reply", 100)]).unwrap();
        let id = s.emails_active().unwrap()[0].id;
        s.archive_email(id).unwrap();
        assert!(s.emails_active().unwrap().is_empty());

        // e1 re-appears (tag changed) and a new e2 arrives.
        s.replace_emails(&[email("e1", "fyi", 150), email("e2", "invite", 120)]).unwrap();
        let active = s.emails_active().unwrap();
        // e1 stays archived (flag preserved), so only e2 is active.
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].external_id, "e2");

        // Dropping e2 from the set deletes it.
        s.replace_emails(&[email("e1", "fyi", 150)]).unwrap();
        assert!(s.emails_active().unwrap().is_empty());
    }

    #[test]
    fn upsert_tasks_merge_keeps_done_state() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_tasks(&[task("jira", Some("J-1"), "old text", None)], 1).unwrap();
        let id = s.open_tasks().unwrap()[0].id;
        s.set_task_done(id, true, 5).unwrap();
        assert!(s.open_tasks().unwrap().is_empty());

        // Same (source, source_ref): text/due refreshed, done/completed_at preserved.
        s.upsert_tasks(&[task("jira", Some("J-1"), "new text", Some(999))], 10).unwrap();
        let snap = s.snapshot().unwrap();
        let t = snap.tasks.iter().find(|t| t.source_ref.as_deref() == Some("J-1")).unwrap();
        assert!(t.done);
        assert_eq!(t.text, "new text");
        assert_eq!(t.due_ts, Some(999));
        assert_eq!(t.completed_at, Some(5));
        assert_eq!(snap.tasks.len(), 1);
    }

    #[test]
    fn add_and_toggle_task_done() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("buy milk", Some(500), 1).unwrap();
        assert_eq!(t.source, "manual");
        assert_eq!(t.source_ref, None);
        assert!(!t.done);
        assert_eq!(t.due_ts, Some(500));

        s.set_task_done(t.id, true, 20).unwrap();
        let done = s.snapshot().unwrap().tasks.into_iter().find(|x| x.id == t.id).unwrap();
        assert!(done.done);
        assert_eq!(done.completed_at, Some(20));

        s.set_task_done(t.id, false, 30).unwrap();
        let open = s.open_tasks().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].completed_at, None);
    }

    #[test]
    fn open_tasks_ordering_due_then_created() {
        let s = Store::open_in_memory().unwrap();
        s.add_task("no due, created first", None, 100).unwrap();
        s.add_task("due soon", Some(50), 200).unwrap();
        s.add_task("due later", Some(80), 150).unwrap();
        let open = s.open_tasks().unwrap();
        let texts: Vec<&str> = open.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["due soon", "due later", "no due, created first"]);
    }

    #[test]
    fn emails_active_ordering_by_tag_then_recency() {
        let s = Store::open_in_memory().unwrap();
        s.replace_emails(&[
            email("a", "fyi", 100),
            email("b", "needs_reply", 50),
            email("c", "invite", 70),
            email("d", "needs_reply", 90),
        ])
        .unwrap();
        let active = s.emails_active().unwrap();
        let ids: Vec<&str> = active.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["d", "b", "c", "a"]);
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
        s.replace_emails(&[email("e1", "needs_reply", 5)]).unwrap();
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
            "\"fromName\"",
            "\"fromAddr\"",
            "\"receivedTs\"",
            "\"reviewState\"",
            "\"updatedTs\"",
            "\"ranAt\"",
        ] {
            assert!(json.contains(key), "expected {key} in snapshot JSON: {json}");
        }
        // snake_case must not leak through.
        assert!(!json.contains("start_ts"));
        assert!(!json.contains("review_state"));
    }
}
