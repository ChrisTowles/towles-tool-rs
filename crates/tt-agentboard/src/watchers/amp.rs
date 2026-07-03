//! Amp agent watcher. Ports slot-1 `runtime/agents/watchers/amp.ts` (320).
//!
//! Watches `~/.local/share/amp/threads/T-*.json` (whole-file JSON per thread) and
//! derives status from the last message's `state`. Externally-driven scan; roots
//! path-parameterized. No process liveness — status is message-derived; the
//! `session.json` `lastThreadId` focus signal emits `idle` for a terminal thread
//! the user has re-opened in Amp (amp.ts:290–336).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::types::{AgentEvent, AgentStatus};
use crate::watcher::{AgentWatcher, STALE_MS, WatcherContext};
use crate::watchers::mtime_ms;

const NAME: &str = "amp";

// --- Tolerant thread-file types ---

#[derive(Debug, Deserialize)]
struct RawThread {
    #[serde(default)]
    v: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    env: Option<RawEnv>,
    #[serde(default)]
    messages: Vec<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawEnv {
    #[serde(default)]
    initial: Option<RawInitial>,
}

#[derive(Debug, Deserialize)]
struct RawInitial {
    #[serde(default)]
    trees: Option<Vec<RawTree>>,
}

#[derive(Debug, Deserialize)]
struct RawTree {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    state: Option<RawState>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawState {
    #[serde(rename = "type", default)]
    state_type: Option<String>,
    #[serde(rename = "stopReason", default)]
    stop_reason: Option<String>,
}

/// Derive status from the last message. Ports amp `determineStatus` (amp.ts:50–83).
pub fn determine_status(
    role: Option<&str>,
    state_type: Option<&str>,
    stop_reason: Option<&str>,
) -> AgentStatus {
    match role {
        None => AgentStatus::Idle,
        Some("user") => AgentStatus::Busy,
        Some("assistant") => match state_type {
            None => AgentStatus::Busy,
            Some("streaming") => AgentStatus::Busy,
            Some("cancelled") | Some("aborted") | Some("interrupted") => AgentStatus::Interrupted,
            Some("error") | Some("errored") | Some("failed") => AgentStatus::Error,
            Some("complete") => match stop_reason {
                Some("tool_use") => AgentStatus::Busy,
                Some("end_turn") => AgentStatus::Complete,
                // Amp uses other stop reasons (e.g. max_tokens) for terminal failures.
                _ => AgentStatus::Error,
            },
            _ => AgentStatus::Waiting,
        },
        Some(_) => AgentStatus::Idle,
    }
}

struct ParsedThread {
    version: i64,
    title: Option<String>,
    project_dir: Option<String>,
    status: AgentStatus,
}

fn parse_thread_file(path: &std::path::Path) -> Option<ParsedThread> {
    let raw = std::fs::read_to_string(path).ok()?;
    let thread: RawThread = serde_json::from_str(&raw).ok()?;
    let title = thread.title.filter(|t| !t.is_empty());
    let uri = thread
        .env
        .and_then(|e| e.initial)
        .and_then(|i| i.trees)
        .and_then(|t| t.into_iter().next())
        .and_then(|tree| tree.uri)
        .unwrap_or_default();
    let project_dir = uri.strip_prefix("file://").map(str::to_string);
    let last = thread.messages.last();
    let status = determine_status(
        last.and_then(|m| m.role.as_deref()),
        last.and_then(|m| m.state.as_ref()).and_then(|s| s.state_type.as_deref()),
        last.and_then(|m| m.state.as_ref()).and_then(|s| s.stop_reason.as_deref()),
    );
    Some(ParsedThread { version: thread.v.unwrap_or(0), title, project_dir, status })
}

#[derive(Debug, Clone)]
struct ThreadSnapshot {
    status: AgentStatus,
    version: i64,
    title: Option<String>,
    project_dir: Option<String>,
    mtime_ms: i64,
}

/// The Amp watcher. Ports `AmpAgentWatcher`, scan driven externally.
pub struct AmpAgentWatcher {
    threads_dir: PathBuf,
    session_file: PathBuf,
    threads: HashMap<String, ThreadSnapshot>,
    seeded: bool,
    last_focused_thread: Option<String>,
}

impl AmpAgentWatcher {
    /// Create with an explicit Amp data dir (contains `threads/` + `session.json`).
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            threads_dir: data_dir.join("threads"),
            session_file: data_dir.join("session.json"),
            threads: HashMap::new(),
            seeded: false,
            last_focused_thread: None,
        }
    }

