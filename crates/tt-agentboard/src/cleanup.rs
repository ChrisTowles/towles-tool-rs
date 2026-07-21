//! Headless pruning of one agentboard instance store (`sessions.json` +
//! `windows.json`): drop state that references checkouts which no longer
//! exist on disk. The running app reconciles its *own* store on hydrate (the
//! client's `pruneWins` + the engine's repo-keyed session prune), but a task
//! removed while no app was running — `tt task rm` in a terminal — leaves
//! ghost folders behind in every other instance's store. `tt task clean`
//! runs this over the unscoped store plus every surviving scope's.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::sessions::SessionStore;
use crate::windows::{self, WindowsStore};

/// What pruning one store dropped (all empty = the store needed nothing).
pub struct StorePrune {
    /// The agentboard dir this describes.
    pub dir: PathBuf,
    /// Folder dirs whose session records were dropped (dir no longer exists).
    pub session_folders_dropped: Vec<String>,
    /// Windows dropped (dead folder, or emptied by the pane prune).
    pub windows_dropped: usize,
    /// Panes dropped, including those that went down with their window.
    pub panes_dropped: usize,
}

/// Prune `agentboard_dir`'s sessions/windows of folders whose directory no
/// longer exists (and window panes whose session records are gone). Returns
/// `None` when the store doesn't exist or nothing needed pruning; `dry_run`
/// reports without writing. Saves go through the stores' merge-by-folder
/// writes, so a concurrently running app instance's other folders are safe.
pub fn prune_store(agentboard_dir: &Path, dry_run: bool) -> std::io::Result<Option<StorePrune>> {
    if !agentboard_dir.is_dir() {
        return Ok(None);
    }

    let mut sessions = SessionStore::new(Some(agentboard_dir.join("sessions.json")));
    let mut keep: HashSet<String> = HashSet::new();
    let mut session_folders_dropped: Vec<String> = Vec::new();
    let mut valid_ids: BTreeSet<String> = BTreeSet::new();
    for (dir, records) in sessions.iter() {
        if Path::new(dir).is_dir() {
            keep.insert(dir.to_string());
            valid_ids.extend(records.iter().map(|r| r.id.clone()));
        } else {
            session_folders_dropped.push(dir.to_string());
        }
    }
    session_folders_dropped.sort();

    let mut wstore = WindowsStore::new(Some(agentboard_dir.join("windows.json")));
    let before = wstore.payload().clone();
    let pruned = windows::prune_dead(&before, &valid_ids, |d| Path::new(d).is_dir());

    if session_folders_dropped.is_empty() && pruned.is_none() {
        return Ok(None);
    }

    let pane_count =
        |p: &windows::WindowsPayload| p.windows.iter().map(|w| w.panes.len()).sum::<usize>();
    let (windows_dropped, panes_dropped) = match &pruned {
        Some((next, _)) => {
            (before.windows.len() - next.windows.len(), pane_count(&before) - pane_count(next))
        }
        None => (0, 0),
    };

    if !dry_run {
        if !session_folders_dropped.is_empty() {
            sessions.prune(&keep);
            sessions.save()?;
        }
        if let Some((next, touched)) = pruned {
            wstore.set(next);
            wstore.save(&touched)?;
        }
    }

    Ok(Some(StorePrune {
        dir: agentboard_dir.to_path_buf(),
        session_folders_dropped,
        windows_dropped,
        panes_dropped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a store whose sessions/windows reference one live folder (the
    /// tempdir itself) and one dead folder, then prune it for real.
    #[test]
    fn prunes_dead_folder_from_sessions_and_windows() {
        let tmp = TempDir::new().unwrap();
        let ab = tmp.path().join("agentboard");
        std::fs::create_dir_all(&ab).unwrap();
        let live = tmp.path().to_string_lossy().to_string();
        let dead = tmp.path().join("gone-checkout").to_string_lossy().to_string();

        let mut sessions = SessionStore::new(Some(ab.join("sessions.json")));
        let s_live = sessions.add(&live, None, 1);
        let s_dead = sessions.add(&dead, None, 2);
        sessions.save().unwrap();

        let mut wstore = WindowsStore::new(Some(ab.join("windows.json")));
        wstore.set(windows::WindowsPayload {
            windows: vec![
                windows::AgWindow {
                    id: "w1".into(),
                    name: "primary".into(),
                    folder_dir: live.clone(),
                    panes: vec![s_live.id.clone()],
                    cols: None,
                },
                windows::AgWindow {
                    id: "w2".into(),
                    name: "primary".into(),
                    folder_dir: dead.clone(),
                    panes: vec![s_dead.id.clone()],
                    cols: None,
                },
            ],
            active_windows: [
                (live.clone(), "w1".to_string()),
                (dead.clone(), "w2".to_string()),
            ]
            .into(),
        });
        wstore.save(&[live.clone(), dead.clone()]).unwrap();

        // Dry run reports without writing…
        let report = prune_store(&ab, true).unwrap().unwrap();
        assert_eq!(report.session_folders_dropped, vec![dead.clone()]);
        assert_eq!(report.windows_dropped, 1);
        assert_eq!(report.panes_dropped, 1);
        let untouched = WindowsStore::new(Some(ab.join("windows.json")));
        assert_eq!(untouched.payload().windows.len(), 2);

        // …the real run rewrites both files.
        let report = prune_store(&ab, false).unwrap().unwrap();
        assert_eq!(report.session_folders_dropped, vec![dead.clone()]);
        let sessions = SessionStore::new(Some(ab.join("sessions.json")));
        assert_eq!(sessions.sessions_for(&live).len(), 1);
        assert!(sessions.sessions_for(&dead).is_empty());
        let wstore = WindowsStore::new(Some(ab.join("windows.json")));
        assert_eq!(wstore.payload().windows.len(), 1);
        assert_eq!(wstore.payload().windows[0].folder_dir, live);
        assert!(!wstore.payload().active_windows.contains_key(&dead));

        // A clean store needs nothing.
        assert!(prune_store(&ab, false).unwrap().is_none());
    }

    #[test]
    fn missing_store_is_none() {
        let tmp = TempDir::new().unwrap();
        assert!(prune_store(&tmp.path().join("nope"), false).unwrap().is_none());
    }
}
