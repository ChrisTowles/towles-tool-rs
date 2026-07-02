//! Watched-repo configuration (agentboard phase 4). The desktop app's session
//! source: a list of absolute repo paths persisted to its OWN file,
//! `~/.config/towles-tool/agentboard/repos.json`.
//!
//! Deliberately NOT stored in the shared `towles-tool.settings.json`: the TS
//! CLI's zod parse/save round-trip could strip keys it doesn't know, and this
//! sits beside `session-order.json` which already established the per-file
//! pattern. Path-parameterized so tests use a tempdir.

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

/// Load the repo-path list. Empty on missing/corrupt file. Ports the loader half.
pub fn load_repos(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<ReposConfig>(&text).map(|c| c.repo_paths).unwrap_or_default()
}

/// Persist the repo-path list as `{"repoPaths":[...]}` (pretty + trailing newline).
pub fn save_repos(path: &Path, repo_paths: &[String]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let config = ReposConfig { repo_paths: repo_paths.to_vec() };
    let json = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, format!("{json}\n"))
}

/// Add `path` if not already present. Returns whether it was added.
pub fn add_repo(repo_paths: &mut Vec<String>, path: &str) -> bool {
    if repo_paths.iter().any(|p| p == path) {
        return false;
    }
    repo_paths.push(path.to_string());
    true
}

/// Remove the repo whose resolved session `name` matches. Returns whether removed.
pub fn remove_repo_by_name(repo_paths: &mut Vec<String>, name: &str) -> bool {
    let Some(entry) = repo_entries(repo_paths).into_iter().find(|e| e.name == name) else {
        return false;
    };
    let before = repo_paths.len();
    repo_paths.retain(|p| p != &entry.dir);
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
    fn remove_repo_by_name_removes_matching_dir() {
        let mut p = paths(&["/work/a/web", "/work/b/web"]);
        assert!(remove_repo_by_name(&mut p, "a/web"));
        assert_eq!(p, paths(&["/work/b/web"]));
        assert!(!remove_repo_by_name(&mut p, "nope"));
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
    fn load_missing_or_corrupt_is_empty() {
        let dir = TempDir::new().unwrap();
        assert!(load_repos(&dir.path().join("nope.json")).is_empty());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        assert!(load_repos(&bad).is_empty());
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
