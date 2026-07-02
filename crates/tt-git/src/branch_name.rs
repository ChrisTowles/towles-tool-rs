//! Branch-name generation, ported from `src/lib/git/branch-name.ts`.

/// Build a branch name `feature/<number>-<slug>` from an issue number and title.
///
/// Mirrors `createBranchNameFromIssue`: lowercase, trim, spaces to `-`, replace any
/// character outside `[0-9a-zA-Z_-]` with `-`, collapse runs of `-`, then strip trailing
/// `-`. Note the regex `[^0-9a-zA-Z_-]` is ASCII-only, so non-ASCII letters (e.g. `ü`)
/// become `-` — matching the TS byte-for-byte.
pub fn create_branch_name_from_issue(number: u64, title: &str) -> String {
    let mut slug = title.to_lowercase();
    let trimmed = slug.trim().to_string();
    slug = trimmed.replace(' ', "-");

    // Replace anything outside [0-9a-zA-Z_-] with '-'.
    slug = slug
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
        .collect();

    // Collapse runs of '-'.
    let mut collapsed = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push('-');
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }

    // Strip trailing '-' (leading dashes are preserved, matching the TS).
    while collapsed.ends_with('-') {
        collapsed.pop();
    }

    format!("feature/{number}-{collapsed}")
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
