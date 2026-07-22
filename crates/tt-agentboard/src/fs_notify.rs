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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
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
        std::thread::spawn(move || debounce_loop(&rx, move |_batch| on_change()));

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

/// Like [`DirNotifier`], but watches a *changing set* of directories under
/// one root instead of the whole tree — built for `~/.claude/projects`,
/// which every Claude Code session on the machine writes into. A plain
/// `DirNotifier` on that root fires the agentboard's eager rescan for *any*
/// session's transcript activity, not just the tracked repos it's actually
/// polling; on a machine with several concurrent sessions (this one
/// included, mid-conversation) that reduces the accelerant to "rescan
/// constantly," which is not faster than the poll it's meant to shortcut.
///
/// [`set_targets`](Self::set_targets) recomputes the watched set from a list
/// of checkout dirs (tracked repos plus discovered worktrees — the same set
/// [`crate::engine::Engine::watch_targets`] hands the host), diffing against
/// what's currently watched so an unchanged set touches no watcher calls.
/// Each target maps to its Claude Code transcript directory via
/// [`crate::watchers::claude_code::encode_project_dir_name`]; a checkout
/// with no Claude sessions yet (the encoded dir doesn't exist) is simply not
/// watched — there's nothing to eagerly refresh for it yet, and the poll
/// loop remains the correctness baseline regardless, so under-watching here
/// is a latency tradeoff, never a staleness bug.
pub struct ScopedDirNotifier {
    watcher: RecommendedWatcher,
    watched: HashSet<PathBuf>,
}

impl ScopedDirNotifier {
    pub fn new<F>(on_change: F) -> notify::Result<Self>
    where
        F: Fn() + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::spawn(move || debounce_loop(&rx, move |_batch| on_change()));
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        })?;
        Ok(Self { watcher, watched: HashSet::new() })
    }

    /// Recompute the watched set from `targets` (absolute checkout dirs).
    /// Idempotent — an unchanged `targets` list is a no-op, so calling this
    /// on every poll tick (cheap: no subprocess, just `notify` add/remove
    /// calls on the *changed* entries) is the intended usage.
    pub fn set_targets(&mut self, projects_dir: &Path, targets: &[String]) {
        let desired: HashSet<PathBuf> = targets
            .iter()
            .map(|dir| {
                projects_dir.join(crate::watchers::claude_code::encode_project_dir_name(dir))
            })
            .filter(|p| p.is_dir())
            .collect();
        for stale in self.watched.difference(&desired).cloned().collect::<Vec<_>>() {
            let _ = self.watcher.unwatch(&stale);
            self.watched.remove(&stale);
        }
        for fresh in desired.difference(&self.watched).cloned().collect::<Vec<_>>() {
            if self.watcher.watch(&fresh, RecursiveMode::Recursive).is_ok() {
                self.watched.insert(fresh);
            }
        }
    }

    /// The currently watched directories — test/diagnostic seam.
    pub fn watched(&self) -> &HashSet<PathBuf> {
        &self.watched
    }
}

/// Watches a *set of files* for on-disk changes with **one** OS watcher
/// instance, debounced like [`DirNotifier`]. Backs the app's code-viewer and
/// diff-pane refresh: an agent editing an open file must show up in the
/// editor, not sit stale until the next manual reopen.
///
/// One instance per checkout, not per file, deliberately: inotify *instances*
/// are a scarce per-user resource (`max_user_instances` defaults to 128), and
/// a 50-file diff pane watching per-file would burn 50 of them. Individual
/// directory *watches* on one instance are the cheap plural resource.
///
/// Files register via [`add`](Self::add)/[`remove`](Self::remove) with
/// refcounts (two panes viewing the same file share one registration). Each
/// file's *parent directory* is watched (non-recursively) rather than the
/// file itself: every well-behaved writer here — the viewer's own atomic
/// save, an agent's tmp+rename replace, a `git checkout` — retires the
/// file's inode, and a watch on the inode goes permanently silent after the
/// first such replace. Events for unregistered siblings are filtered out;
/// registered paths touched within a debounce window flush as one
/// deduplicated batch.
///
/// Known degradation: if a watched *parent directory* itself is deleted,
/// the OS drops its watch and a later recreation of the directory is not
/// re-watched until every registration inside it drains and re-adds. Files
/// there go silent rather than wrong — the consumers' poll-driven refresh
/// (git-stats keyed) remains the safety net, and pane rebuilds re-register.
pub struct MultiFileNotifier {
    watcher: RecommendedWatcher,
    /// Refcount per registered file — shared with the watcher callback,
    /// which filters events against its key set.
    targets: Arc<Mutex<HashMap<PathBuf, u32>>>,
    /// Watched parent directory → number of distinct registered files in it.
    parents: HashMap<PathBuf, u32>,
}

