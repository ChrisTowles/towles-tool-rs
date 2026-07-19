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
/// `files_changed`/`lines_added`/`lines_removed` and `commits_ahead`/
/// `commits_behind` all measure against the *same* baseline: `compared_base`
/// (see [`resolve_base_ref`]) — a per-folder override, else a worktree slot's
/// own creation base, else origin/main-or-master. They agree by construction,
/// unlike the old design where the two used different baselines (a branch's
/// own upstream vs. always origin/main) and could silently disagree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: String,
    pub is_worktree: bool,
    pub files_changed: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    /// Commits on HEAD that `compared_base` doesn't have.
    pub commits_ahead: i64,
    /// Commits on `compared_base` that HEAD doesn't have. Kept separate from
    /// `commits_ahead` (not a signed delta) so "3 ahead, 2 behind" doesn't
    /// collapse to a meaningless "+1".
    pub commits_behind: i64,
    /// True when `git status --porcelain` reports anything (staged, unstaged,
    /// or untracked) — an actual dirty working tree. Distinct from
    /// `files_changed`/`lines_added`/`lines_removed`, which measure the
    /// branch's whole *committed* diff vs `compared_base` and stay nonzero
    /// for any real feature branch even once it's merged — those answer "what
    /// does this branch contain", not "is anything uncommitted".
    pub dirty: bool,
    /// Of `commits_ahead`, how many aren't yet patch-equivalent to a commit
    /// already on `compared_base` (`git cherry`, which compares diffs rather
    /// than commit SHAs). 0 whenever `commits_ahead` is 0, but — unlike
    /// `commits_ahead` — it *also* drops to 0 once a rebase or squash merge
    /// has landed this branch's changes upstream, even though the landed
    /// commits carry brand-new SHAs that never become reachable from this
    /// branch's HEAD. `commits_ahead` alone can never reach 0 in that case,
    /// which is exactly why it can't answer "has everything landed".
    pub commits_unlanded: i64,
    /// `git remote get-url origin`, if the checkout has an origin remote.
    /// Display-only (repo name derivation) — NOT the Folder Rail nesting key;
    /// two unrelated clones can share an origin without being linked worktrees
    /// of each other. See [`Self::common_dir`] for that.
    pub origin_url: Option<String>,
    /// Absolute path to `git rev-parse --git-common-dir` (canonicalized), empty
    /// for a non-repo dir or before this folder's git info has ever been
    /// computed. Identical across every linked `git worktree` of one repo
    /// (main + slots) and nowhere else — this is what
    /// [`crate::bridge::assemble_state`] groups [`crate::types::FolderData`]s
    /// into one [`crate::types::RepoData`] by, regardless of whether each
    /// checkout is separately tracked in `repos.json` or only discovered via
    /// `git worktree list` — so only *actual* worktrees of one repo nest
    /// together, never merely folders that happen to share an origin remote.
    pub common_dir: String,
    /// Absolute paths of this repo's OTHER `git worktree` checkouts (this dir
    /// excluded), from `git worktree list`. Not part of the wire payload — the
    /// engine uses it to auto-discover worktrees that aren't in `repoPaths` yet.
    pub worktree_dirs: Vec<String>,
    /// True when `dir` doesn't exist on disk (a tracked repo whose checkout was
    /// moved or deleted). Distinguishes a genuinely-missing directory from a
    /// present-but-non-git one — both otherwise yield an empty [`GitInfo`].
    /// [`crate::bridge::build_folder`] copies this onto the wire `FolderData`.
    pub dir_missing: bool,
    /// For a worktree slot only: the ref it was actually created from, read
    /// from its `.tt-slot` marker (see [`tt_slots::read_slot_base`]). `None`
    /// for a non-slot checkout. Lets the diff pane (and [`resolve_base_ref`])
    /// know what to auto-compare against without the user typing an override.
    pub slot_base_branch: Option<String>,
    /// The ref every stat on this struct (`files_changed`, `commits_ahead`,
    /// …) was actually compared against — [`resolve_base_ref`]'s result, e.g.
    /// `"origin/main"` or `"origin/docs/readme-slot-clean"`. Lets the Folder
    /// Rail label its stats with what they mean instead of always implying
    /// "vs main". Empty when `compute_git_info` never ran (default/missing).
    pub compared_base: String,
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
    /// it. Ports `refreshGitInfo` (synchronous, no in-flight de-dup). No
    /// per-folder base-branch override — callers that have one (the app's
    /// git-stat poll) call [`compute_git_info`] directly instead.
    pub fn refresh(&mut self, dir: &str, now_ms: i64) -> GitInfo {
        let info = compute_git_info(dir, None);
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

/// Prefix `args` with `-C <dir>` for a `git` invocation — shared by every
/// shell-out in this module.
fn git_args<'a>(dir: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut full = vec!["-C", dir];
    full.extend_from_slice(args);
    full
}

