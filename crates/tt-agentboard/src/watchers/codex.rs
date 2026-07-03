//! Codex agent watcher. Ports slot-1 `runtime/agents/watchers/codex.ts` (370).
//!
//! Watches `$CODEX_HOME/sessions/**/*.jsonl` transcripts (default `~/.codex`),
//! deriving status from transcript events and the working dir from a
//! `turn_context` entry. Thread names come from `$CODEX_HOME/session_index.jsonl`
//! (JSONL, not sqlite in this TS version — the workspace's only SQLite watcher is
//! opencode). Externally-driven scan; incremental byte-offset reads.
//!
//! Deviation: like the TS, the read offset advances to the full file size (no
//! newline-boundary fix), so a line split across two reads is dropped — faithful
//! to codex.ts:246–276. No process liveness — status is transcript-derived.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::types::{AgentEvent, AgentStatus};
use crate::watcher::{AgentWatcher, STALE_MS, WatcherContext};
use crate::watchers::mtime_ms;

const NAME: &str = "codex";
const THREAD_NAME_MAX: usize = 80;

#[derive(Debug, Deserialize)]
pub struct RawEntry {
    #[serde(rename = "type", default)]
    entry_type: Option<String>,
    #[serde(default)]
    payload: Option<RawPayload>,
}

#[derive(Debug, Deserialize)]
struct RawPayload {
    #[serde(rename = "type", default)]
    payload_type: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    content: Option<Vec<RawContentItem>>,
}

#[derive(Debug, Deserialize)]
struct RawContentItem {
    #[serde(rename = "type", default)]
    item_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

fn assistant_status(phase: Option<&str>) -> AgentStatus {
    if phase == Some("commentary") { AgentStatus::Busy } else { AgentStatus::Complete }
}

/// Derive a status from one transcript entry, or `None` (keeps prior). Ports
/// codex `determineStatus` (codex.ts:47–83).
pub fn determine_status(entry: &RawEntry) -> Option<AgentStatus> {
    let payload = entry.payload.as_ref()?;
    match entry.entry_type.as_deref() {
        Some("event_msg") => match payload.payload_type.as_deref() {
            Some("task_complete") => Some(AgentStatus::Complete),
            Some("turn_aborted") => Some(AgentStatus::Interrupted),
            Some("user_message") => Some(AgentStatus::Busy),
            Some("agent_message") => Some(assistant_status(payload.phase.as_deref())),
            Some("error") => Some(AgentStatus::Error),
            _ => None,
        },
        Some("response_item") => match payload.payload_type.as_deref() {
            Some("message") => match payload.role.as_deref() {
                Some("user") => Some(AgentStatus::Busy),
                Some("assistant") => Some(assistant_status(payload.phase.as_deref())),
                _ => None,
            },
            Some("function_call") | Some("function_call_output") | Some("reasoning") => {
                Some(AgentStatus::Busy)
            }
            _ => None,
        },
        _ => None,
    }
}

fn normalize_thread_name(text: Option<&str>) -> Option<String> {
    let text = text?;
    let line = text.split('\n').map(str::trim).find(|s| !s.is_empty())?;
    Some(line.chars().take(THREAD_NAME_MAX).collect())
}

fn extract_thread_name(entry: &RawEntry) -> Option<String> {
    let payload = entry.payload.as_ref()?;
    match (entry.entry_type.as_deref(), payload.payload_type.as_deref()) {
        (Some("event_msg"), Some("user_message")) => {
            normalize_thread_name(payload.message.as_deref())
        }
        (Some("response_item"), Some("message")) if payload.role.as_deref() == Some("user") => {
            let text = payload.content.as_ref().map(|items| {
                items
                    .iter()
                    .filter(|i| i.item_type.as_deref() == Some("input_text"))
                    .map(|i| i.text.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            let candidate = normalize_thread_name(text.as_deref())?;
            if candidate.starts_with("# AGENTS.md")
                || candidate.starts_with("<environment_context>")
            {
                return None;
            }
            Some(candidate)
        }
        _ => None,
    }
}

/// Whether `s` is a canonical 8-4-4-4-12 UUID.
fn is_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    s.char_indices().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => c == '-',
        _ => c.is_ascii_hexdigit(),
    })
}

/// Thread id = the trailing UUID in the filename stem, else the whole stem. Ports
/// `parseThreadId`.
fn parse_thread_id(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    if stem.is_ascii() && stem.len() >= 36 {
        let tail = &stem[stem.len() - 36..];
        if is_uuid(tail) {
            return tail.to_string();
        }
    }
    stem
}

#[derive(Debug, Clone)]
struct SessionSnapshot {
    status: AgentStatus,
    file_size: u64,
    project_dir: Option<String>,
    thread_name: Option<String>,
}

/// Fold JSONL `text` onto `base`, capturing project dir / thread name / status.
/// Ports `applyEntries`.
fn apply_entries(
    text: &str,
    mut base: SessionSnapshot,
    indexed_thread_name: Option<String>,
) -> SessionSnapshot {
    if base.thread_name.is_none() {
        base.thread_name = indexed_thread_name;
    }
    for line in text.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<RawEntry>(line) else {
            continue;
        };
        if base.project_dir.is_none()
            && entry.entry_type.as_deref() == Some("turn_context")
            && let Some(cwd) = entry.payload.as_ref().and_then(|p| p.cwd.clone())
        {
            base.project_dir = Some(cwd);
        }
        if base.thread_name.is_none() {
            base.thread_name = extract_thread_name(&entry);
        }
        if let Some(status) = determine_status(&entry) {
            base.status = status;
        }
    }
    base
}