impl MultiFileNotifier {
    /// Create an empty notifier. `on_change` receives the deduplicated batch
    /// of registered files touched during a debounce window — content
    /// writes, replaces, deletions alike, leaving "what changed" to the
    /// caller's re-read.
    pub fn new<F>(on_change: F) -> notify::Result<Self>
    where
        F: Fn(Vec<PathBuf>) + Send + 'static,
    {
        let targets: Arc<Mutex<HashMap<PathBuf, u32>>> = Arc::default();
        let (tx, rx) = mpsc::channel::<PathBuf>();
        std::thread::spawn(move || debounce_loop(&rx, on_change));

        let cb_targets = targets.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                // A rename lists both halves (tmp name and final name) in
                // `paths`, so a tmp+rename replace still matches its target.
                let hits: Vec<PathBuf> = {
                    let map = cb_targets.lock().unwrap();
                    event.paths.iter().filter(|p| map.contains_key(*p)).cloned().collect()
                };
                for path in hits {
                    let _ = tx.send(path);
                }
            }
        })?;
        Ok(Self { watcher, targets, parents: HashMap::new() })
    }

    /// Register `file` (absolute path, parent must exist). Refcounted — a
    /// second registration of the same path is free and requires a matching
    /// [`remove`](Self::remove).
    pub fn add(&mut self, file: &Path) -> notify::Result<()> {
        if let Some(count) = self.targets.lock().unwrap().get_mut(file) {
            *count += 1;
            return Ok(());
        }
        let parent = file.parent().unwrap_or(Path::new("/")).to_path_buf();
        match self.parents.get_mut(&parent) {
            Some(n) => *n += 1,
            None => {
                self.watcher.watch(&parent, RecursiveMode::NonRecursive)?;
                self.parents.insert(parent, 1);
            }
        }
        self.targets.lock().unwrap().insert(file.to_path_buf(), 1);
        Ok(())
    }

    /// Drop one reference to `file`; the last drop stops delivering it and
    /// releases its parent-directory watch when no sibling needs it.
    /// Unmatched calls are a no-op.
    pub fn remove(&mut self, file: &Path) {
        {
            let mut targets = self.targets.lock().unwrap();
            let Some(count) = targets.get_mut(file) else {
                return;
            };
            *count -= 1;
            if *count > 0 {
                return;
            }
            targets.remove(file);
        }
        let parent = file.parent().unwrap_or(Path::new("/")).to_path_buf();
        if let Some(n) = self.parents.get_mut(&parent) {
            *n -= 1;
            if *n == 0 {
                self.parents.remove(&parent);
                let _ = self.watcher.unwatch(&parent);
            }
        }
    }

    /// No files registered — the owner can drop the whole notifier.
    pub fn is_empty(&self) -> bool {
        self.targets.lock().unwrap().is_empty()
    }
}

