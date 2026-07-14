//! Branch / worktree / diff-stat computation with a short cache. Ports slot-1
//! `runtime/server/git-info.ts`.
//!
//! What ports here: the git shell-outs, the porcelain/numstat/ahead-behind
//! parsing, and the 5s-TTL cache with stale-serve + explicit invalidation. What
//! does **not** port (transport/watcher concerns, left to the Tauri layer): the
//! `setInterval` git poll (`startGitPoll`/`poll.ts`), the `fs.watch` on
//! `.git/HEAD` (`syncGitWatchers`), and the WS broadcast.
//!
//! Time is injected via `now_ms` instead of a background clock. Cache misses
//! compute synchronously here rather than TS's async background refresh +
//! in-flight de-dup (deviation noted in the port report).

use std::collections::HashMap;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Working-tree/commit stats for a session directory. Ports `GitInfo`.
///
/// `files_changed`/`lines_added`/`lines_removed` measure the working tree against
/// the pushed baseline (merge-base with upstream, else origin/main).
/// `commits_ahead`/`commits_behind` use a different baseline (distance from
/// origin/main), so the two can disagree on a feature branch tracking its own
/// remote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: String,
    pub is_worktree: bool,
    pub files_changed: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    /// Commits on HEAD that origin/main doesn't have.
    pub commits_ahead: i64,
    /// Commits on origin/main that HEAD doesn't have. Kept separate from
    /// `commits_ahead` (not a signed delta) so "3 ahead, 2 behind" doesn't
    /// collapse to a meaningless "+1".
    pub commits_behind: i64,
    /// `git remote get-url origin`, if the checkout has an origin remote. Used to
    /// group folders (checkouts) of the same logical repo in the Folder Rail.
    pub origin_url: Option<String>,
    /// Absolute paths of this repo's OTHER `git worktree` checkouts (this dir
    /// excluded), from `git worktree list`. Not part of the wire payload — the
    /// engine uses it to auto-discover worktrees that aren't in `repoPaths` yet.
    pub worktree_dirs: Vec<String>,
    /// True when `dir` doesn't exist on disk (a tracked repo whose checkout was
    /// moved or deleted). Distinguishes a genuinely-missing directory from a
    /// present-but-non-git one — both otherwise yield an empty [`GitInfo`].
    /// [`crate::bridge::build_folder`] copies this onto the wire `FolderData`.
    pub dir_missing: bool,
}

/// Must stay above the git poll interval so the poll keeps entries warm.
const GIT_CACHE_TTL_MS: i64 = 5000;

/// Cache of git info per directory, with a 5s freshness window and stale-serve.
/// Ports the module-global `gitInfoCache` as an owned struct. The poll loop that
/// drives `refresh` on an interval lives in the Tauri layer, not here.
#[derive(Debug, Default)]
pub struct GitInfoCache {
    entries: HashMap<String, (GitInfo, i64)>,
}

impl GitInfoCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert/replace an entry stamped at `now_ms` (used by tests and `refresh`).
    pub fn insert(&mut self, dir: &str, info: GitInfo, now_ms: i64) {
        self.entries.insert(dir.to_string(), (info, now_ms));
    }

    /// Whether the entry for `dir` exists and is within the TTL.
    pub fn is_fresh(&self, dir: &str, now_ms: i64) -> bool {
        self.entries.get(dir).is_some_and(|(_, ts)| now_ms - ts < GIT_CACHE_TTL_MS)
    }

    /// Synchronous cache-only read: returns the cached info (fresh or stale), or
    /// empty when nothing is cached. Ports `getGitInfo`'s serve-stale behavior
    /// (without the background refresh — that's the poll's job via [`Self::refresh`]).
    pub fn get(&self, dir: &str) -> GitInfo {
        if dir.is_empty() {
            return GitInfo::default();
        }
        self.entries.get(dir).map(|(info, _)| info.clone()).unwrap_or_default()
    }

    /// Mark entries stale (ts=0) so the next read still serves them but they're no
    /// longer fresh. Ports `invalidateGitCache`.
    pub fn invalidate(&mut self, dir: Option<&str>) {
        match dir {
            Some(dir) => {
                if let Some(entry) = self.entries.get_mut(dir) {
                    entry.1 = 0;
                }
            }
            None => {
                for entry in self.entries.values_mut() {
                    entry.1 = 0;
                }
            }
        }
    }

    /// Recompute git info for `dir` (shells out), cache it at `now_ms`, and return
    /// it. Ports `refreshGitInfo` (synchronous, no in-flight de-dup).
    pub fn refresh(&mut self, dir: &str, now_ms: i64) -> GitInfo {
        let info = compute_git_info(dir);
        self.insert(dir, info.clone(), now_ms);
        info
    }

    /// Return fresh cached info if available, else recompute. Convenience wrapper.
    pub fn get_or_refresh(&mut self, dir: &str, now_ms: i64) -> GitInfo {
        if self.is_fresh(dir, now_ms) {
            return self.get(dir);
        }
        self.refresh(dir, now_ms)
    }
}

