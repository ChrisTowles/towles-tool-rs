//! Task naming and layout rules: worktree tasks live *inside* the checkout at
//! `<checkout>/.claude/worktrees/<name>/` — Claude Code's native worktree
//! location — so any plain git checkout is task-capable with no restructuring
//! and no sibling directories, and worktrees created by Claude Code's own
//! surfaces (`claude --worktree`, background sessions) land in the same place
//! `tt task` manages (via the repo's WorktreeCreate/WorktreeRemove hooks).
//!
//! The main checkout is a normal clone (it is where the user runs the app
//! themselves); tasks are branch-named, ephemeral worktrees created from it
//! and removed when their branch merges. `git clean -fdx` at the checkout
//! root skips nested worktrees (git refuses to touch untracked repositories
//! without a second `-f`), so the nesting is safe.

use std::path::{Path, PathBuf};

/// The per-task marker file, written at render time and ignored via the
/// main checkout's `.git/info/exclude` (so no repo `.gitignore` change is
/// needed). Records the task's identity for other tooling (state scoping,
/// agents landing cold).
pub const MARKER_FILE: &str = ".tt-task";

/// First path segment of the worktrees dir under the checkout root.
pub const CLAUDE_DIR: &str = ".claude";

/// Second path segment: `<checkout>/.claude/worktrees/<name>`.
pub const WORKTREES_DIR: &str = "worktrees";

/// The directory holding a checkout's worktree tasks:
/// `<checkout>/.claude/worktrees`.
pub fn worktrees_dir(checkout: &Path) -> PathBuf {
    checkout.join(CLAUDE_DIR).join(WORKTREES_DIR)
}

/// If `dir` is a task checkout (`<main>/.claude/worktrees/<name>`), the main
/// checkout `<main>`. Pure path shape — no filesystem probes; callers verify
/// `.git` presence themselves.
pub fn main_checkout_for(dir: &Path) -> Option<&Path> {
    let worktrees = dir.parent()?;
    let claude = worktrees.parent()?;
    (worktrees.file_name()? == WORKTREES_DIR && claude.file_name()? == CLAUDE_DIR)
        .then(|| claude.parent())?
}

/// A validated task name: exactly one safe path segment, the thing
/// [`crate::ops::TaskRoot::task_dir`] joins under the worktrees dir.
/// Parse-don't-validate — the only constructor is [`TaskName::parse`], so a
/// name that reached a `TaskName` can never traverse out of the worktrees
/// dir. This matters most for names read back off disk (the port registry's
/// owner fields, directory listings): a corrupt or hand-edited `"../x"`
/// entry must die at the parse, not at a path join.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskName(String);

