//! Persisted PTY-session records per folder (Folder Rail). A "session" is one
//! xterm PTY shell rooted in a checkout; Claude Code runs *inside* one. The
//! records (id + display name + createdAt) persist to the app's own file,
//! `~/.config/towles-tool/agentboard/sessions.json`, so a folder's sessions
//! survive restarts even though the PTYs themselves are respawned lazily.
//!
//! Sits beside `repos.json` / `session-order.json` (same per-file, not-in-shared-
//! settings pattern; see [`crate::repos`]). Path-parameterized so tests use a
//! tempdir; `now_ms` is injected rather than read from the clock.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One PTY session record: a stable id (also the PTY `term_id` / `TT_SESSION_ID`)
/// and a user-facing name. Serialized camelCase to match the wire client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

/// On-disk shape: `{ "<folderDir>": [ {id,name,createdAt}, ... ] }`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionsConfig {
    #[serde(default)]
    folders: HashMap<String, Vec<SessionRecord>>,
}

/// Owns the folder→sessions map plus its file path. Loaded once; saved on each
/// mutation by the caller (engine), mirroring `SessionOrder`.
#[derive(Debug, Default)]
pub struct SessionStore {
    path: Option<PathBuf>,
    folders: HashMap<String, Vec<SessionRecord>>,
}

/// Default location: `~/.config/towles-tool/agentboard/sessions.json`.
pub fn default_sessions_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("towles-tool")
        .join("agentboard")
        .join("sessions.json")
}

impl SessionStore {
    /// Load from `path` (empty on missing/corrupt). `None` = in-memory only (tests).
    pub fn new(path: Option<PathBuf>) -> Self {
        let folders = match &path {
            Some(p) => load(p),
            None => HashMap::new(),
        };
        Self { path, folders }
    }

    /// The persisted records for a folder (empty slice if none yet).
    pub fn sessions_for(&self, dir: &str) -> &[SessionRecord] {
        self.folders.get(dir).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every folder with its session records (for listing/inspection).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[SessionRecord])> {
        self.folders.iter().map(|(dir, list)| (dir.as_str(), list.as_slice()))
    }

    /// Guarantee a folder has ≥1 session, seeding a default `shell 1` if empty.
    /// Returns whether a record was created (caller persists on `true`).
    pub fn ensure_default(&mut self, dir: &str, now_ms: i64) -> bool {
        if self.folders.get(dir).map(Vec::is_empty).unwrap_or(true) {
            self.add(dir, None, now_ms);
            return true;
        }
        false
    }

    /// Append a new session to a folder. `name` defaults to `shell <n>`. Returns
    /// the created record. Caller persists.
    pub fn add(&mut self, dir: &str, name: Option<&str>, now_ms: i64) -> SessionRecord {
        let list = self.folders.entry(dir.to_string()).or_default();
        let seq = list.len();
        let name = name.map(str::to_string).unwrap_or_else(|| format!("shell {}", seq + 1));
        let id = gen_id(dir, now_ms, seq);
        let record = SessionRecord { id, name, created_at: now_ms };
        list.push(record.clone());
        record
    }

    /// Rename the session with `id` (in any folder). Returns whether it changed.
    pub fn rename(&mut self, id: &str, new_name: &str) -> bool {
        for list in self.folders.values_mut() {
            if let Some(rec) = list.iter_mut().find(|r| r.id == id) {
                if rec.name != new_name {
                    rec.name = new_name.to_string();
                    return true;
                }
                return false;
            }
        }
        false
    }

    /// Remove the session with `id`. Returns whether it was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        for list in self.folders.values_mut() {
            let before = list.len();
            list.retain(|r| r.id != id);
            if list.len() != before {
                return true;
            }
        }
        false
    }

    /// Drop records for folders no longer in `dirs` (called after a repo removal).
    pub fn prune(&mut self, dirs: &std::collections::HashSet<String>) {
        self.folders.retain(|dir, _| dirs.contains(dir));
    }

    /// Persist to the configured path (no-op for in-memory stores).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let config = SessionsConfig { folders: self.folders.clone() };
        let json = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(path, format!("{json}\n"))
    }
}

/// Load the folder→sessions map from `path` (empty on missing/corrupt).
fn load(path: &Path) -> HashMap<String, Vec<SessionRecord>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<SessionsConfig>(&text).map(|c| c.folders).unwrap_or_default()
}

/// Generate a session id unique across folders and creations: a hash of the
/// folder dir, the creation time, and the in-folder sequence number. Also serves
/// as the PTY `term_id` and the injected `TT_SESSION_ID`.
fn gen_id(dir: &str, now_ms: i64, seq: usize) -> String {
    let mut h = DefaultHasher::new();
    dir.hash(&mut h);
    now_ms.hash(&mut h);
    seq.hash(&mut h);
    format!("s{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    #[test]
    fn ensure_default_seeds_one_shell() {
        let mut store = SessionStore::new(None);
        assert!(store.sessions_for("/r/a").is_empty());
        assert!(store.ensure_default("/r/a", 1000));
        let list = store.sessions_for("/r/a");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "shell 1");
        assert_eq!(list[0].created_at, 1000);
        // Idempotent once seeded.
        assert!(!store.ensure_default("/r/a", 2000));
        assert_eq!(store.sessions_for("/r/a").len(), 1);
    }

    #[test]
    fn add_names_sequentially_and_unique_ids() {
        let mut store = SessionStore::new(None);
        let a = store.add("/r/a", None, 1);
        let b = store.add("/r/a", None, 2);
        assert_eq!(a.name, "shell 1");
        assert_eq!(b.name, "shell 2");
        assert_ne!(a.id, b.id);
        // Different folder, same time/seq → still distinct (dir is hashed in).
        let c = store.add("/r/b", None, 1);
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn add_with_explicit_name() {
        let mut store = SessionStore::new(None);
        let rec = store.add("/r/a", Some("build"), 1);
        assert_eq!(rec.name, "build");
    }

    #[test]
    fn rename_and_remove_by_id() {
        let mut store = SessionStore::new(None);
        let rec = store.add("/r/a", None, 1);
        assert!(store.rename(&rec.id, "logs"));
        assert!(!store.rename(&rec.id, "logs")); // unchanged
        assert_eq!(store.sessions_for("/r/a")[0].name, "logs");
        assert!(store.remove(&rec.id));
        assert!(store.sessions_for("/r/a").is_empty());
        assert!(!store.remove(&rec.id));
    }

    #[test]
    fn prune_drops_unconfigured_folders() {
        let mut store = SessionStore::new(None);
        store.add("/r/a", None, 1);
        store.add("/r/b", None, 1);
        let keep: HashSet<String> = ["/r/a".to_string()].into_iter().collect();
        store.prune(&keep);
        assert!(!store.sessions_for("/r/a").is_empty());
        assert!(store.sessions_for("/r/b").is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("sessions.json");
        let mut store = SessionStore::new(Some(path.clone()));
        store.add("/r/a", Some("one"), 5);
        store.save().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("createdAt"));
        assert!(raw.ends_with('\n'));
        let reloaded = SessionStore::new(Some(path));
        assert_eq!(reloaded.sessions_for("/r/a").len(), 1);
        assert_eq!(reloaded.sessions_for("/r/a")[0].name, "one");
    }
}
