//! Slot naming and hub-layout rules: `<root>/<repo>.git` + `<root>/<repo>-slot-N/`.

/// The per-slot marker file, written at render time and ignored via the hub's
/// `info/exclude` (so no repo `.gitignore` change is needed). Records the
/// slot's identity for other tooling (state scoping, agents landing cold).
pub const MARKER_FILE: &str = ".tt-slot";

/// Repo name from a hub directory name: `blog.git` → `blog`.
pub fn repo_from_hub(hub_dir_name: &str) -> Option<&str> {
    let repo = hub_dir_name.strip_suffix(".git")?;
    (!repo.is_empty()).then_some(repo)
}

/// Directory name for slot `n` of `repo`.
pub fn slot_dir_name(repo: &str, n: u32) -> String {
    format!("{repo}-slot-{n}")
}

/// Parse a slot directory name for `repo`: `blog-slot-3` → `Some(3)`.
/// Rejects anything else, including parked `*.old` dirs and other repos' slots.
pub fn parse_slot(repo: &str, dir_name: &str) -> Option<u32> {
    let suffix = dir_name.strip_prefix(repo)?.strip_prefix("-slot-")?;
    (!suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
        .then(|| suffix.parse().ok())
        .flatten()
}

/// First unused slot number given the sibling directory names (fills gaps).
pub fn next_slot_number(repo: &str, existing_dir_names: &[String]) -> u32 {
    let taken: std::collections::BTreeSet<u32> =
        existing_dir_names.iter().filter_map(|name| parse_slot(repo, name)).collect();
    (0..).find(|n| !taken.contains(n)).unwrap_or(0)
}

/// Contents of the `.tt-slot` marker. Line-oriented `key=value` so any
/// language can read it without a parser dependency.
pub fn marker_contents(slot_name: &str, base_branch: &str, stream: &str) -> String {
    format!("name={slot_name}\nbase={base_branch}\nstream={stream}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_from_hub_strips_git_suffix() {
        assert_eq!(repo_from_hub("blog.git"), Some("blog"));
        assert_eq!(repo_from_hub("towles-tool-rs.git"), Some("towles-tool-rs"));
        assert_eq!(repo_from_hub("blog"), None);
        assert_eq!(repo_from_hub(".git"), None);
    }

    #[test]
    fn parse_slot_accepts_only_this_repos_slots() {
        assert_eq!(parse_slot("blog", "blog-slot-0"), Some(0));
        assert_eq!(parse_slot("blog", "blog-slot-12"), Some(12));
        assert_eq!(parse_slot("blog", "blog-slot-3.old"), None);
        assert_eq!(parse_slot("blog", "blog-x-slot-1"), None);
        assert_eq!(parse_slot("blog", "other-slot-1"), None);
        assert_eq!(parse_slot("blog", "blog-slot-"), None);
        assert_eq!(parse_slot("blog", "blog.git"), None);
    }

    #[test]
    fn next_slot_number_fills_gaps() {
        let names: Vec<String> = ["blog-slot-0", "blog-slot-2", "blog-slot-3.old", "junk"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(next_slot_number("blog", &names), 1);
        assert_eq!(next_slot_number("blog", &[]), 0);
    }

    #[test]
    fn marker_is_line_oriented() {
        let m = marker_contents("blog-slot-2", "main", "main");
        assert_eq!(m, "name=blog-slot-2\nbase=main\nstream=main\n");
    }
}
