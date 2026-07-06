//! Persisted folder-rail collapse/expand state (issue #52). Keyed by the
//! frontend's own opaque row key — `RepoData.key` for a solo-repo row, or
//! `"<repoKey>::<folderDir>"` for a sub-folder row inside a multi-checkout
//! repo (see `apps/client/src/screens/agentboard.tsx`'s `RepoGroup`). Both are
//! stable across restarts and across Agentboard windows (derived from the git
//! remote origin URL, or the folder's own path when there's no remote).
//!
//! Absence of a key means "expanded" (the default), so unlike a window layout
//! a collapse entry is never ambiguous between "never touched" and
//! "explicitly cleared" — collapsing sets `true`, expanding removes the key.
//! That means every mutation is an exact single-key op, so — unlike
//! `windows.rs` — the frontend doesn't need to track "touched" keys itself;
//! `CollapseStore::set` always knows precisely which key changed.
//!
//! Stored at `~/.config/towles-tool/agentboard/collapse.json`. Path-
//! parameterized so tests use a tempdir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk shape: `{ "collapsed": { "<key>": true, ... } }`. Keys default to
/// expanded, so only collapsed rows are ever stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollapsePayload {
    #[serde(default)]
    pub collapsed: BTreeMap<String, bool>,
}

/// Owns the collapse map plus its file path. Loaded once; each `set` persists
/// via a reread-merge-write (see [`CollapseStore::save`]).
#[derive(Debug, Default)]
pub struct CollapseStore {
    path: Option<PathBuf>,
    payload: CollapsePayload,
    /// The single key changed by the last `set`, if not yet saved.
    dirty_key: Option<String>,
}

/// Default location: `~/.config/towles-tool/agentboard/collapse.json`.
pub fn default_collapse_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("towles-tool")
        .join("agentboard")
        .join("collapse.json")
}

impl CollapseStore {
    /// Load from `path` (default-empty on missing/corrupt). `None` = in-memory.
    pub fn new(path: Option<PathBuf>) -> Self {
        let payload = match &path {
            Some(p) => load(p),
            None => CollapsePayload::default(),
        };
        Self { path, payload, dirty_key: None }
    }

    pub fn payload(&self) -> &CollapsePayload {
        &self.payload
    }

    /// Set (`true`) or clear (`false`, the default — removes the key) one
    /// row's collapsed state. Returns whether it changed; caller persists on
    /// `true`.
    pub fn set(&mut self, key: &str, collapsed: bool) -> bool {
        let current = self.payload.collapsed.get(key).copied().unwrap_or(false);
        if current == collapsed {
            return false;
        }
        if collapsed {
            self.payload.collapsed.insert(key.to_string(), true);
        } else {
            self.payload.collapsed.remove(key);
        }
        self.dirty_key = Some(key.to_string());
        true
    }

    /// Persist the key touched by the last `set`. Rereads the file fresh and
    /// overwrites only that one key, leaving every other key exactly as found
    /// on disk — this file is shared by every Agentboard window
    /// (`tt:parallel-slots` runs one per checkout), so a blind whole-map
    /// overwrite from this instance's hydrate-once, possibly-stale copy would
    /// silently revert another window's toggle of a row we never touched.
    /// Same-key concurrent toggles are still last-write-wins; there's no
    /// cross-process locking here.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        let Some(key) = self.dirty_key.take() else {
            return Ok(());
        };
        let mut on_disk = load(&path);
        match self.payload.collapsed.get(&key) {
            Some(value) => {
                on_disk.collapsed.insert(key, *value);
            }
            None => {
                on_disk.collapsed.remove(&key);
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&on_disk).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(&path, format!("{json}\n"))?;
        self.payload = on_disk;
        Ok(())
    }
}

/// Load the collapse map from `path` (default-empty on missing/corrupt).
fn load(path: &Path) -> CollapsePayload {
    let Ok(text) = std::fs::read_to_string(path) else {
        return CollapsePayload::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_reports_change_and_defaults_to_expanded() {
        let mut store = CollapseStore::new(None);
        assert!(!store.payload().collapsed.contains_key("repo:a"));
        assert!(store.set("repo:a", true));
        assert!(!store.set("repo:a", true)); // unchanged
        assert_eq!(store.payload().collapsed.get("repo:a"), Some(&true));
    }

    #[test]
    fn expanding_removes_the_key_rather_than_storing_false() {
        let mut store = CollapseStore::new(None);
        store.set("repo:a", true);
        assert!(store.set("repo:a", false));
        assert!(!store.payload().collapsed.contains_key("repo:a"));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("tt-collapse-{}", std::process::id()));
        let path = dir.join("collapse.json");
        let mut store = CollapseStore::new(Some(path.clone()));
        store.set("repo:a::/repo/a", true);
        store.save().unwrap();

        let reloaded = CollapseStore::new(Some(path));
        assert_eq!(reloaded.payload().collapsed.get("repo:a::/repo/a"), Some(&true));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_with_no_pending_change_is_a_noop() {
        let dir = std::env::temp_dir().join(format!("tt-collapse-noop-{}", std::process::id()));
        let path = dir.join("collapse.json");
        let store = CollapseStore::new(Some(path.clone()));
        // No `set` call, so no dirty key — save() must not create the file.
        let mut store = store;
        store.save().unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_instances_dont_clobber_each_others_keys() {
        let dir =
            std::env::temp_dir().join(format!("tt-collapse-concurrent-{}", std::process::id()));
        let path = dir.join("collapse.json");

        // Two long-lived "app instances" both loaded before either has saved.
        let mut a = CollapseStore::new(Some(path.clone()));
        let mut b = CollapseStore::new(Some(path.clone()));

        a.set("repo:a", true);
        a.save().unwrap();

        // B never learned about repo:a (loaded before A's save), but toggles
        // its own row — must not erase A's toggle.
        b.set("repo:b", true);
        b.save().unwrap();

        let reloaded = CollapseStore::new(Some(path));
        assert_eq!(reloaded.payload().collapsed.get("repo:a"), Some(&true));
        assert_eq!(reloaded.payload().collapsed.get("repo:b"), Some(&true));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_loads_default() {
        let dir = std::env::temp_dir().join(format!("tt-collapse-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("collapse.json");
        std::fs::write(&path, "not json").unwrap();
        let store = CollapseStore::new(Some(path));
        assert_eq!(store.payload(), &CollapsePayload::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
