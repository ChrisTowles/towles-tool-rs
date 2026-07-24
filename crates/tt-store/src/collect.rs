//! Collector bookkeeping and cross-domain reads: Slack DMs, collector-run
//! freshness, the MCP call log, and the aggregate dashboard snapshot.

use rusqlite::params;

use crate::model::*;
use crate::{Result, Store};

impl Store {
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
