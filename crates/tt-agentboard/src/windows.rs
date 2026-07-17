//! Persisted window layouts (Folder Rail): a *window* is a named, ordered list
//! of pane session-ids tiled side-by-side in the app's main area, scoped to a
//! single folder (a window never mixes panes from more than one checkout).
//! The layout is frontend-owned — the client mutates it locally and saves the
//! whole blob via one debounced command — and hydrates from `ab_get_state`.
//! Stored at `~/.config/towles-tool/agentboard/windows.json` (same per-file
//! pattern as [`crate::sessions`]). Path-parameterized so tests use a tempdir.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One window: a named tiling of pane session-ids (1 full, 2 halves, 3 thirds,
/// 4+ a 2×2 grid — the client owns the tiling math), scoped to `folder_dir`.
/// A window always holds at least one pane (the client makes the empty state
/// unrepresentable); paneless windows in old blobs are residue that
/// [`prune_dead`] sweeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgWindow {
    pub id: String,
    pub name: String,
    pub folder_dir: String,
    #[serde(default)]
    pub panes: Vec<String>,
    /// User-dragged column widths in per-mille of the tiling width (client-
    /// owned — see `paneRects` in `apps/client/src/lib/agentboard.ts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<Vec<u16>>,
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

/// Default location: `<agentboard_dir>/windows.json` (slot-scoped in a slot
/// checkout; see [`tt_config::agentboard_dir`]).
pub fn default_windows_path() -> PathBuf {
    tt_config::agentboard_dir_lossy().join("windows.json")
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

    /// Drop windows (and active-window entries) for folders no longer in
    /// `dirs` — e.g. a removed worktree slot's layout, which would otherwise
    /// linger in windows.json forever. Returns the folder dirs that lost
    /// state so the caller can persist them via [`Self::save`] (an empty
    /// return means nothing changed).
    pub fn prune(&mut self, dirs: &std::collections::HashSet<String>) -> Vec<String> {
        let mut gone = std::collections::BTreeSet::new();
        for window in &self.payload.windows {
            if !dirs.contains(&window.folder_dir) {
                gone.insert(window.folder_dir.clone());
            }
        }
        for dir in self.payload.active_windows.keys() {
            if !dirs.contains(dir) {
                gone.insert(dir.clone());
            }
        }
        if gone.is_empty() {
            return Vec::new();
        }
        self.payload.windows.retain(|w| dirs.contains(&w.folder_dir));
        self.payload.active_windows.retain(|dir, _| dirs.contains(dir));
        gone.into_iter().collect()
    }

    /// Drop every window (and the active-window entry) for one folder right
    /// now — used ahead of a slot removal, before the checkout disappears
    /// from disk, so the layout doesn't wait for the next poll's
    /// [`Self::prune`] to notice. Returns whether anything changed; caller
    /// persists via [`Self::save`] with `dir` in `touched`.
    pub fn remove_folder(&mut self, dir: &str) -> bool {
        let had_windows = self.payload.windows.iter().any(|w| w.folder_dir == dir);
        let had_active = self.payload.active_windows.contains_key(dir);
        if !had_windows && !had_active {
            return false;
        }
        self.payload.windows.retain(|w| w.folder_dir != dir);
        self.payload.active_windows.remove(dir);
        true
    }

    /// Persist `touched` folder dirs — the ones whose windows/active-window
    /// actually changed since the last save (the caller, `agentboard.tsx`'s
    /// `updateWins`, tracks this because a window's frontend blob doesn't
    /// distinguish "never touched" from "explicitly emptied"). Rereads the
    /// file fresh, replaces only those folders' entries, and leaves every
    /// other folder exactly as found on disk: this file is shared by every
    /// Agentboard window (`tt slot` runs one per checkout), so a
    /// blind whole-payload overwrite from this instance's hydrate-once,
    /// possibly-stale copy would erase another window's edits to folders we
    /// never touched. Same-folder concurrent edits are still last-write-wins;
    /// there's no cross-process locking here.
    pub fn save(&mut self, touched: &[String]) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if touched.is_empty() {
            return Ok(());
        }
        let mut on_disk = load(&path);
        for dir in touched {
            on_disk.windows.retain(|w| &w.folder_dir != dir);
            on_disk
                .windows
                .extend(self.payload.windows.iter().filter(|w| &w.folder_dir == dir).cloned());
            match self.payload.active_windows.get(dir) {
                Some(id) => {
                    on_disk.active_windows.insert(dir.clone(), id.clone());
                }
                None => {
                    on_disk.active_windows.remove(dir);
                }
            }
        }
        let json = serde_json::to_string_pretty(&on_disk).unwrap_or_else(|_| "{}".to_string());
        crate::persist::write_atomic(&path, &format!("{json}\n"))?;
        self.payload = on_disk;
        Ok(())
    }
}

