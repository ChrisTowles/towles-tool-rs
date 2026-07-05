//! Persisted window layouts (Folder Rail): a *window* is a named, ordered list
//! of pane session-ids tiled side-by-side in the app's main area, scoped to a
//! single folder (a window never mixes panes from more than one checkout).
//! The layout is frontend-owned — the client mutates it locally and saves the
//! whole blob via one debounced command — and hydrates from `ab_get_state`.
//! Stored at `~/.config/towles-tool/agentboard/windows.json` (same per-file
//! pattern as [`crate::sessions`]). Path-parameterized so tests use a tempdir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One window: a named tiling of pane session-ids (1 full, 2 halves, 3 thirds,
/// 4+ a 2×2 grid — the client owns the tiling math), scoped to `folder_dir`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgWindow {
    pub id: String,
    pub name: String,
    pub folder_dir: String,
    #[serde(default)]
    pub panes: Vec<String>,
}

/// The whole layout: every window plus which one is focused per folder.
/// Serialized verbatim to disk and onto `StatePayload.windows`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsPayload {
    #[serde(default)]
    pub windows: Vec<AgWindow>,
    /// Focused window id per folder dir.
    #[serde(default)]
    pub active_windows: BTreeMap<String, String>,
}

/// Owns the layout plus its file path. Loaded once; saved on each set.
#[derive(Debug, Default)]
pub struct WindowsStore {
    path: Option<PathBuf>,
    payload: WindowsPayload,
}

/// Default location: `~/.config/towles-tool/agentboard/windows.json`.
pub fn default_windows_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("towles-tool")
        .join("agentboard")
        .join("windows.json")
}

impl WindowsStore {
    /// Load from `path` (default-empty on missing/corrupt). `None` = in-memory.
    pub fn new(path: Option<PathBuf>) -> Self {
        let payload = match &path {
            Some(p) => load(p),
            None => WindowsPayload::default(),
        };
        Self { path, payload }
    }

    pub fn payload(&self) -> &WindowsPayload {
        &self.payload
    }

    /// Replace the layout wholesale (the client owns it). Returns whether it
    /// changed; caller persists on `true`.
    pub fn set(&mut self, payload: WindowsPayload) -> bool {
        if self.payload == payload {
            return false;
        }
        self.payload = payload;
        true
    }

    /// Persist to the configured path (no-op for in-memory stores).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.payload).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(path, format!("{json}\n"))
    }
}

/// Load the layout from `path` (default-empty on missing/corrupt).
fn load(path: &Path) -> WindowsPayload {
    let Ok(text) = std::fs::read_to_string(path) else {
        return WindowsPayload::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> WindowsPayload {
        WindowsPayload {
            windows: vec![AgWindow {
                id: "w1".into(),
                name: "checkout push".into(),
                folder_dir: "/repo/checkout".into(),
                panes: vec!["s1".into(), "s2".into()],
            }],
            active_windows: BTreeMap::from([("/repo/checkout".into(), "w1".into())]),
        }
    }

    #[test]
    fn set_reports_change() {
        let mut store = WindowsStore::new(None);
        assert!(store.set(layout()));
        assert!(!store.set(layout())); // unchanged
        assert_eq!(store.payload().windows[0].panes, vec!["s1", "s2"]);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("tt-windows-{}", std::process::id()));
        let path = dir.join("windows.json");
        let mut store = WindowsStore::new(Some(path.clone()));
        store.set(layout());
        store.save().unwrap();

        let reloaded = WindowsStore::new(Some(path));
        assert_eq!(reloaded.payload(), &layout());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_loads_default() {
        let dir = std::env::temp_dir().join(format!("tt-windows-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("windows.json");
        std::fs::write(&path, "not json").unwrap();
        let store = WindowsStore::new(Some(path));
        assert_eq!(store.payload(), &WindowsPayload::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
