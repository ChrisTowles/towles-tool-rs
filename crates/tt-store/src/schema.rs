//! Schema definitions and migrations: the `CREATE TABLE` batches, the on-disk
//! schema version, and the in-place migrations that carry an older database
//! forward. `Store::open`/`open_default`/`open_in_memory` live here too, since
//! opening a store is exactly "connect, then migrate".

use std::path::Path;

use rusqlite::{Connection, params};

use crate::{Error, Result, Store};

/// Current on-disk schema version, stored in the `meta` table.
pub(crate) const SCHEMA_VERSION: i64 = 16;

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
        // Additive `ALTER TABLE ADD COLUMN` migrations run last, after every
        // rebuild-style migration above (v2/v7/v11/v13/v14 all `CREATE TABLE
        // tasks_vN` + `INSERT INTO ... SELECT` a fixed column list) — a rebuild
        // that predates a column silently drops it, the same trap v4's `notes`
        // dodges by living here rather than before v7.
        self.migrate_tasks_goal_v16()?;
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

    /// v16: `goal` — the objective a task was created for, distinct from its
    /// `text` title. Same nullable-ADD-COLUMN idiom as [`Self::migrate_tasks_notes_v4`].
    fn migrate_tasks_goal_v16(&self) -> Result<()> {
        let mut has_goal = false;
        {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tasks)")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == "goal" {
                    has_goal = true;
                }
            }
        }
        if !has_goal {
            self.conn.execute_batch("ALTER TABLE tasks ADD COLUMN goal TEXT;")?;
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
}