/// Pane-id prefix marking a *diff* pane, which encodes a folder dir instead
/// of a session id. Must match `DIFF_PANE_PREFIX` in
/// `apps/client/src/lib/agentboard.ts`.
pub const DIFF_PANE_PREFIX: &str = "~diff:";

/// The folder dir a diff pane id points at (`None` for session panes).
fn diff_pane_dir(pane_id: &str) -> Option<&str> {
    pane_id.strip_prefix(DIFF_PANE_PREFIX)
}

/// Headless port of the client's `pruneWins` + active-window normalization
/// (`apps/client/src/lib/agentboard.ts` — keep the two in lockstep): reconcile
/// a persisted layout against what still exists. The blob on disk outlives its
/// referents — a slot removed while no app was running leaves windows whose
/// `folderDir` is gone and panes whose session records vanished with it.
///
/// Drops windows whose folder fails `folder_valid`, then panes that are
/// neither a known session id nor a valid folder's diff pane. A window is a
/// tiling of at least one pane (the client makes the empty state
/// unrepresentable): one emptied by the prune — or a paneless one persisted
/// before that rule — vanishes; the client mints a fresh "primary" lazily
/// when the folder next opens a pane. Returns the pruned payload plus the
/// folder dirs whose slice changed (the `touched` list
/// [`WindowsStore::save`] needs), or `None` when nothing changed.
pub fn prune_dead(
    payload: &WindowsPayload,
    valid_sessions: &BTreeSet<String>,
    mut folder_valid: impl FnMut(&str) -> bool,
) -> Option<(WindowsPayload, Vec<String>)> {
    let mut kept: Vec<AgWindow> = Vec::new();
    for win in &payload.windows {
        if !folder_valid(&win.folder_dir) {
            continue;
        }
        let panes: Vec<String> = win
            .panes
            .iter()
            .filter(|p| match diff_pane_dir(p) {
                Some(dir) => folder_valid(dir),
                None => valid_sessions.contains(*p),
            })
            .cloned()
            .collect();
        if panes.is_empty() {
            continue;
        }
        kept.push(AgWindow { panes, ..win.clone() });
    }

    let mut active_windows: BTreeMap<String, String> = BTreeMap::new();
    for win in &kept {
        if active_windows.contains_key(&win.folder_dir) {
            continue;
        }
        let cur = payload.active_windows.get(&win.folder_dir);
        let id = match cur {
            Some(cur) if kept.iter().any(|x| x.folder_dir == win.folder_dir && x.id == *cur) => {
                cur.clone()
            }
            _ => win.id.clone(),
        };
        active_windows.insert(win.folder_dir.clone(), id);
    }

    let next = WindowsPayload { windows: kept, active_windows };
    let touched = changed_folder_dirs(payload, &next);
    if touched.is_empty() { None } else { Some((next, touched)) }
}