    /// Default location: `~/.local/share/amp`.
    pub fn with_defaults() -> Self {
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("share")
            .join("amp");
        Self::new(data_dir)
    }

    fn emit_thread(
        ctx: &mut dyn WatcherContext,
        thread_id: &str,
        snapshot: &ThreadSnapshot,
        now_ms: i64,
    ) {
        let Some(project_dir) = &snapshot.project_dir else {
            return;
        };
        if snapshot.status == AgentStatus::Idle {
            return;
        }
        let Some(session) = ctx.resolve_session(project_dir) else {
            return;
        };
        ctx.emit(AgentEvent {
            agent: NAME.to_string(),
            session,
            status: snapshot.status,
            ts: now_ms,
            thread_id: Some(thread_id.to_string()),
            thread_name: snapshot.title.clone(),
            unseen: None,
            pane_id: None,
            details: None,
        });
    }

    fn process_thread(
        &mut self,
        ctx: &mut dyn WatcherContext,
        file_path: &std::path::Path,
        now_ms: i64,
    ) {
        let Some(mtime) = mtime_ms(file_path) else {
            return;
        };
        let thread_id = file_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let prev = self.threads.get(&thread_id).cloned();

        // Quick mtime check — skip if unchanged since last seen.
        if let Some(prev) = &prev
            && mtime <= prev.mtime_ms
        {
            return;
        }

        let Some(parsed) = parse_thread_file(file_path) else {
            return;
        };
        let status_changed = prev.as_ref().map(|p| p.status) != Some(parsed.status);
        let title_changed = prev.as_ref().and_then(|p| p.title.clone()) != parsed.title;
        let project_changed =
            prev.as_ref().and_then(|p| p.project_dir.clone()) != parsed.project_dir;

        if let Some(prev) = &prev
            && parsed.version == prev.version
            && !status_changed
            && !title_changed
            && !project_changed
        {
            // Bump mtime to avoid re-reading; no state change.
            self.threads.get_mut(&thread_id).unwrap().mtime_ms = mtime;
            return;
        }

        let snapshot = ThreadSnapshot {
            status: parsed.status,
            version: parsed.version,
            title: parsed.title,
            project_dir: parsed.project_dir,
            mtime_ms: mtime,
        };
        self.threads.insert(thread_id.clone(), snapshot.clone());

        if !self.seeded {
            return;
        }
        if status_changed || title_changed {
            Self::emit_thread(ctx, &thread_id, &snapshot, now_ms);
        }
    }

    /// Read `session.json`'s `lastThreadId`; if it changed to a tracked terminal
    /// thread, emit `idle` (the user "saw" it). Ports `checkSessionFocus`.
    fn check_session_focus(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        if !self.seeded {
            return;
        }
        let Ok(raw) = std::fs::read_to_string(&self.session_file) else {
            return;
        };
        #[derive(Deserialize)]
        struct RawSession {
            #[serde(rename = "lastThreadId", default)]
            last_thread_id: Option<String>,
        }
        let Ok(session) = serde_json::from_str::<RawSession>(&raw) else {
            return;
        };
        let Some(thread_id) = session.last_thread_id else {
            return;
        };
        if Some(&thread_id) == self.last_focused_thread.as_ref() {
            return;
        }
        self.last_focused_thread = Some(thread_id.clone());

        let Some(snapshot) = self.threads.get(&thread_id) else {
            return;
        };
        let Some(project_dir) = &snapshot.project_dir else {
            return;
        };
        if !snapshot.status.is_terminal() {
            return;
        }
        let Some(session) = ctx.resolve_session(project_dir) else {
            return;
        };
        let title = snapshot.title.clone();
        ctx.emit(AgentEvent {
            agent: NAME.to_string(),
            session,
            status: AgentStatus::Idle,
            ts: now_ms,
            thread_id: Some(thread_id),
            thread_name: title,
            unseen: None,
            pane_id: None,
            details: None,
        });
    }
}

impl AgentWatcher for AmpAgentWatcher {
    fn name(&self) -> &str {
        NAME
    }

