//! `claude agents --all --json` as the authoritative source for live Claude
//! Code sessions (phase T7 of docs/AGENTBOARD-PORT.md; Chris's call,
//! 2026-07-03: prefer the supported CLI surface over parsing raw
//! `~/.claude/sessions/<pid>.json` files and inferring status from journals).
//!
//! One entry per live process: pid, cwd, kind (interactive/background),
//! startedAt, sessionId, name, status (`busy` / `waiting` (+waitingFor) /
//! `idle`). The CLI does NOT expose model/tool/usage/subagents — journal
//! reads remain for those (enrichment only, in the claude-code watcher).
//!
//! The parse is pure and fixture-tested; the fetch is a thin subprocess
//! wrapper with a process-wide cache so the watcher (2s), engine pinning
//! (every rebuild), and pane scan (3s) share one ~170ms CLI call.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::types::AgentStatus;

/// One live Claude Code process as reported by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliAgent {
    pub pid: i32,
    pub cwd: String,
    /// `interactive` or `background`.
    pub kind: Option<String>,
    pub started_at: Option<i64>,
    pub session_id: String,
    pub name: Option<String>,
    /// Raw CLI status: `busy` / `waiting` / `idle`.
    pub status: Option<String>,
    /// What a `waiting` session waits for (e.g. `permission prompt`).
    pub waiting_for: Option<String>,
}

impl CliAgent {
    /// The CLI status in the agentboard vocabulary — a 1:1 mapping now that
    /// the vocabulary follows the CLI's own naming. `idle` callers may
    /// refine with journal knowledge (a completed turn shows `complete`).
    pub fn agent_status(&self) -> Option<AgentStatus> {
        match self.status.as_deref() {
            Some("busy") => Some(AgentStatus::Busy),
            Some("waiting") => Some(AgentStatus::Waiting),
            Some("idle") => Some(AgentStatus::Idle),
            _ => None,
        }
    }
}

/// Parse the CLI's JSON array. Tolerant: entries missing pid/sessionId are
/// skipped; unknown fields ignored.
pub fn parse_agents(json: &str) -> Vec<CliAgent> {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let pid = entry.get("pid")?.as_i64()? as i32;
            let session_id = entry.get("sessionId")?.as_str()?.to_string();
            Some(CliAgent {
                pid,
                cwd: entry.get("cwd").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                kind: entry.get("kind").and_then(|v| v.as_str()).map(str::to_string),
                started_at: entry.get("startedAt").and_then(|v| v.as_i64()),
                session_id,
                name: entry.get("name").and_then(|v| v.as_str()).map(str::to_string),
                status: entry.get("status").and_then(|v| v.as_str()).map(str::to_string),
                waiting_for: entry.get("waitingFor").and_then(|v| v.as_str()).map(str::to_string),
            })
        })
        .collect()
}

/// Run the CLI. Failures (claude missing, non-zero exit, bad JSON) yield an
/// empty list — the watcher treats that as "no live sessions visible".
pub fn fetch_agents() -> Vec<CliAgent> {
    match tt_exec::run("claude", &["agents", "--all", "--json"]) {
        Ok(out) if out.ok() => parse_agents(&out.stdout),
        _ => Vec::new(),
    }
}

static CACHE: Mutex<Option<(Instant, Vec<CliAgent>)>> = Mutex::new(None);

/// [`fetch_agents`] behind a process-wide cache: at most one CLI call per
/// `ttl`. All agentboard consumers share the same snapshot.
pub fn fetch_agents_cached(ttl: Duration) -> Vec<CliAgent> {
    {
        let cache = CACHE.lock().unwrap();
        if let Some((at, agents)) = cache.as_ref()
            && at.elapsed() < ttl
        {
            return agents.clone();
        }
    }
    let agents = fetch_agents();
    *CACHE.lock().unwrap() = Some((Instant::now(), agents.clone()));
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {"pid": 100, "cwd": "/home/u/proj", "kind": "interactive", "startedAt": 1783087499085,
         "sessionId": "aaa-111", "name": "proj-44", "status": "busy"},
        {"pid": 200, "id": "9dbc92c8", "cwd": "/home/u/other", "kind": "background",
         "sessionId": "bbb-222", "name": "Fix the flaky test", "status": "waiting",
         "waitingFor": "permission prompt", "state": "blocked"},
        {"pid": 300, "cwd": "/home/u/x", "kind": "interactive", "sessionId": "ccc-333",
         "status": "idle"},
        {"pid": 400, "status": "busy"}
    ]"#;

    #[test]
    fn parses_entries_and_skips_incomplete() {
        let agents = parse_agents(FIXTURE);
        assert_eq!(agents.len(), 3); // pid 400 has no sessionId
        assert_eq!(agents[0].pid, 100);
        assert_eq!(agents[0].session_id, "aaa-111");
        assert_eq!(agents[0].kind.as_deref(), Some("interactive"));
        assert_eq!(agents[0].started_at, Some(1_783_087_499_085));
        assert_eq!(agents[1].name.as_deref(), Some("Fix the flaky test"));
        assert_eq!(agents[1].waiting_for.as_deref(), Some("permission prompt"));
    }

    #[test]
    fn status_mapping() {
        let agents = parse_agents(FIXTURE);
        assert_eq!(agents[0].agent_status(), Some(AgentStatus::Busy));
        assert_eq!(agents[1].agent_status(), Some(AgentStatus::Waiting));
        assert_eq!(agents[2].agent_status(), Some(AgentStatus::Idle));
    }

    #[test]
    fn malformed_json_yields_empty() {
        assert!(parse_agents("not json").is_empty());
        assert!(parse_agents("{}").is_empty());
    }
}
