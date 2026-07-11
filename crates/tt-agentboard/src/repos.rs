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

/// Default location: `~/.config/towles-tool/agentboard/repos.json`.
pub fn default_repos_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("towles-tool")
        .join("agentboard")
        .join("repos.json")
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
/// on disk is preserved.
pub fn save_repos(path: &Path, repo_paths: &[String]) -> std::io::Result<()> {
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
/// may have gone stale. Multiple Agentboard windows (one per `tt:parallel-slots`
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
/// whether `dir` was actually present to remove.
pub fn remove_repo_persisted(path: &Path, dir: &str) -> std::io::Result<(Vec<String>, bool)> {
    let mut config = load_config(path);
    let removed = remove_repo_by_dir(&mut config.repo_paths, dir);
    save_config(path, &config)?;
    Ok((config.repo_paths, removed))
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

/// Resolve a watcher's project dir to a session name (longest prefix match).
///
/// Handles both forms the watchers produce: claude-code passes Claude's *encoded*
/// folder name (`/`→`-`, adopted fix #3 — matched encoded↔encoded to sidestep the
/// lossy decode); amp/codex/opencode pass a real absolute path (matched directly
/// against repo dirs). An input starting with `/` is treated as a real path.
pub fn resolve_session_name(dir: &str, entries: &[RepoEntry]) -> Option<String> {
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
    best.map(|(entry, _)| entry.name.clone())
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
        std::fs::create_dir_all(base.join("p/repos/slot/.git")).unwrap();
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
        assert_eq!(rel, vec!["p/proj", "p/repos/slot", "w/wt"]);
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
    fn resolve_real_path_for_amp_codex_opencode() {
        let entries = repo_entries(&paths(&["/home/u/proj"]));
        // Exact real path (amp uri / codex cwd / opencode directory).
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
}
