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

mod gh;
mod issues;
mod prompts;
mod prs;
mod slack;

pub use slack::SlackDmConfig;

use std::path::PathBuf;
use std::time::Duration;

use tt_store::{EventInput, Store};

/// Hard cap on a `claude -p` calendar run. Generous for MCP tool calls; without
/// it a wedged claude (auth prompt, dead MCP server) blocks its caller forever —
/// in the app that stalls every collector, since the scheduler awaits batches
/// serially.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(180);

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

/// The stable `record_run` key for the calendar collector.
const CALENDAR_KEY: &str = "claude:calendar";

/// Collect today's calendar events via `claude -p` (using the given provider's
/// prompt) and replace the stored event set. Records `claude:calendar`.
pub fn collect_calendar(store: &Store, provider: CalendarProvider, now_ms: i64) -> CollectSummary {
    let events = match run_claude(provider.prompt()).and_then(|v| {
        serde_json::from_value::<Vec<EventInput>>(v)
            .map_err(|e| format!("invalid calendar JSON: {e}"))
    }) {
        Ok(events) => events,
        Err(msg) => return finish(store, CALENDAR_KEY, false, 0, Some(msg), now_ms),
    };
    store_calendar_events(store, &events, now_ms)
}

/// Apply a parsed calendar result to the store, guarding against a suspicious
/// empty sweep.
///
/// `collect_calendar` normally does a full [`Store::replace_events`] snapshot,
/// but a `claude -p` run can return a syntactically-valid empty `[]` when the
/// model hedges or the calendar MCP is momentarily down. Replacing on that
/// would wipe today's events and blank the Cockpit next-meeting countdown until
/// the next tick. So when the result is empty *and* the store still holds
/// events later today, treat the run as suspect: keep the existing rows and
/// record `ok = false` with an explanatory message. A genuinely empty day (no
/// future rows either) still clears normally and records `ok = true`.
fn store_calendar_events(store: &Store, events: &[EventInput], now_ms: i64) -> CollectSummary {
    if events.is_empty() {
        match has_future_events_today(store, now_ms) {
            Ok(true) => {
                let msg = "calendar returned no events but future events remain for \
                           today; kept existing events"
                    .to_string();
                return finish(store, CALENDAR_KEY, false, 0, Some(msg), now_ms);
            }
            Ok(false) => {}
            Err(msg) => return finish(store, CALENDAR_KEY, false, 0, Some(msg), now_ms),
        }
    }
    match store.replace_events(events, now_ms) {
        Ok(count) => finish(store, CALENDAR_KEY, true, count, None, now_ms),
        Err(e) => finish(store, CALENDAR_KEY, false, 0, Some(e.to_string()), now_ms),
    }
}

/// Whether the store holds any event still upcoming today (local time).
///
/// "Today" is the local calendar day containing `now_ms`; the window is
/// `[now_ms, local midnight)`, so only still-to-start events count. Reads the
/// system time zone for the day boundary but never the wall clock — the instant
/// is always the injected `now_ms`.
fn has_future_events_today(store: &Store, now_ms: i64) -> Result<bool, String> {
    let end_ms = local_day_end_ms(now_ms);
    store.events_between(now_ms, end_ms).map(|events| !events.is_empty()).map_err(|e| e.to_string())
}

/// The epoch-ms instant of the next local midnight after `now_ms` — the
/// exclusive end of the local calendar day containing `now_ms`.
fn local_day_end_ms(now_ms: i64) -> i64 {
    use chrono::{Duration, Local, TimeZone};
    let now = match Local.timestamp_millis_opt(now_ms) {
        chrono::LocalResult::Single(dt) => dt,
        // Ambiguous/nonexistent local instants are a DST edge; fall back to
        // one day out so the guard stays conservative (keeps events).
        _ => return now_ms + Duration::days(1).num_milliseconds(),
    };
    let next_midnight = now.date_naive() + Duration::days(1);
    match now.timezone().from_local_datetime(&next_midnight.and_hms_opt(0, 0, 0).unwrap()) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            dt.timestamp_millis()
        }
        // Midnight doesn't exist (spring-forward at 00:00, rare): step to the
        // start of the following day instead.
        chrono::LocalResult::None => now_ms + Duration::days(1).num_milliseconds(),
    }
}

