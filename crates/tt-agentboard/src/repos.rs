//! Watched-repo configuration (agentboard). The desktop app's session source: a
//! list of absolute repo paths plus the add-repo picker's scan roots, persisted
//! to the app's OWN file, `~/.config/towles-tool/agentboard/repos.json`.
//!
//! Kept out of the shared `towles-tool.settings.json` on purpose: this is
//! app-runtime state owned entirely by the Rust/Tauri app — the TypeScript CLI
//! never reads it — and it sits beside `session-order.json` which established
//! the per-file pattern. Path-parameterized so tests use a tempdir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A resolved repo: the session `name` (dir basename, disambiguated on collision)
/// and its absolute `dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    pub name: String,
    pub dir: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ReposConfig {
    #[serde(default, rename = "repoPaths")]
    repo_paths: Vec<String>,
    /// Roots the add-repo picker scans for git repos. Empty ⇒ caller's default
    /// (`~/code`). May contain a leading `~`; expansion is the caller's job.
    #[serde(default, rename = "scanRoots")]
    scan_roots: Vec<String>,
}

/// Default location: `<agentboard_shared_dir>/repos.json` — which repos exist
/// on this machine is a shared fact, so every checkout's app reads one copy
/// (see [`tt_config::agentboard_shared_dir`]).
pub fn default_repos_path() -> PathBuf {
    tt_config::agentboard_shared_dir_lossy().join("repos.json")
}

/// Load the full config. Defaulted (both fields empty) on missing/corrupt file.
fn load_config(path: &Path) -> ReposConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return ReposConfig::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Load the repo-path list, distinguishing a torn/corrupt read from a
/// legitimately absent file: `Some(vec![])` when the file doesn't exist (no
/// repos configured yet), but `None` when it exists and can't be read or
/// parsed — most likely a read that raced another instance's write (#75).
/// Callers should keep their previous in-memory list on `None` rather than
/// degrading to empty.
pub fn try_load_repos(path: &Path) -> Option<Vec<String>> {
    if !path.exists() {
        return Some(Vec::new());
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<ReposConfig>(&text).ok().map(|c| c.repo_paths)
}

/// Load the repo-path list. Empty on missing/corrupt file. Ports the loader half.
pub fn load_repos(path: &Path) -> Vec<String> {
    load_config(path).repo_paths
}

/// Load the configured scan roots (`scanRoots`). Empty on missing/corrupt file
/// or when the key is absent — callers substitute their own default.
pub fn load_scan_roots(path: &Path) -> Vec<String> {
    load_config(path).scan_roots
}

/// Persist `config` as pretty JSON with a trailing newline. Atomic
/// (temp+rename) so a concurrent reader in another instance never sees a
/// truncated fragment; see [`crate::persist::write_atomic`].
fn save_config(path: &Path, config: &ReposConfig) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(config).unwrap_or_else(|_| "{}".to_string());
    crate::persist::write_atomic(path, &format!("{json}\n"))
}

/// Persist the repo-path list as `{"repoPaths":[...]}`. Any existing `scanRoots`
/// on disk is preserved. Test-only file-seeding fixture — production writes go
/// through the `*_persisted` helpers (the CLI shell that called this was
/// removed in the 2026-07-19 trim).
#[cfg(test)]
fn save_repos(path: &Path, repo_paths: &[String]) -> std::io::Result<()> {
    let mut config = load_config(path);
    config.repo_paths = repo_paths.to_vec();
    save_config(path, &config)
}

/// Persist the scan roots (`scanRoots`), preserving the existing repo list.
pub fn save_scan_roots(path: &Path, scan_roots: &[String]) -> std::io::Result<()> {
    let mut config = load_config(path);
    config.scan_roots = scan_roots.to_vec();
    save_config(path, &config)
}

/// Add `path` straight against the on-disk file: reread fresh immediately
/// before writing, rather than trusting a caller's in-memory repo list that
/// may have gone stale. Multiple Agentboard windows (one per `tt task`
/// checkout) share this one `repos.json`; without a fresh reread here, one
/// window adding repo A while another adds repo B — both starting from the
/// same stale snapshot — would have the second save silently drop the first's
/// addition. Returns the merged repo list plus whether `path` was newly added.
pub fn add_repo_persisted(path: &Path, new_path: &str) -> std::io::Result<(Vec<String>, bool)> {
    let mut config = load_config(path);
    let added = add_repo(&mut config.repo_paths, new_path);
    save_config(path, &config)?;
    Ok((config.repo_paths, added))
}

