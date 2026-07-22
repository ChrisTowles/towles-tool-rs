//! Persisted per-folder metadata (Folder Rail): the *base-branch override* —
//! what this checkout's diff/ahead-behind stats compare against — and the
//! *quiet override*, which forces a folder to count as quiet for the
//! frontend's "hide inactive" rail filter (`isFolderQuiet` in
//! `apps/client/src/lib/agentboard.ts`) even when its own activity signals
//! say otherwise — the same flag whether it got set by hand or (in future) by
//! some other rule; nothing here distinguishes "manual" from any other
//! source. Stored in the app's own file,
//! `~/.config/towles-tool/agentboard/folder_meta.json`, keyed by the folder's
//! absolute dir (same per-file pattern as [`crate::sessions`]).
//! Path-parameterized so tests use a tempdir.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Metadata for one folder. A struct rather than a bare string so the on-disk
/// value stays a named object — a second field can land without rewriting
/// every existing file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderMeta {
    /// Branch this folder's diff/ahead-behind stats compare against, overriding
    /// the origin/main-or-master auto-detect — for a long-running branch that
    /// didn't fork from main (e.g. a release line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Forces this folder to count as quiet — the rail treats it as quiet
    /// regardless of its actual activity, so it collapses into the stub row
    /// under "hide inactive" the same as an auto-detected one. Only ever
    /// `Some(true)` on disk; the setter normalizes `false` to absent so "not
    /// forced" is one state rather than two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<bool>,
}

impl FolderMeta {
    fn is_empty(&self) -> bool {
        self.base_branch.is_none() && self.quiet.is_none()
    }
}

/// On-disk shape: `{ "folders": { "<folderDir>": { "baseBranch": "...",
/// "quiet": true } } }`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FolderMetaConfig {
    #[serde(default)]
    folders: HashMap<String, FolderMeta>,
}

/// Owns the folder→meta map plus its file path. Loaded once; saved on each
/// mutation by the caller (engine), mirroring [`crate::sessions::SessionStore`]
/// — including its merge-on-save behavior, since this file is likewise shared
/// across every Agentboard window.
#[derive(Debug, Default)]
pub struct FolderMetaStore {
    path: Option<PathBuf>,
    folders: HashMap<String, FolderMeta>,
    /// Folder dirs mutated since the last successful `save()`.
    dirty: HashSet<String>,
}

/// Default location: `<agentboard_dir>/folder_meta.json` (task-scoped in a task
/// checkout; see [`tt_config::agentboard_dir`]).
pub fn default_folder_meta_path() -> PathBuf {
    tt_config::agentboard_dir_lossy().join("folder_meta.json")
}

impl FolderMetaStore {
    /// Load from `path` (empty on missing/corrupt). `None` = in-memory only (tests).
    pub fn new(path: Option<PathBuf>) -> Self {
        let folders = match &path {
            Some(p) => load(p),
            None => HashMap::new(),
        };
        Self { path, folders, dirty: HashSet::new() }
    }

    /// The base-branch override for a folder, if one is set (empty counts as unset).
    pub fn base_branch_for(&self, dir: &str) -> Option<&str> {
        self.folders.get(dir).and_then(|m| m.base_branch.as_deref()).filter(|b| !b.is_empty())
    }

    /// Set (or clear with `None`/blank) a folder's base-branch override.
    /// Returns whether it changed. Caller persists on `true`.
    pub fn set_base_branch(&mut self, dir: &str, base_branch: Option<&str>) -> bool {
        let normalized = base_branch.map(str::trim).filter(|b| !b.is_empty()).map(str::to_string);
        let current = self.folders.get(dir).and_then(|m| m.base_branch.clone());
        if current == normalized {
            return false;
        }
        self.folders.entry(dir.to_string()).or_default().base_branch = normalized;
        self.drop_if_empty(dir);
        self.dirty.insert(dir.to_string());
        true
    }

    /// Whether a folder's "hide inactive" quiet state is forced, regardless of
    /// its own activity signals.
    pub fn quiet_for(&self, dir: &str) -> bool {
        self.folders.get(dir).and_then(|m| m.quiet).unwrap_or(false)
    }

    /// Set (`true`) or clear (`false`, the default) a folder's quiet override.
    /// Returns whether it changed. Caller persists on `true`.
    pub fn set_quiet(&mut self, dir: &str, quiet: bool) -> bool {
        let normalized = quiet.then_some(true);
        let current = self.folders.get(dir).and_then(|m| m.quiet);
        if current == normalized {
            return false;
        }
        self.folders.entry(dir.to_string()).or_default().quiet = normalized;
        self.drop_if_empty(dir);
        self.dirty.insert(dir.to_string());
        true
    }

    /// Remove `dir`'s entry outright once every field is back to default,
    /// rather than leaving a `{}` behind for every folder ever touched.
    fn drop_if_empty(&mut self, dir: &str) {
        if self.folders.get(dir).is_some_and(FolderMeta::is_empty) {
            self.folders.remove(dir);
        }
    }

    /// Drop metadata for folders no longer in `dirs` (called after a repo removal).
    pub fn prune(&mut self, dirs: &HashSet<String>) {
        let removed: Vec<String> =
            self.folders.keys().filter(|dir| !dirs.contains(*dir)).cloned().collect();
        self.folders.retain(|dir, _| dirs.contains(dir));
        self.dirty.extend(removed);
    }