/// Collect open issues assigned to me across `repo_dirs` via `gh` and update the
/// stored issue set. Records `issues`. With no repo dirs this is a clean no-op.
///
/// Failure containment: rows are only replaced for repos whose `gh` calls
/// succeeded. A repo that errors (rate limit, network, auth) keeps its
/// last-known-good rows — a transient outage must not blank the dashboard —
/// and the run is recorded `ok = false` so staleness is visible. Only a fully
/// clean sweep does a full-table replace (which also purges rows of repos no
/// longer tracked).
pub fn collect_issues(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    let outcome = sweep_repos(repo_dirs, issues::collect_repo_issues);
    let write = |all: &[tt_store::IssueInput], repos: Option<&[String]>| match repos {
        None => store.replace_issues(all),
        Some(repos) => store.replace_issues_for_repos(repos, all),
    };
    finish_sweep(store, "issues", outcome, write, |i| (i.repo.clone(), i.number), now_ms)
}

/// Collect open + review-requested PRs across `repo_dirs` via `gh` and update
/// the stored PR set. Records `prs`. Failure containment matches
/// [`collect_issues`]: failed repos keep their last-known-good rows.
pub fn collect_prs(store: &Store, repo_dirs: &[PathBuf], now_ms: i64) -> CollectSummary {
    let outcome = sweep_repos(repo_dirs, prs::collect_repo_prs);
    let write = |all: &[tt_store::PrInput], repos: Option<&[String]>| match repos {
        None => store.replace_prs(all),
        Some(repos) => store.replace_prs_for_repos(repos, all),
    };
    finish_sweep(store, "prs", outcome, write, |p| (p.repo.clone(), p.number), now_ms)
}

/// Collect the watched Slack DM's latest state via the Slack Web API and
/// upsert it into the store. Records `slack:dm`. Missing token/user-id is a
/// recorded failure (the caller gates on `enabled`, so reaching here without
/// credentials is a misconfiguration worth surfacing, not a silent no-op).
pub fn collect_slack_dm(store: &Store, config: &SlackDmConfig, now_ms: i64) -> CollectSummary {
    const KEY: &str = "slack:dm";
    if config.token.trim().is_empty() || config.watch_user_id.trim().is_empty() {
        let msg = "slack collector needs both a token and a watch user id".to_string();
        return finish(store, KEY, false, 0, Some(msg), now_ms);
    }
    match slack::fetch_dm(config) {
        Ok(Some(dm)) => match store.upsert_dm(&dm, now_ms) {
            Ok(()) => finish(store, KEY, true, 1, None, now_ms),
            Err(e) => finish(store, KEY, false, 0, Some(e.to_string()), now_ms),
        },
        Ok(None) => finish(store, KEY, true, 0, Some("no messages in DM yet".to_string()), now_ms),
        Err(msg) => finish(store, KEY, false, 0, Some(msg), now_ms),
    }
}

/// Per-repo results of one collector sweep.
struct Sweep<T> {
    /// `(owner/name, items)` for every repo whose `gh` calls succeeded —
    /// including repos with zero items, which still need their rows cleared.
    successes: Vec<(String, Vec<T>)>,
    errors: Vec<String>,
    skipped: Vec<String>,
}

/// Run `collect_repo` over every existing repo dir, partitioning outcomes.
fn sweep_repos<T>(
    repo_dirs: &[PathBuf],
    collect_repo: impl Fn(&std::path::Path) -> Result<(String, Vec<T>), String>,
) -> Sweep<T> {
    let mut sweep = Sweep { successes: Vec::new(), errors: Vec::new(), skipped: Vec::new() };
    for dir in repo_dirs {
        // Tracked repos can go stale (moved/deleted dirs); a missing cwd makes
        // `Command` fail with a misleading "gh not found" error, so skip them
        // here and surface the skip in the run message instead.
        if !dir.is_dir() {
            sweep.skipped.push(format!("skipped missing repo dir {}", dir.display()));
            continue;
        }
        match collect_repo(dir) {
            Ok(result) => sweep.successes.push(result),
            Err(e) => sweep.errors.push(e),
        }
    }
    sweep
}

