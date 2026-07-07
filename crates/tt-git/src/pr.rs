//! Pull-request title/body generation, ported from `src/commands/gh/pr.ts`.

/// A generated PR title and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrContent {
    pub title: String,
    pub body: String,
}

/// Type prefixes stripped from a branch name when deriving a title.
const BRANCH_PREFIXES: &[&str] = &["feature", "fix", "bugfix", "hotfix", "chore", "refactor"];

/// Extract the first run of digits in `branch`, mirroring `branch.match(/(\d+)/)`.
fn extract_issue_number(branch: &str) -> Option<&str> {
    let start = branch.find(|c: char| c.is_ascii_digit())?;
    let end = branch[start..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| start + i)
        .unwrap_or(branch.len());
    Some(&branch[start..end])
}

/// Turn a branch name into a title: strip a leading `type/` prefix and a leading `\d+-`,
/// replace `-` with spaces, then upper-case the first character of each word.
fn title_from_branch(branch: &str) -> String {
    let mut s = branch;

    // `^(feature|fix|...)/`
    for prefix in BRANCH_PREFIXES {
        if let Some(rest) = s.strip_prefix(prefix)
            && let Some(rest) = rest.strip_prefix('/')
        {
            s = rest;
            break;
        }
    }

    // `^\d+-`
    let owned = strip_leading_number_dash(s);
    // `-` -> ` `
    let spaced = owned.replace('-', " ");
    // `\b\w` -> upper-case (ASCII word chars, matching JS `\w`).
    title_case_word_starts(&spaced)
}

fn strip_leading_number_dash(s: &str) -> String {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &s[digits.len()..];
        if let Some(rest) = after.strip_prefix('-') {
            return rest.to_string();
        }
    }
    s.to_string()
}

fn title_case_word_starts(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_word = false;
    for ch in s.chars() {
        let is_word = ch.is_ascii_alphanumeric() || ch == '_';
        if is_word && !prev_word {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push(ch);
        }
        prev_word = is_word;
    }
    out
}

/// Generate a PR title and body from the current branch and its commit subjects.
/// Ports `generatePrContent`.
pub fn generate_pr_content(branch: &str, commits: &[String]) -> PrContent {
    let issue_number = extract_issue_number(branch);

    let title = if commits.len() == 1 { commits[0].clone() } else { title_from_branch(branch) };

    let mut lines: Vec<String> = vec!["## Summary".to_string(), String::new()];

    if commits.len() == 1 {
        lines.push(format!("- {}", commits[0]));
    } else {
        for commit in commits.iter().take(10) {
            lines.push(format!("- {commit}"));
        }
        if commits.len() > 10 {
            lines.push(format!("- ... and {} more commits", commits.len() - 10));
        }
    }

    lines.push(String::new());

    if let Some(num) = issue_number {
        lines.push(format!("Closes #{num}"));
        lines.push(String::new());
    }

    lines.push("## Test plan".to_string());
    lines.push(String::new());
    lines.push("- [ ] Tests pass".to_string());
    lines.push("- [ ] Manual testing".to_string());

    PrContent { title, body: lines.join("\n") }
}

/// Decide whether `git push` is needed before opening a PR, given the `## …`
/// branch-header line of `git status -sb`.
///
/// Push when the branch has no upstream, has commits *ahead* of its upstream, or
/// its upstream is `[gone]` (deleted on the remote). A branch that is up to date
/// or only *behind* already has its head on the remote, so pushing is skipped —
/// otherwise an existing-upstream branch with local commits would open a PR from
/// a stale remote head.
///
/// The header line looks like `## <branch>` (no upstream) or
/// `## <branch>...<upstream>[ <tracking>]`, where `<tracking>` is `[ahead N]`,
/// `[behind M]`, `[ahead N, behind M]`, or `[gone]`.
pub fn should_push(status_branch_line: &str) -> bool {
    let line = status_branch_line.trim_start_matches("## ").trim();
    let Some((_, upstream)) = line.split_once("...") else {
        // No `...<upstream>` → the branch has never been pushed.
        return true;
    };
    // `[gone]` upstream (deleted remote) must re-push; `[ahead …]` means we have
    // local commits the remote lacks.
    upstream.contains("[gone]") || upstream.contains("[ahead ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_commit_uses_commit_as_title() {
        let content = generate_pr_content("feature/12-thing", &v(&["Add the thing"]));
        assert_eq!(content.title, "Add the thing");
        assert!(content.body.contains("- Add the thing"));
        assert!(content.body.contains("Closes #12"));
        assert!(content.body.contains("## Test plan"));
    }

    #[test]
    fn multi_commit_derives_title_from_branch() {
        let content = generate_pr_content("feature/123-some-feature", &v(&["c1", "c2", "c3"]));
        assert_eq!(content.title, "Some Feature");
        assert!(content.body.contains("- c1"));
        assert!(content.body.contains("- c3"));
        assert!(content.body.contains("Closes #123"));
    }

    #[test]
    fn caps_commit_list_at_ten_with_overflow_note() {
        let commits: Vec<String> = (1..=12).map(|n| format!("commit {n}")).collect();
        let content = generate_pr_content("fix/1-x", &commits);
        assert!(content.body.contains("- commit 10"));
        assert!(!content.body.contains("- commit 11"));
        assert!(content.body.contains("- ... and 2 more commits"));
    }

    #[test]
    fn no_issue_number_omits_closes() {
        let content = generate_pr_content("just-words", &v(&["a", "b"]));
        assert_eq!(content.title, "Just Words");
        assert!(!content.body.contains("Closes #"));
    }

    #[test]
    fn strips_type_prefix_and_leading_number() {
        // feature/ prefix + 42- leading number both stripped.
        let content = generate_pr_content("refactor/42-clean-up-code", &v(&["a", "b"]));
        assert_eq!(content.title, "Clean Up Code");
    }

    // ── should_push ──

    #[test]
    fn push_when_no_upstream() {
        assert!(should_push("## feat"));
    }

    #[test]
    fn no_push_when_up_to_date() {
        assert!(!should_push("## feat...origin/feat"));
    }

    #[test]
    fn push_when_ahead() {
        assert!(should_push("## feat...origin/feat [ahead 2]"));
    }

    #[test]
    fn no_push_when_only_behind() {
        assert!(!should_push("## feat...origin/feat [behind 1]"));
    }

    #[test]
    fn push_when_ahead_and_behind() {
        assert!(should_push("## feat...origin/feat [ahead 1, behind 2]"));
    }

    #[test]
    fn push_when_upstream_gone() {
        assert!(should_push("## feat...origin/feat [gone]"));
    }
}