/// Run `git -C <dir> <args...>` and return trimmed stdout, or "" on any failure
/// — including a timeout: a repo on a stale network mount must degrade to
/// empty stats, not hang its caller. Mirrors the TS `shellAsync` (ignores
/// stderr and exit code; empty on error).
fn git_out(dir: &str, args: &[&str]) -> String {
    match tt_exec::run_with_timeout("git", &git_args(dir, args), std::time::Duration::from_secs(5))
    {
        Ok(out) => out.stdout.trim().to_string(),
        Err(_) => String::new(),
    }
}

/// Compute git info for `dir` by shelling out. Ports `computeGitInfo`. Returns
/// empty [`GitInfo`] when the directory isn't a git repo. This is the thin
/// subprocess layer — the parsing it delegates to is unit-tested separately.
///
/// `base_branch_override` is the folder's manual "vs main" override (see
/// [`resolve_base_ref`]), threaded in by the caller from
/// `FolderMetaStore::base_branch_for` — `compute_git_info` has no store
/// access of its own, only `dir`.
pub fn compute_git_info(dir: &str, base_branch_override: Option<&str>) -> GitInfo {
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
    let compared_base = resolve_base_ref(dir, base_branch_override);
    let merge_base = git_out(dir, &["merge-base", "HEAD", &compared_base]);
    let base = if merge_base.is_empty() { "HEAD".to_string() } else { merge_base };
    let diff_out = git_out(dir, &["diff", "--numstat", &base]);
    let ahead_behind = git_out(
        dir,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{compared_base}...HEAD"),
        ],
    );

    let mut info =
        compute_git_info_from_outputs(&branch, &git_dir, &status_out, &diff_out, &ahead_behind);
    let origin_url = git_out(dir, &["remote", "get-url", "origin"]);
    info.origin_url = (!origin_url.is_empty()).then_some(origin_url);
    info.common_dir = git_common_dir(dir);
    info.worktree_dirs = list_other_worktrees(dir);
    info.slot_base_branch = tt_slots::read_slot_base(std::path::Path::new(dir));
    // Only worth the extra shell-out once there's something to check —
    // nothing ahead trivially means nothing unlanded.
    info.commits_unlanded = if info.commits_ahead > 0 {
        parse_cherry_unlanded(&git_out(dir, &["cherry", &compared_base, "HEAD"]))
    } else {
        0
    };
    info.compared_base = compared_base;
    info
}

