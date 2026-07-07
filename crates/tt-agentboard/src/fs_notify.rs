//! Optional filesystem-change accelerant. Isolated from the deterministic scan
//! core: polling (the bridge calling [`crate::watcher::AgentWatcher::scan`]) is
//! the reliable path; this just lets the bridge trigger an *eager* rescan when a
//! journal file changes, cutting latency. Ports the low-latency-supplement role
//! of the TS `setupWatchers`/`watchDir` fs-watches (§1).
//!
//! Events are debounced: a streaming agent appends its JSONL several times a
//! second (plus subagent/meta writes), and firing `on_change` per inotify event
//! made the host rescan back-to-back continuously — pegging a core exactly
//! while the user's agents were busiest. Bursts coalesce into at most one
//! callback per [`DEBOUNCE`] window; worst-case added latency is one window.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Coalescing window for filesystem-event bursts.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Watches a directory tree and invokes `on_change` (debounced) when anything
/// under it changes. Hold the returned value alive for as long as
/// notifications are wanted; dropping it stops watching and ends the debounce
/// thread.
pub struct DirNotifier {
    _watcher: RecommendedWatcher,
}

impl DirNotifier {
    /// Start watching `dir` recursively, calling `on_change` at most once per
    /// debounce window while events keep arriving.
    pub fn watch<F>(dir: &Path, on_change: F) -> notify::Result<Self>
    where
        F: Fn() + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::spawn(move || debounce_loop(&rx, on_change));

        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(dir, RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher })
    }
}

/// Block for a first event, drain everything else arriving within the window,
/// then fire once. Exits when the sender (the watcher closure) drops.
fn debounce_loop<F: Fn()>(rx: &mpsc::Receiver<()>, on_change: F) {
    while rx.recv().is_ok() {
        let deadline = Instant::now() + DEBOUNCE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(()) => continue,
                Err(_) => break,
            }
        }
        on_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn burst_of_events_fires_once() {
        let (tx, rx) = mpsc::channel::<()>();
        let fired = Arc::new(AtomicUsize::new(0));
        let fired2 = fired.clone();
        let handle = std::thread::spawn(move || {
            debounce_loop(&rx, move || {
                fired2.fetch_add(1, Ordering::SeqCst);
            })
        });

        for _ in 0..25 {
            tx.send(()).unwrap();
        }
        drop(tx); // burst then disconnect: one coalesced callback, then exit
        handle.join().unwrap();
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spaced_bursts_fire_separately() {
        let (tx, rx) = mpsc::channel::<()>();
        let fired = Arc::new(AtomicUsize::new(0));
        let fired2 = fired.clone();
        let handle = std::thread::spawn(move || {
            debounce_loop(&rx, move || {
                fired2.fetch_add(1, Ordering::SeqCst);
            })
        });

        tx.send(()).unwrap();
        std::thread::sleep(DEBOUNCE + Duration::from_millis(80));
        assert_eq!(fired.load(Ordering::SeqCst), 1, "first burst flushed after its window");
        tx.send(()).unwrap();
        drop(tx);
        handle.join().unwrap();
        assert_eq!(fired.load(Ordering::SeqCst), 2);
    }
}
