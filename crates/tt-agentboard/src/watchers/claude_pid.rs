//! PID liveness lookup for the claude-code watcher. Ports slot-1
//! `runtime/agents/watchers/claude-pid.ts`.
//!
//! Reads `~/.claude/sessions/<pid>.json` (`{ pid, sessionId }`) to map a
//! session/thread id to its OS pid, and checks liveness. The sessions dir is
//! path-parameterized and `is_alive` is injectable so tests never probe real
//! processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default liveness check: process exists (Linux `/proc/<pid>`).
///
/// Deviation from the TS `process.kill(pid, 0)`: uses `/proc` (the deployment
/// target is Linux). Tests and other platforms inject their own probe.
fn default_is_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Maps thread/session ids to pids (rebuilt each scan via [`Self::invalidate`]).
/// Ports `createClaudePidLookup`.
pub struct ClaudePidLookup {
    sessions_dir: PathBuf,
    cache: Option<HashMap<String, i32>>,
    is_alive_fn: fn(i32) -> bool,
}

#[derive(serde::Deserialize)]
struct RawSession {
    #[serde(default)]
    pid: Option<i64>,
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
}

impl ClaudePidLookup {
    /// Lookup rooted at `sessions_dir` using the real (Linux `/proc`) liveness probe.
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir, cache: None, is_alive_fn: default_is_alive }
    }

    /// Lookup with an injected liveness probe (for tests / non-Linux).
    pub fn with_is_alive(sessions_dir: PathBuf, is_alive_fn: fn(i32) -> bool) -> Self {
        Self { sessions_dir, cache: None, is_alive_fn }
    }

    /// Default location: `~/.claude/sessions`.
    pub fn default_dir() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("sessions")
    }

    fn load_cache(&self) -> HashMap<String, i32> {
        let mut map = HashMap::new();
        let Ok(entries) = std::fs::read_dir(&self.sessions_dir) else {
            return map;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(data) = serde_json::from_str::<RawSession>(&text) else {
                continue;
            };
            if let (Some(pid), Some(session_id)) = (data.pid, data.session_id) {
                map.insert(session_id, pid as i32);
            }
        }
        map
    }

    /// The pid for a thread/session id, building the cache lazily. Ports `pidForThread`.
    pub fn pid_for_thread(&mut self, thread_id: &str) -> Option<i32> {
        if self.cache.is_none() {
            self.cache = Some(self.load_cache());
        }
        self.cache.as_ref().and_then(|c| c.get(thread_id).copied())
    }

    /// Whether a pid is alive. Ports `isAlive`.
    pub fn is_alive(&self, pid: i32) -> bool {
        (self.is_alive_fn)(pid)
    }

    /// Drop the cache so the next lookup re-reads the sessions dir. Ports `invalidate`.
    pub fn invalidate(&mut self) {
        self.cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_session(dir: &Path, pid: i32, session_id: &str) {
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::json!({ "pid": pid, "sessionId": session_id, "cwd": "/tmp" }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn finds_pid_by_thread_id() {
        let dir = TempDir::new().unwrap();
        write_session(dir.path(), 12345, "thread-a");
        write_session(dir.path(), 67890, "thread-b");
        let mut lookup = ClaudePidLookup::new(dir.path().to_path_buf());
        assert_eq!(lookup.pid_for_thread("thread-a"), Some(12345));
        assert_eq!(lookup.pid_for_thread("thread-b"), Some(67890));
        assert_eq!(lookup.pid_for_thread("thread-missing"), None);
    }

    #[test]
    fn missing_dir_returns_none() {
        let dir = TempDir::new().unwrap();
        let mut lookup = ClaudePidLookup::new(dir.path().join("nope"));
        assert_eq!(lookup.pid_for_thread("thread-a"), None);
    }

    #[test]
    fn skips_invalid_session_json() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("bad.json"), "not json").unwrap();
        write_session(dir.path(), 12345, "thread-a");
        let mut lookup = ClaudePidLookup::new(dir.path().to_path_buf());
        assert_eq!(lookup.pid_for_thread("thread-a"), Some(12345));
    }

    #[test]
    fn invalidate_picks_up_new_sessions() {
        let dir = TempDir::new().unwrap();
        let mut lookup = ClaudePidLookup::new(dir.path().to_path_buf());
        assert_eq!(lookup.pid_for_thread("thread-a"), None); // primes empty cache
        write_session(dir.path(), 12345, "thread-a");
        assert_eq!(lookup.pid_for_thread("thread-a"), None); // stale cache
        lookup.invalidate();
        assert_eq!(lookup.pid_for_thread("thread-a"), Some(12345));
    }

    #[test]
    fn injected_is_alive_is_used() {
        let dir = TempDir::new().unwrap();
        let always_dead = ClaudePidLookup::with_is_alive(dir.path().to_path_buf(), |_| false);
        assert!(!always_dead.is_alive(1));
        let always_alive = ClaudePidLookup::with_is_alive(dir.path().to_path_buf(), |_| true);
        assert!(always_alive.is_alive(999_999_999));
    }
}