/// Apply a sweep's results to the store and record the run.
///
/// `write(all, None)` performs a full-table replace; `write(all, Some(repos))`
/// replaces only the named repos' rows. `key_of` yields the `(repo, number)`
/// identity used to dedup items collected from two checkouts of one repo
/// (parallel worktree slots).
fn finish_sweep<T>(
    store: &Store,
    key: &str,
    sweep: Sweep<T>,
    write: impl Fn(&[T], Option<&[String]>) -> tt_store::Result<usize>,
    key_of: impl Fn(&T) -> (String, i64),
    now_ms: i64,
) -> CollectSummary {
    let Sweep { successes, errors, skipped } = sweep;

    if successes.is_empty() {
        // Nothing succeeded: never touch existing rows. All-skipped (or an
        // empty tracked list) is a clean no-op; any error marks the run failed.
        let ok = errors.is_empty();
        let mut notes: Vec<String> = errors.into_iter().chain(skipped).collect();
        if notes.is_empty() {
            notes.push("no repos configured".to_string());
        }
        return finish(store, key, ok, 0, Some(notes.join("; ")), now_ms);
    }

    let repos: Vec<String> = successes.iter().map(|(repo, _)| repo.clone()).collect();
    let mut by_key: std::collections::HashMap<(String, i64), T> = std::collections::HashMap::new();
    for (_, items) in successes {
        for item in items {
            by_key.insert(key_of(&item), item);
        }
    }
    let all: Vec<T> = by_key.into_values().collect();
    let count = all.len();

    let clean_sweep = errors.is_empty() && skipped.is_empty();
    let scope = if clean_sweep { None } else { Some(repos.as_slice()) };
    if let Err(e) = write(&all, scope) {
        return finish(store, key, false, count, Some(e.to_string()), now_ms);
    }

    let notes: Vec<String> = errors.iter().cloned().chain(skipped).collect();
    let message = if notes.is_empty() { None } else { Some(notes.join("; ")) };
    finish(store, key, errors.is_empty(), count, message, now_ms)
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

/// Run the collectors a manual "refresh now" fires: issues, then PRs, then —
/// only when a Slack config is supplied — the watched DM. Calendar is
/// deliberately excluded: every calendar run spends `claude` tokens, so it
/// stays on its scheduled cadence and is never triggered by a button press.
/// `slack` is `Some` only when the collector is enabled and configured (the
/// caller decides), so passing `None` cleanly skips it rather than recording a
/// misconfiguration failure on every manual refresh.
pub fn collect_manual(
    store: &Store,
    repo_dirs: &[PathBuf],
    slack: Option<&SlackDmConfig>,
    now_ms: i64,
) -> Vec<CollectSummary> {
    let mut summaries = vec![
        collect_issues(store, repo_dirs, now_ms),
        collect_prs(store, repo_dirs, now_ms),
    ];
    if let Some(config) = slack {
        summaries.push(collect_slack_dm(store, config, now_ms));
    }
    summaries
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

/// Run `claude -p <prompt>` (capped at [`CLAUDE_TIMEOUT`]) and extract a JSON
/// value from its stdout. Returns a human-readable error string on spawn
/// failure, timeout, non-zero exit, or no parseable JSON.
fn run_claude(prompt: &str) -> Result<serde_json::Value, String> {
    log::debug!("claude -p ({} byte prompt)", prompt.len());
    let output = tt_exec::run_with_timeout("claude", &["-p", prompt], CLAUDE_TIMEOUT)
        .map_err(|e| e.to_string())?;
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

/// Leniently extract the first parseable balanced JSON array or object from
/// `raw`.
///
/// Bracket-scans (respecting strings and escapes) from each `[`/`{` in turn; a
/// candidate that is unbalanced or fails to parse — prose like `[3 total]`
/// ahead of the real payload — moves the scan to the next opener instead of
/// giving up. The raw text is never rewritten (a fence marker inside a JSON
/// string must survive), and fences don't need stripping: the scan simply
/// starts at the first opener. Returns `None` when nothing in `raw` parses.
pub fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let mut from = 0;
    while let Some(offset) = raw[from..].find(['[', '{']) {
        let start = from + offset;
        if let Some(value) = parse_balanced_at(raw, start) {
            return Some(value);
        }
        // This opener didn't yield JSON; resume after it.
        from = start + 1;
    }
    None
}

/// Parse the balanced bracket run starting at byte `start` (which must be `[`
/// or `{`), or `None` if it never closes or isn't valid JSON.
fn parse_balanced_at(raw: &str, start: usize) -> Option<serde_json::Value> {
    let (open, close) = if raw.as_bytes()[start] == b'[' { ('[', ']') } else { ('{', '}') };

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
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
                    return serde_json::from_str(&raw[start..end]).ok();
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
    fn extract_unbalanced_array_salvages_inner_object() {
        // The array never closes, but the scan moves to the next opener and
        // rescues the complete object inside it.
        let v = extract_json(r#"[{"a": 1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn extract_fully_unbalanced_is_none() {
        assert!(extract_json(r#"[{"a": 1"#).is_none());
    }

    #[test]
    fn extract_skips_prose_brackets_before_the_payload() {
        // claude routinely narrates before the JSON; a bracketed fragment in
        // that prose must not abort extraction.
        let raw = r#"Here are today's events [3 total]:
[{"externalId":"e1","title":"standup","startTs":1}]"#;
        let v = extract_json(raw).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["title"], "standup");
    }

    #[test]
    fn extract_skips_unparseable_brace_fragment() {
        let raw = r#"I'll check {your} calendar: [{"title":"standup"}]"#;
        let v = extract_json(raw).unwrap();
        assert_eq!(v[0]["title"], "standup");
    }

    #[test]
    fn extract_preserves_fence_marker_inside_string_values() {
        // The old implementation rewrote the raw text to strip fences, which
        // corrupted fence markers inside JSON strings.
        let raw = "```json\n{\"title\": \"use ```json blocks\"}\n```";
        let v = extract_json(raw).unwrap();
        assert_eq!(v["title"], "use ```json blocks");
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
    fn collect_manual_runs_issues_and_prs_but_never_calendar() {
        let store = Store::open_in_memory().unwrap();
        let summaries = collect_manual(&store, &[], None, 1);
        let keys: Vec<&str> = summaries.iter().map(|s| s.collector.as_str()).collect();
        assert_eq!(keys, ["issues", "prs"], "manual refresh runs issues + PRs, skips calendar");
        assert!(!keys.contains(&"claude:calendar"), "calendar is never manually triggered");
    }

    #[test]
    fn collect_manual_includes_slack_only_when_configured() {
        let store = Store::open_in_memory().unwrap();
        // `watch_user_id` left empty so `collect_slack_dm` records its
        // misconfiguration failure and returns before any network call — enough
        // to prove the slack summary is appended when a config is supplied.
        let config = SlackDmConfig {
            token: "xoxp-test".to_string(),
            watch_user_id: String::new(),
            watch_name: String::new(),
        };
        let keys: Vec<String> = collect_manual(&store, &[], Some(&config), 1)
            .into_iter()
            .map(|s| s.collector)
            .collect();
        assert_eq!(keys, ["issues", "prs", "slack:dm"]);
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

    fn cal_event(ext: &str, start_ts: i64) -> EventInput {
        EventInput {
            external_id: ext.to_string(),
            title: format!("event {ext}"),
            start_ts,
            end_ts: Some(start_ts + 30 * 60 * 1000),
            attendees: vec![],
            location: None,
            join_url: None,
        }
    }

    /// Local noon on a fixed day, so a `now + 1h` event stays inside the same
    /// local calendar day on any machine's time zone (avoids midnight flakiness).
    fn local_noon_ms() -> i64 {
        use chrono::{Local, TimeZone};
        Local.with_ymd_and_hms(2026, 7, 12, 12, 0, 0).unwrap().timestamp_millis()
    }

    #[test]
    fn calendar_non_empty_result_replaces_events() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        store.replace_events(&[cal_event("old", now + 60 * 60 * 1000)], now).unwrap();

        let fresh = cal_event("new", now + 2 * 60 * 60 * 1000);
        let summary = store_calendar_events(&store, std::slice::from_ref(&fresh), now);

        assert!(summary.ok);
        assert_eq!(summary.count, 1);
        let stored = store.events_between(now, now + 24 * 60 * 60 * 1000).unwrap();
        assert_eq!(stored.len(), 1, "full replace swapped the old row for the new one");
        assert_eq!(stored[0].external_id, "new");
    }

    #[test]
    fn calendar_empty_result_with_future_events_preserves_and_fails() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        store.replace_events(&[cal_event("keep", now + 60 * 60 * 1000)], now).unwrap();

        let summary = store_calendar_events(&store, &[], now);

        assert!(!summary.ok, "a suspicious empty result is recorded as a failed run");
        assert_eq!(summary.count, 0);
        assert!(summary.message.unwrap().contains("kept existing events"));
        let stored = store.events_between(now, now + 24 * 60 * 60 * 1000).unwrap();
        assert_eq!(stored.len(), 1, "existing future events survive the empty sweep");
        assert_eq!(stored[0].external_id, "keep");
        // The failed run is recorded under the calendar key.
        let runs = store.runs().unwrap();
        assert_eq!(runs[0].collector, "claude:calendar");
        assert!(!runs[0].ok);
    }

    #[test]
    fn calendar_empty_result_with_no_future_events_clears_ok() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        // Only a past event today; nothing still upcoming.
        store.replace_events(&[cal_event("past", now - 60 * 60 * 1000)], now).unwrap();

        let summary = store_calendar_events(&store, &[], now);

        assert!(summary.ok, "a genuinely empty day is not suspect");
        assert_eq!(summary.count, 0);
        assert!(
            store
                .events_between(now - 24 * 60 * 60 * 1000, now + 24 * 60 * 60 * 1000)
                .unwrap()
                .is_empty(),
            "the empty result clears the stale past row"
        );
        let runs = store.runs().unwrap();
        assert_eq!(runs[0].collector, "claude:calendar");
        assert!(runs[0].ok);
    }

    #[test]
    fn calendar_empty_result_on_empty_store_is_clean_noop() {
        let store = Store::open_in_memory().unwrap();
        let now = local_noon_ms();
        let summary = store_calendar_events(&store, &[], now);
        assert!(summary.ok);
        assert_eq!(summary.count, 0);
    }

    fn issue(repo: &str, number: i64) -> tt_store::IssueInput {
        tt_store::IssueInput {
            repo: repo.to_string(),
            number,
            title: format!("issue {number}"),
            labels: vec![],
            state: "open".to_string(),
            url: format!("https://github.com/{repo}/issues/{number}"),
            updated_ts: 1,
        }
    }

    fn issue_write(
        store: &Store,
    ) -> impl Fn(&[tt_store::IssueInput], Option<&[String]>) -> tt_store::Result<usize> + '_ {
        |all, repos| match repos {
            None => store.replace_issues(all),
            Some(repos) => store.replace_issues_for_repos(repos, all),
        }
    }

    #[test]
    fn all_failed_sweep_preserves_existing_rows() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1)]).unwrap();

        let sweep = Sweep {
            successes: vec![],
            errors: vec!["gh: rate limited".to_string()],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );

        assert!(!summary.ok);
        assert_eq!(store.issues().unwrap().len(), 1, "last-known-good rows survive a dead sweep");
        assert!(summary.message.unwrap().contains("rate limited"));
    }

    #[test]
    fn partial_sweep_replaces_only_succeeded_repos() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/a", 1), issue("o/b", 2)]).unwrap();

        // o/a re-collected (fresh row 3); o/b errored.
        let sweep = Sweep {
            successes: vec![("o/a".to_string(), vec![issue("o/a", 3)])],
            errors: vec!["gh failed in /repos/b: boom".to_string()],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );

        assert!(!summary.ok, "a failed repo marks the run failed even though data was written");
        let issues = store.issues().unwrap();
        assert!(issues.iter().any(|i| i.repo == "o/a" && i.number == 3));
        assert!(issues.iter().any(|i| i.repo == "o/b" && i.number == 2), "failed repo keeps rows");
        assert!(!issues.iter().any(|i| i.number == 1), "succeeded repo's stale rows are gone");
    }

    #[test]
    fn clean_sweep_purges_untracked_repos() {
        let store = Store::open_in_memory().unwrap();
        store.replace_issues(&[issue("o/gone", 1)]).unwrap();

        let sweep = Sweep {
            successes: vec![("o/a".to_string(), vec![issue("o/a", 2)])],
            errors: vec![],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );

        assert!(summary.ok);
        let issues = store.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].repo, "o/a", "full replace drops repos no longer tracked");
    }

    #[test]
    fn sweep_dedups_same_repo_from_two_checkouts() {
        let store = Store::open_in_memory().unwrap();
        // Two worktree slots of one repo both succeed and report the same issue.
        let sweep = Sweep {
            successes: vec![
                ("o/a".to_string(), vec![issue("o/a", 1)]),
                ("o/a".to_string(), vec![issue("o/a", 1)]),
            ],
            errors: vec![],
            skipped: vec![],
        };
        let summary = finish_sweep(
            &store,
            "issues",
            sweep,
            issue_write(&store),
            |i| (i.repo.clone(), i.number),
            9,
        );
        assert!(summary.ok);
        assert_eq!(summary.count, 1);
        assert_eq!(store.issues().unwrap().len(), 1);
    }
}