/// The folder dirs whose slice of the layout (their windows, in order, or
/// their active-window entry) differs between two payloads — the `touched`
/// list [`WindowsStore::save`]'s merge-by-folder needs. Mirrors the client's
/// `changedFolderDirs`.
fn changed_folder_dirs(a: &WindowsPayload, b: &WindowsPayload) -> Vec<String> {
    let dirs: BTreeSet<&String> = a
        .windows
        .iter()
        .chain(&b.windows)
        .map(|w| &w.folder_dir)
        .chain(a.active_windows.keys())
        .chain(b.active_windows.keys())
        .collect();
    let sig = |p: &'_ WindowsPayload, dir: &String| -> (Vec<AgWindow>, Option<String>) {
        (
            p.windows.iter().filter(|w| &w.folder_dir == dir).cloned().collect(),
            p.active_windows.get(dir).cloned(),
        )
    };
    dirs.into_iter().filter(|d| sig(a, d) != sig(b, d)).cloned().collect()
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
                cols: Some(vec![333, 667]),
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
        store.save(&["/repo/checkout".to_string()]).unwrap();

        let reloaded = WindowsStore::new(Some(path));
        assert_eq!(reloaded.payload(), &layout());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_instances_dont_clobber_each_others_folders() {
        let dir =
            std::env::temp_dir().join(format!("tt-windows-concurrent-{}", std::process::id()));
        let path = dir.join("windows.json");

        // Two long-lived "app instances" both loaded before either has saved.
        let mut a = WindowsStore::new(Some(path.clone()));
        let mut b = WindowsStore::new(Some(path.clone()));

        a.set(WindowsPayload {
            windows: vec![AgWindow {
                id: "wa".into(),
                name: "primary".into(),
                folder_dir: "/repo/a".into(),
                panes: vec!["sa".into()],
                cols: None,
            }],
            active_windows: BTreeMap::from([("/repo/a".into(), "wa".into())]),
        });
        a.save(&["/repo/a".to_string()]).unwrap();

        // B never learned about /repo/a (loaded before A's save), but saves
        // its own folder's windows — must not erase A's folder.
        b.set(WindowsPayload {
            windows: vec![AgWindow {
                id: "wb".into(),
                name: "primary".into(),
                folder_dir: "/repo/b".into(),
                panes: vec!["sb".into()],
                cols: None,
            }],
            active_windows: BTreeMap::from([("/repo/b".into(), "wb".into())]),
        });
        b.save(&["/repo/b".to_string()]).unwrap();

        let reloaded = WindowsStore::new(Some(path));
        let dirs: std::collections::HashSet<_> =
            reloaded.payload().windows.iter().map(|w| w.folder_dir.clone()).collect();
        assert!(dirs.contains("/repo/a"));
        assert!(dirs.contains("/repo/b"));
        assert_eq!(reloaded.payload().active_windows.get("/repo/a"), Some(&"wa".to_string()));
        assert_eq!(reloaded.payload().active_windows.get("/repo/b"), Some(&"wb".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_untouched_folder_is_a_noop_even_if_stale_locally() {
        let dir = std::env::temp_dir().join(format!("tt-windows-untouched-{}", std::process::id()));
        let path = dir.join("windows.json");

        let mut a = WindowsStore::new(Some(path.clone()));
        a.set(layout());
        a.save(&["/repo/checkout".to_string()]).unwrap();

        // B loaded before A's save, so its in-memory copy is empty/stale for
        // /repo/checkout. B saves a folder it never touches at all — with an
        // empty touched list this must leave A's data alone.
        let mut b = WindowsStore::new(Some(path.clone()));
        b.save(&[]).unwrap();

        let reloaded = WindowsStore::new(Some(path));
        assert_eq!(reloaded.payload(), &layout());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn win(id: &str, folder: &str, panes: &[&str]) -> AgWindow {
        AgWindow {
            id: id.into(),
            name: id.into(),
            folder_dir: folder.into(),
            panes: panes.iter().map(|p| p.to_string()).collect(),
            cols: None,
        }
    }

    fn ids(sessions: &[&str]) -> BTreeSet<String> {
        sessions.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prune_drops_dead_folders_and_their_active_entries() {
        let payload = WindowsPayload {
            windows: vec![win("w1", "/live", &["s1"]), win("w2", "/dead", &["s2"])],
            active_windows: BTreeMap::from([
                ("/live".into(), "w1".into()),
                ("/dead".into(), "w2".into()),
            ]),
        };
        let (next, touched) = prune_dead(&payload, &ids(&["s1", "s2"]), |d| d == "/live").unwrap();
        assert_eq!(next.windows, vec![win("w1", "/live", &["s1"])]);
        assert_eq!(next.active_windows, BTreeMap::from([("/live".into(), "w1".into())]));
        assert_eq!(touched, vec!["/dead".to_string()]);
    }

    #[test]
    fn prune_drops_ghost_panes_but_keeps_valid_diff_panes() {
        let payload = WindowsPayload {
            windows: vec![win(
                "w1",
                "/live",
                &["s1", "s-gone", "~diff:/live", "~diff:/dead"],
            )],
            active_windows: BTreeMap::from([("/live".into(), "w1".into())]),
        };
        let (next, touched) = prune_dead(&payload, &ids(&["s1"]), |d| d == "/live").unwrap();
        assert_eq!(next.windows, vec![win("w1", "/live", &["s1", "~diff:/live"])]);
        assert_eq!(touched, vec!["/live".to_string()]);
    }

    #[test]
    fn prune_emptied_window_vanishes_when_a_sibling_survives() {
        let payload = WindowsPayload {
            windows: vec![win("w1", "/live", &["s-gone"]), win("w2", "/live", &["s1"])],
            active_windows: BTreeMap::from([("/live".into(), "w1".into())]),
        };
        let (next, _) = prune_dead(&payload, &ids(&["s1"]), |_| true).unwrap();
        assert_eq!(next.windows, vec![win("w2", "/live", &["s1"])]);
        // The active pointer moves to the surviving window.
        assert_eq!(next.active_windows.get("/live"), Some(&"w2".to_string()));
    }

    #[test]
    fn prune_drops_the_folder_layout_when_every_window_empties() {
        // No empty landing-surface window survives — the client mints a fresh
        // "primary" lazily when the folder next opens a pane.
        let payload = WindowsPayload {
            windows: vec![
                win("w1", "/live", &["s-gone"]),
                win("w2", "/live", &["s-gone2"]),
            ],
            active_windows: BTreeMap::from([("/live".into(), "w2".into())]),
        };
        let (next, _) = prune_dead(&payload, &ids(&[]), |_| true).unwrap();
        assert!(next.windows.is_empty());
        assert!(next.active_windows.is_empty());
    }

    #[test]
    fn prune_sweeps_legacy_paneless_windows() {
        // Empty windows can't be created anymore; one persisted before that
        // rule is residue and goes.
        let payload = WindowsPayload {
            windows: vec![win("w1", "/live", &[])],
            active_windows: BTreeMap::from([("/live".into(), "w1".into())]),
        };
        let (next, touched) = prune_dead(&payload, &ids(&[]), |_| true).unwrap();
        assert!(next.windows.is_empty());
        assert!(next.active_windows.is_empty());
        assert_eq!(touched, vec!["/live".to_string()]);
    }

    #[test]
    fn prune_returns_none_when_nothing_changed() {
        let payload = WindowsPayload {
            windows: vec![win("w1", "/live", &["s1"])],
            active_windows: BTreeMap::from([("/live".into(), "w1".into())]),
        };
        assert!(prune_dead(&payload, &ids(&["s1"]), |_| true).is_none());
        assert!(prune_dead(&WindowsPayload::default(), &ids(&[]), |_| false).is_none());
    }

    #[test]
    fn prune_drops_gone_folders_and_persists_the_removal() {
        let dir = std::env::temp_dir().join(format!("tt-windows-prune-{}", std::process::id()));
        let path = dir.join("windows.json");
        let mut store = WindowsStore::new(Some(path.clone()));
        let mut payload = layout();
        payload.windows.push(AgWindow {
            id: "w2".into(),
            name: "removed slot".into(),
            folder_dir: "/repo/slots/gone".into(),
            panes: vec!["s3".into()],
            cols: None,
        });
        payload.active_windows.insert("/repo/slots/gone".into(), "w2".into());
        store.set(payload);
        store.save(&["/repo/checkout".to_string(), "/repo/slots/gone".to_string()]).unwrap();

        let kept: std::collections::HashSet<String> = ["/repo/checkout".to_string()].into();
        let gone = store.prune(&kept);
        assert_eq!(gone, vec!["/repo/slots/gone".to_string()]);
        store.save(&gone).unwrap();

        let reloaded = WindowsStore::new(Some(path));
        assert_eq!(reloaded.payload(), &layout());
        // second prune is a no-op — nothing left to drop
        let mut reloaded = reloaded;
        assert!(reloaded.prune(&kept).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_folder_drops_its_windows_and_active_entry_only() {
        let mut store = WindowsStore::new(None);
        let mut payload = layout();
        payload.windows.push(win("w2", "/repo/other", &["s3"]));
        payload.active_windows.insert("/repo/other".into(), "w2".into());
        store.set(payload);

        assert!(store.remove_folder("/repo/checkout"));
        assert_eq!(store.payload().windows, vec![win("w2", "/repo/other", &["s3"])]);
        assert!(!store.payload().active_windows.contains_key("/repo/checkout"));
        assert_eq!(store.payload().active_windows.get("/repo/other"), Some(&"w2".to_string()));

        // Nothing left for that folder — a second call is a no-op.
        assert!(!store.remove_folder("/repo/checkout"));
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