/// Remove `dir` straight against the on-disk file, same reread-then-write
/// rationale as [`add_repo_persisted`]. Returns the merged repo list plus
/// whether `dir` was actually present to remove; a no-op when it wasn't (the
/// file is not rewritten — `tt task rm` calls this for never-tracked tasks).
pub fn remove_repo_persisted(path: &Path, dir: &str) -> std::io::Result<(Vec<String>, bool)> {
    let mut config = load_config(path);
    let removed = remove_repo_by_dir(&mut config.repo_paths, dir);
    if removed {
        save_config(path, &config)?;
    }
    Ok((config.repo_paths, removed))
}

/// Every path currently tracked in `repos.json` that names the same physical
/// directory as `dir`, by realpath, other than `dir`'s own literal string.
///
/// `git worktree add` persists a symlink-resolved (realpath) form of the path
/// it was given, which can diverge byte-for-byte from the literal path a
/// caller built for that same directory (`tt-tasks::ops::create_task` never
/// canonicalizes) — so a worktree discovered via `git worktree list`, and
/// deleted by clicking its row on the rail, can carry a `dir` that
/// exact-matches neither `repos.json` nor a bound board row's `worktree_dir`,
/// even though both name the same directory. A plain string-equality untrack
/// then silently does nothing, leaving the entry to strand as a "directory
/// missing" ghost once the worktree is actually gone.
///
/// `canonicalize` needs the directory to still exist, so **this must be
/// called before anything removes it** — called any later it just returns
/// nothing, same as if there were no alias. A tracked path that can't be
/// canonicalized (already gone, permission denied) is skipped rather than
/// guessed at: a false negative just leaves an already-orphaned string for
/// the rail's ordinary "missing" handling to catch, a false positive would
/// untrack the wrong repo.
pub fn aliases_for(repos_path: &Path, dir: &Path) -> Vec<String> {
    let Ok(dir_real) = std::fs::canonicalize(dir) else {
        return Vec::new();
    };
    let dir_s = dir.to_string_lossy().to_string();
    load_config(repos_path)
        .repo_paths
        .into_iter()
        .filter(|p| *p != dir_s)
        .filter(|p| std::fs::canonicalize(p).ok().as_deref() == Some(dir_real.as_path()))
        .collect()
}

/// Tracked paths whose directory is gone (per `exists`). Pure — the caller
/// supplies the probe so tests need no real filesystem.
pub fn missing_repo_dirs(repo_paths: &[String], exists: impl Fn(&str) -> bool) -> Vec<String> {
    repo_paths.iter().filter(|p| !exists(p)).cloned().collect()
}

/// Untrack every repo whose directory no longer exists on disk (the rail's
/// "missing" ghosts — e.g. removed worktrees), straight against the
/// on-disk file (same reread-then-write rationale as [`add_repo_persisted`]).
/// Returns the merged repo list plus the dirs that were dropped; a no-op when
/// nothing is missing (the file is not rewritten).
pub fn untrack_missing_persisted(path: &Path) -> std::io::Result<(Vec<String>, Vec<String>)> {
    let mut config = load_config(path);
    let missing = missing_repo_dirs(&config.repo_paths, |p| Path::new(p).exists());
    if !missing.is_empty() {
        config.repo_paths.retain(|p| !missing.contains(p));
        save_config(path, &config)?;
    }
    Ok((config.repo_paths, missing))
}

/// Whether `dir` is a linked worktree whose `.git` file points at a gitdir
/// that no longer exists — e.g. the main checkout's `.git/worktrees/<name>`
/// registration was removed (by hand, or a `git worktree remove`/prune that
/// raced a delete) while the worktree's own directory is still on disk. Every
/// git subprocess run in `dir` then fails with `fatal: not a git repository`,
/// which [`missing_repo_dirs`]' plain [`Path::exists`] check can't catch since
/// the directory itself is still there. A `.git` *directory* (a normal clone)
/// is never "broken" by this check — only a linked worktree can go stale this
/// way.
fn is_broken_worktree(dir: &Path) -> bool {
    let dotgit = dir.join(".git");
    let Ok(text) = std::fs::read_to_string(&dotgit) else {
        return false;
    };
    let Some(gitdir) = text.strip_prefix("gitdir:").map(str::trim) else {
        return false;
    };
    let gitdir =
        if Path::new(gitdir).is_absolute() { PathBuf::from(gitdir) } else { dir.join(gitdir) };
    !gitdir.exists()
}

