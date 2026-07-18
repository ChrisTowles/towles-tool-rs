//! Persisted PTY-session records per folder (Folder Rail). A "session" is one
//! xterm PTY shell rooted in a checkout; Claude Code runs *inside* one. The
//! records (id + display name + createdAt) persist to the app's own file,
//! `~/.config/towles-tool/agentboard/sessions.json`, so a folder's sessions
//! survive restarts even though the PTYs themselves are respawned lazily.
//!
//! Sits beside `repos.json` / `session-order.json` (same per-file, not-in-shared-
//! settings pattern; see [`crate::repos`]). Path-parameterized so tests use a
//! tempdir; `now_ms` is injected rather than read from the clock.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
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
    /// User-authored "what am I working toward in this session" — captured
    /// when starting Claude, so a later look at the rail explains why this
    /// session exists. Empty counts as unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Last Claude Code session (thread) id observed running in this PTY.
    /// Live attribution dies with the agent process, so persisting it is what
    /// lets a *crashed* run still say "pane X was running session Y" — the
    /// input to the resume picker (see [`crate::resume`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_claude_session_id: Option<String>,
}

/// On-disk shape: `{ "folders": { "<folderDir>": [ {id,name,createdAt}, ... ] },
/// "nextSeq": { "<folderDir>": n } }`. `nextSeq` is the per-folder add counter:
/// it bumps on every `add` and only ever increases, so a removed session's
/// number is never reused by a later `add` and two adds can never share a
/// sequence — even within the same millisecond (the id hashes it; see
/// [`gen_id`]).
#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionsConfig {
    #[serde(default)]
    folders: HashMap<String, Vec<SessionRecord>>,
    #[serde(default)]
    next_seq: HashMap<String, usize>,
}

/// Owns the folder→sessions map plus its file path. Loaded once; saved on each
/// mutation by the caller (engine), mirroring `SessionOrder`. `save()` only
/// ever rewrites the folders touched since the last save (see `dirty`) — this
/// file is shared by every Agentboard window (`tt slot` runs one per
/// checkout), so a save must never clobber another window's folders that this
/// in-memory copy simply hasn't heard about yet.
#[derive(Debug, Default)]
pub struct SessionStore {
    path: Option<PathBuf>,
    folders: HashMap<String, Vec<SessionRecord>>,
    /// Per-folder monotonic add counter (names + id salt); see
    /// [`SessionsConfig`].
    next_seq: HashMap<String, usize>,
    /// Folder dirs mutated since the last successful `save()`.
    dirty: HashSet<String>,
}

/// Default location: `<agentboard_dir>/sessions.json` (slot-scoped in a slot
/// checkout; see [`tt_config::agentboard_dir`]).
pub fn default_sessions_path() -> PathBuf {
    tt_config::agentboard_dir_lossy().join("sessions.json")
}

impl SessionStore {
    /// Load from `path` (empty on missing/corrupt). `None` = in-memory only (tests).
    pub fn new(path: Option<PathBuf>) -> Self {
        let (folders, next_seq) = match &path {
            Some(p) => load(p),
            None => (HashMap::new(), HashMap::new()),
        };
        Self { path, folders, next_seq, dirty: HashSet::new() }
    }

    /// The persisted records for a folder (empty slice if none yet).
    pub fn sessions_for(&self, dir: &str) -> &[SessionRecord] {
        self.folders.get(dir).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every folder with its session records (for listing/inspection).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[SessionRecord])> {
        self.folders.iter().map(|(dir, list)| (dir.as_str(), list.as_slice()))
    }

    /// Seed a default `shell 1` for a folder we've never seen before. A folder
    /// whose sessions were all deliberately closed keeps its (empty) entry and
    /// is NOT re-seeded — zero-session folders are a legitimate state the UI
    /// renders as "no sessions". Returns whether a record was created (caller
    /// persists on `true`).
    pub fn ensure_default(&mut self, dir: &str, now_ms: i64) -> bool {
        if !self.folders.contains_key(dir) {
            self.add(dir, None, now_ms);
            return true;
        }
        false
    }

    /// Append a new session to a folder. `name` defaults to `shell <n>`, where
    /// `<n>` comes from a per-folder counter that only ever increases — a
    /// removed session's number is never reused, so numbers can't collide or
    /// silently repeat across add/remove cycles. The counter bumps on every
    /// add (named or not) and salts the id, so an add/remove/add cycle within
    /// one millisecond still yields distinct ids — an id reuse would let a
    /// stale agent-event/PTY-exit race attribute to the wrong (newly created)
    /// session. Returns the created record. Caller persists.
    pub fn add(&mut self, dir: &str, name: Option<&str>, now_ms: i64) -> SessionRecord {
        let counter = self.next_seq.entry(dir.to_string()).or_insert(0);
        *counter += 1;
        let seq = *counter;
        let name = match name {
            Some(n) => n.to_string(),
            None => format!("shell {seq}"),
        };
        let id = gen_id(dir, now_ms, seq);
        let record = SessionRecord {
            id,
            name,
            created_at: now_ms,
            purpose: None,
            last_claude_session_id: None,
        };
        self.folders.entry(dir.to_string()).or_default().push(record.clone());
        self.dirty.insert(dir.to_string());
        record
    }