impl TaskName {
    /// Accepts exactly the names [`task_name_from_branch`] can produce (its
    /// `[A-Za-z0-9._-]` alphabet, `.`/`-` trimmed at the ends) — which is
    /// also what rules out every traversal shape (`/`, `\`, `..`, empty).
    pub fn parse(raw: &str) -> Option<Self> {
        (!raw.is_empty() && sanitize_segment(raw) == raw).then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TaskName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Task directory name for a branch: the *whole* branch, reduced to
/// `[A-Za-z0-9._-]` (`feat/thing` → `feat-thing`) — the folder keeps the
/// branch's full shape instead of discarding its type prefix.
///
/// This mapping is strictly ONE-WAY (branch → folder). Nothing may ever
/// derive a branch back from a folder name — a dash-from-slash and a literal
/// dash are indistinguishable, so any reverse parse would be a guess. The
/// branch is always taken from ground truth instead: read from git in the
/// task (`branch --show-current`), or supplied verbatim by the caller (the
/// WorktreeCreate hook uses the requested worktree name AS the branch).
/// Distinct branches can collide on one slug (`feat/thing` vs a literal
/// `feat-thing`); creation then fails loudly with `TaskExists`.
pub fn task_name_from_branch(branch: &str) -> Option<String> {
    let name = sanitize_segment(branch);
    (!name.is_empty()).then_some(name)
}

/// The task name for a worktree directory: its basename, which is the slugged
/// branch by construction (see [`task_name_from_branch`]). Pure path shape —
/// no filesystem probe — so it works for a directory that is already gone,
/// which is exactly when the caller can't read the name off the checkout.
/// Falls back to the whole path string for a path with no final component.
pub fn task_name_from_dir(dir: &Path) -> String {
    dir.file_name().and_then(|n| n.to_str()).unwrap_or(&dir.to_string_lossy()).to_string()
}

fn sanitize_segment(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_string()
}

/// Contents of the `.tt-task` marker. Line-oriented `key=value` so any
/// language can read it without a parser dependency.
pub fn marker_contents(task_name: &str, base_branch: &str, stream: &str) -> String {
    format!("name={task_name}\nbase={base_branch}\nstream={stream}\n")
}

/// Parse `.tt-task` marker contents (as written by [`marker_contents`]) into
/// its `key=value` lines. Pure — callers own reading the file.
pub fn parse_marker(contents: &str) -> std::collections::HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The `base=` field from a task's `.tt-task` marker at `task_dir`, if the
/// marker exists and records a non-empty base. `None` for a non-task
/// checkout (no marker) or a marker missing/blank on `base`.
pub fn read_task_base(task_dir: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(task_dir.join(MARKER_FILE)).ok()?;
    parse_marker(&contents).remove("base").filter(|s| !s.is_empty())
}

/// Whether `dir` is a worktree task `tt task` (or a repo's wired
/// WorktreeCreate hook) actually created: it sits at
/// `<checkout>/.claude/worktrees/<name>` ([`main_checkout_for`]) AND carries
/// the `.tt-task` marker written at creation time. A worktree satisfying
/// only one of the two is NOT a managed task — e.g. `claude --worktree` ran
/// in a repo whose hooks aren't wired, so the marker was never written, or a
/// worktree added by hand somewhere else on disk entirely — even though
/// `git worktree list` still discovers it. The canonical check callers use
/// to tell a `tt task` worktree apart from any other.
pub fn is_managed_task(dir: &Path) -> bool {
    main_checkout_for(dir).is_some() && dir.join(MARKER_FILE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_name_accepts_what_branch_slugging_produces() {
        for name in ["feat-thing", "fix-1.2", "a_b", "x"] {
            assert!(TaskName::parse(name).is_some(), "{name} should parse");
        }
    }

    #[test]
    fn task_name_rejects_every_traversal_shape() {
        // The reason the type exists: names read back off disk (registry
        // owners, hand-typed args) must not be able to escape the worktrees
        // dir via a path join.
        for name in ["", ".", "..", "../x", "a/b", "a\\b", "/abs", "-trimmed-"] {
            assert!(TaskName::parse(name).is_none(), "{name:?} must be rejected");
        }
    }

    #[test]
    fn worktrees_dir_nests_under_claude() {
        assert_eq!(
            worktrees_dir(Path::new("/home/u/blog")),
            PathBuf::from("/home/u/blog/.claude/worktrees")
        );
    }

    #[test]
    fn main_checkout_for_matches_only_the_nested_shape() {
        assert_eq!(
            main_checkout_for(Path::new("/home/u/blog/.claude/worktrees/thing")),
            Some(Path::new("/home/u/blog"))
        );
        assert_eq!(main_checkout_for(Path::new("/home/u/blog")), None);
        assert_eq!(main_checkout_for(Path::new("/home/u/blog/.claude/other/thing")), None);
        assert_eq!(main_checkout_for(Path::new("/home/u/tasks/thing")), None);
    }

    #[test]
    fn task_name_keeps_the_whole_branch_shape() {
        assert_eq!(task_name_from_branch("feat/task-migrate"), Some("feat-task-migrate".into()));
        assert_eq!(task_name_from_branch("fix/rail-overflow"), Some("fix-rail-overflow".into()));
        assert_eq!(task_name_from_branch("standalone"), Some("standalone".into()));
        assert_eq!(task_name_from_branch("chris/wip/thing"), Some("chris-wip-thing".into()));
    }

    #[test]
    fn task_name_sanitizes_and_trims() {
        assert_eq!(task_name_from_branch("feat/hello world!"), Some("feat-hello-world".into()));
        assert_eq!(task_name_from_branch("feat/---"), Some("feat".into()));
        // slug degenerates to separators only → no name
        assert_eq!(task_name_from_branch("///"), None);
    }

    #[test]
    fn task_name_from_dir_is_the_basename() {
        assert_eq!(
            task_name_from_dir(Path::new("/home/u/blog/.claude/worktrees/feat-thing")),
            "feat-thing"
        );
        assert_eq!(task_name_from_dir(Path::new("/home/u/tasks/standalone")), "standalone");
        // A trailing slash still yields the final component.
        assert_eq!(task_name_from_dir(Path::new("/home/u/tasks/thing/")), "thing");
    }

    #[test]
    fn marker_is_line_oriented() {
        let m = marker_contents("task-migrate", "main", "main");
        assert_eq!(m, "name=task-migrate\nbase=main\nstream=main\n");
    }

    #[test]
    fn parse_marker_reads_key_value_lines() {
        let fields = parse_marker("name=task-migrate\nbase=develop\nstream=main\n");
        assert_eq!(fields.get("base"), Some(&"develop".to_string()));
        assert_eq!(fields.get("name"), Some(&"task-migrate".to_string()));
    }

    #[test]
    fn read_task_base_finds_marker_in_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(MARKER_FILE), marker_contents("s", "develop", "main"))
            .unwrap();
        assert_eq!(read_task_base(dir.path()), Some("develop".to_string()));
    }

    #[test]
    fn read_task_base_none_without_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(read_task_base(dir.path()), None);
    }

    #[test]
    fn is_managed_task_requires_both_location_and_marker() {
        let root = tempfile::TempDir::new().unwrap();
        let checkout = root.path().join("checkout");
        let task_dir = checkout.join(CLAUDE_DIR).join(WORKTREES_DIR).join("thing");
        std::fs::create_dir_all(&task_dir).unwrap();

        // Right location, no marker: an unwired-hook Claude Code worktree.
        assert!(!is_managed_task(&task_dir));

        // Right location, marker present: a real `tt task`.
        std::fs::write(task_dir.join(MARKER_FILE), marker_contents("thing", "main", "main"))
            .unwrap();
        assert!(is_managed_task(&task_dir));

        // Wrong location, even with a stray marker file: not a task shape.
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join(MARKER_FILE), marker_contents("thing", "main", "main"))
            .unwrap();
        assert!(!is_managed_task(&elsewhere));
    }
}
