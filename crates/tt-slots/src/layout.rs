//! Slot naming and layout rules: `<root>/<repo>-primary/` + `<root>/slots/<name>/`,
//! with a flat fallback for repos that don't use that nested convention:
//! `<parent>/<repo>/` + a sibling `<parent>/<repo>-slots/<name>/`.
//!
//! The primary is a normal clone that always holds the default branch (it is
//! where the user runs the app themselves); slots are branch-named, ephemeral
//! worktrees created from the primary and removed when their branch merges.
//! The flat fallback exists so any plain checkout (not laid out under a
//! dedicated `<root>` holding just that one repo) can still use `tt slot` —
//! its slots land next to it instead of requiring a restructure.

/// The per-slot marker file, written at render time and ignored via the
/// primary's `.git/info/exclude` (so no repo `.gitignore` change is needed).
/// Records the slot's identity for other tooling (state scoping, agents
/// landing cold).
pub const MARKER_FILE: &str = ".tt-slot";

/// Directory-name suffix that marks a repo's primary checkout.
pub const PRIMARY_SUFFIX: &str = "-primary";

/// Directory under the root that holds the worktree slots.
pub const SLOTS_DIR: &str = "slots";

/// Directory-name suffix for the flat fallback's sibling slots dir:
/// `<parent>/<repo>-slots/`, next to a plain `<parent>/<repo>/` checkout.
pub const SLOTS_SUFFIX: &str = "-slots";

/// Repo name from a primary directory name: `blog-primary` → `blog`.
pub fn repo_from_primary(dir_name: &str) -> Option<&str> {
    let repo = dir_name.strip_suffix(PRIMARY_SUFFIX)?;
    (!repo.is_empty()).then_some(repo)
}

/// Repo name from a flat-fallback slots directory name: `blog-slots` → `blog`.
pub fn repo_from_slots_dir(dir_name: &str) -> Option<&str> {
    let repo = dir_name.strip_suffix(SLOTS_SUFFIX)?;
    (!repo.is_empty()).then_some(repo)
}

/// Slot directory name for a branch: the segment after the last `/` (branch
/// type prefixes like `feat/` carry no information inside `slots/`), reduced
/// to `[A-Za-z0-9._-]`. Falls back to the whole branch when the last segment
/// sanitizes to nothing.
pub fn slot_name_from_branch(branch: &str) -> Option<String> {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let name = sanitize_segment(last);
    if !name.is_empty() {
        return Some(name);
    }
    let whole = sanitize_segment(branch);
    (!whole.is_empty()).then_some(whole)
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
pub fn read_slot_base(slot_dir: &std::path::Path) -> Option<String> {
    let contents = std::fs::read_to_string(slot_dir.join(MARKER_FILE)).ok()?;
    parse_marker(&contents).remove("base").filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_from_primary_strips_suffix() {
        assert_eq!(repo_from_primary("blog-primary"), Some("blog"));
        assert_eq!(repo_from_primary("towles-tool-rs-primary"), Some("towles-tool-rs"));
        assert_eq!(repo_from_primary("blog"), None);
        assert_eq!(repo_from_primary("-primary"), None);
    }

    #[test]
    fn repo_from_slots_dir_strips_suffix() {
        assert_eq!(repo_from_slots_dir("scribed-slots"), Some("scribed"));
        assert_eq!(repo_from_slots_dir("scribed"), None);
        assert_eq!(repo_from_slots_dir("-slots"), None);
    }

    #[test]
    fn slot_name_takes_last_branch_segment() {
        assert_eq!(slot_name_from_branch("feat/slot-migrate"), Some("slot-migrate".into()));
        assert_eq!(slot_name_from_branch("fix/rail-overflow"), Some("rail-overflow".into()));
        assert_eq!(slot_name_from_branch("standalone"), Some("standalone".into()));
        assert_eq!(slot_name_from_branch("chris/wip/thing"), Some("thing".into()));
    }

    #[test]
    fn slot_name_sanitizes_and_falls_back() {
        assert_eq!(slot_name_from_branch("feat/hello world!"), Some("hello-world".into()));
        // last segment sanitizes to nothing → whole branch, slugged
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