/// Run `git -C <dir> <args...>` and return trimmed stdout, or "" on any failure
/// — including a timeout: a repo on a stale network mount must degrade to
/// empty stats, not hang its caller. Mirrors the TS `shellAsync` (ignores
/// stderr and exit code; empty on error).
fn git_out(dir: &str, args: &[&str]) -> String {
    let mut full: Vec<&str> = vec!["-C", dir];
    full.extend_from_slice(args);
    match tt_exec::run_with_timeout("git", &full, std::time::Duration::from_secs(5)) {
        Ok(out) => out.stdout.trim().to_string(),
        Err(_) => String::new(),
    }
}

/// Compute git info for `dir` by shelling out. Ports `computeGitInfo`. Returns
/// empty [`GitInfo`] when the directory isn't a git repo. This is the thin
/// subprocess layer — the parsing it delegates to is unit-tested separately.
pub fn compute_git_info(dir: &str) -> GitInfo {
    if dir.is_empty() {
        return GitInfo::default();
    }
    // A tracked checkout that was moved or deleted: flag it so the rail can show
    // it as a ghost rather than a silent empty-stats folder.
    if !std::path::Path::new(dir).is_dir() {
        return GitInfo { dir_missing: true, ..Default::default() };
    }
    let branch = git_out(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if branch.is_empty() {
        return GitInfo::default();
    }
    let git_dir = git_out(dir, &["rev-parse", "--git-dir"]);
    let status_out = git_out(dir, &["status", "--porcelain"]);
    let origin_main = resolve_origin_main(dir);
    let base = resolve_pushed_base(dir, &origin_main);
    let diff_out = git_out(dir, &["diff", "--numstat", &base]);
    let ahead_behind = git_out(
        dir,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{origin_main}...HEAD"),
        ],
    );

    let mut info =
        compute_git_info_from_outputs(&branch, &git_dir, &status_out, &diff_out, &ahead_behind);
    let origin_url = git_out(dir, &["remote", "get-url", "origin"]);
    info.origin_url = (!origin_url.is_empty()).then_some(origin_url);
    info.worktree_dirs = list_other_worktrees(dir);
    info
}

/// Fetch `origin` for each distinct repo among `dirs`, deduped by common git
/// dir so N worktrees of the same repo (the common slot pattern) trigger one
/// network call, not N. Network I/O, so a longer timeout than [`git_out`]'s
/// 5s; failures (offline, no origin, auth prompt) are swallowed the same
/// way — this only refreshes the `origin/main` ref that [`compute_git_info`]
/// reads, it never surfaces errors to the user.
pub fn fetch_all(dirs: &[String]) {
    let mut seen = HashSet::new();
    for dir in dirs {
        let key = git_common_dir(dir);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        fetch_origin(dir);
    }
}

/// `git fetch --quiet origin`, ignoring the outcome — best-effort refresh of
/// the local `origin/main` remote-tracking ref.
fn fetch_origin(dir: &str) {
    let full = ["-C", dir, "fetch", "--quiet", "origin"];
    let _ = tt_exec::run_with_timeout("git", &full, std::time::Duration::from_secs(20));
}