fn collect_session_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// The Codex watcher. Ports `CodexAgentWatcher`, scan driven externally.
pub struct CodexAgentWatcher {
    sessions_dir: PathBuf,
    session_index_file: PathBuf,
    sessions: HashMap<String, SessionSnapshot>,
    thread_names: HashMap<String, String>,
    seeded: bool,
}

impl CodexAgentWatcher {
    /// Create with an explicit `CODEX_HOME` (contains `sessions/` + `session_index.jsonl`).
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            sessions_dir: codex_home.join("sessions"),
            session_index_file: codex_home.join("session_index.jsonl"),
            sessions: HashMap::new(),
            thread_names: HashMap::new(),
            seeded: false,
        }
    }

    /// Default location: `$CODEX_HOME` or `~/.codex`.
    pub fn with_defaults() -> Self {
        let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".codex")
        });
        Self::new(codex_home)
    }

    fn load_thread_index(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.session_index_file) else {
            return;
        };
        let mut names = HashMap::new();
        #[derive(Deserialize)]
        struct IndexEntry {
            #[serde(default)]
            id: Option<String>,
            #[serde(rename = "thread_name", default)]
            thread_name: Option<String>,
        }
        for line in text.split('\n') {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<IndexEntry>(line)
                && let (Some(id), Some(name)) = (entry.id, entry.thread_name)
            {
                names.insert(id, name);
            }
        }
        self.thread_names = names;
    }

    fn process_file(&mut self, ctx: &mut dyn WatcherContext, file_path: &Path, now_ms: i64) {
        let Ok(meta) = std::fs::metadata(file_path) else {
            return;
        };
        let size = meta.len();
        let thread_id = parse_thread_id(file_path);
        let prev = self.sessions.get(&thread_id).cloned();

        if let Some(prev) = &prev
            && size == prev.file_size
        {
            return;
        }

        let indexed_thread_name = self.thread_names.get(&thread_id).cloned();
        let next = match &prev {
            Some(prev) if size > prev.file_size => {
                let Ok(bytes) = std::fs::read(file_path) else {
                    return;
                };
                let start = (prev.file_size as usize).min(bytes.len());
                let text = String::from_utf8_lossy(&bytes[start..]);
                apply_entries(
                    &text,
                    SessionSnapshot { file_size: size, ..prev.clone() },
                    indexed_thread_name,
                )
            }
            _ => {
                let Ok(text) = std::fs::read_to_string(file_path) else {
                    return;
                };
                apply_entries(
                    &text,
                    SessionSnapshot {
                        status: AgentStatus::Idle,
                        file_size: size,
                        project_dir: None,
                        thread_name: None,
                    },
                    indexed_thread_name,
                )
            }
        };

        let prev_status = prev.as_ref().map(|p| p.status);
        self.sessions.insert(thread_id.clone(), next.clone());

        if !self.seeded {
            return;
        }
        if Some(next.status) == prev_status {
            return;
        }
        let Some(project_dir) = &next.project_dir else {
            return;
        };
        let Some(session) = ctx.resolve_session(project_dir) else {
            return;
        };
        if prev.is_none() && next.status == AgentStatus::Idle {
            return;
        }
        ctx.emit(AgentEvent {
            agent: NAME.to_string(),
            session,
            status: next.status,
            ts: now_ms,
            thread_id: Some(thread_id),
            thread_name: next.thread_name.clone(),
            unseen: None,
            pane_id: None,
            details: None,
        });
    }
}

impl AgentWatcher for CodexAgentWatcher {
    fn name(&self) -> &str {
        NAME
    }

