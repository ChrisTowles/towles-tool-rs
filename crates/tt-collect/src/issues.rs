//! `gh`-backed issue collector.
//!
//! Mirrors [`crate::prs`]: subprocess plumbing (cwd, timeout, name cache) lives
//! in [`crate::gh`]. The JSON-to-[`IssueInput`] mapping is factored into
//! [`map_issue_list`] so it can be unit-tested with inline fixtures.

use std::path::Path;

use tt_store::IssueInput;

use crate::gh;

/// The `--json` field set requested from `gh issue list`.
const ISSUE_LIST_FIELDS: &str = "number,title,labels,state,url,updatedAt";

/// Collect the open issues assigned to me for one repo dir.
///
/// Returns the repo's `owner/name` alongside its issues so callers know which
/// repo a (possibly empty) result belongs to. Returns an error string (never
/// panics) if `gh` is missing, times out, exits non-zero, or emits unparseable
/// JSON.
pub(crate) fn collect_repo_issues(dir: &Path) -> Result<(String, Vec<IssueInput>), String> {
    let repo = gh::repo_name_with_owner(dir)?;
    let list = gh::run_json(
        dir,
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            gh::LIST_LIMIT,
            "--json",
            ISSUE_LIST_FIELDS,
            "--assignee",
            "@me",
        ],
    )?;
    let issues = map_issue_list(&list, &repo);
    Ok((repo, issues))
}

/// Live fetch of open issues for the import picker: `assigned_to_me` toggles
/// `--assignee @me`, `milestone` optionally scopes to one milestone title.
/// Unlike [`collect_repo_issues`] this never writes the store — it's a
/// read-only lookup for the "Import from GitHub" dialog.
pub fn fetch_importable_issues(
    dir: &Path,
    assigned_to_me: bool,
    milestone: Option<&str>,
) -> Result<Vec<IssueInput>, String> {
    let repo = gh::repo_name_with_owner(dir)?;
    let args = importable_issue_list_args(assigned_to_me, milestone);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let list = gh::run_json(dir, &arg_refs)?;
    Ok(map_issue_list(&list, &repo))
}

/// Build the `gh issue list` args for [`fetch_importable_issues`]. Factored
/// out so the assignee/milestone combinations are unit-testable without
/// shelling out.
fn importable_issue_list_args(assigned_to_me: bool, milestone: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "issue",
        "list",
        "--state",
        "open",
        "--limit",
        gh::LIST_LIMIT,
        "--json",
        ISSUE_LIST_FIELDS,
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if assigned_to_me {
        args.push("--assignee".to_string());
        args.push("@me".to_string());
    }
    if let Some(milestone) = milestone {
        args.push("--milestone".to_string());
        args.push(milestone.to_string());
    }
    args
}

/// Open milestone titles for the repo rooted at `dir`, for the import
/// dialog's milestone filter.
pub fn fetch_repo_milestones(dir: &Path) -> Result<Vec<String>, String> {
    let repo = gh::repo_name_with_owner(dir)?;
    let stdout = gh::run(
        dir,
        &[
            "api",
            &format!("repos/{repo}/milestones"),
            "--jq",
            ".[].title",
        ],
    )?;
    Ok(stdout.lines().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
}

/// Map a parsed `gh issue list` JSON array to [`IssueInput`]s. Non-array input
/// yields an empty list.
pub(crate) fn map_issue_list(list: &serde_json::Value, repo: &str) -> Vec<IssueInput> {
    let Some(items) = list.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let number = item.get("number").and_then(|v| v.as_i64())?;
            Some(IssueInput {
                repo: repo.to_string(),
                number,
                title: str_field(item, "title"),
                labels: label_names(item),
                state: str_field(item, "state").to_ascii_lowercase(),
                url: str_field(item, "url"),
                updated_ts: parse_iso_ms(
                    item.get("updatedAt").and_then(|v| v.as_str()).unwrap_or_default(),
                ),
            })
        })
        .collect()
}

fn str_field(item: &serde_json::Value, key: &str) -> String {
    item.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

/// Extract label display names from an issue's `labels` array of `{name, ...}`.
fn label_names(item: &serde_json::Value) -> Vec<String> {
    item.get("labels")
        .and_then(|v| v.as_array())
        .map(|labels| {
            labels
                .iter()
                .filter_map(|l| l.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse an RFC 3339 / ISO-8601 timestamp to epoch milliseconds; 0 on failure.
fn parse_iso_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.timestamp_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn map_issue_list_maps_fields_and_labels() {
        let list = json!([{
            "number": 390,
            "title": "Refunds double-charge on retry",
            "labels": [{"name": "bug"}, {"name": "P1"}],
            "state": "OPEN",
            "url": "https://github.com/o/r/issues/390",
            "updatedAt": "2024-01-02T03:04:05Z"
        }]);
        let issues = map_issue_list(&list, "o/r");
        assert_eq!(issues.len(), 1);
        let i = &issues[0];
        assert_eq!(i.repo, "o/r");
        assert_eq!(i.number, 390);
        assert_eq!(i.title, "Refunds double-charge on retry");
        assert_eq!(i.labels, vec!["bug".to_string(), "P1".to_string()]);
        assert_eq!(i.state, "open");
        assert_eq!(i.url, "https://github.com/o/r/issues/390");
        assert_eq!(i.updated_ts, 1704164645000);
    }

    #[test]
    fn importable_issue_list_args_covers_assignee_and_milestone_combinations() {
        let base = importable_issue_list_args(false, None);
        assert!(!base.iter().any(|a| a == "--assignee"));
        assert!(!base.iter().any(|a| a == "--milestone"));

        let assigned = importable_issue_list_args(true, None);
        assert!(assigned.windows(2).any(|w| w == ["--assignee", "@me"]));

        let milestoned = importable_issue_list_args(false, Some("v2"));
        assert!(milestoned.windows(2).any(|w| w == ["--milestone", "v2"]));

        let both = importable_issue_list_args(true, Some("v2"));
        assert!(both.windows(2).any(|w| w == ["--assignee", "@me"]));
        assert!(both.windows(2).any(|w| w == ["--milestone", "v2"]));
    }

    #[test]
    fn map_issue_list_skips_entries_without_number_and_handles_non_array() {
        let list = json!([{"title": "no number"}, {"number": 7, "state": "OPEN"}]);
        let issues = map_issue_list(&list, "o/r");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 7);
        assert!(issues[0].labels.is_empty());
        // A non-array (e.g. gh error object) yields nothing rather than panicking.
        assert!(map_issue_list(&json!({"message": "boom"}), "o/r").is_empty());
    }
}
