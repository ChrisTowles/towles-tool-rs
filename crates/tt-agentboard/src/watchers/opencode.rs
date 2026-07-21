//! OpenCode agent watcher. Ports §task§-1 `runtime/agents/watchers/opencode.ts` (225).
//!
//! Polls OpenCode's SQLite DB (`~/.local/share/opencode/opencode.db`, or
//! `$OPENCODE_DB_PATH`) — read-only, tolerant of a missing/locked DB (it's
//! another tool's live database: never write, never exclusive-lock). Status comes
//! from each session's latest message + parts; the `time_updated` column is the
//! activity signal (only emit when it changes = OpenCode is actively writing).
//! Externally-driven scan; DB path parameterized so tests use a fixture DB.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::types::{AgentEvent, AgentStatus};
use crate::watcher::{AgentWatcher, STALE_MS, WatcherContext};

const NAME: &str = "opencode";

#[derive(Debug, Deserialize, Default)]
pub struct MessageData {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    finish: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PartData {
    #[serde(rename = "type", default)]
    part_type: Option<String>,
}

/// Derive status from the latest message + its parts. Ports opencode
/// `determineStatus` (opencode.ts:43–58).
pub fn determine_status(msg: Option<&MessageData>, parts: &[PartData]) -> AgentStatus {
    let Some(msg) = msg else {
        return AgentStatus::Idle;
    };
    match msg.role.as_deref() {
        Some("assistant") => {
            if msg.finish.as_deref() == Some("tool-calls") {
                return AgentStatus::Busy;
            }
            if parts.iter().any(|p| p.part_type.as_deref() == Some("tool")) {
                return AgentStatus::Busy;
            }
            AgentStatus::Complete
        }
        Some("user") => AgentStatus::Busy,
        _ => AgentStatus::Idle,
    }
}

struct SessionRow {
    id: String,
    title: Option<String>,
    directory: String,
    time_updated: i64,
}

/// The OpenCode watcher. Ports `OpenCodeAgentWatcher`, poll driven externally.
pub struct OpenCodeAgentWatcher {
    db_path: PathBuf,
    session_timestamps: HashMap<String, i64>,
    session_statuses: HashMap<String, AgentStatus>,
    seeded: bool,
}

impl OpenCodeAgentWatcher {
    /// Create pointed at a specific `opencode.db` path.
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            session_timestamps: HashMap::new(),
            session_statuses: HashMap::new(),
            seeded: false,
        }
    }

    /// Default location: `$OPENCODE_DB_PATH` or `~/.local/share/opencode/opencode.db`.
    pub fn with_defaults() -> Self {
        let db_path =
            std::env::var_os("OPENCODE_DB_PATH").map(PathBuf::from).unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local")
                    .join("share")
                    .join("opencode")
                    .join("opencode.db")
            });
        Self::new(db_path)
    }

    /// Open the DB read-only. `None` if missing or unopenable (tolerated).
    fn open(&self) -> Option<Connection> {
        if !self.db_path.exists() {
            return None;
        }
        Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
    }

    fn recent_sessions(conn: &Connection, stale_threshold: i64) -> Option<Vec<SessionRow>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, directory, time_updated FROM session \
                 WHERE time_updated > ?1 ORDER BY time_updated DESC",
            )
            .ok()?;
        let rows = stmt
            .query_map([stale_threshold], |r| {
                Ok(SessionRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    directory: r.get(2)?,
                    time_updated: r.get(3)?,
                })
            })
            .ok()?;
        Some(rows.filter_map(Result::ok).collect())
    }

    /// Read a session's latest message + parts → status. `None` if the query fails.
    /// Ports `readSessionStatus`.
    fn read_session_status(conn: &Connection, session_id: &str) -> Option<AgentStatus> {
        let msg: Option<(String, String)> = conn
            .query_row(
                "SELECT id, data FROM message WHERE session_id = ?1 \
                 ORDER BY time_created DESC LIMIT 1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();

        let Some((msg_id, msg_data)) = msg else {
            return Some(determine_status(None, &[]));
        };

        let mut parts: Vec<PartData> = Vec::new();
        if let Ok(mut stmt) =
            conn.prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY time_created ASC")
            && let Ok(rows) = stmt.query_map([&msg_id], |r| r.get::<_, String>(0))
        {
            for data in rows.filter_map(Result::ok) {
                if let Ok(part) = serde_json::from_str::<PartData>(&data) {
                    parts.push(part);
                }
            }
        }

        let msg_data = serde_json::from_str::<MessageData>(&msg_data).ok();
        Some(determine_status(msg_data.as_ref(), &parts))
    }

    fn emit_session(
        ctx: &mut dyn WatcherContext,
        row: &SessionRow,
        status: AgentStatus,
        now_ms: i64,
    ) {
        let Some(session) = ctx.resolve_session(&row.directory) else {
            return;
        };
        ctx.emit(AgentEvent {
            agent: NAME.to_string(),
            session,
            status,
            ts: now_ms,
            thread_id: Some(row.id.clone()),
            thread_name: row.title.clone(),
            unseen: None,
            pane_id: None,
            details: None,
        });
    }
}

impl AgentWatcher for OpenCodeAgentWatcher {
    fn name(&self) -> &str {
        NAME
    }

