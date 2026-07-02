//! `gh issue list` argument construction and output parsing, ported from the pure parts
//! of `src/lib/git/gh-cli-wrapper.ts`. Process execution lives in the CLI layer.

use crate::picker::strip_ansi;
use crate::{Error, Issue, Result};

/// Build the argument vector for `gh issue list`. Mirrors `getIssues`.
pub fn issue_list_args(assigned_to_me: bool, label: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "issue".to_string(),
        "list".to_string(),
        "--json".to_string(),
        "labels,number,title,state".to_string(),
    ];
    if assigned_to_me {
        args.push("--assignee".to_string());
        args.push("@me".to_string());
    }
    if let Some(label) = label {
        args.push("--label".to_string());
        args.push(label.to_string());
    }
    args
}

/// Parse `gh issue list --json ...` stdout into issues. ANSI is stripped first (matching
/// the TS), and a parse failure includes a truncated preview of the raw output.
pub fn parse_issues(stdout: &str) -> Result<Vec<Issue>> {
    let stripped = strip_ansi(stdout);
    serde_json::from_str(&stripped).map_err(|_| {
        let preview: String = stripped.chars().take(200).collect();
        let suffix = if stripped.chars().count() > 200 { "..." } else { "" };
        Error::ParseIssues(format!("{preview}{suffix}"))
    })
}

/// Whether `gh --version` output indicates the GitHub CLI is installed. Mirrors
/// `isGithubCliInstalled`, which checks for the CLI's homepage URL in the output.
pub fn gh_version_indicates_installed(version_stdout: &str) -> bool {
    version_stdout.contains("https://github.com/cli/cli")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_include_json_fields() {
        let args = issue_list_args(false, None);
        assert_eq!(args, vec!["issue", "list", "--json", "labels,number,title,state"]);
    }

    #[test]
    fn args_add_assignee_when_assigned_to_me() {
        let args = issue_list_args(true, None);
        assert!(args.windows(2).any(|w| w == ["--assignee", "@me"]));
        assert!(!args.iter().any(|a| a == "--label"));
    }

    #[test]
    fn args_add_label_when_provided() {
        let args = issue_list_args(false, Some("auto-claude"));
        assert!(args.windows(2).any(|w| w == ["--label", "auto-claude"]));
    }

    #[test]
    fn parses_issue_json() {
        let json = r#"[{"number":4,"title":"Short bug","state":"open","labels":[{"name":"bug","color":"d73a4a"}]}]"#;
        let issues = parse_issues(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 4);
        assert_eq!(issues[0].labels[0].name, "bug");
    }

    #[test]
    fn parses_empty_array() {
        assert!(parse_issues("[]").unwrap().is_empty());
    }

    #[test]
    fn strips_ansi_before_parsing() {
        let json = "\u{1b}[32m[]\u{1b}[0m";
        assert!(parse_issues(json).unwrap().is_empty());
    }

    #[test]
    fn parse_error_includes_preview() {
        let err = parse_issues("not json").unwrap_err();
        assert!(err.to_string().contains("not json"));
    }

    #[test]
    fn version_detection() {
        assert!(gh_version_indicates_installed("gh version 2.0.0 (https://github.com/cli/cli)"));
        assert!(!gh_version_indicates_installed("command not found"));
    }
}
