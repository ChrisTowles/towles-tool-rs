//! `gh`-backed pull-request collector.
//!
//! Subprocess plumbing (cwd, timeout, name cache) lives in [`crate::gh`]. The
//! JSON-to-[`PrInput`] mapping is factored into pure functions ([`map_pr_list`],
//! [`checks_status`]) so it can be unit-tested with inline fixtures without
//! invoking `gh`.

use std::collections::HashMap;
use std::path::Path;

use tt_store::PrInput;

use crate::gh;

/// The `--json` field set requested from `gh pr list`.
const PR_LIST_FIELDS: &str =
    "number,title,headRefName,state,statusCheckRollup,reviewDecision,url,updatedAt";

/// Page cap for the recently-merged authored PRs fetch — just enough to catch
/// a just-merged branch before its worktree is removed, without pulling a
/// repo's whole merge history.
const MERGED_LIST_LIMIT: &str = "20";

/// Collect and dedup the authored + review-requested open PRs, plus a handful
/// of recently-merged authored PRs, for one repo dir.
///
/// Returns the repo's `owner/name` alongside its PRs so callers know which repo
/// a (possibly empty) result belongs to. Returns an error string (never panics)
/// if `gh` is missing, times out, exits non-zero, or emits unparseable JSON.
/// Dedup is by PR number; a review-requested entry wins over an authored one
/// for the same PR (a merged PR can't collide with either, since GitHub never
/// reports the same number as both open and merged).
pub(crate) fn collect_repo_prs(dir: &Path) -> Result<(String, Vec<PrInput>), String> {
    let repo = gh::repo_name_with_owner(dir)?;
    let authored = gh_pr_list(dir, "open", gh::LIST_LIMIT, &["--author", "@me"])?;
    let review = gh_pr_list(dir, "open", gh::LIST_LIMIT, &["--search", "review-requested:@me"])?;
    let merged = gh_pr_list(dir, "merged", MERGED_LIST_LIMIT, &["--author", "@me"])?;

    let mut by_number: HashMap<i64, PrInput> = HashMap::new();
    for pr in map_pr_list(&merged, &repo, false) {
        by_number.insert(pr.number, pr);
    }
    for pr in map_pr_list(&authored, &repo, false) {
        by_number.insert(pr.number, pr);
    }
    // Insert review-requested last so it wins on collision.
    for pr in map_pr_list(&review, &repo, true) {
        by_number.insert(pr.number, pr);
    }
    Ok((repo, by_number.into_values().collect()))
}

/// Run `gh pr list` in `dir` for the given `--state`/`--limit` plus `extra` filters.
/// Targeted state fetch for one PR: the sweep only stores open PRs plus a
/// bounded recently-merged list, so a task-linked PR absent from that
/// snapshot needs an explicit `gh pr view` to learn whether it merged or
/// closed. Returns the lowercased state (`open` | `merged` | `closed`).
pub(crate) fn fetch_pr_state(dir: &Path, number: i64) -> Result<String, String> {
    let value = gh::run_json(dir, &["pr", "view", &number.to_string(), "--json", "state"])?;
    crate::issues::parse_state_field(&value)
        .ok_or_else(|| format!("gh pr view {number}: no state in JSON"))
}

fn gh_pr_list(
    dir: &Path,
    state: &str,
    limit: &str,
    extra: &[&str],
) -> Result<serde_json::Value, String> {
    let mut args = vec![
        "pr",
        "list",
        "--state",
        state,
        "--limit",
        limit,
        "--json",
        PR_LIST_FIELDS,
    ];
    args.extend_from_slice(extra);
    gh::run_json(dir, &args)
}

/// Map a parsed `gh pr list` JSON array to [`PrInput`]s. Non-array input yields
/// an empty list. Review-requested rows get `review_state = "review_requested"`;
/// authored rows derive it from GitHub's `reviewDecision` (see
/// [`review_decision_state`]) so an approved PR reads differently from one with
/// changes requested.
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
                    review_decision_state(
                        item.get("reviewDecision").and_then(|v| v.as_str()).unwrap_or_default(),
                    )
                },
                url: str_field(item, "url"),
                updated_ts: parse_iso_ms(
                    item.get("updatedAt").and_then(|v| v.as_str()).unwrap_or_default(),
                ),
            })
        })
        .collect()
}

/// Map GitHub's `reviewDecision` to a stored `review_state` for an authored PR.
///
/// `reviewDecision` is `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or
/// empty (no reviews required). Unknown/empty values map to `""` so an authored
/// PR with no verdict stays indistinguishable from a quiet one, as before.
fn review_decision_state(decision: &str) -> String {
    match decision.to_ascii_uppercase().as_str() {
        "APPROVED" => "approved".to_string(),
        "CHANGES_REQUESTED" => "changes_requested".to_string(),
        "REVIEW_REQUIRED" => "review_required".to_string(),
        _ => String::new(),
    }
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
            "reviewDecision": "",
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
    fn map_pr_list_lowercases_a_merged_state() {
        let list = json!([{
            "number": 6,
            "title": "Merged thing",
            "headRefName": "feat/thing",
            "state": "MERGED",
            "statusCheckRollup": [{"conclusion": "SUCCESS"}],
            "reviewDecision": "",
            "url": "https://github.com/o/r/pull/6",
            "updatedAt": "2024-01-02T03:04:05Z"
        }]);
        assert_eq!(map_pr_list(&list, "o/r", false)[0].state, "merged");
    }

    #[test]
    fn review_decision_state_maps_each_verdict() {
        assert_eq!(review_decision_state("APPROVED"), "approved");
        assert_eq!(review_decision_state("CHANGES_REQUESTED"), "changes_requested");
        assert_eq!(review_decision_state("REVIEW_REQUIRED"), "review_required");
        // Empty (no reviews required) and anything unknown stay indistinguishable.
        assert_eq!(review_decision_state(""), "");
        assert_eq!(review_decision_state("SOMETHING_ELSE"), "");
    }

    #[test]
    fn map_pr_list_derives_authored_review_state_from_decision() {
        let row = |decision: &str| {
            json!([{
                "number": 1,
                "title": "t",
                "headRefName": "b",
                "state": "OPEN",
                "statusCheckRollup": [],
                "reviewDecision": decision,
                "url": "u",
                "updatedAt": "2024-01-02T03:04:05Z"
            }])
        };
        assert_eq!(map_pr_list(&row("APPROVED"), "o/r", false)[0].review_state, "approved");
        assert_eq!(
            map_pr_list(&row("CHANGES_REQUESTED"), "o/r", false)[0].review_state,
            "changes_requested"
        );
        assert_eq!(
            map_pr_list(&row("REVIEW_REQUIRED"), "o/r", false)[0].review_state,
            "review_required"
        );
        // A review-requested row ignores reviewDecision and keeps its own state.
        assert_eq!(map_pr_list(&row("APPROVED"), "o/r", true)[0].review_state, "review_requested");
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