    fn scan(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        let initial_seed = !self.seeded;
        if let Ok(entries) = std::fs::read_dir(&self.threads_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with("T-") || !name.ends_with(".json") {
                    continue;
                }
                let file_path = entry.path();
                let Some(mtime) = mtime_ms(&file_path) else {
                    continue;
                };
                if now_ms - mtime > STALE_MS {
                    continue;
                }
                self.process_thread(ctx, &file_path, now_ms);
            }
        }
        if initial_seed {
            self.seeded = true;
            let snapshots: Vec<(String, ThreadSnapshot)> =
                self.threads.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (thread_id, snapshot) in snapshots {
                Self::emit_thread(ctx, &thread_id, &snapshot, now_ms);
            }
        }
        self.check_session_focus(ctx, now_ms);
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
        assert_eq!(determine_status(None, None, None), AgentStatus::Idle);
        assert_eq!(determine_status(Some("user"), None, None), AgentStatus::Busy);
        assert_eq!(determine_status(Some("assistant"), None, None), AgentStatus::Busy);
        assert_eq!(determine_status(Some("assistant"), Some("streaming"), None), AgentStatus::Busy);
        assert_eq!(
            determine_status(Some("assistant"), Some("aborted"), None),
            AgentStatus::Interrupted
        );
        assert_eq!(determine_status(Some("assistant"), Some("failed"), None), AgentStatus::Error);
        assert_eq!(
            determine_status(Some("assistant"), Some("complete"), Some("end_turn")),
            AgentStatus::Complete
        );
        assert_eq!(
            determine_status(Some("assistant"), Some("complete"), Some("tool_use")),
            AgentStatus::Busy
        );
        assert_eq!(
            determine_status(Some("assistant"), Some("complete"), Some("max_tokens")),
            AgentStatus::Error
        );
        assert_eq!(determine_status(Some("assistant"), Some("other"), None), AgentStatus::Waiting);
        assert_eq!(determine_status(Some("system"), None, None), AgentStatus::Idle);
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

    fn write_thread(dir: &std::path::Path, id: &str, json: serde_json::Value) {
        let threads = dir.join("threads");
        std::fs::create_dir_all(&threads).unwrap();
        std::fs::write(threads.join(format!("{id}.json")), json.to_string()).unwrap();
    }

    fn thread_json(
        role: &str,
        state_type: Option<&str>,
        stop: Option<&str>,
        title: &str,
    ) -> serde_json::Value {
        let mut msg = serde_json::json!({ "role": role });
        if let Some(t) = state_type {
            msg["state"] = serde_json::json!({ "type": t, "stopReason": stop });
        }
        serde_json::json!({
            "v": 1,
            "title": title,
            "env": { "initial": { "trees": [{ "uri": "file:///home/u/proj" }] } },
            "messages": [msg]
        })
    }

    #[test]
    fn seed_emits_non_idle_then_status_change() {
        let dir = TempDir::new().unwrap();
        write_thread(dir.path(), "T-1", thread_json("assistant", Some("streaming"), None, "work"));
        let mut w = AmpAgentWatcher::new(dir.path().to_path_buf());
        let mut c = ctx();
        let now = now_real_ms();
        w.scan(&mut c, now); // seed → running emitted
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].status, AgentStatus::Busy);
        assert_eq!(c.events[0].thread_name.as_deref(), Some("work"));
        c.events.clear();

        // Complete the turn (status change → emit done). Bump mtime by rewriting.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_thread(
            dir.path(),
            "T-1",
            thread_json("assistant", Some("complete"), Some("end_turn"), "work"),
        );
        w.scan(&mut c, now + 1);
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].status, AgentStatus::Complete);
    }

    #[test]
    fn session_focus_emits_idle_for_terminal_thread() {
        let dir = TempDir::new().unwrap();
        write_thread(
            dir.path(),
            "T-1",
            thread_json("assistant", Some("complete"), Some("end_turn"), "done work"),
        );
        std::fs::write(
            dir.path().join("session.json"),
            serde_json::json!({ "lastThreadId": "T-1" }).to_string(),
        )
        .unwrap();
        let mut w = AmpAgentWatcher::new(dir.path().to_path_buf());
        let mut c = ctx();
        let now = now_real_ms();
        w.scan(&mut c, now);
        // seed emits done; focus check emits idle for the just-viewed terminal thread.
        assert!(c.events.iter().any(|e| e.status == AgentStatus::Complete));
        assert!(c.events.iter().any(|e| e.status == AgentStatus::Idle));
    }

    #[test]
    fn missing_threads_dir_is_noop() {
        let dir = TempDir::new().unwrap();
        let mut w = AmpAgentWatcher::new(dir.path().join("nope"));
        let mut c = ctx();
        w.scan(&mut c, now_real_ms());
        assert!(c.events.is_empty());
    }
}
