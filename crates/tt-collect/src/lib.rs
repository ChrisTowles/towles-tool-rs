//! Data-hub collectors for the towles-tool personal dashboard.
//!
//! Each collector gathers one slice of state — calendar events, cross-repo
//! issues, and pull-request status — and writes it into the shared
//! [`tt_store::Store`]. The calendar collector shells out to `claude -p` (via
//! [`tt_exec`]); the issue and PR collectors shell out to `gh`.
//!
//! Tauri-free (the shared-crate rule): both the CLI (`ttr collect`) and the
//! desktop app's scheduler drive this crate against the same [`CollectSummary`]
//! contract.
//!
//! ## Robustness contract
//!
//! The public `collect_*` functions **never panic and never return `Err`**.
//! Every failure mode — a missing `claude`/`gh` binary, a non-zero exit,
//! unparseable output — is captured as a [`CollectSummary`] with `ok = false`
//! and a `message`, and is also recorded via [`tt_store::Store::record_run`]
//! under a stable collector key: `claude:calendar`, `issues`, or `prs`.

mod issues;
mod prompts;
mod prs;

use std::path::PathBuf;

use tt_store::{EventInput, Store};

/// Which calendar backend the `claude -p` prompt should drive. Selected from
/// config so the same app works at home (Google MCP) and work (Outlook MCP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarProvider {
    Google,
    Outlook,
}

impl CalendarProvider {
    /// Parse a config string; defaults to Google for unknown values.
    pub fn from_str_lenient(s: &str) -> CalendarProvider {
        match s.trim().to_ascii_lowercase().as_str() {
            "outlook" => CalendarProvider::Outlook,
            _ => CalendarProvider::Google,
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            CalendarProvider::Google => prompts::CALENDAR_GOOGLE,
            CalendarProvider::Outlook => prompts::CALENDAR_OUTLOOK,
        }
    }
}

/// The outcome of a single collector run.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectSummary {
    /// Stable collector key (also the `record_run` key).
    pub collector: String,
    /// Whether the run succeeded end-to-end.
    pub ok: bool,
    /// Number of items written to the store (0 on failure).
    pub count: usize,
    /// A human-readable note: the error on failure, or context on success
    /// (e.g. `"no repos configured"`).
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Public collectors
// ---------------------------------------------------------------------------

/// Collect today's calendar events via `claude -p` (using the given provider's
/// prompt) and replace the stored event set. Records `claude:calendar`.
pub fn collect_calendar(store: &Store, provider: CalendarProvider, now_ms: i64) -> CollectSummary {
    const KEY: &str = "claude:calendar";
    let events = match run_claude(provider.prompt()).and_then(|v| {
        serde_json::from_value::<Vec<EventInput>>(v)
            .map_err(|e| format!("invalid calendar JSON: {e}"))
    }) {
        Ok(events) => events,
        Err(msg) => return finish(store, KEY, false, 0, Some(msg), now_ms),
    };
    match store.replace_events(&events, now_ms) {
        Ok(count) => finish(store, KEY, true, count, None, now_ms),
        Err(e) => finish(store, KEY, false, 0, Some(e.to_string()), now_ms),
    }
}

