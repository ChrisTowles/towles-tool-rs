//! Data-hub collectors for the towles-tool personal dashboard.
//!
//! Each collector gathers one slice of state — calendar events, triaged inbox
//! emails, action-item tasks, and pull-request status — and writes it into the
//! shared [`tt_store::Store`]. The claude-backed collectors shell out to
//! `claude -p` (via [`tt_exec`]); the PR collector shells out to `gh`.
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
//! under a stable collector key: `claude:calendar`, `claude:email`,
//! `claude:tasks`, or `prs`.

mod prompts;
mod prs;

use std::path::PathBuf;

use tt_store::{EmailInput, EventInput, Store, TaskInput};

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

/// Collect today's + next-7-days calendar events via `claude -p` and replace the
/// stored event set. Records `claude:calendar`.
pub fn collect_calendar(store: &Store, now_ms: i64) -> CollectSummary {
    const KEY: &str = "claude:calendar";
    let events = match run_claude(prompts::CALENDAR).and_then(|v| {
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

/// Collect triaged inbox emails and their implied action items via a single
/// `claude -p` call, replacing the stored email set and upserting the tasks
/// (`source = "claude"`). Records **both** `claude:email` and `claude:tasks`,
/// returning one summary for each.
pub fn collect_email_and_tasks(store: &Store, now_ms: i64) -> Vec<CollectSummary> {
    const EMAIL_KEY: &str = "claude:email";
    const TASKS_KEY: &str = "claude:tasks";

    let (emails, tasks) = match run_claude(prompts::EMAIL).and_then(parse_email_payload) {
        Ok(parsed) => parsed,
        Err(msg) => {
            // A single upstream failure fails both derived collectors.
            return vec![
                finish(store, EMAIL_KEY, false, 0, Some(msg.clone()), now_ms),
                finish(store, TASKS_KEY, false, 0, Some(msg), now_ms),
            ];
        }
    };

    let email_summary = match store.replace_emails(&emails) {
        Ok(count) => finish(store, EMAIL_KEY, true, count, None, now_ms),
        Err(e) => finish(store, EMAIL_KEY, false, 0, Some(e.to_string()), now_ms),
    };
    let task_summary = match store.upsert_tasks(&tasks, now_ms) {
        Ok(count) => finish(store, TASKS_KEY, true, count, None, now_ms),
        Err(e) => finish(store, TASKS_KEY, false, 0, Some(e.to_string()), now_ms),
    };
    vec![email_summary, task_summary]
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

/// Run every collector: calendar, email + tasks, then PRs.
pub fn collect_all(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> Vec<CollectSummary> {
    let mut out = Vec::with_capacity(4);
    out.push(collect_calendar(store, now_ms));
    out.extend(collect_email_and_tasks(store, now_ms));
    out.push(collect_prs(store, repo_dirs, now_ms));
    out
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

/// Intermediate shape for the email prompt's `{"emails": [...], "tasks": [...]}`.
#[derive(serde::Deserialize)]
struct EmailPayload {
    #[serde(default)]
    emails: Vec<EmailInput>,
    #[serde(default)]
    tasks: Vec<RawTask>,
}

/// A task as emitted by the email prompt (no `source`; it is fixed to `claude`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTask {
    text: String,
    #[serde(default)]
    due_ts: Option<i64>,
    #[serde(default)]
    source_ref: Option<String>,
}

/// Parse the email payload into store inputs, sanitizing email tags and stamping
/// `source = "claude"` on every task.
fn parse_email_payload(
    value: serde_json::Value,
) -> Result<(Vec<EmailInput>, Vec<TaskInput>), String> {
    let payload: EmailPayload =
        serde_json::from_value(value).map_err(|e| format!("invalid email JSON: {e}"))?;
    let emails = payload
        .emails
        .into_iter()
        .map(|mut e| {
            e.tag = sanitize_tag(&e.tag);
            e
        })
        .collect();
    let tasks = payload
        .tasks
        .into_iter()
        .map(|t| TaskInput {
            source: "claude".to_string(),
            source_ref: t.source_ref,
            text: t.text,
            due_ts: t.due_ts,
        })
        .collect();
    Ok((emails, tasks))
}

/// Coerce an email tag to the known set, defaulting unknown values to `"fyi"`.
fn sanitize_tag(tag: &str) -> String {
    match tag {
        "needs_reply" | "invite" | "fyi" => tag.to_string(),
        _ => "fyi".to_string(),
    }
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
        let raw = "Sure! Here is the data you asked for:\n{\"emails\": [], \"tasks\": []}\nHope that helps.";
        let v = extract_json(raw).unwrap();
        assert!(v.get("emails").is_some());
        assert!(v.get("tasks").is_some());
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
    fn sanitize_tag_defaults_unknown_to_fyi() {
        assert_eq!(sanitize_tag("needs_reply"), "needs_reply");
        assert_eq!(sanitize_tag("invite"), "invite");
        assert_eq!(sanitize_tag("fyi"), "fyi");
        assert_eq!(sanitize_tag("URGENT"), "fyi");
        assert_eq!(sanitize_tag(""), "fyi");
    }

    #[test]
    fn parse_email_payload_stamps_source_and_sanitizes_tags() {
        let value = serde_json::json!({
            "emails": [{
                "externalId": "m1",
                "fromName": "A",
                "fromAddr": "a@example.com",
                "subject": "Hi",
                "summary": "one line",
                "tag": "bogus",
                "receivedTs": 100
            }],
            "tasks": [{"text": "do it", "dueTs": 200, "sourceRef": "m1"}]
        });
        let (emails, tasks) = parse_email_payload(value).unwrap();
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].tag, "fyi"); // sanitized
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].source, "claude");
        assert_eq!(tasks[0].source_ref.as_deref(), Some("m1"));
        assert_eq!(tasks[0].due_ts, Some(200));
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
}
