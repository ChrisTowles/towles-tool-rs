//! Optional filesystem-change accelerant. Isolated from the deterministic scan
//! core: polling (the bridge calling [`crate::watcher::AgentWatcher::scan`]) is
//! the reliable path; this just lets the bridge trigger an *eager* rescan when a
//! journal file changes, cutting latency. Ports the low-latency-supplement role
//! of the TS `setupWatchers`/`watchDir` fs-watches (§1).
//!
//! Not unit-tested (pure I/O); kept thin so nothing in the core depends on it.

use std::path::Path;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Watches a directory tree and invokes `on_change` on any filesystem event. Hold
/// the returned value alive for as long as notifications are wanted; dropping it
/// stops watching.
pub struct DirNotifier {
    _watcher: RecommendedWatcher,
}

impl DirNotifier {
    /// Start watching `dir` recursively, calling `on_change` for each event.
    pub fn watch<F>(dir: &Path, on_change: F) -> notify::Result<Self>
    where
        F: Fn() + Send + 'static,
    {
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    on_change();
                }
            })?;
        watcher.watch(dir, RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher })
    }
}
