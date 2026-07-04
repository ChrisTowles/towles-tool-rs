//! `gh`-backed pull-request collector.
//!
//! `gh` has no cwd support in [`tt_exec`], so this module shells out with
//! [`std::process::Command`] directly to run each repo's `gh` in its own working
//! directory. The JSON-to-[`PrInput`] mapping is factored into pure functions
//! ([`map_pr_list`], [`checks_status`]) so it can be unit-tested with inline
//! fixtures without invoking `gh`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use tt_store::PrInput;

/// The `--json` field set requested from `gh pr list`.
const PR_LIST_FIELDS: &str = "number,title,headRefName,state,statusCheckRollup,url,updatedAt";

/// Collect and dedup the authored + review-requested open PRs for one repo dir.
///
/// Returns an error string (never panics) if `gh` is missing, exits non-zero, or
/// emits unparseable JSON. Dedup is by PR number; a review-requested entry wins
/// over an authored one for the same PR.
pub(crate) fn collect_repo_prs(dir: &Path) -> Result<Vec<PrInput>, String> {
    let repo = repo_name_with_owner(dir)?;
    let authored = gh_pr_list(dir, &["--author", "@me"])?;
    let review = gh_pr_list(dir, &["--search", "review-requested:@me"])?;

    let mut by_number: HashMap<i64, PrInput> = HashMap::new();
    for pr in map_pr_list(&authored, &repo, false) {
        by_number.insert(pr.number, pr);
    }
    // Insert review-requested last so it wins on collision.
    for pr in map_pr_list(&review, &repo, true) {
        by_number.insert(pr.number, pr);
    }
    Ok(by_number.into_values().collect())
}

