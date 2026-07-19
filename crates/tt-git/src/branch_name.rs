//! Branch-name generation, ported from `src/lib/git/branch-name.ts`.

/// Build a branch name `feature/<number>-<slug>` from an issue number and
/// title. Mirrors `createBranchNameFromIssue`; the slug rules are [`slug`].
pub fn create_branch_name_from_issue(number: u64, title: &str) -> String {
    format!("feature/{number}-{}", slug(title))
}

/// The slug rules on their own, without a prefix: lowercase, trim, spaces to
/// `-`, anything outside `[0-9a-zA-Z_-]` to `-`, collapse runs of `-`, strip
/// trailing `-` (leading dashes are preserved, matching the TS). The character
/// class is ASCII-only, so non-ASCII letters (e.g. `ü`) become `-` — matching
/// the TS byte-for-byte.
///
/// Shared rather than re-derived: `tt-slots`' suggestion fallback and the
/// new-task dialog's own branch field both want exactly this, and a slug rule
/// maintained in parallel copies drifts.
pub fn slug(text: &str) -> String {
    let lowered = text.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for c in lowered.trim().chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' };
        // Collapse runs of '-'.
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_title() {
        assert_eq!(
            create_branch_name_from_issue(
                4,
                "Long Issue Title - with a lot of words     and stuff "
            ),
            "feature/4-long-issue-title-with-a-lot-of-words-and-stuff"
        );
    }

    #[test]
    fn special_characters() {
        assert_eq!(
            create_branch_name_from_issue(123, "Fix bug: @user reported $100 issue!"),
            "feature/123-fix-bug-user-reported-100-issue"
        );
    }

    #[test]
    fn only_numbers() {
        assert_eq!(create_branch_name_from_issue(42, "123 456"), "feature/42-123-456");
    }

    #[test]
    fn trims_trailing_dashes() {
        assert_eq!(create_branch_name_from_issue(7, "Update docs ---"), "feature/7-update-docs");
    }

    #[test]
    fn unicode_characters() {
        assert_eq!(
            create_branch_name_from_issue(99, "Fix für Übersetzung"),
            "feature/99-fix-f-r-bersetzung"
        );
    }

    #[test]
    fn empty_ish_title() {
        assert_eq!(create_branch_name_from_issue(1, "   "), "feature/1-");
    }

    #[test]
    fn underscores_preserved() {
        assert_eq!(
            create_branch_name_from_issue(50, "snake_case_title"),
            "feature/50-snake_case_title"
        );
    }

    #[test]
    fn brackets_and_parens_keep_leading_dash() {
        assert_eq!(
            create_branch_name_from_issue(33, "[Bug] Fix (critical) issue"),
            "feature/33--bug-fix-critical-issue"
        );
    }

    #[test]
    fn collapses_consecutive_dashes() {
        assert_eq!(
            create_branch_name_from_issue(15, "Fix   multiple    spaces"),
            "feature/15-fix-multiple-spaces"
        );
    }
}