    /// Persist the folders touched since the last save; see
    /// [`crate::sessions::SessionStore::save`] for why this rereads the file
    /// fresh and only overwrites the dirty folders rather than the whole map.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if self.dirty.is_empty() {
            return Ok(());
        }
        let dirty: Vec<String> = self.dirty.drain().collect();
        let mut on_disk = load(&path);
        for dir in &dirty {
            match self.folders.get(dir) {
                Some(meta) => {
                    on_disk.insert(dir.clone(), meta.clone());
                }
                None => {
                    on_disk.remove(dir);
                }
            }
        }
        let config = FolderMetaConfig { folders: on_disk.clone() };
        let json = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string());
        crate::persist::write_atomic(&path, &format!("{json}\n"))?;
        self.folders = on_disk;
        Ok(())
    }
}

/// Load the folder→meta map from `path` (empty on missing/corrupt).
fn load(path: &Path) -> HashMap<String, FolderMeta> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<FolderMetaConfig>(&text).map(|c| c.folders).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_read_base_branch() {
        let mut store = FolderMetaStore::new(None);
        assert_eq!(store.base_branch_for("/r/a"), None);
        assert!(store.set_base_branch("/r/a", Some("release/2.0")));
        assert_eq!(store.base_branch_for("/r/a"), Some("release/2.0"));
        // Unchanged write reports false.
        assert!(!store.set_base_branch("/r/a", Some("release/2.0")));
    }

    #[test]
    fn blank_or_none_clears() {
        let mut store = FolderMetaStore::new(None);
        store.set_base_branch("/r/a", Some("develop"));
        assert!(store.set_base_branch("/r/a", Some("   ")));
        assert_eq!(store.base_branch_for("/r/a"), None);
        store.set_base_branch("/r/a", Some("develop"));
        assert!(store.set_base_branch("/r/a", None));
        assert_eq!(store.base_branch_for("/r/a"), None);
        // Clearing an unset folder is a no-op.
        assert!(!store.set_base_branch("/r/b", None));
    }

    #[test]
    fn trims_whitespace() {
        let mut store = FolderMetaStore::new(None);
        store.set_base_branch("/r/a", Some("  release/2.0  "));
        assert_eq!(store.base_branch_for("/r/a"), Some("release/2.0"));
    }

    #[test]
    fn set_and_clear_quiet() {
        let mut store = FolderMetaStore::new(None);
        assert!(!store.quiet_for("/r/a"));
        assert!(store.set_quiet("/r/a", true));
        assert!(store.quiet_for("/r/a"));
        // Unchanged write reports false.
        assert!(!store.set_quiet("/r/a", true));
        assert!(store.set_quiet("/r/a", false));
        assert!(!store.quiet_for("/r/a"));
        // Clearing an unset folder is a no-op.
        assert!(!store.set_quiet("/r/b", false));
    }

    #[test]
    fn quiet_and_base_branch_coexist_on_one_folder() {
        let mut store = FolderMetaStore::new(None);
        store.set_base_branch("/r/a", Some("develop"));
        store.set_quiet("/r/a", true);
        assert_eq!(store.base_branch_for("/r/a"), Some("develop"));
        assert!(store.quiet_for("/r/a"));

        // Clearing one field must not wipe out the other.
        store.set_quiet("/r/a", false);
        assert_eq!(store.base_branch_for("/r/a"), Some("develop"));
        assert!(!store.quiet_for("/r/a"));

        // Now clearing the last field drops the entry outright.
        store.set_base_branch("/r/a", None);
        assert_eq!(store.base_branch_for("/r/a"), None);
        assert!(!store.quiet_for("/r/a"));
    }

    #[test]
    fn prune_drops_removed_dirs() {
        let mut store = FolderMetaStore::new(None);
        store.set_base_branch("/r/a", Some("develop"));
        store.set_base_branch("/r/b", Some("release/2.0"));
        let keep: std::collections::HashSet<String> = ["/r/a".to_string()].into();
        store.prune(&keep);
        assert_eq!(store.base_branch_for("/r/a"), Some("develop"));
        assert_eq!(store.base_branch_for("/r/b"), None);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("tt-folder-meta-{}", std::process::id()));
        let path = dir.join("folder_meta.json");
        let mut store = FolderMetaStore::new(Some(path.clone()));
        store.set_base_branch("/r/a", Some("develop"));
        store.save().unwrap();

        let reloaded = FolderMetaStore::new(Some(path));
        assert_eq!(reloaded.base_branch_for("/r/a"), Some("develop"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_instances_dont_clobber_each_others_folders() {
        let dir =
            std::env::temp_dir().join(format!("tt-folder-meta-concurrent-{}", std::process::id()));
        let path = dir.join("folder_meta.json");

        let mut a = FolderMetaStore::new(Some(path.clone()));
        let mut b = FolderMetaStore::new(Some(path.clone()));

        a.set_base_branch("/r/a", Some("develop"));
        a.save().unwrap();

        b.set_base_branch("/r/b", Some("release/2.0"));
        b.save().unwrap();

        let reloaded = FolderMetaStore::new(Some(path));
        assert_eq!(reloaded.base_branch_for("/r/a"), Some("develop"));
        assert_eq!(reloaded.base_branch_for("/r/b"), Some("release/2.0"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