    fn scan(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        let Some(conn) = self.open() else { return };
        let stale_threshold = now_ms - STALE_MS;
        let Some(sessions) = Self::recent_sessions(&conn, stale_threshold) else {
            return;
        };

        // First poll: record timestamps, then emit current non-idle state.
        if !self.seeded {
            for row in &sessions {
                self.session_timestamps.insert(row.id.clone(), row.time_updated);
            }
            self.seeded = true;
            for row in &sessions {
                let Some(status) = Self::read_session_status(&conn, &row.id) else {
                    continue;
                };
                if status == AgentStatus::Idle {
                    continue;
                }
                self.session_statuses.insert(row.id.clone(), status);
                Self::emit_session(ctx, row, status, now_ms);
            }
            return;
        }

        for row in &sessions {
            let prev_ts = self.session_timestamps.get(&row.id).copied();
            if prev_ts == Some(row.time_updated) {
                continue;
            }
            self.session_timestamps.insert(row.id.clone(), row.time_updated);
            // Only emit when we had a prior timestamp — a change means OpenCode is
            // actively writing this session right now.
            if prev_ts.is_none() {
                continue;
            }
            let Some(status) = Self::read_session_status(&conn, &row.id) else {
                continue;
            };
            if self.session_statuses.get(&row.id) == Some(&status) {
                continue;
            }
            self.session_statuses.insert(row.id.clone(), status);
            Self::emit_session(ctx, row, status, now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as Map;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn now_real_ms() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    #[test]
    fn status_table() {
        assert_eq!(determine_status(None, &[]), AgentStatus::Idle);
        let user = MessageData { role: Some("user".into()), finish: None };
        assert_eq!(determine_status(Some(&user), &[]), AgentStatus::Busy);
        let asst_done = MessageData { role: Some("assistant".into()), finish: Some("stop".into()) };
        assert_eq!(determine_status(Some(&asst_done), &[]), AgentStatus::Complete);
        let asst_tool =
            MessageData { role: Some("assistant".into()), finish: Some("tool-calls".into()) };
        assert_eq!(determine_status(Some(&asst_tool), &[]), AgentStatus::Busy);
        let parts = vec![PartData { part_type: Some("tool".into()) }];
        assert_eq!(determine_status(Some(&asst_done), &parts), AgentStatus::Busy);
    }

    /// Create a fixture opencode.db with the schema the watcher reads.
    fn make_db(path: &std::path::Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_updated INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, data TEXT, time_created INTEGER);
             CREATE TABLE part (data TEXT, message_id TEXT, time_created INTEGER);",
        )
        .unwrap();
        conn
    }

    struct Ctx {
        events: Vec<AgentEvent>,
        resolve: Map<String, String>,
    }
    impl WatcherContext for Ctx {
        fn resolve_session(&self, project_dir: &str) -> Option<String> {
            self.resolve.get(project_dir).cloned()
        }
        fn emit(&mut self, event: AgentEvent) {
            self.events.push(event);
        }
    }
    fn ctx() -> Ctx {
        let mut resolve = Map::new();
        resolve.insert("/home/u/proj".to_string(), "proj".to_string());
        Ctx { events: Vec::new(), resolve }
    }

    #[test]
    fn missing_db_is_noop() {
        let dir = TempDir::new().unwrap();
        let mut w = OpenCodeAgentWatcher::new(dir.path().join("nope.db"));
        let mut c = ctx();
        w.scan(&mut c, now_real_ms());
        assert!(c.events.is_empty());
    }

    #[test]
    fn seed_emits_non_idle_then_change_on_new_timestamp() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("opencode.db");
        let now = now_real_ms();
        {
            let conn = make_db(&db_path);
            conn.execute(
                "INSERT INTO session VALUES ('s1', 'My Session', '/home/u/proj', ?1)",
                [now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message VALUES ('m1', 's1', ?1, 1)",
                [serde_json::json!({"role":"user"}).to_string()],
            )
            .unwrap();
        }
        let mut w = OpenCodeAgentWatcher::new(db_path.clone());
        let mut c = ctx();
        w.scan(&mut c, now); // seed → running
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].status, AgentStatus::Busy);
        assert_eq!(c.events[0].thread_id.as_deref(), Some("s1"));
        assert_eq!(c.events[0].thread_name.as_deref(), Some("My Session"));
        c.events.clear();

        // Assistant completes; bump time_updated → status change to done.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO message VALUES ('m2', 's1', ?1, 2)",
                [serde_json::json!({"role":"assistant","finish":"stop"}).to_string()],
            )
            .unwrap();
            conn.execute("UPDATE session SET time_updated = ?1 WHERE id = 's1'", [now + 5])
                .unwrap();
        }
        w.scan(&mut c, now + 10);
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].status, AgentStatus::Complete);
    }

    #[test]
    fn stale_sessions_excluded() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("opencode.db");
        let now = now_real_ms();
        {
            let conn = make_db(&db_path);
            // Updated long ago → older than STALE_MS.
            conn.execute(
                "INSERT INTO session VALUES ('old', 't', '/home/u/proj', ?1)",
                [now - 10 * 60 * 1000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message VALUES ('m1', 'old', ?1, 1)",
                [serde_json::json!({"role":"user"}).to_string()],
            )
            .unwrap();
        }
        let mut w = OpenCodeAgentWatcher::new(db_path);
        let mut c = ctx();
        w.scan(&mut c, now);
        assert!(c.events.is_empty());
    }
}