    /// Rename the session with `id` (in any folder). Returns whether it changed.
    pub fn rename(&mut self, id: &str, new_name: &str) -> bool {
        for (dir, list) in self.folders.iter_mut() {
            if let Some(rec) = list.iter_mut().find(|r| r.id == id) {
                if rec.name != new_name {
                    rec.name = new_name.to_string();
                    self.dirty.insert(dir.clone());
                    return true;
                }
                return false;
            }
        }
        false
    }

    /// Set (or clear with `None`/blank) the session's user-authored purpose.
    /// Returns whether it changed. Caller persists on `true`.
    pub fn set_purpose(&mut self, id: &str, purpose: Option<&str>) -> bool {
        let normalized = purpose.map(str::trim).filter(|p| !p.is_empty()).map(str::to_string);
        for list in self.folders.values_mut() {
            if let Some(rec) = list.iter_mut().find(|r| r.id == id) {
                if rec.purpose != normalized {
                    rec.purpose = normalized;
                    return true;
                }
                return false;
            }
        }
        false
    }

    /// Record that Claude session `claude_session_id` was seen running in PTY
    /// session `id`. Returns whether the link changed; the caller persists on
    /// `true`. This runs on every payload rebuild, so an unchanged link must
    /// not mark the folder dirty.
    pub fn note_agent(&mut self, id: &str, claude_session_id: &str) -> bool {
        for (dir, list) in self.folders.iter_mut() {
            if let Some(rec) = list.iter_mut().find(|r| r.id == id) {
                if rec.last_claude_session_id.as_deref() == Some(claude_session_id) {
                    return false;
                }
                rec.last_claude_session_id = Some(claude_session_id.to_string());
                self.dirty.insert(dir.clone());
                return true;
            }
        }
        false
    }

    /// Remove the session with `id`. Returns whether it was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        for (dir, list) in self.folders.iter_mut() {
            let before = list.len();
            list.retain(|r| r.id != id);
            if list.len() != before {
                self.dirty.insert(dir.clone());
                return true;
            }
        }
        false
    }

    /// Drop records for folders no longer in `dirs` (called after a repo removal).
    pub fn prune(&mut self, dirs: &HashSet<String>) {
        let removed: Vec<String> =
            self.folders.keys().filter(|dir| !dirs.contains(*dir)).cloned().collect();
        self.folders.retain(|dir, _| dirs.contains(dir));
        self.dirty.extend(removed);
    }

    /// Persist the folders touched since the last save. Rereads the file
    /// fresh first and only overwrites *those* folders' entries, leaving any
    /// other folder exactly as found on disk — a concurrent Agentboard window
    /// may have added/renamed/removed sessions for a different folder in the
    /// meantime, and a blind whole-file overwrite from this instance's
    /// possibly-stale in-memory copy would silently erase that. Same-folder
    /// concurrent edits are still last-write-wins; there's no cross-process
    /// locking here.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if self.dirty.is_empty() {
            return Ok(());
        }
        let dirty: Vec<String> = self.dirty.drain().collect();
        let (mut on_disk_folders, mut on_disk_next_seq) = load(&path);
        for dir in &dirty {
            match self.folders.get(dir) {
                Some(list) => {
                    on_disk_folders.insert(dir.clone(), list.clone());
                }
                None => {
                    on_disk_folders.remove(dir);
                }
            }
            if let Some(seq) = self.next_seq.get(dir) {
                on_disk_next_seq.insert(dir.clone(), *seq);
            }
        }
        let config =
            SessionsConfig { folders: on_disk_folders.clone(), next_seq: on_disk_next_seq.clone() };
        let json = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string());
        crate::persist::write_atomic(&path, &format!("{json}\n"))?;
        self.folders = on_disk_folders;
        self.next_seq = on_disk_next_seq;
        Ok(())
    }
}

/// Load the folder→sessions map and next-seq counters from `path` (empty on
/// missing/corrupt).
fn load(path: &Path) -> (HashMap<String, Vec<SessionRecord>>, HashMap<String, usize>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (HashMap::new(), HashMap::new());
    };
    let config = serde_json::from_str::<SessionsConfig>(&text).unwrap_or_default();
    (config.folders, config.next_seq)
}