/// `owner/repo` for the repo rooted at `dir`, via `gh repo view`.
fn repo_name_with_owner(dir: &Path) -> Result<String, String> {
    let output = Command::new("gh")
        .args(["repo", "view", "--json", "nameWithOwner"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to spawn gh in {}: {e}", dir.display()))?;
    if !output.status.success() {
        return Err(format!(
            "gh repo view failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid gh JSON: {e}"))?;
    value
        .get("nameWithOwner")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("gh repo view returned no nameWithOwner for {}", dir.display()))
}

/// Run `gh pr list` in `dir` with the shared field set plus `extra` filters.
fn gh_pr_list(dir: &Path, extra: &[&str]) -> Result<serde_json::Value, String> {
    let mut args = vec!["pr", "list", "--state", "open", "--json", PR_LIST_FIELDS];
    args.extend_from_slice(extra);
    log::debug!("gh {} (cwd {})", args.join(" "), dir.display());
    let output = Command::new("gh")
        .args(&args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to spawn gh in {}: {e}", dir.display()))?;
    if !output.status.success() {
        return Err(format!(
            "gh pr list failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid gh JSON: {e}"))
}

/// Map a parsed `gh pr list` JSON array to [`PrInput`]s. Non-array input yields
/// an empty list. `review_requested` sets each row's `review_state`.
pub(crate) fn map_pr_list(
    list: &serde_json::Value,
    repo: &str,
    review_requested: bool,
) -> Vec<PrInput> {
    let Some(items) = list.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let number = item.get("number").and_then(|v| v.as_i64())?;
            Some(PrInput {
                repo: repo.to_string(),
                number,
                title: str_field(item, "title"),
                branch: str_field(item, "headRefName"),
                state: str_field(item, "state").to_ascii_lowercase(),
                checks: checks_status(
                    item.get("statusCheckRollup").unwrap_or(&serde_json::Value::Null),
                ),
                review_state: if review_requested {
                    "review_requested".to_string()
                } else {
                    String::new()
                },
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

/// Summarize a `statusCheckRollup` array into `"passing" | "failing" | "pending" | "none"`.
///
/// Empty/absent rollup → `"none"`. Any failing conclusion wins → `"failing"`.
/// Any missing/unrecognized (in-progress) conclusion → `"pending"`. Otherwise
/// all checks are success-like → `"passing"`.
pub(crate) fn checks_status(rollup: &serde_json::Value) -> String {
    let Some(checks) = rollup.as_array() else {
        return "none".to_string();
    };
    if checks.is_empty() {
        return "none".to_string();
    }
    let mut any_pending = false;
    for check in checks {
        // Check runs expose `conclusion`; legacy status contexts expose `state`.
        let verdict = check
            .get("conclusion")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| check.get("state").and_then(|v| v.as_str()));
        match verdict.map(str::to_ascii_uppercase).as_deref() {
            Some("SUCCESS") | Some("NEUTRAL") | Some("SKIPPED") => {}
            Some("FAILURE") | Some("ERROR") | Some("CANCELLED") | Some("TIMED_OUT") => {
                return "failing".to_string();
            }
            _ => any_pending = true,
        }
    }
    if any_pending { "pending".to_string() } else { "passing".to_string() }
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
    fn checks_status_covers_each_state() {
        assert_eq!(checks_status(&json!([])), "none");
        assert_eq!(checks_status(&serde_json::Value::Null), "none");
        assert_eq!(
            checks_status(&json!([{"conclusion": "SUCCESS"}, {"conclusion": "SKIPPED"}])),
            "passing"
        );
        assert_eq!(
            checks_status(&json!([{"conclusion": "SUCCESS"}, {"conclusion": "FAILURE"}])),
            "failing"
        );
        // Missing conclusion (still running) → pending.
        assert_eq!(
            checks_status(&json!([{"conclusion": "SUCCESS"}, {"status": "IN_PROGRESS"}])),
            "pending"
        );
        // A failure outranks a pending sibling.
        assert_eq!(
            checks_status(&json!([{"status": "IN_PROGRESS"}, {"conclusion": "ERROR"}])),
            "failing"
        );
        // Legacy status context via `state`.
        assert_eq!(checks_status(&json!([{"state": "SUCCESS"}])), "passing");
    }

    #[test]
    fn map_pr_list_maps_fields_and_review_state() {
        let list = json!([{
            "number": 42,
            "title": "Fix the thing",
            "headRefName": "feat/thing",
            "state": "OPEN",
            "statusCheckRollup": [{"conclusion": "SUCCESS"}],
            "url": "https://github.com/o/r/pull/42",
            "updatedAt": "2024-01-02T03:04:05Z"
        }]);
        let prs = map_pr_list(&list, "o/r", false);
        assert_eq!(prs.len(), 1);
        let pr = &prs[0];
        assert_eq!(pr.repo, "o/r");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "Fix the thing");
        assert_eq!(pr.branch, "feat/thing");
        assert_eq!(pr.state, "open");
        assert_eq!(pr.checks, "passing");
        assert_eq!(pr.review_state, "");
        assert_eq!(pr.url, "https://github.com/o/r/pull/42");
        assert_eq!(pr.updated_ts, 1704164645000);

        let review = map_pr_list(&list, "o/r", true);
        assert_eq!(review[0].review_state, "review_requested");
    }

    #[test]
    fn map_pr_list_skips_entries_without_number_and_handles_non_array() {
        let list = json!([{"title": "no number"}, {"number": 7, "state": "OPEN"}]);
        let prs = map_pr_list(&list, "o/r", false);
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 7);
        // A non-array (e.g. gh error object) yields nothing rather than panicking.
        assert!(map_pr_list(&json!({"message": "boom"}), "o/r", false).is_empty());
    }

    #[test]
    fn parse_iso_ms_is_zero_on_garbage() {
        assert_eq!(parse_iso_ms("not a date"), 0);
        assert_eq!(parse_iso_ms(""), 0);
        assert_eq!(parse_iso_ms("2024-01-02T03:04:05Z"), 1704164645000);
    }
}