    fn scan(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        self.load_thread_index();
        let mut files = Vec::new();
        collect_session_files(&self.sessions_dir, &mut files);
        for file_path in files {
            let Some(mtime) = mtime_ms(&file_path) else {
                continue;
            };
            if now_ms - mtime > STALE_MS {
                continue;
            }
            self.process_file(ctx, &file_path, now_ms);
        }

        if !self.seeded {
            self.seeded = true;
            let snapshots: Vec<(String, SessionSnapshot)> =
                self.sessions.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (thread_id, snapshot) in snapshots {
                if snapshot.status == AgentStatus::Idle {
                    continue;
                }
                let Some(project_dir) = &snapshot.project_dir else {
                    continue;
                };
                let Some(session) = ctx.resolve_session(project_dir) else {
                    continue;
                };
                ctx.emit(AgentEvent {
                    agent: NAME.to_string(),
                    session,
                    status: snapshot.status,
                    ts: now_ms,
                    thread_id: Some(thread_id),
                    thread_name: snapshot.thread_name.clone(),
                    unseen: None,
                    pane_id: None,
                    details: None,
                });
            }
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

    fn entry(json: serde_json::Value) -> RawEntry {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn status_table() {
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"event_msg","payload":{"type":"task_complete"}})
            )),
            Some(AgentStatus::Complete)
        );
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted"}})
            )),
            Some(AgentStatus::Interrupted)
        );
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"event_msg","payload":{"type":"error"}})
            )),
            Some(AgentStatus::Error)
        );
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","phase":"commentary"}})
            )),
            Some(AgentStatus::Busy)
        );
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message"}})
            )),
            Some(AgentStatus::Complete)
        );
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"response_item","payload":{"type":"function_call"}})
            )),
            Some(AgentStatus::Busy)
        );
        assert_eq!(
            determine_status(&entry(
                serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user"}})
            )),
            Some(AgentStatus::Busy)
        );
        assert_eq!(
            determine_status(&entry(serde_json::json!({"type":"other","payload":{}}))),
            None
        );
    }

    #[test]
    fn parse_thread_id_extracts_trailing_uuid() {
        let uuid = "12345678-1234-1234-1234-123456789abc";
        let p = PathBuf::from(format!("/x/rollout-2026-{uuid}.jsonl"));
        assert_eq!(parse_thread_id(&p), uuid);
        let p2 = PathBuf::from("/x/plainname.jsonl");
        assert_eq!(parse_thread_id(&p2), "plainname");
    }

    #[test]
    fn thread_name_skips_agents_and_env_prefixes() {
        let e = entry(
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"  \n Fix the bug  "}}),
        );
        assert_eq!(extract_thread_name(&e).as_deref(), Some("Fix the bug"));
        let e2 = entry(
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md guide"}]}}),
        );
        assert_eq!(extract_thread_name(&e2), None);
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

    fn write_session(dir: &Path, id: &str, lines: &[serde_json::Value]) {
        let sessions = dir.join("sessions").join("2026");
        std::fs::create_dir_all(&sessions).unwrap();
        let mut body = String::new();
        for l in lines {
            body.push_str(&l.to_string());
            body.push('\n');
        }
        std::fs::write(sessions.join(format!("rollout-{id}.jsonl")), body).unwrap();
    }

    #[test]
    fn seed_emits_then_incremental_status_change() {
        let dir = TempDir::new().unwrap();
        let uuid = "12345678-1234-1234-1234-123456789abc";
        write_session(
            dir.path(),
            uuid,
            &[
                serde_json::json!({"type":"turn_context","payload":{"cwd":"/home/u/proj"}}),
                serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"do it"}}),
            ],
        );
        let mut w = CodexAgentWatcher::new(dir.path().to_path_buf());
        let mut c = ctx();
        let now = now_real_ms();
        w.scan(&mut c, now); // seed → running emitted
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].status, AgentStatus::Busy);
        assert_eq!(c.events[0].thread_id.as_deref(), Some(uuid));
        assert_eq!(c.events[0].thread_name.as_deref(), Some("do it"));
        c.events.clear();

        // Append a completion → status change to done.
        write_session(
            dir.path(),
            uuid,
            &[
                serde_json::json!({"type":"turn_context","payload":{"cwd":"/home/u/proj"}}),
                serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"do it"}}),
                serde_json::json!({"type":"event_msg","payload":{"type":"task_complete"}}),
            ],
        );
        w.scan(&mut c, now + 1);
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].status, AgentStatus::Complete);
    }

    #[test]
    fn uses_session_index_for_thread_name() {
        let dir = TempDir::new().unwrap();
        let uuid = "12345678-1234-1234-1234-123456789abc";
        std::fs::write(
            dir.path().join("session_index.jsonl"),
            serde_json::json!({"id": uuid, "thread_name": "Indexed Name"}).to_string(),
        )
        .unwrap();
        write_session(
            dir.path(),
            uuid,
            &[
                serde_json::json!({"type":"turn_context","payload":{"cwd":"/home/u/proj"}}),
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","phase":"commentary"}}),
            ],
        );
        let mut w = CodexAgentWatcher::new(dir.path().to_path_buf());
        let mut c = ctx();
        w.scan(&mut c, now_real_ms());
        assert_eq!(c.events[0].thread_name.as_deref(), Some("Indexed Name"));
    }

    #[test]
    fn missing_sessions_dir_is_noop() {
        let dir = TempDir::new().unwrap();
        let mut w = CodexAgentWatcher::new(dir.path().join("nope"));
        let mut c = ctx();
        w.scan(&mut c, now_real_ms());
        assert!(c.events.is_empty());
    }
}