/// Generate a session id unique across folders and creations: a hash of the
/// folder dir, the creation time, and the folder's monotonic add counter (never
/// reused, so same-millisecond add/remove/add cycles still differ). Also serves
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
    fn add_never_reuses_a_removed_number() {
        let mut store = SessionStore::new(None);
        let a = store.add("/r/a", None, 1); // "shell 1"
        let b = store.add("/r/a", None, 2); // "shell 2"
        assert!(store.remove(&a.id));
        let c = store.add("/r/a", None, 3);
        assert_eq!(b.name, "shell 2");
        assert_eq!(c.name, "shell 3"); // not "shell 2" again, and not the freed "shell 1"
    }

    #[test]
    fn add_remove_add_in_same_millisecond_yields_distinct_ids() {
        // An id reuse here would let a stale agent-event/PTY-exit race
        // attribute to the "new" pane instead of the session that produced it.
        let mut store = SessionStore::new(None);
        let a = store.add("/r/a", None, 7);
        assert!(store.remove(&a.id));
        let b = store.add("/r/a", None, 7);
        assert_ne!(a.id, b.id);
        // Explicitly named adds consume a sequence too — same guarantee.
        let c = store.add("/r/a", Some("build"), 7);
        assert!(store.remove(&c.id));
        let d = store.add("/r/a", Some("build"), 7);
        assert_ne!(c.id, d.id);
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
    fn set_and_clear_purpose() {
        let mut store = SessionStore::new(None);
        let rec = store.add("/r/a", None, 1);
        assert_eq!(store.sessions_for("/r/a")[0].purpose, None);
        assert!(store.set_purpose(&rec.id, Some("  ship the checkout flow  ")));
        assert_eq!(
            store.sessions_for("/r/a")[0].purpose.as_deref(),
            Some("ship the checkout flow")
        );
        // Unchanged write reports false.
        assert!(!store.set_purpose(&rec.id, Some("ship the checkout flow")));
        // Blank/None clears it.
        assert!(store.set_purpose(&rec.id, Some("   ")));
        assert_eq!(store.sessions_for("/r/a")[0].purpose, None);
        // Unknown id is a no-op.
        assert!(!store.set_purpose("nope", Some("x")));
    }

    #[test]
    fn note_agent_records_link_and_only_dirties_on_change() {
        let mut store = SessionStore::new(None);
        let rec = store.add("/r/a", None, 1);
        assert_eq!(store.sessions_for("/r/a")[0].last_claude_session_id, None);

        assert!(store.note_agent(&rec.id, "claude-1"));
        assert_eq!(
            store.sessions_for("/r/a")[0].last_claude_session_id.as_deref(),
            Some("claude-1")
        );

        // Same link on a later tick: no rewrite of the shared file requested.
        assert!(!store.note_agent(&rec.id, "claude-1"));

        // A different Claude session in the same pane replaces the link.
        assert!(store.note_agent(&rec.id, "claude-2"));
        assert_eq!(
            store.sessions_for("/r/a")[0].last_claude_session_id.as_deref(),
            Some("claude-2")
        );

        assert!(!store.note_agent("nope", "claude-3"));
    }

    #[test]
    fn agent_link_survives_a_save_load_round_trip() {
        // The whole point of persisting it: after a crash the process env is
        // gone, so the file is the only surviving record of the pairing.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.json");
        let mut store = SessionStore::new(Some(path.clone()));
        let rec = store.add("/r/a", None, 1);
        store.note_agent(&rec.id, "claude-1");
        store.save().unwrap();

        let reloaded = SessionStore::new(Some(path));
        assert_eq!(
            reloaded.sessions_for("/r/a")[0].last_claude_session_id.as_deref(),
            Some("claude-1")
        );
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

    #[test]
    fn concurrent_instances_dont_clobber_each_others_folders() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.json");

        // Two long-lived "app instances" both loaded before either has saved.
        let mut a = SessionStore::new(Some(path.clone()));
        let mut b = SessionStore::new(Some(path.clone()));

        a.add("/r/a", Some("only-a"), 1);
        a.save().unwrap();

        // B never learned about /r/a (loaded before A's save), but adds its
        // own folder — its save must not erase A's folder.
        b.add("/r/b", Some("only-b"), 2);
        b.save().unwrap();

        let reloaded = SessionStore::new(Some(path));
        assert_eq!(reloaded.sessions_for("/r/a").len(), 1);
        assert_eq!(reloaded.sessions_for("/r/a")[0].name, "only-a");
        assert_eq!(reloaded.sessions_for("/r/b").len(), 1);
        assert_eq!(reloaded.sessions_for("/r/b")[0].name, "only-b");
    }
}
