//! Persisted per-folder metadata (Folder Rail): today just the user-authored
//! *purpose* — "what am I working toward in this checkout". Stored in the app's
//! own file, `~/.config/towles-tool/agentboard/folder_meta.json`, keyed by the
//! folder's absolute dir (same per-file pattern as [`crate::sessions`]).
//! Path-parameterized so tests use a tempdir.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Metadata for one folder. A struct (not a bare string) so future per-folder
/// fields land here without a file-format break.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Branch this folder's diff/ahead-behind stats compare against, overriding
    /// the origin/main-or-master auto-detect — for a long-running branch that
    /// didn't fork from main (e.g. a release line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

/// On-disk shape: `{ "folders": { "<folderDir>": { "purpose": "..." } } }`.
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

/// Default location: `<agentboard_dir>/folder_meta.json` (slot-scoped in a slot
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

    /// The purpose text for a folder, if one is set (empty counts as unset).
    pub fn purpose_for(&self, dir: &str) -> Option<&str> {
        self.folders.get(dir).and_then(|m| m.purpose.as_deref()).filter(|p| !p.is_empty())
    }

    /// Set (or clear with `None`/blank) a folder's purpose. Returns whether it
    /// changed. Caller persists on `true`.
    pub fn set_purpose(&mut self, dir: &str, purpose: Option<&str>) -> bool {
        let normalized = purpose.map(str::trim).filter(|p| !p.is_empty()).map(str::to_string);
        let current = self.folders.get(dir).and_then(|m| m.purpose.clone());
        if current == normalized {
            return false;
        }
        match normalized {
            Some(p) => self.folders.entry(dir.to_string()).or_default().purpose = Some(p),
            None => {
                if let Some(meta) = self.folders.get_mut(dir) {
                    meta.purpose = None;
                    if *meta == FolderMeta::default() {
                        self.folders.remove(dir);
                    }
                }
            }
        }
        self.dirty.insert(dir.to_string());
        true
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
        match normalized {
            Some(b) => self.folders.entry(dir.to_string()).or_default().base_branch = Some(b),
            None => {
                if let Some(meta) = self.folders.get_mut(dir) {
                    meta.base_branch = None;
                    if *meta == FolderMeta::default() {
                        self.folders.remove(dir);
                    }
                }
            }
        }
        self.dirty.insert(dir.to_string());
        true
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
    fn set_and_read_purpose() {
        let mut store = FolderMetaStore::new(None);
        assert_eq!(store.purpose_for("/r/a"), None);
        assert!(store.set_purpose("/r/a", Some("ship the checkout flow")));
        assert_eq!(store.purpose_for("/r/a"), Some("ship the checkout flow"));
        // Unchanged write reports false.
        assert!(!store.set_purpose("/r/a", Some("ship the checkout flow")));
    }

    #[test]
    fn blank_or_none_clears() {
        let mut store = FolderMetaStore::new(None);
        store.set_purpose("/r/a", Some("x"));
        assert!(store.set_purpose("/r/a", Some("   ")));
        assert_eq!(store.purpose_for("/r/a"), None);
        store.set_purpose("/r/a", Some("y"));
        assert!(store.set_purpose("/r/a", None));
        assert_eq!(store.purpose_for("/r/a"), None);
        // Clearing an unset folder is a no-op.
        assert!(!store.set_purpose("/r/b", None));
    }

    #[test]
    fn trims_whitespace() {
        let mut store = FolderMetaStore::new(None);
        store.set_purpose("/r/a", Some("  fix webhooks  "));
        assert_eq!(store.purpose_for("/r/a"), Some("fix webhooks"));
    }

    #[test]
    fn prune_drops_removed_dirs() {
        let mut store = FolderMetaStore::new(None);
        store.set_purpose("/r/a", Some("a"));
        store.set_purpose("/r/b", Some("b"));
        let keep: std::collections::HashSet<String> = ["/r/a".to_string()].into();
        store.prune(&keep);
        assert_eq!(store.purpose_for("/r/a"), Some("a"));
        assert_eq!(store.purpose_for("/r/b"), None);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("tt-folder-meta-{}", std::process::id()));
        let path = dir.join("folder_meta.json");
        let mut store = FolderMetaStore::new(Some(path.clone()));
        store.set_purpose("/r/a", Some("ship it"));
        store.save().unwrap();

        let reloaded = FolderMetaStore::new(Some(path));
        assert_eq!(reloaded.purpose_for("/r/a"), Some("ship it"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_and_read_base_branch() {
        let mut store = FolderMetaStore::new(None);
        assert_eq!(store.base_branch_for("/r/a"), None);
        assert!(store.set_base_branch("/r/a", Some("release/2.0")));
        assert_eq!(store.base_branch_for("/r/a"), Some("release/2.0"));
        assert!(!store.set_base_branch("/r/a", Some("release/2.0")));
        assert!(store.set_base_branch("/r/a", None));
        assert_eq!(store.base_branch_for("/r/a"), None);
    }

    #[test]
    fn purpose_and_base_branch_are_independent() {
        let mut store = FolderMetaStore::new(None);
        store.set_purpose("/r/a", Some("ship it"));
        store.set_base_branch("/r/a", Some("develop"));
        assert_eq!(store.purpose_for("/r/a"), Some("ship it"));
        assert_eq!(store.base_branch_for("/r/a"), Some("develop"));
        // Clearing one leaves the other intact.
        store.set_purpose("/r/a", None);
        assert_eq!(store.purpose_for("/r/a"), None);
        assert_eq!(store.base_branch_for("/r/a"), Some("develop"));
    }

    #[test]
    fn concurrent_instances_dont_clobber_each_others_folders() {
        let dir =
            std::env::temp_dir().join(format!("tt-folder-meta-concurrent-{}", std::process::id()));
        let path = dir.join("folder_meta.json");

        let mut a = FolderMetaStore::new(Some(path.clone()));
        let mut b = FolderMetaStore::new(Some(path.clone()));

        a.set_purpose("/r/a", Some("ship it"));
        a.save().unwrap();

        b.set_purpose("/r/b", Some("fix it"));
        b.save().unwrap();

        let reloaded = FolderMetaStore::new(Some(path));
        assert_eq!(reloaded.purpose_for("/r/a"), Some("ship it"));
        assert_eq!(reloaded.purpose_for("/r/b"), Some("fix it"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