/// Tracked paths that are broken linked worktrees per [`is_broken_worktree`].
fn broken_worktree_dirs(repo_paths: &[String]) -> Vec<String> {
    repo_paths.iter().filter(|p| is_broken_worktree(Path::new(p))).cloned().collect()
}

/// Untrack every repo whose linked worktree has gone stale (its gitdir
/// registration is gone, even though the directory itself remains) — straight
/// against the on-disk file, same reread-then-write rationale as
/// [`add_repo_persisted`]. Returns the merged repo list plus the dirs that
/// were dropped; a no-op when nothing is broken (the file is not rewritten).
pub fn untrack_broken_persisted(path: &Path) -> std::io::Result<(Vec<String>, Vec<String>)> {
    let mut config = load_config(path);
    let broken = broken_worktree_dirs(&config.repo_paths);
    if !broken.is_empty() {
        config.repo_paths.retain(|p| !broken.contains(p));
        save_config(path, &config)?;
    }
    Ok((config.repo_paths, broken))
}

/// Dirs skipped while scanning: hidden dirs plus common heavy build/dep dirs.
fn is_skippable(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "dist" | "build")
}

/// Scan `roots` for git repositories — a dir holding a `.git` entry (the `.git`
/// dir of a normal clone, or the `.git` *file* a worktree uses). Descends at
/// most `max_depth` levels, never into a repo once found, nor into hidden/heavy
/// dirs. Returns absolute dirs, sorted and deduped. Missing roots are ignored,
/// so this never fails — an unreadable dir just yields nothing.
pub fn discover_git_repos(roots: &[PathBuf], max_depth: usize) -> Vec<String> {
    let mut out = Vec::new();
    for root in roots {
        scan_git(root, 0, max_depth, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn scan_git(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<String>) {
    if dir.join(".git").exists() {
        if let Some(s) = dir.to_str() {
            out.push(s.to_string());
        }
        return; // a repo is a leaf — don't descend into it
    }
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if is_skippable(&entry.file_name().to_string_lossy()) {
            continue;
        }
        scan_git(&path, depth + 1, max_depth, out);
    }
}

/// Add `path` if not already present. Returns whether it was added.
pub fn add_repo(repo_paths: &mut Vec<String>, path: &str) -> bool {
    if repo_paths.iter().any(|p| p == path) {
        return false;
    }
    repo_paths.push(path.to_string());
    true
}

/// Reorder `current` to match `desired` — the rail's user-chosen repo order.
///
/// `repoPaths` *is* the order (nothing else expresses it), so reordering the
/// list is the whole feature. Deliberately tolerant of a stale `desired`,
/// because the client that dragged a row may have been looking at a snapshot
/// another window has since changed: dirs in `desired` are taken in that
/// order, anything in `current` it doesn't mention keeps its relative order
/// and lands after, and anything in `desired` that isn't tracked is ignored.
/// So a concurrent add is never dropped by a drag that predates it.
///
/// Note an untracked-then-retracked repo lands at the end rather than back in
/// its old task — deliberately, and deliberately unlike `repo_meta`, which
/// *does* survive that round trip (see `RepoMetaStore::forget`). Re-dragging
/// one row is cheap; re-picking a glyph and a hex colour is not.
pub fn reorder_repos(current: &[String], desired: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(current.len());
    for dir in desired {
        if current.contains(dir) && !out.contains(dir) {
            out.push(dir.clone());
        }
    }
    for dir in current {
        if !out.contains(dir) {
            out.push(dir.clone());
        }
    }
    out
}

/// Apply [`reorder_repos`] straight against the on-disk file, same
/// reread-then-write rationale as [`add_repo_persisted`]. Returns the new list.
pub fn reorder_repos_persisted(path: &Path, desired: &[String]) -> std::io::Result<Vec<String>> {
    let mut config = load_config(path);
    let reordered = reorder_repos(&config.repo_paths, desired);
    if reordered == config.repo_paths {
        return Ok(reordered);
    }
    config.repo_paths = reordered.clone();
    save_config(path, &config)?;
    Ok(reordered)
}

/// Remove the repo at `dir` exactly. Returns whether removed.
///
/// Dir, not the resolved session `name`, is the only safe key to remove by:
/// `repo_entries`' collision disambiguation (`parent/base` vs bare `base`) is
/// recomputed fresh on every call, so a caller removing several repos in one
/// batch by name would see earlier removals change later names out from
/// under it (e.g. removing `a/web` first un-collides `b/web` down to a bare
/// `web`, so a subsequent by-name removal of `b/web` silently misses).
pub fn remove_repo_by_dir(repo_paths: &mut Vec<String>, dir: &str) -> bool {
    let before = repo_paths.len();
    repo_paths.retain(|p| p != dir);
    repo_paths.len() != before
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn parent_basename(path: &str) -> Option<String> {
    Path::new(path).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string())
}

/// Resolve repo paths to `(name, dir)` entries. Session name = dir basename; when
/// two basenames collide, the parent-dir basename is prefixed (`parent/base`).
pub fn repo_entries(repo_paths: &[String]) -> Vec<RepoEntry> {
    // Count basenames to detect collisions.
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for p in repo_paths {
        *counts.entry(basename(p)).or_default() += 1;
    }
    repo_paths
        .iter()
        .map(|dir| {
            let base = basename(dir);
            let name = if counts.get(&base).copied().unwrap_or(0) > 1 {
                match parent_basename(dir) {
                    Some(parent) => format!("{parent}/{base}"),
                    None => base,
                }
            } else {
                base
            };
            RepoEntry { name, dir: dir.clone() }
        })
        .collect()
}

/// Resolve a watcher's project dir to its matching repo entry (longest prefix
/// match). Shared by [`resolve_session_name`] (session-name form) and
/// [`resolve_repo_dir`] (dir form) — see [`resolve_session_name`]'s doc for
/// the encoded/real-path duality this handles.
fn resolve_repo_entry<'a>(dir: &str, entries: &'a [RepoEntry]) -> Option<&'a RepoEntry> {
    let real_path = dir.starts_with('/');
    let sep = if real_path { '/' } else { '-' };
    let mut best: Option<(&RepoEntry, usize)> = None;
    for entry in entries {
        let candidate = if real_path { entry.dir.clone() } else { entry.dir.replace('/', "-") };
        let matches = dir == candidate || dir.starts_with(&format!("{candidate}{sep}"));
        if matches {
            let len = candidate.len();
            if best.is_none_or(|(_, best_len)| len > best_len) {
                best = Some((entry, len));
            }
        }
    }
    best.map(|(entry, _)| entry)
}

