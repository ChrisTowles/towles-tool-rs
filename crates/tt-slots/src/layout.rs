//! Slot naming and layout rules: worktree slots live *inside* the checkout at
//! `<checkout>/.claude/worktrees/<name>/` — Claude Code's native worktree
//! location — so any plain git checkout is slot-capable with no restructuring
//! and no sibling directories, and worktrees created by Claude Code's own
//! surfaces (`claude --worktree`, background sessions) land in the same place
//! `tt slot` manages (via the repo's WorktreeCreate/WorktreeRemove hooks).
//!
//! The main checkout is a normal clone (it is where the user runs the app
//! themselves); slots are branch-named, ephemeral worktrees created from it
//! and removed when their branch merges. `git clean -fdx` at the checkout
//! root skips nested worktrees (git refuses to touch untracked repositories
//! without a second `-f`), so the nesting is safe.

use std::path::{Path, PathBuf};

/// The per-slot marker file, written at render time and ignored via the
/// main checkout's `.git/info/exclude` (so no repo `.gitignore` change is
/// needed). Records the slot's identity for other tooling (state scoping,
/// agents landing cold).
pub const MARKER_FILE: &str = ".tt-slot";

/// First path segment of the worktrees dir under the checkout root.
pub const CLAUDE_DIR: &str = ".claude";

/// Second path segment: `<checkout>/.claude/worktrees/<name>`.
pub const WORKTREES_DIR: &str = "worktrees";

/// The directory holding a checkout's worktree slots:
/// `<checkout>/.claude/worktrees`.
pub fn worktrees_dir(checkout: &Path) -> PathBuf {
    checkout.join(CLAUDE_DIR).join(WORKTREES_DIR)
}

/// If `dir` is a slot checkout (`<main>/.claude/worktrees/<name>`), the main
/// checkout `<main>`. Pure path shape — no filesystem probes; callers verify
/// `.git` presence themselves.
pub fn main_checkout_for(dir: &Path) -> Option<&Path> {
    let worktrees = dir.parent()?;
    let claude = worktrees.parent()?;
    (worktrees.file_name()? == WORKTREES_DIR && claude.file_name()? == CLAUDE_DIR)
        .then(|| claude.parent())?
}

/// Slot directory name for a branch: the *whole* branch, reduced to
/// `[A-Za-z0-9._-]` (`feat/thing` → `feat-thing`) — the folder keeps the
/// branch's full shape instead of discarding its type prefix.
///
/// This mapping is strictly ONE-WAY (branch → folder). Nothing may ever
/// derive a branch back from a folder name — a dash-from-slash and a literal
/// dash are indistinguishable, so any reverse parse would be a guess. The
/// branch is always taken from ground truth instead: read from git in the
/// slot (`branch --show-current`), or supplied verbatim by the caller (the
/// WorktreeCreate hook uses the requested worktree name AS the branch).
/// Distinct branches can collide on one slug (`feat/thing` vs a literal
/// `feat-thing`); creation then fails loudly with `SlotExists`.
pub fn slot_name_from_branch(branch: &str) -> Option<String> {
    let name = sanitize_segment(branch);
    (!name.is_empty()).then_some(name)
}

fn sanitize_segment(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_string()
}

/// Contents of the `.tt-slot` marker. Line-oriented `key=value` so any
/// language can read it without a parser dependency.
pub fn marker_contents(slot_name: &str, base_branch: &str, stream: &str) -> String {
    format!("name={slot_name}\nbase={base_branch}\nstream={stream}\n")
}

/// Parse `.tt-slot` marker contents (as written by [`marker_contents`]) into
/// its `key=value` lines. Pure — callers own reading the file.
pub fn parse_marker(contents: &str) -> std::collections::HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The `base=` field from a slot's `.tt-slot` marker at `slot_dir`, if the
/// marker exists and records a non-empty base. `None` for a non-slot
/// checkout (no marker) or a marker missing/blank on `base`.
pub fn read_slot_base(slot_dir: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(slot_dir.join(MARKER_FILE)).ok()?;
    parse_marker(&contents).remove("base").filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(main_checkout_for(Path::new("/home/u/slots/thing")), None);
    }

    #[test]
    fn slot_name_keeps_the_whole_branch_shape() {
        assert_eq!(slot_name_from_branch("feat/slot-migrate"), Some("feat-slot-migrate".into()));
        assert_eq!(slot_name_from_branch("fix/rail-overflow"), Some("fix-rail-overflow".into()));
        assert_eq!(slot_name_from_branch("standalone"), Some("standalone".into()));
        assert_eq!(slot_name_from_branch("chris/wip/thing"), Some("chris-wip-thing".into()));
    }

    #[test]
    fn slot_name_sanitizes_and_trims() {
        assert_eq!(slot_name_from_branch("feat/hello world!"), Some("feat-hello-world".into()));
        // slug degenerates to separators only → no name
        assert_eq!(slot_name_from_branch("feat/---"), Some("feat".into()));
        assert_eq!(slot_name_from_branch("///"), None);
    }

    #[test]
    fn marker_is_line_oriented() {
        let m = marker_contents("slot-migrate", "main", "main");
        assert_eq!(m, "name=slot-migrate\nbase=main\nstream=main\n");
    }

    #[test]
    fn parse_marker_reads_key_value_lines() {
        let fields = parse_marker("name=slot-migrate\nbase=develop\nstream=main\n");
        assert_eq!(fields.get("base"), Some(&"develop".to_string()));
        assert_eq!(fields.get("name"), Some(&"slot-migrate".to_string()));
    }

    #[test]
    fn read_slot_base_finds_marker_in_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(MARKER_FILE), marker_contents("s", "develop", "main"))
            .unwrap();
        assert_eq!(read_slot_base(dir.path()), Some("develop".to_string()));
    }

    #[test]
    fn read_slot_base_none_without_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(read_slot_base(dir.path()), None);
    }
}