/// Absolute path to the repo's shared `.git` dir (same for every worktree of
/// one repo), used to dedup fetches. Empty for a non-repo dir.
fn git_common_dir(dir: &str) -> String {
    let raw = git_out(dir, &["rev-parse", "--git-common-dir"]);
    if raw.is_empty() {
        return String::new();
    }
    let path = std::path::Path::new(&raw);
    let abs =
        if path.is_absolute() { path.to_path_buf() } else { std::path::Path::new(dir).join(path) };
    std::fs::canonicalize(&abs).unwrap_or(abs).to_string_lossy().into_owned()
}

/// This repo's OTHER `git worktree` checkouts (`dir` itself excluded). Empty
/// for a plain clone (no linked worktrees) or a non-repo dir.
fn list_other_worktrees(dir: &str) -> Vec<String> {
    let out = git_out(dir, &["worktree", "list", "--porcelain"]);
    parse_worktree_list(&out).into_iter().filter(|w| w != dir).collect()
}

/// Parse `git worktree list --porcelain` into the absolute path of each
/// worktree (main + linked). Pure — unit-tested on fixture output.
fn parse_worktree_list(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(str::to_string)
        .collect()
}

/// origin/main, or origin/master if that's what the remote uses. Ports `resolveOriginMain`.
fn resolve_origin_main(dir: &str) -> String {
    let verified = git_out(dir, &["rev-parse", "--verify", "--quiet", "origin/main"]);
    if verified.is_empty() { "origin/master".to_string() } else { "origin/main".to_string() }
}

/// The ref the diff pane's "vs main" mode compares against, highest priority
/// first:
///
/// 1. `base_branch` — a per-folder override for a long-running branch that
///    didn't fork from main, set via
///    [`crate::folder_meta::FolderMetaStore::set_base_branch`].
/// 2. The worktree slot's own `.tt-slot` marker `base=` field (see
///    [`tt_slots::read_slot_base`]) — the ref the slot was actually created
///    from, which may not be main. Not present for a non-slot checkout.
/// 3. The origin/main-or-master auto-detect.
///
/// Whichever name wins resolves to `origin/<name>`, never the local branch:
/// both the local ref and its origin remote-tracking ref may have moved since
/// the slot was created, and the diff pane wants the current pushed baseline,
/// matching [`resolve_origin_main`]. Falls back to the local branch only when
/// no `origin/<name>` ref exists at all (e.g. a base branch never pushed).
fn resolve_base_ref(dir: &str, base_branch: Option<&str>) -> String {
    let candidates = [base_branch.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string)]
        .into_iter()
        .flatten()
        .chain(tt_slots::read_slot_base(std::path::Path::new(dir)));
    for name in candidates {
        let name = name.trim_start_matches("origin/");
        let remote = format!("origin/{name}");
        if !git_out(dir, &["rev-parse", "--verify", "--quiet", &remote]).is_empty() {
            return remote;
        }
        if !git_out(dir, &["rev-parse", "--verify", "--quiet", name]).is_empty() {
            return name.to_string();
        }
    }
    resolve_origin_main(dir)
}

/// The commit HEAD diverged from: merge-base with upstream if set, else with
/// origin/main, else HEAD. Ports `resolvePushedBase`.
fn resolve_pushed_base(dir: &str, origin_main: &str) -> String {
    let upstream = git_out(
        dir,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    let base_ref = if upstream.is_empty() { origin_main } else { &upstream };
    let merge_base = git_out(dir, &["merge-base", "HEAD", base_ref]);
    if merge_base.is_empty() { "HEAD".to_string() } else { merge_base }
}

/// Pure assembly of [`GitInfo`] from raw git command outputs. Unit-tested on
/// fixture strings. Ports the parsing half of `computeGitInfo`.
pub fn compute_git_info_from_outputs(
    branch: &str,
    git_dir: &str,
    status_out: &str,
    diff_out: &str,
    ahead_behind: &str,
) -> GitInfo {
    let (lines_added, lines_removed, changed_files) = parse_numstat(diff_out);

    // Untracked files aren't in the diff but still count as changed.
    let untracked = status_out.lines().filter(|l| l.starts_with("??")).count() as i64;
    let files_changed = changed_files.len() as i64 + untracked;

    let (commits_ahead, commits_behind) = parse_ahead_behind(ahead_behind);
    GitInfo {
        branch: branch.to_string(),
        is_worktree: git_dir.contains("/worktrees/"),
        files_changed,
        lines_added,
        lines_removed,
        commits_ahead,
        commits_behind,
        // The pure parser has no origin/worktree-list knowledge; `compute_git_info`
        // fills both in. Existence is decided before shelling out, so a parsed
        // result is never "missing".
        origin_url: None,
        worktree_dirs: Vec::new(),
        dir_missing: false,
    }
}

/// What baseline the diff pane compares the working tree against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    /// Everything on this branch vs where it forked from origin/main
    /// (merge-base) — committed and uncommitted work alike.
    Main,
    /// Only what isn't committed yet: `git diff HEAD` (staged + unstaged).
    Uncommitted,
}