/// Count commits `git cherry <upstream> <head>` reports as NOT yet
/// patch-equivalent to anything on `<upstream>` (`+`-prefixed lines; a `-`
/// prefix means that commit's diff already exists there, however it got
/// there). Pure — unit-tested on fixture output; the shell-out lives in
/// [`compute_git_info`].
fn parse_cherry_unlanded(cherry_out: &str) -> i64 {
    cherry_out.lines().filter(|l| l.starts_with("+ ")).count() as i64
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

/// This repo's OTHER `git worktree` checkouts (`dir` itself excluded), MINUS
/// any that aren't a `tt slot`-managed worktree ([`tt_slots::is_managed_slot`])
/// — a worktree Claude Code created via an unwired `WorktreeCreate` hook, or
/// one added by hand outside the slot convention, must not auto-populate the
/// Folder Rail unprompted. This is the only place auto-discovered worktree
/// paths enter the engine's discovery pipeline
/// ([`crate::engine::Engine::expand_with_worktrees`] reads nothing else), so
/// filtering here is sufficient — no downstream code needs to know about
/// unmanaged worktrees at all. A directory the user explicitly tracks (in
/// `repos.json`) is unaffected: this fn only prunes the auto-discovery
/// candidate list, never the user's own configured paths. Empty for a plain
/// clone (no linked worktrees) or a non-repo dir.
fn list_other_worktrees(dir: &str) -> Vec<String> {
    let out = git_out(dir, &["worktree", "list", "--porcelain"]);
    parse_worktree_list(&out)
        .into_iter()
        .filter(|w| w != dir && tt_slots::is_managed_slot(std::path::Path::new(w)))
        .collect()
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

/// The ref every "vs main" comparison compares against — the diff pane's
/// `DiffMode::Main` *and* [`compute_git_info`]'s `files_changed`/
/// `commits_ahead`/etc. stats, so the Folder Rail's numbers always match what
/// the diff pane actually shows. Highest priority first:
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
    let dirty = status_out.lines().any(|l| !l.trim().is_empty());

    let (commits_ahead, commits_behind) = parse_ahead_behind(ahead_behind);
    GitInfo {
        branch: branch.to_string(),
        is_worktree: git_dir.contains("/worktrees/"),
        files_changed,
        lines_added,
        lines_removed,
        commits_ahead,
        commits_behind,
        dirty,
        // `compute_git_info` fills this in — it needs `compared_base`, which
        // this pure parser never sees.
        commits_unlanded: 0,
        // The pure parser has no origin/common-dir/worktree-list/slot-marker/
        // base-ref knowledge; `compute_git_info` fills all of these in.
        // Existence is decided before shelling out, so a parsed result is
        // never "missing".
        origin_url: None,
        common_dir: String::new(),
        worktree_dirs: Vec::new(),
        dir_missing: false,
        slot_base_branch: None,
        compared_base: String::new(),
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
    let base = resolve_diff_base(dir, mode, base_branch);
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

/// The commit the diff pane compares against: merge-base with the resolved
/// base ref for [`DiffMode::Main`], HEAD for [`DiffMode::Uncommitted`].
fn resolve_diff_base(dir: &str, mode: DiffMode, base_branch: Option<&str>) -> String {
    match mode {
        DiffMode::Main => {
            let base_ref = resolve_base_ref(dir, base_branch);
            let merge_base = git_out(dir, &["merge-base", "HEAD", &base_ref]);
            if merge_base.is_empty() { "HEAD".to_string() } else { merge_base }
        }
        DiffMode::Uncommitted => "HEAD".to_string(),
    }
}

/// One changed file in the diff pane's file list. `status` is git's
/// name-status letter (`M`/`A`/`D`/`R`/`C`/`T`, or `?` for untracked);
/// `old_path` is set on renames/copies (content at the base lives there).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub lines_added: i64,
    pub lines_removed: i64,
}

/// The diff pane's changed-file list, baseline picked like [`diff_patch`]:
/// `git diff --name-status`/`--numstat` (rename-aware) merged per file, plus
/// untracked files from `git status` (status `?`, no line counts — they have
/// no diff yet). Empty when `dir` isn't a repo or nothing changed.
pub fn diff_files(dir: &str, mode: DiffMode, base_branch: Option<&str>) -> Vec<DiffFile> {
    if dir.is_empty() {
        return Vec::new();
    }
    let base = resolve_diff_base(dir, mode, base_branch);
    let name_status = git_out(dir, &["diff", "--name-status", "-M", &base]);
    let numstat = git_out(dir, &["diff", "--numstat", "-M", &base]);
    let untracked_out = git_out(dir, &["status", "--porcelain"]);
    parse_diff_files(&name_status, &numstat, &untracked_out)
}

/// A file's content at the diff baseline (`git show <base>:<path>`), for the
/// original side of the diff editor. `None` when the file doesn't exist there
/// (added/untracked) or `dir` isn't a repo. Untrimmed — a stripped trailing
/// newline would show up as a phantom EOL change.
pub fn base_file_content(
    dir: &str,
    mode: DiffMode,
    base_branch: Option<&str>,
    path: &str,
) -> Option<String> {
    if dir.is_empty() || path.is_empty() {
        return None;
    }
    let base = resolve_diff_base(dir, mode, base_branch);
    let spec = format!("{base}:{path}");
    let out = tt_exec::run_with_timeout(
        "git",
        &["-C", dir, "show", &spec],
        std::time::Duration::from_secs(5),
    )
    .ok()?;
    out.ok().then_some(out.stdout)
}

/// Pure parse behind [`diff_files`]: merge `--name-status` (status letter +
/// rename old/new paths) with `--numstat` (per-file ± counts; `-` for binary)
/// and append `git status --porcelain`'s `??` untracked entries. Both diff
/// outputs are rename-aware (`-M`), so the numstat path arrow/brace forms are
/// normalized to the post-rename path before matching.
fn parse_diff_files(name_status: &str, numstat: &str, untracked_out: &str) -> Vec<DiffFile> {
    let mut counts: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        counts.insert(
            numstat_new_path(path),
            (added.parse().unwrap_or(0), removed.parse().unwrap_or(0)),
        );
    }
    let mut files = Vec::new();
    for line in name_status.lines() {
        let mut parts = line.split('\t');
        let Some(status_field) = parts.next() else {
            continue;
        };
        let status = status_field.chars().next().unwrap_or_default();
        if !status.is_ascii_alphabetic() {
            continue;
        }
        let (old_path, path) = match (parts.next(), parts.next()) {
            (Some(old), Some(new)) => (Some(old.to_string()), new.to_string()),
            (Some(only), None) => (None, only.to_string()),
            _ => continue,
        };
        let (lines_added, lines_removed) = counts.get(&path).copied().unwrap_or((0, 0));
        files.push(DiffFile {
            path,
            old_path,
            status: status.to_string(),
            lines_added,
            lines_removed,
        });
    }
    for line in untracked_out.lines() {
        if let Some(path) = line.strip_prefix("??") {
            files.push(DiffFile {
                path: path.trim().to_string(),
                old_path: None,
                status: "?".to_string(),
                lines_added: 0,
                lines_removed: 0,
            });
        }
    }
    files
}

/// Normalize a `--numstat` rename path to the post-rename path: either the
/// brace form `dir/{old => new}/x` or the whole-path arrow `old => new`.
fn numstat_new_path(path: &str) -> String {
    if let (Some(open), Some(close)) = (path.find('{'), path.find('}'))
        && let Some(arrow) = path[open..close].find(" => ")
    {
        let new_part = &path[open + arrow + 4..close];
        let joined = format!("{}{}{}", &path[..open], new_part, &path[close + 1..]);
        return joined.replace("//", "/");
    }
    if let Some((_, new)) = path.split_once(" => ") {
        return new.to_string();
    }
    path.to_string()
}

/// One commit ahead of `compared_base`, with its own line-count diff — not
/// the branch's cumulative total ([`GitInfo::lines_added`]/`lines_removed`
/// for that). Powers the `DiffButton` hover's per-commit breakdown, oldest
/// first, so a many-commit branch's ± tally isn't one anonymous blob.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommitStat {
    pub sha: String,
    pub subject: String,
    pub lines_added: i64,
    pub lines_removed: i64,
}