/// Block for a first message, drain everything else arriving within the
/// window into a deduplicated batch, then flush once. Exits when the sender
/// drops. [`DirNotifier`] sends `()` (the batch collapses to one entry and
/// the payload is ignored); [`MultiFileNotifier`] sends the changed paths.
fn debounce_loop<T: PartialEq, F: Fn(Vec<T>)>(rx: &mpsc::Receiver<T>, on_change: F) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + DEBOUNCE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(message) => {
                    if !batch.contains(&message) {
                        batch.push(message);
                    }
                }
                Err(_) => break,
            }
        }
        on_change(std::mem::take(&mut batch));
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
            debounce_loop(&rx, move |_batch| {
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
    fn debounce_loop_dedupes_within_a_window() {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let batches: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::default();
        let batches2 = batches.clone();
        let handle = std::thread::spawn(move || {
            debounce_loop(&rx, move |batch| batches2.lock().unwrap().push(batch))
        });

        for _ in 0..5 {
            tx.send(PathBuf::from("/a")).unwrap();
            tx.send(PathBuf::from("/b")).unwrap();
        }
        drop(tx); // burst then disconnect: one deduped batch, then exit
        handle.join().unwrap();
        let flushed = batches.lock().unwrap();
        assert_eq!(flushed.as_slice(), &[vec![PathBuf::from("/a"), PathBuf::from("/b")]]);
    }

    /// The core reason this type exists: `set_targets` must only ever watch
    /// directories that correspond to a currently tracked checkout, never
    /// the whole `~/.claude/projects` tree — a repo dropped from the tracked
    /// set (task removed, repo untracked) must stop being watched, not just
    /// stop being added to.
    #[test]
    fn set_targets_watches_only_tracked_checkouts_and_drops_untracked_ones() {
        let root = tempfile::TempDir::new().unwrap();
        let projects = root.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        // Two checkouts with a session transcript dir already on disk, one
        // without — the third proves a checkout with no Claude session yet
        // is simply never watched, not an error.
        std::fs::create_dir_all(projects.join("-home-u-repo-a")).unwrap();
        std::fs::create_dir_all(projects.join("-home-u-repo-b")).unwrap();

        let mut n = ScopedDirNotifier::new(|| {}).unwrap();
        n.set_targets(&projects, &["/home/u/repo-a".into(), "/home/u/no-session-yet".into()]);
        assert_eq!(n.watched(), &HashSet::from([projects.join("-home-u-repo-a")]));

        // Swap the tracked set entirely: repo-a drops out, repo-b comes in.
        n.set_targets(&projects, &["/home/u/repo-b".into()]);
        assert_eq!(n.watched(), &HashSet::from([projects.join("-home-u-repo-b")]));
    }

    /// A worktree checkout's path runs through `.claude/worktrees/...` — the
    /// literal dot is exactly the case the naive `/`→`-` guess used to miss
    /// (see `watchers::claude_code::encode_project_dir_name`'s doc). This is
    /// the scenario the storm investigation actually hit: this app's own
    /// worktree tasks must resolve to their real transcript directory, not
    /// silently go unwatched.
    #[test]
    fn set_targets_resolves_a_worktree_checkout_through_the_dot_in_its_path() {
        let root = tempfile::TempDir::new().unwrap();
        let projects = root.path().join("projects");
        let encoded = "-home-u-repo--claude-worktrees-fix-thing";
        std::fs::create_dir_all(projects.join(encoded)).unwrap();

        let mut n = ScopedDirNotifier::new(|| {}).unwrap();
        n.set_targets(&projects, &["/home/u/repo/.claude/worktrees/fix-thing".into()]);
        assert_eq!(n.watched(), &HashSet::from([projects.join(encoded)]));
    }

    /// Calling `set_targets` again with an identical list must not touch the
    /// watcher at all — this is what makes it cheap enough to call on every
    /// 2s poll tick rather than only when the tracked set is known to have
    /// changed.
    #[test]
    fn set_targets_is_a_no_op_when_the_target_list_is_unchanged() {
        let root = tempfile::TempDir::new().unwrap();
        let projects = root.path().join("projects");
        std::fs::create_dir_all(projects.join("-home-u-repo-a")).unwrap();

        let mut n = ScopedDirNotifier::new(|| {}).unwrap();
        let targets = vec!["/home/u/repo-a".to_string()];
        n.set_targets(&projects, &targets);
        let watched_after_first = n.watched().clone();
        n.set_targets(&projects, &targets);
        assert_eq!(n.watched(), &watched_after_first);
    }

    /// End-to-end proof, not just set bookkeeping: a write inside an
    /// untracked project dir (some other Claude Code session, anywhere on
    /// the machine) must never fire the callback, while a write inside a
    /// tracked one still does. This is the actual behavior the storm
    /// investigation needed — a global `DirNotifier` on `~/.claude/projects`
    /// fires on both.
    #[test]
    fn only_a_tracked_checkouts_journal_write_fires_the_callback() {
        let root = tempfile::TempDir::new().unwrap();
        let projects = root.path().join("projects");
        let tracked = projects.join("-home-u-repo-a");
        let untracked = projects.join("-home-u-someone-elses-session");
        std::fs::create_dir_all(&tracked).unwrap();
        std::fs::create_dir_all(&untracked).unwrap();

        let (tx, rx) = mpsc::channel::<()>();
        let mut n = ScopedDirNotifier::new(move || {
            let _ = tx.send(());
        })
        .unwrap();
        n.set_targets(&projects, &["/home/u/repo-a".into()]);

        std::fs::write(untracked.join("sid.jsonl"), "noise").unwrap();
        assert!(
            rx.recv_timeout(DEBOUNCE * 3).is_err(),
            "an untracked session's transcript write must not fire the accelerant"
        );

        std::fs::write(tracked.join("sid.jsonl"), "a message").unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "a tracked checkout's own transcript write must still fire it"
        );
    }

    /// Real-filesystem check of the behaviors the viewer/diff pane depend on:
    /// unregistered-sibling churn stays silent, a tmp+rename replace (the
    /// atomic-save shape agents and the viewer itself use) fires with the
    /// right path, files in different subdirectories share the one instance,
    /// and a removed registration goes silent again.
    #[test]
    fn multi_file_notifier_filters_and_routes_by_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a")).unwrap();
        std::fs::create_dir(root.path().join("b")).unwrap();
        let one = root.path().join("a/one.txt");
        let two = root.path().join("b/two.txt");
        std::fs::write(&one, "v1").unwrap();
        std::fs::write(&two, "v1").unwrap();

        let (fired_tx, fired_rx) = mpsc::channel::<Vec<PathBuf>>();
        let mut notifier = MultiFileNotifier::new(move |batch| {
            let _ = fired_tx.send(batch);
        })
        .unwrap();
        notifier.add(&one).unwrap();
        notifier.add(&two).unwrap();

        std::fs::write(root.path().join("a/noise.txt"), "noise").unwrap();
        assert!(
            fired_rx.recv_timeout(DEBOUNCE * 3).is_err(),
            "unregistered sibling write must not fire"
        );

        let tmp = root.path().join("a/.one.txt.tmp");
        std::fs::write(&tmp, "v2").unwrap();
        std::fs::rename(&tmp, &one).unwrap();
        let batch = fired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(batch, vec![one.clone()], "tmp+rename replace fires with the replaced path");

        std::fs::write(&two, "v2").unwrap();
        let batch = fired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(batch, vec![two.clone()], "second registered file routes independently");

        notifier.remove(&one);
        std::fs::write(&one, "v3").unwrap();
        assert!(
            fired_rx.recv_timeout(DEBOUNCE * 3).is_err(),
            "a removed registration must go silent"
        );
        assert!(!notifier.is_empty(), "the other file is still registered");
    }

    #[test]
    fn multi_file_notifier_refcounts_registrations() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("shared.txt");
        std::fs::write(&file, "v1").unwrap();

        let (fired_tx, fired_rx) = mpsc::channel::<Vec<PathBuf>>();
        let mut notifier = MultiFileNotifier::new(move |batch| {
            let _ = fired_tx.send(batch);
        })
        .unwrap();
        notifier.add(&file).unwrap();
        notifier.add(&file).unwrap();
        notifier.remove(&file);

        std::fs::write(&file, "v2").unwrap();
        assert!(
            fired_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "one of two registrations dropped — the file must still fire"
        );
        assert!(!notifier.is_empty());
        notifier.remove(&file);
        assert!(notifier.is_empty(), "matched removes drain the registration");
    }
}