/// Full unified diff for the diff pane, baseline picked by `mode`. `base_branch`
/// overrides `DiffMode::Main`'s comparison ref (see [`resolve_base_ref`]);
/// ignored for `DiffMode::Uncommitted`. Untracked files don't appear in `git
/// diff`, so they're listed by name in a trailing block rather than silently
/// dropped. Empty string when `dir` isn't a git repo or has no changes.
pub fn diff_patch(dir: &str, mode: DiffMode, base_branch: Option<&str>) -> String {
    if dir.is_empty() {
        return String::new();
    }
    let base = match mode {
        DiffMode::Main => {
            let base_ref = resolve_base_ref(dir, base_branch);
            let merge_base = git_out(dir, &["merge-base", "HEAD", &base_ref]);
            if merge_base.is_empty() { "HEAD".to_string() } else { merge_base }
        }
        DiffMode::Uncommitted => "HEAD".to_string(),
    };
    let mut patch = git_out(dir, &["diff", &base]);

    let status_out = git_out(dir, &["status", "--porcelain"]);
    let untracked: Vec<&str> =
        status_out.lines().filter(|l| l.starts_with("??")).map(|l| l[2..].trim()).collect();
    if !untracked.is_empty() {
        if !patch.is_empty() {
            patch.push_str("\n\n");
        }
        patch.push_str("# Untracked files (not shown):\n");
        for f in untracked {
            patch.push_str("?? ");
            patch.push_str(f);
            patch.push('\n');
        }
    }
    patch
}

/// Parse `git diff --numstat` output into (added, removed, changed file set).
/// Binary files (`-`/`-`) contribute to the file set but not line counts.
fn parse_numstat(diff_out: &str) -> (i64, i64, HashSet<String>) {
    let mut lines_added = 0;
    let mut lines_removed = 0;
    let mut changed_files: HashSet<String> = HashSet::new();
    for line in diff_out.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let added = parts.next().unwrap_or("");
        let removed = parts.next().unwrap_or("");
        let file = parts.next().unwrap_or("");
        if !file.is_empty() {
            changed_files.insert(file.to_string());
        }
        if added == "-" || removed == "-" {
            continue; // binary
        }
        lines_added += added.parse::<i64>().unwrap_or(0);
        lines_removed += removed.parse::<i64>().unwrap_or(0);
    }
    (lines_added, lines_removed, changed_files)
}