/// Commits on HEAD that `compared_base` doesn't have, oldest first, each with
/// its own `git diff --numstat` line-count diff. `base_branch` is the same
/// per-folder override [`compute_git_info`]/[`diff_patch`] take. Empty when
/// `dir` isn't a repo or nothing is ahead.
pub fn commit_stats(dir: &str, base_branch: Option<&str>) -> Vec<CommitStat> {
    if dir.is_empty() {
        return Vec::new();
    }
    let compared_base = resolve_base_ref(dir, base_branch);
    let log_out = git_out(
        dir,
        &[
            "log",
            "--reverse",
            "--numstat",
            "--pretty=format:\x01%H\x1f%s",
            &format!("{compared_base}..HEAD"),
        ],
    );
    parse_commit_stats(&log_out)
}

/// Pure parse of `git log --reverse --numstat --pretty=format:\x01%H\x1f%s`
/// output: a `\x01`-prefixed header line per commit (sha/subject split on
/// `\x1f`), followed by that commit's `--numstat` lines. Unit-tested on
/// fixture output; the shell-out lives in [`commit_stats`].
fn parse_commit_stats(log_out: &str) -> Vec<CommitStat> {
    let mut stats = Vec::new();
    let mut current: Option<CommitStat> = None;
    for line in log_out.lines() {
        if let Some(header) = line.strip_prefix('\x01') {
            if let Some(c) = current.take() {
                stats.push(c);
            }
            let mut parts = header.splitn(2, '\x1f');
            let sha = parts.next().unwrap_or("").to_string();
            let subject = parts.next().unwrap_or("").to_string();
            current = Some(CommitStat { sha, subject, lines_added: 0, lines_removed: 0 });
        } else if line.is_empty() {
            continue;
        } else if let Some(c) = current.as_mut() {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next().unwrap_or("");
            let removed = parts.next().unwrap_or("");
            if added != "-" {
                c.lines_added += added.parse::<i64>().unwrap_or(0);
            }
            if removed != "-" {
                c.lines_removed += removed.parse::<i64>().unwrap_or(0);
            }
        }
    }
    if let Some(c) = current.take() {
        stats.push(c);
    }
    stats
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

/// Every file in the checkout worth telling a Claude session about: tracked
/// plus untracked-but-not-ignored, repo-relative, sorted, deduped, capped at
/// `cap` (a runaway vendored tree must not ship megabytes to the webview).
/// Empty when `dir` isn't a git repo — same degradation as the rest of this
/// module.
pub fn list_files(dir: &str, cap: usize) -> Vec<String> {
    let tracked = git_out(dir, &["ls-files"]);
    let untracked = git_out(dir, &["ls-files", "--others", "--exclude-standard"]);
    merge_file_lists(&tracked, &untracked, cap)
}

/// Pure merge of the two `ls-files` outputs (unit-tested; the subprocess layer
/// above is not).
fn merge_file_lists(tracked: &str, untracked: &str, cap: usize) -> Vec<String> {
    let mut files: Vec<String> = tracked
        .lines()
        .chain(untracked.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    files.truncate(cap);
    files
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
    fn parse_commit_stats_splits_header_lines_from_numstat() {
        let log = "\x01aaa111\x1ffirst commit\n10\t2\tsrc/a.rs\n5\t0\tsrc/b.rs\n\
            \x01bbb222\x1fsecond commit\n1\t1\tsrc/a.rs\n";
        let stats = parse_commit_stats(log);
        assert_eq!(
            stats,
            vec![
                CommitStat {
                    sha: "aaa111".into(),
                    subject: "first commit".into(),
                    lines_added: 15,
                    lines_removed: 2,
                },
                CommitStat {
                    sha: "bbb222".into(),
                    subject: "second commit".into(),
                    lines_added: 1,
                    lines_removed: 1,
                },
            ],
        );
    }

    #[test]
    fn parse_commit_stats_skips_binary_numstat_and_handles_empty_input() {
        let log = "\x01ccc333\x1fbinary asset\n-\t-\tassets/logo.png\n";
        assert_eq!(
            parse_commit_stats(log),
            vec![CommitStat {
                sha: "ccc333".into(),
                subject: "binary asset".into(),
                lines_added: 0,
                lines_removed: 0,
            }],
        );
        assert!(parse_commit_stats("").is_empty());
    }

    #[test]
    fn commit_stats_lists_ahead_commits_oldest_first_with_own_line_counts() {
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
        std::fs::write(repo.join("f.txt"), "1\n").unwrap();
        run(&["add", "f.txt"]);
        run(&["commit", "--quiet", "-m", "init"]);
        run(&["update-ref", "refs/remotes/origin/main", "main"]);

        run(&["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "1\n2\n").unwrap();
        run(&["commit", "--quiet", "-am", "first"]);
        std::fs::write(repo.join("f.txt"), "1\n2\n3\n4\n").unwrap();
        run(&["commit", "--quiet", "-am", "second"]);

        let dir = repo.to_str().unwrap();
        let stats = commit_stats(dir, None);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].subject, "first");
        assert_eq!(stats[0].lines_added, 1);
        assert_eq!(stats[0].lines_removed, 0);
        assert_eq!(stats[1].subject, "second");
        assert_eq!(stats[1].lines_added, 2);
        assert_eq!(stats[1].lines_removed, 0);
        assert_eq!(stats.iter().map(|c| c.lines_added).sum::<i64>(), 3);
    }

    #[test]
    fn commit_stats_empty_for_non_repo_or_blank_dir() {
        let root = tempfile::TempDir::new().unwrap();
        assert!(commit_stats(root.path().to_str().unwrap(), None).is_empty());
        assert!(commit_stats("", None).is_empty());
    }

    #[test]
    fn merge_file_lists_sorts_dedupes_and_caps() {
        let tracked = "src/b.rs\nsrc/a.rs\nREADME.md\n";
        let untracked = "notes.txt\nsrc/a.rs\n\n  \n";
        assert_eq!(
            merge_file_lists(tracked, untracked, 10),
            vec!["README.md", "notes.txt", "src/a.rs", "src/b.rs"],
        );
        assert_eq!(merge_file_lists(tracked, untracked, 2), vec!["README.md", "notes.txt"]);
        assert!(merge_file_lists("", "", 10).is_empty());
    }

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
    fn dirty_reflects_any_porcelain_line_blank_or_not() {
        let info = compute_git_info_from_outputs("main", "/repo/.git", "", "", "");
        assert!(!info.dirty);
        let dirty = compute_git_info_from_outputs("main", "/repo/.git", "?? new.txt\n", "", "");
        assert!(dirty.dirty);
    }

    #[test]
    fn cherry_unlanded_counts_only_plus_prefixed_lines() {
        assert_eq!(parse_cherry_unlanded(""), 0);
        assert_eq!(
            parse_cherry_unlanded("- 1111111 landed already\n+ 2222222 not landed yet\n"),
            1,
        );
        assert_eq!(parse_cherry_unlanded("- 1111111 one\n- 2222222 two\n"), 0,);
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
    fn worktree_dirs_excludes_unmanaged_worktrees() {
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

        // A managed slot: at `.claude/worktrees/<name>` with a `.tt-slot` marker.
        let managed = main.join(".claude").join("worktrees").join("thing");
        std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
        run(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "thing",
                managed.to_str().unwrap(),
            ],
        );
        std::fs::write(
            managed.join(tt_slots::MARKER_FILE),
            tt_slots::marker_contents("thing", "main", "main"),
        )
        .unwrap();

        // An unmanaged worktree: a plain sibling dir, no marker at all —
        // e.g. `claude --worktree` in a repo whose hooks aren't wired, or a
        // worktree someone added by hand.
        let unmanaged = root.path().join("scratch-ext");
        run(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "scratch-ext",
                unmanaged.to_str().unwrap(),
            ],
        );

        let dirs = list_other_worktrees(main.to_str().unwrap());
        assert_eq!(dirs, vec![managed.to_str().unwrap().to_string()]);
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
        let info = compute_git_info(gone.to_str().unwrap(), None);
        assert!(info.dir_missing);
        assert!(info.branch.is_empty());
    }

    #[test]
    fn compute_does_not_flag_an_existing_dir() {
        let root = tempfile::TempDir::new().unwrap();
        // Present but not a git repo: still not "missing".
        let info = compute_git_info(root.path().to_str().unwrap(), None);
        assert!(!info.dir_missing);
    }

    #[test]
    fn compute_reads_slot_base_branch_from_marker() {
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

        // A non-slot checkout has no marker: no slot base surfaced.
        let info = compute_git_info(repo.to_str().unwrap(), None);
        assert_eq!(info.slot_base_branch, None);

        // Writing the `.tt-slot` marker surfaces its `base=` field, so the
        // diff pane can show what a slot auto-compares against.
        std::fs::write(
            repo.join(tt_slots::MARKER_FILE),
            tt_slots::marker_contents("s", "develop", "main"),
        )
        .unwrap();
        let info = compute_git_info(repo.to_str().unwrap(), None);
        assert_eq!(info.slot_base_branch, Some("develop".to_string()));
    }

    /// The bug this module used to have: `commits_ahead`/`files_changed` were
    /// always measured against origin/main, even for a folder whose diff pane
    /// compares against something else — so the Folder Rail's numbers
    /// disagreed with what the diff pane actually showed. Both must now come
    /// from the same `resolve_base_ref` baseline.
    #[test]
    fn compute_measures_stats_against_the_resolved_base_not_always_main() {
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

        run(&["checkout", "--quiet", "-b", "develop"]);
        std::fs::write(repo.join("f.txt"), "2").unwrap();
        run(&["commit", "--quiet", "-am", "on develop"]);

        run(&["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "3").unwrap();
        run(&["commit", "--quiet", "-am", "on feature"]);

        // Fake remote-tracking refs (no real remote needed for this test).
        run(&["update-ref", "refs/remotes/origin/main", "main"]);
        run(&["update-ref", "refs/remotes/origin/develop", "develop"]);

        let dir = repo.to_str().unwrap();

        // vs origin/main (auto-detect, no override): both commits count.
        let vs_main = compute_git_info(dir, None);
        assert_eq!(vs_main.compared_base, "origin/main");
        assert_eq!(vs_main.commits_ahead, 2);

        // vs an explicit "develop" override: only feature's own commit counts.
        let vs_develop = compute_git_info(dir, Some("develop"));
        assert_eq!(vs_develop.compared_base, "origin/develop");
        assert_eq!(vs_develop.commits_ahead, 1);
    }

    /// The scenario this field exists for: this repo's convention only allows
    /// rebase merges (see root CLAUDE.md), which replay a branch's commits
    /// onto main under brand-new SHAs. `commits_ahead` (SHA reachability)
    /// then never reaches 0 for that branch's own checkout, forever — even
    /// though its content landed. `commits_unlanded` (patch-id equivalence
    /// via `git cherry`) must reach 0 anyway, since that's the only way a
    /// "safe to delete" signal can ever fire on this repo's workflow.
    #[test]
    fn commits_unlanded_reaches_zero_after_a_rebase_style_landing_even_though_ahead_does_not() {
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

        run(&["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "2").unwrap();
        run(&["commit", "--quiet", "-am", "on feature"]);
        let feature_commit = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout;
        let feature_commit = String::from_utf8(feature_commit).unwrap().trim().to_string();

        // Simulate what a rebase-merged PR leaves behind: the same change,
        // landed on main as a brand-new commit (different SHA — main moves on
        // with an unrelated commit first, same as real life, so the
        // cherry-picked commit gets a different parent) via cherry-pick
        // rather than a fast-forward/true-merge.
        run(&["checkout", "--quiet", "main"]);
        std::fs::write(repo.join("other.txt"), "unrelated").unwrap();
        run(&["add", "other.txt"]);
        run(&["commit", "--quiet", "-m", "unrelated on main"]);
        run(&["cherry-pick", "--quiet", &feature_commit]);
        run(&["update-ref", "refs/remotes/origin/main", "main"]);
        run(&["checkout", "--quiet", "feature"]);

        let dir = repo.to_str().unwrap();
        let info = compute_git_info(dir, None);
        // Still "ahead" by SHA reachability — feature's own commit is a
        // different object than the one cherry-picked onto main.
        assert_eq!(info.commits_ahead, 1);
        // But fully landed by content — nothing to unland.
        assert_eq!(info.commits_unlanded, 0);
    }

    #[test]
    fn commits_unlanded_counts_a_commit_whose_content_never_landed() {
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
        run(&["update-ref", "refs/remotes/origin/main", "main"]);

        run(&["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "2").unwrap();
        run(&["commit", "--quiet", "-am", "on feature, never merged"]);

        let dir = repo.to_str().unwrap();
        let info = compute_git_info(dir, None);
        assert_eq!(info.commits_ahead, 1);
        assert_eq!(info.commits_unlanded, 1);
    }

    #[test]
    fn from_outputs_never_flags_missing() {
        // The pure parser only sees git command strings; existence is decided
        // by `compute_git_info` before any shell-out.
        let info = compute_git_info_from_outputs("main", "/repo/.git", "", "", "");
        assert!(!info.dir_missing);
    }

    #[test]
    fn parse_diff_files_merges_status_counts_and_untracked() {
        let name_status =
            "M\tsrc/a.rs\nA\tsrc/b.rs\nD\told.rs\nR087\tsrc/old_name.rs\tsrc/new_name.rs";
        let numstat = "3\t1\tsrc/a.rs\n10\t0\tsrc/b.rs\n0\t5\told.rs\n2\t2\tsrc/{old_name.rs => new_name.rs}\n-\t-\tbin.png";
        let untracked_out = "?? notes.md\n M src/a.rs";
        let files = parse_diff_files(name_status, numstat, untracked_out);
        assert_eq!(files.len(), 5);
        assert_eq!(
            files[0],
            DiffFile {
                path: "src/a.rs".into(),
                old_path: None,
                status: "M".into(),
                lines_added: 3,
                lines_removed: 1,
            }
        );
        assert_eq!(files[2].status, "D");
        assert_eq!(
            files[3],
            DiffFile {
                path: "src/new_name.rs".into(),
                old_path: Some("src/old_name.rs".into()),
                status: "R".into(),
                lines_added: 2,
                lines_removed: 2,
            }
        );
        assert_eq!(
            files[4],
            DiffFile {
                path: "notes.md".into(),
                old_path: None,
                status: "?".into(),
                lines_added: 0,
                lines_removed: 0,
            }
        );
    }

    #[test]
    fn numstat_new_path_handles_arrow_forms() {
        assert_eq!(numstat_new_path("plain/path.rs"), "plain/path.rs");
        assert_eq!(numstat_new_path("old.rs => new.rs"), "new.rs");
        assert_eq!(numstat_new_path("src/{old.rs => new.rs}"), "src/new.rs");
        // A rename out of a directory leaves an empty brace side.
        assert_eq!(numstat_new_path("src/{lib => }/mod.rs"), "src/mod.rs");
    }
}