/// Resolve a watcher's project dir to a session name (longest prefix match).
///
/// Handles both forms the watcher produces: the transcript scan passes Claude's
/// *encoded* folder name (`/`→`-`, adopted fix #3 — matched encoded↔encoded to
/// sidestep the lossy decode), while the pid→cwd fallback passes a real absolute
/// path (matched directly against repo dirs). An input starting with `/` is
/// treated as a real path.
pub fn resolve_session_name(dir: &str, entries: &[RepoEntry]) -> Option<String> {
    resolve_repo_entry(dir, entries).map(|e| e.name.clone())
}

/// Resolve a real absolute path to an already-registered repo's `dir` (longest
/// prefix match, so a path *under* a registered checkout still resolves to
/// it). The "is this already on the rail" half of opening a past session in
/// Agentboard — see [`find_repo_root`] for the "it isn't yet" half. `dir` must
/// be a real path, not Claude's encoded form (see [`resolve_session_name`]).
pub fn resolve_repo_dir(dir: &str, entries: &[RepoEntry]) -> Option<String> {
    resolve_repo_entry(dir, entries).map(|e| e.dir.clone())
}

/// Walk up from `start` looking for a git repo root: a dir containing `.git`
/// (a normal clone's dir, or the file a worktree uses — same test
/// [`discover_git_repos`] applies). Returns `start` unchanged if no ancestor
/// has one, so the caller always gets a usable path back rather than an
/// `Option` to unwrap — registering that path as a folder is still better
/// than failing outright, just less clean than the true repo root.
pub fn find_repo_root(start: &Path) -> PathBuf {
    for ancestor in start.ancestors() {
        if ancestor.join(".git").exists() {
            return ancestor.to_path_buf();
        }
    }
    start.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn entries_use_basename() {
        let entries = repo_entries(&paths(&["/home/u/proj", "/home/u/other"]));
        assert_eq!(entries[0].name, "proj");
        assert_eq!(entries[1].name, "other");
    }

    #[test]
    fn colliding_basenames_get_parent_prefix() {
        let entries = repo_entries(&paths(&["/work/a/web", "/work/b/web", "/work/a/api"]));
        assert_eq!(entries[0].name, "a/web");
        assert_eq!(entries[1].name, "b/web");
        assert_eq!(entries[2].name, "api"); // unique → bare basename
    }

    #[test]
    fn add_repo_dedupes() {
        let mut p = paths(&["/a/x"]);
        assert!(!add_repo(&mut p, "/a/x"));
        assert!(add_repo(&mut p, "/a/y"));
        assert_eq!(p, paths(&["/a/x", "/a/y"]));
    }

    #[test]
    fn remove_repo_by_dir_removes_exact_match() {
        let mut p = paths(&["/work/a/web", "/work/b/web"]);
        assert!(remove_repo_by_dir(&mut p, "/work/a/web"));
        assert_eq!(p, paths(&["/work/b/web"]));
        assert!(!remove_repo_by_dir(&mut p, "/nope"));
    }

    #[test]
    fn remove_repo_by_dir_survives_collision_disambiguation_shifting() {
        // Removing colliding-basename checkouts one at a time, by dir, must
        // not be thrown off by `repo_entries` recomputing names (a/web,
        // b/web) fresh on every call — the bug a by-name removal loop hits
        // once the first removal un-collides the second down to a bare name.
        let mut p = paths(&["/work/a/web", "/work/b/web"]);
        assert!(remove_repo_by_dir(&mut p, "/work/a/web"));
        assert!(remove_repo_by_dir(&mut p, "/work/b/web"));
        assert!(p.is_empty());
    }

    #[test]
    fn missing_repo_dirs_filters_by_probe() {
        let p = paths(&["/gone/a", "/here/b", "/gone/c"]);
        assert_eq!(
            missing_repo_dirs(&p, |d| d.starts_with("/here")),
            paths(&["/gone/a", "/gone/c"])
        );
        assert!(missing_repo_dirs(&p, |_| true).is_empty());
    }

    #[test]
    fn untrack_missing_persisted_drops_only_gone_dirs() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real-repo");
        std::fs::create_dir_all(&real).unwrap();
        let real_s = real.to_string_lossy().to_string();
        let gone = dir.path().join("gone-repo").to_string_lossy().to_string();

        let path = dir.path().join("repos.json");
        save_repos(&path, &[real_s.clone(), gone.clone()]).unwrap();

        let (merged, removed) = untrack_missing_persisted(&path).unwrap();
        assert_eq!(merged, vec![real_s.clone()]);
        assert_eq!(removed, vec![gone]);
        // persisted: a fresh load sees only the surviving repo
        assert_eq!(load_repos(&path), vec![real_s]);

        // second run is a clean no-op
        let (merged, removed) = untrack_missing_persisted(&path).unwrap();
        assert_eq!(merged.len(), 1);
        assert!(removed.is_empty());
    }

    #[test]
    fn untrack_broken_persisted_drops_stale_worktrees_only() {
        let dir = TempDir::new().unwrap();

        // A normal clone: `.git` is a directory, never "broken".
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(clone.join(".git")).unwrap();

        // A healthy linked worktree: its gitdir target exists.
        let live_gitdir = dir.path().join("main/.git/worktrees/live");
        std::fs::create_dir_all(&live_gitdir).unwrap();
        let live_wt = dir.path().join("live-wt");
        std::fs::create_dir_all(&live_wt).unwrap();
        std::fs::write(live_wt.join(".git"), format!("gitdir: {}\n", live_gitdir.display()))
            .unwrap();

        // A stale linked worktree: the directory is still on disk, but its
        // gitdir registration under the main checkout is gone — the exact
        // shape of the reported bug.
        let stale_wt = dir.path().join("stale-wt");
        std::fs::create_dir_all(&stale_wt).unwrap();
        std::fs::write(
            stale_wt.join(".git"),
            format!("gitdir: {}\n", dir.path().join("main/.git/worktrees/stale").display()),
        )
        .unwrap();

        let clone_s = clone.to_string_lossy().to_string();
        let live_s = live_wt.to_string_lossy().to_string();
        let stale_s = stale_wt.to_string_lossy().to_string();

        let path = dir.path().join("repos.json");
        save_repos(&path, &[clone_s.clone(), live_s.clone(), stale_s.clone()]).unwrap();

        let (merged, removed) = untrack_broken_persisted(&path).unwrap();
        assert_eq!(merged, vec![clone_s.clone(), live_s.clone()]);
        assert_eq!(removed, vec![stale_s]);
        assert_eq!(load_repos(&path), vec![clone_s, live_s]);

        // second run is a clean no-op
        let (merged, removed) = untrack_broken_persisted(&path).unwrap();
        assert_eq!(merged.len(), 2);
        assert!(removed.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("repos.json");
        save_repos(&path, &paths(&["/a/x", "/b/y"])).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"repoPaths\""));
        assert!(raw.ends_with('\n'));
        assert_eq!(load_repos(&path), paths(&["/a/x", "/b/y"]));
    }

    #[test]
    fn save_preserves_scan_roots() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("repos.json");
        std::fs::write(&path, r#"{"repoPaths":["/a/x"],"scanRoots":["~/code","/srv/work"]}"#)
            .unwrap();
        assert_eq!(load_scan_roots(&path), paths(&["~/code", "/srv/work"]));
        // Adding a repo must not wipe the configured scan roots.
        save_repos(&path, &paths(&["/a/x", "/a/y"])).unwrap();
        assert_eq!(load_scan_roots(&path), paths(&["~/code", "/srv/work"]));
        assert_eq!(load_repos(&path), paths(&["/a/x", "/a/y"]));
        // Editing scan roots must not wipe the repo list.
        save_scan_roots(&path, &paths(&["~/dev"])).unwrap();
        assert_eq!(load_scan_roots(&path), paths(&["~/dev"]));
        assert_eq!(load_repos(&path), paths(&["/a/x", "/a/y"]));
    }

    #[test]
    fn discover_finds_repos_and_prunes() {
        let root = TempDir::new().unwrap();
        let base = root.path();
        // A repo at depth 1 (normal clone).
        std::fs::create_dir_all(base.join("p/proj/.git")).unwrap();
        // A repo nested under a non-repo container at depth 2.
        std::fs::create_dir_all(base.join("p/repos/task/.git")).unwrap();
        // A worktree whose `.git` is a file, not a dir.
        std::fs::create_dir_all(base.join("w/wt")).unwrap();
        std::fs::write(base.join("w/wt/.git"), "gitdir: /elsewhere").unwrap();
        // Heavy/hidden dirs must be skipped, and repos aren't descended into.
        std::fs::create_dir_all(base.join("p/proj/node_modules/pkg/.git")).unwrap();
        std::fs::create_dir_all(base.join(".hidden/nope/.git")).unwrap();

        let found = discover_git_repos(&[base.to_path_buf()], 4);
        let rel: Vec<String> = found
            .iter()
            .map(|d| {
                d.strip_prefix(base.to_str().unwrap()).unwrap().trim_start_matches('/').to_string()
            })
            .collect();
        assert_eq!(rel, vec!["p/proj", "p/repos/task", "w/wt"]);
    }

    #[test]
    fn discover_missing_root_is_empty() {
        let root = TempDir::new().unwrap();
        assert!(discover_git_repos(&[root.path().join("nope")], 4).is_empty());
    }

    #[test]
    fn concurrent_adds_from_two_stale_snapshots_dont_clobber_each_other() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("repos.json");
        save_repos(&path, &paths(&["/shared/base"])).unwrap();

        // Two instances both load the same starting snapshot...
        let snapshot_a = load_repos(&path);
        let snapshot_b = load_repos(&path);
        assert_eq!(snapshot_a, snapshot_b);

        // ...then each adds a different repo, straight to disk.
        let (merged_a, added_a) = add_repo_persisted(&path, "/repo/a").unwrap();
        assert!(added_a);
        assert_eq!(merged_a, paths(&["/shared/base", "/repo/a"]));

        let (merged_b, added_b) = add_repo_persisted(&path, "/repo/b").unwrap();
        assert!(added_b);
        // B's write must still see A's addition, not just its own stale snapshot.
        assert_eq!(merged_b, paths(&["/shared/base", "/repo/a", "/repo/b"]));

        assert_eq!(load_repos(&path), paths(&["/shared/base", "/repo/a", "/repo/b"]));
    }

    #[test]
    fn remove_persisted_preserves_concurrently_added_repo() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("repos.json");
        save_repos(&path, &paths(&["/repo/a", "/repo/b"])).unwrap();

        // Instance A removes /repo/a while, in between, instance B (unmodeled
        // here) has already persisted a fresh /repo/c addition.
        add_repo_persisted(&path, "/repo/c").unwrap();
        let (merged, removed) = remove_repo_persisted(&path, "/repo/a").unwrap();
        assert!(removed);
        assert_eq!(merged, paths(&["/repo/b", "/repo/c"]));
    }

    /// The bug this guards against: a checkout reached through a symlink gets
    /// tracked under the literal (symlinked) path, but `git worktree add`
    /// persists the realpath — so a worktree discovered via `git worktree
    /// list` and deleted from the rail carries a `dir` that never
    /// exact-matches the tracked entry, even though both name the same
    /// directory.
    #[test]
    #[cfg(unix)]
    fn aliases_for_finds_a_symlinked_tracked_entry_by_realpath() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let path = tmp.path().join("repos.json");
        let link_s = link.to_string_lossy().to_string();
        save_repos(&path, &paths(&[&link_s, "/kept/elsewhere"])).unwrap();

        // `dir` here is the realpath form — what git itself would report —
        // not the literal symlinked path tracked in repos.json.
        let aliases = aliases_for(&path, &real);
        assert_eq!(aliases, vec![link_s]);
    }

    /// No symlink involved: nothing to alias, and the literal string is
    /// excluded from its own alias list (the caller already matches it
    /// directly).
    #[test]
    fn aliases_for_is_empty_with_no_divergence() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        let path = tmp.path().join("repos.json");
        save_repos(&path, &paths(&[&dir.to_string_lossy()])).unwrap();

        assert!(aliases_for(&path, &dir).is_empty());
    }

    /// Called after the directory is already gone (the common case — this
    /// runs post-removal for most callers): `canonicalize` can't resolve
    /// anything, so it degrades to no aliases rather than guessing.
    #[test]
    fn aliases_for_is_empty_once_the_directory_is_gone() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("gone");
        let path = tmp.path().join("repos.json");
        save_repos(&path, &paths(&[&dir.to_string_lossy()])).unwrap();

        assert!(aliases_for(&path, &dir).is_empty());
    }

    #[test]
    fn load_missing_or_corrupt_is_empty() {
        let dir = TempDir::new().unwrap();
        assert!(load_repos(&dir.path().join("nope.json")).is_empty());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        assert!(load_repos(&bad).is_empty());
    }

    #[test]
    fn try_load_distinguishes_missing_from_torn_read() {
        let dir = TempDir::new().unwrap();
        // Missing file: legitimately no repos configured.
        assert_eq!(try_load_repos(&dir.path().join("nope.json")), Some(vec![]));
        // Valid file: the list.
        let path = dir.path().join("repos.json");
        save_repos(&path, &paths(&["/a/x"])).unwrap();
        assert_eq!(try_load_repos(&path), Some(paths(&["/a/x"])));
        // Existing-but-unparseable (a torn read of a concurrent write): None,
        // so callers keep their previous in-memory list instead of pruning
        // everything against an empty set (#75).
        std::fs::write(&path, r#"{"repoPaths":["/a/x"#).unwrap();
        assert_eq!(try_load_repos(&path), None);
        std::fs::write(&path, "").unwrap();
        assert_eq!(try_load_repos(&path), None);
    }

    #[test]
    fn resolve_session_matches_encoded_prefix_longest() {
        let entries = repo_entries(&paths(&["/home/u/proj", "/home/u/proj/sub"]));
        // Exact repo dir.
        assert_eq!(resolve_session_name("-home-u-proj", &entries).as_deref(), Some("proj"));
        // A project dir *under* proj/sub → longest match wins (proj/sub, named "sub").
        assert_eq!(
            resolve_session_name("-home-u-proj-sub-deeper", &entries).as_deref(),
            Some("sub")
        );
        // Unrelated dir → None.
        assert_eq!(resolve_session_name("-var-tmp-x", &entries), None);
    }

    #[test]
    fn resolve_real_absolute_path_against_repo_dirs() {
        let entries = repo_entries(&paths(&["/home/u/proj"]));
        // Exact real path (the pid→cwd fallback form).
        assert_eq!(resolve_session_name("/home/u/proj", &entries).as_deref(), Some("proj"));
        // A cwd under the repo.
        assert_eq!(resolve_session_name("/home/u/proj/src", &entries).as_deref(), Some("proj"));
        // Unrelated real path.
        assert_eq!(resolve_session_name("/var/tmp", &entries), None);
        // A sibling that only shares a prefix segment must not match.
        assert_eq!(resolve_session_name("/home/u/project-x", &entries), None);
    }

    #[test]
    fn resolve_handles_literal_dashes_via_encoded_match() {
        // A repo whose name contains a literal dash encodes unambiguously here.
        let entries = repo_entries(&paths(&["/home/u/my-proj"]));
        assert_eq!(resolve_session_name("-home-u-my-proj", &entries).as_deref(), Some("my-proj"));
    }

    #[test]
    fn resolve_repo_dir_matches_exact_and_subdir() {
        let entries = repo_entries(&paths(&["/home/u/proj"]));
        assert_eq!(resolve_repo_dir("/home/u/proj", &entries).as_deref(), Some("/home/u/proj"));
        // A session cwd nested under the repo still resolves to the repo's dir.
        assert_eq!(
            resolve_repo_dir("/home/u/proj/crates/sub", &entries).as_deref(),
            Some("/home/u/proj")
        );
        assert_eq!(resolve_repo_dir("/var/tmp", &entries), None);
    }

    #[test]
    fn find_repo_root_walks_up_to_git() {
        let root = TempDir::new().unwrap();
        let repo = root.path().join("p/proj");
        let sub = repo.join("crates/sub");
        std::fs::create_dir_all(sub.join("deeper")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        assert_eq!(find_repo_root(&sub.join("deeper")), repo);
        assert_eq!(find_repo_root(&repo), repo); // already the root
    }

    #[test]
    fn find_repo_root_handles_worktree_git_file() {
        let root = TempDir::new().unwrap();
        let wt = root.path().join("w/wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /elsewhere").unwrap();

        assert_eq!(find_repo_root(&wt), wt);
    }

    #[test]
    fn find_repo_root_falls_back_to_start_when_no_git_found() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("no/git/here");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(find_repo_root(&dir), dir);
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reorder_applies_the_requested_order() {
        assert_eq!(
            reorder_repos(&v(&["/a", "/b", "/c"]), &v(&["/c", "/a", "/b"])),
            v(&["/c", "/a", "/b"])
        );
    }

    #[test]
    fn reorder_keeps_repos_the_client_never_saw() {
        // Another window added /d after this client rendered its list. A drag
        // based on the older snapshot must not untrack it.
        assert_eq!(
            reorder_repos(&v(&["/a", "/b", "/d"]), &v(&["/b", "/a"])),
            v(&["/b", "/a", "/d"]),
            "an unmentioned repo keeps its relative position, after the ordered ones"
        );
    }

    #[test]
    fn reorder_ignores_repos_that_are_no_longer_tracked() {
        // Mirror image: another window untracked /b while this client dragged.
        assert_eq!(reorder_repos(&v(&["/a", "/c"]), &v(&["/b", "/c", "/a"])), v(&["/c", "/a"]));
    }

    #[test]
    fn reorder_tolerates_a_duplicated_dir_in_the_request() {
        assert_eq!(reorder_repos(&v(&["/a", "/b"]), &v(&["/b", "/b", "/a"])), v(&["/b", "/a"]));
    }

    #[test]
    fn reorder_persisted_writes_and_is_a_noop_when_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.json");
        save_repos(&path, &v(&["/a", "/b", "/c"])).unwrap();

        let out = reorder_repos_persisted(&path, &v(&["/c", "/b", "/a"])).unwrap();
        assert_eq!(out, v(&["/c", "/b", "/a"]));
        assert_eq!(load_repos(&path), v(&["/c", "/b", "/a"]), "order must survive the round-trip");

        // Re-applying the same order must not rewrite the file.
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let again = reorder_repos_persisted(&path, &v(&["/c", "/b", "/a"])).unwrap();
        assert_eq!(again, v(&["/c", "/b", "/a"]));
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn reorder_preserves_scan_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.json");
        save_repos(&path, &v(&["/a", "/b"])).unwrap();
        save_scan_roots(&path, &v(&["~/code"])).unwrap();

        reorder_repos_persisted(&path, &v(&["/b", "/a"])).unwrap();
        assert_eq!(load_scan_roots(&path), v(&["~/code"]), "reordering must not drop scanRoots");
    }
}