/// Collect open issues assigned to me across `repo_dirs` via `gh` and replace the
/// stored issue set. Records `issues`. With no repo dirs this is a clean no-op.
pub fn collect_issues(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    const KEY: &str = "issues";
    if repo_dirs.is_empty() {
        return finish(store, KEY, true, 0, Some("no repos configured".to_string()), now_ms);
    }

    let mut by_key: std::collections::HashMap<(String, i64), tt_store::IssueInput> =
        std::collections::HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for dir in repo_dirs {
        if !dir.is_dir() {
            skipped.push(format!("skipped missing repo dir {}", dir.display()));
            continue;
        }
        match issues::collect_repo_issues(dir) {
            Ok(list) => {
                for issue in list {
                    by_key.insert((issue.repo.clone(), issue.number), issue);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    let all: Vec<tt_store::IssueInput> = by_key.into_values().collect();
    let count = all.len();
    if let Err(e) = store.replace_issues(&all) {
        return finish(store, KEY, false, count, Some(e.to_string()), now_ms);
    }
    let notes: Vec<String> = errors.iter().cloned().chain(skipped).collect();
    let message = if notes.is_empty() { None } else { Some(notes.join("; ")) };
    finish(store, KEY, errors.is_empty(), count, message, now_ms)
}

/// Collect open + review-requested PRs across `repo_dirs` via `gh` and replace
/// the stored PR set. Records `prs`. With no repo dirs this is a clean no-op
/// (`ok = true`, message `"no repos configured"`).
pub fn collect_prs(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    const KEY: &str = "prs";
    if repo_dirs.is_empty() {
        return finish(store, KEY, true, 0, Some("no repos configured".to_string()), now_ms);
    }

    let mut by_key: std::collections::HashMap<(String, i64), tt_store::PrInput> =
        std::collections::HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for dir in repo_dirs {
        // Tracked repos can go stale (moved/deleted dirs); a missing cwd makes
        // `Command` fail with a misleading "gh not found" error, so skip them
        // here and surface the skip in the run message instead.
        if !dir.is_dir() {
            skipped.push(format!("skipped missing repo dir {}", dir.display()));
            continue;
        }
        match prs::collect_repo_prs(dir) {
            Ok(list) => {
                for pr in list {
                    by_key.insert((pr.repo.clone(), pr.number), pr);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    let all: Vec<tt_store::PrInput> = by_key.into_values().collect();
    let count = all.len();
    if let Err(e) = store.replace_prs(&all) {
        return finish(store, KEY, false, count, Some(e.to_string()), now_ms);
    }
    let notes: Vec<String> = errors.iter().cloned().chain(skipped).collect();
    let message = if notes.is_empty() { None } else { Some(notes.join("; ")) };
    finish(store, KEY, errors.is_empty(), count, message, now_ms)
}

/// Run every collector: calendar, issues, then PRs.
pub fn collect_all(
    store: &Store,
    provider: CalendarProvider,
    repo_dirs: &[PathBuf],
    now_ms: i64,
) -> Vec<CollectSummary> {
    vec![
        collect_calendar(store, provider, now_ms),
        collect_issues(store, repo_dirs, now_ms),
        collect_prs(store, repo_dirs, now_ms),
    ]
}

/// The tracked repo directories from the agentboard repos config, or an empty
/// vec if the config is missing/empty.
pub fn tracked_repo_dirs() -> Vec<PathBuf> {
    let path = tt_agentboard::repos::default_repos_path();
    tt_agentboard::repos::load_repos(&path).into_iter().map(PathBuf::from).collect()
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Record the run and build the matching summary. A failed `record_run` write is
/// ignored (the collector contract forbids surfacing it as an error/panic).
fn finish(
    store: &Store,
    collector: &str,
    ok: bool,
    count: usize,
    message: Option<String>,
    now_ms: i64,
) -> CollectSummary {
    let _ = store.record_run(collector, ok, message.as_deref(), now_ms);
    CollectSummary { collector: collector.to_string(), ok, count, message }
}

/// Run `claude -p <prompt>` and extract a JSON value from its stdout. Returns a
/// human-readable error string on spawn failure, non-zero exit, or no parseable
/// JSON.
fn run_claude(prompt: &str) -> Result<serde_json::Value, String> {
    log::debug!("claude -p ({} byte prompt)", prompt.len());
    let output = tt_exec::run("claude", &["-p", prompt]).map_err(|e| e.to_string())?;
    if !output.ok() {
        let stderr = output.stderr.trim();
        return Err(if stderr.is_empty() {
            format!("claude exited with code {}", output.exit_code)
        } else {
            format!("claude failed: {stderr}")
        });
    }
    extract_json(&output.stdout).ok_or_else(|| "no parseable JSON in claude output".to_string())
}

/// Leniently extract the first balanced JSON array or object from `raw`.
///
/// Strips Markdown code fences, then bracket-scans (respecting strings and
/// escapes) for the first `[`/`{` and its matching close. Returns `None` when
/// nothing parses — an unbalanced fragment, prose with no JSON, or a claude
/// error sentence.
pub fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let cleaned = raw.replace("```json", "").replace("```JSON", "").replace("```", "");
    let bytes = cleaned.as_bytes();
    let start = bytes.iter().position(|&b| b == b'[' || b == b'{')?;
    let (open, close) = if bytes[start] == b'[' { ('[', ']') } else { ('{', '}') };

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in cleaned[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return serde_json::from_str(&cleaned[start..end]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_clean_array() {
        let v = extract_json(r#"[{"a":1},{"a":2}]"#).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn extract_fenced_array() {
        let raw = "```json\n[1, 2, 3]\n```";
        assert_eq!(extract_json(raw).unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn extract_prose_wrapped_object() {
        let raw = "Sure! Here is the data you asked for:\n{\"events\": []}\nHope that helps.";
        let v = extract_json(raw).unwrap();
        assert!(v.get("events").is_some());
    }

    #[test]
    fn extract_object_with_nested_arrays_and_braces_in_strings() {
        let raw = r#"{"title": "a } weird ] title", "attendees": ["x", "y"]}"#;
        let v = extract_json(raw).unwrap();
        assert_eq!(v.get("title").unwrap(), "a } weird ] title");
        assert_eq!(v.get("attendees").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn extract_unbalanced_is_none() {
        assert!(extract_json(r#"[{"a": 1}"#).is_none());
    }

    #[test]
    fn extract_error_sentence_is_none() {
        assert!(extract_json("I could not access your calendar tools.").is_none());
    }

    #[test]
    fn calendar_provider_parses_leniently() {
        assert_eq!(CalendarProvider::from_str_lenient("outlook"), CalendarProvider::Outlook);
        assert_eq!(CalendarProvider::from_str_lenient("Outlook"), CalendarProvider::Outlook);
        assert_eq!(CalendarProvider::from_str_lenient("google"), CalendarProvider::Google);
        assert_eq!(CalendarProvider::from_str_lenient("whatever"), CalendarProvider::Google);
    }

    #[test]
    fn collect_prs_no_repos_is_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let summary = collect_prs(&store, &[], 1);
        assert!(summary.ok);
        assert_eq!(summary.count, 0);
        assert_eq!(summary.message.as_deref(), Some("no repos configured"));
        // The run is recorded under the `prs` key.
        let runs = store.runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].collector, "prs");
        assert!(runs[0].ok);
    }

    #[test]
    fn collect_issues_no_repos_is_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let summary = collect_issues(&store, &[], 1);
        assert!(summary.ok);
        assert_eq!(summary.count, 0);
        assert_eq!(summary.message.as_deref(), Some("no repos configured"));
        let runs = store.runs().unwrap();
        assert_eq!(runs[0].collector, "issues");
    }
}