/// Parse `git rev-list --left-right --count <origin>...HEAD` ("behind\tahead")
/// into `(ahead, behind)` counts vs origin/main.
fn parse_ahead_behind(ahead_behind: &str) -> (i64, i64) {
    if ahead_behind.is_empty() {
        return (0, 0);
    }
    let mut parts = ahead_behind.split('\t');
    let behind = parts.next().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    let ahead = parts.next().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    (ahead, behind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_sums_lines_and_collects_files_skipping_binary() {
        let diff = "10\t2\tsrc/a.rs\n5\t0\tsrc/b.rs\n-\t-\tassets/logo.png\n";
        let info = compute_git_info_from_outputs("main", "/repo/.git", "", diff, "");
        assert_eq!(info.lines_added, 15);
        assert_eq!(info.lines_removed, 2);
        // 3 distinct files (including the binary one), no untracked.
        assert_eq!(info.files_changed, 3);
    }

    #[test]
    fn untracked_files_counted_from_porcelain() {
        let status = "?? new1.txt\n?? new2.txt\n M tracked.rs\n";
        let diff = "1\t1\ttracked.rs\n";
        let info = compute_git_info_from_outputs("main", "/repo/.git", status, diff, "");
        // 1 changed file (tracked.rs) + 2 untracked.
        assert_eq!(info.files_changed, 3);
    }

    #[test]
    fn worktree_detected_from_git_dir() {
        let info = compute_git_info_from_outputs("feat", "/repo/.git/worktrees/feat", "", "", "");
        assert!(info.is_worktree);
        let info2 = compute_git_info_from_outputs("main", "/repo/.git", "", "", "");
        assert!(!info2.is_worktree);
    }

    #[test]
    fn ahead_behind_parsed_as_separate_counts() {
        // "behind\tahead" → (ahead, behind)
        assert_eq!(parse_ahead_behind("0\t3"), (3, 0));
        assert_eq!(parse_ahead_behind("2\t0"), (0, 2));
        assert_eq!(parse_ahead_behind("1\t4"), (4, 1));
        assert_eq!(parse_ahead_behind(""), (0, 0));
    }

    #[test]
    fn branch_and_ahead_behind_flow_through() {
        let info = compute_git_info_from_outputs("feature/x", "/repo/.git", "", "", "2\t5");
        assert_eq!(info.branch, "feature/x");
        assert_eq!(info.commits_ahead, 5);
        assert_eq!(info.commits_behind, 2);
    }

    #[test]
    fn cache_fresh_stale_and_invalidate() {
        let mut cache = GitInfoCache::new();
        let info = GitInfo { branch: "main".into(), ..Default::default() };
        // Use epoch-scale timestamps: invalidate() zeroes the stamp, which only
        // reads as stale when `now_ms` is a real epoch (≫ TTL), matching TS.
        let t0 = 1_700_000_000_000;
        cache.insert("/repo", info.clone(), t0);
        assert!(cache.is_fresh("/repo", t0));
        assert!(cache.is_fresh("/repo", t0 + 4999)); // < 5000ms later
        assert!(!cache.is_fresh("/repo", t0 + 5000)); // exactly TTL later → stale
        // Stale entries still serve.
        assert_eq!(cache.get("/repo"), info);
        // Invalidate forces stale immediately (stamp → 0).
        cache.invalidate(Some("/repo"));
        assert!(!cache.is_fresh("/repo", t0));
        assert_eq!(cache.get("/repo"), info); // still served
    }

    #[test]
    fn cache_get_empty_for_unknown_or_blank_dir() {
        let cache = GitInfoCache::new();
        assert_eq!(cache.get("/nope"), GitInfo::default());
        assert_eq!(cache.get(""), GitInfo::default());
    }

    #[test]
    fn get_or_refresh_returns_fresh_without_recompute() {
        let mut cache = GitInfoCache::new();
        let info = GitInfo { branch: "cached".into(), ..Default::default() };
        cache.insert("/repo", info.clone(), 1000);
        // Fresh → returns cached value without shelling out to git.
        assert_eq!(cache.get_or_refresh("/repo", 2000), info);
    }

    #[test]
    fn worktree_list_parses_each_entry_path() {
        let porcelain = "worktree /repo/main\nHEAD abc\nbranch refs/heads/main\n\n\
            worktree /repo/.claude/worktrees/feat\nHEAD def\nbranch refs/heads/feat\n";
        assert_eq!(
            parse_worktree_list(porcelain),
            vec!["/repo/main", "/repo/.claude/worktrees/feat"],
        );
    }

    #[test]
    fn worktree_list_empty_for_plain_clone_or_blank_output() {
        assert_eq!(parse_worktree_list(""), Vec::<String>::new());
    }

    #[test]
    fn git_common_dir_matches_across_worktrees_of_one_repo() {
        let root = tempfile::TempDir::new().unwrap();
        let main = root.path().join("main");
        std::fs::create_dir(&main).unwrap();
        let run = |dir: &std::path::Path, args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&main, &["init", "--quiet", "-b", "main"]);
        run(&main, &["config", "user.email", "test@example.com"]);
        run(&main, &["config", "user.name", "Test"]);
        std::fs::write(main.join("f.txt"), "1").unwrap();
        run(&main, &["add", "f.txt"]);
        run(&main, &["commit", "--quiet", "-m", "init"]);
        let linked = root.path().join("linked");
        run(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feat",
                linked.to_str().unwrap(),
            ],
        );

        let main_key = git_common_dir(main.to_str().unwrap());
        let linked_key = git_common_dir(linked.to_str().unwrap());
        assert!(!main_key.is_empty());
        assert_eq!(main_key, linked_key);
    }

    #[test]
    fn resolve_base_ref_prefers_a_verified_override_over_the_main_default() {
        let root = tempfile::TempDir::new().unwrap();
        let repo = root.path();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "--quiet", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "1").unwrap();
        run(&["add", "f.txt"]);
        run(&["commit", "--quiet", "-m", "init"]);
        run(&["branch", "develop"]);

        let dir = repo.to_str().unwrap();
        // A local branch with no matching remote ref: the override resolves
        // directly to the local branch name.
        assert_eq!(resolve_base_ref(dir, Some("develop")), "develop");
        // A leading "origin/" on the override is stripped before re-adding it,
        // so passing either form of the same branch resolves identically.
        assert_eq!(resolve_base_ref(dir, Some("origin/develop")), "develop");
        // An override that resolves to nothing (no such branch, no remote)
        // falls back to the origin/main-or-master auto-detect.
        assert_eq!(resolve_base_ref(dir, Some("no-such-branch")), resolve_origin_main(dir));
        // No override at all: same auto-detect.
        assert_eq!(resolve_base_ref(dir, None), resolve_origin_main(dir));
    }

    #[test]
    fn resolve_base_ref_uses_the_slots_own_creation_base_over_main() {
        let root = tempfile::TempDir::new().unwrap();
        let repo = root.path();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "--quiet", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "1").unwrap();
        run(&["add", "f.txt"]);
        run(&["commit", "--quiet", "-m", "init"]);
        // A local "origin/develop" remote-tracking ref so resolve_base_ref
        // has something to prefer over the local "develop" branch.
        run(&["branch", "develop"]);
        run(&["update-ref", "refs/remotes/origin/develop", "develop"]);
        run(&["update-ref", "refs/remotes/origin/main", "main"]);

        std::fs::write(
            repo.join(tt_slots::MARKER_FILE),
            tt_slots::marker_contents("slot-name", "develop", "main"),
        )
        .unwrap();

        let dir = repo.to_str().unwrap();
        // No explicit override: the slot's own marker base wins over the
        // origin/main auto-detect, and resolves to the origin remote copy.
        assert_eq!(resolve_base_ref(dir, None), "origin/develop");
        // An explicit per-folder override still takes priority over the
        // slot's recorded creation base.
        run(&["branch", "release"]);
        run(&["update-ref", "refs/remotes/origin/release", "release"]);
        assert_eq!(resolve_base_ref(dir, Some("release")), "origin/release");
    }

    #[test]
    fn git_common_dir_empty_for_non_repo() {
        let root = tempfile::TempDir::new().unwrap();
        assert_eq!(git_common_dir(root.path().to_str().unwrap()), "");
    }

    #[test]
    fn compute_flags_a_missing_dir() {
        let root = tempfile::TempDir::new().unwrap();
        let gone = root.path().join("moved-away");
        let info = compute_git_info(gone.to_str().unwrap());
        assert!(info.dir_missing);
        assert!(info.branch.is_empty());
    }

    #[test]
    fn compute_does_not_flag_an_existing_dir() {
        let root = tempfile::TempDir::new().unwrap();
        // Present but not a git repo: still not "missing".
        let info = compute_git_info(root.path().to_str().unwrap());
        assert!(!info.dir_missing);
    }

    #[test]
    fn from_outputs_never_flags_missing() {
        // The pure parser only sees git command strings; existence is decided
        // by `compute_git_info` before any shell-out.
        let info = compute_git_info_from_outputs("main", "/repo/.git", "", "", "");
        assert!(!info.dir_missing);
    }
}
