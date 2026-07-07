//! A Model Context Protocol (MCP) server for towles-tool, spoken over stdio.
//!
//! The transport is newline-delimited JSON-RPC 2.0: one request object per line
//! on stdin, one single-line response object per request on stdout (flushed
//! after each write), diagnostics on stderr via `log`. There is no async
//! runtime — [`serve`] is a hand-rolled blocking read loop over [`Dispatcher`],
//! which is the testable core: [`Dispatcher::handle_at`] takes the request line
//! plus an injected `now_ms` so tool logic never reads the clock itself.
//!
//! Exposed tools surface the towles-tool "personal dashboard" ([`tt_store`]),
//! the journal ([`tt_journal`]), and the agentboard engine ([`tt_agentboard`]).

use std::io::{self, BufRead, Write};
use std::path::Path;

use chrono::{Duration, Local, NaiveDate, TimeZone};
use serde_json::{Value, json};
use tt_config::JournalSettings;
use tt_store::Store;

/// Protocol version advertised when the client does not send one of its own.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// The stateful core of the server: owns the [`Store`] and dispatches JSON-RPC
/// requests to tool handlers. Kept free of stdio so it can be driven directly in
/// tests.
pub struct Dispatcher {
    store: Store,
    /// When set, `journal_append` uses these settings instead of loading from
    /// disk. Tests inject a tempdir here so they never touch the real `$HOME`;
    /// the stdio/CLI path leaves it `None` and loads the shared settings file.
    journal_settings: Option<JournalSettings>,
}

impl Dispatcher {
    /// Build a dispatcher over `store`; `journal_append` loads settings from the
    /// shared config file on demand.
    pub fn new(store: Store) -> Dispatcher {
        Dispatcher { store, journal_settings: None }
    }

    /// Build a dispatcher with fixed [`JournalSettings`] (test hook — keeps
    /// `journal_append` off the real `$HOME`).
    pub fn with_journal_settings(store: Store, journal_settings: JournalSettings) -> Dispatcher {
        Dispatcher { store, journal_settings: Some(journal_settings) }
    }

    /// Handle one request line, reading the wall clock at the boundary. Returns
    /// the response line, or `None` for notifications (which get no response).
    pub fn handle(&mut self, request_json: &str) -> Option<String> {
        self.handle_at(request_json, now_ms())
    }

    /// Handle one request line with an injected `now_ms` (deterministic tests).
    pub fn handle_at(&mut self, request_json: &str, now_ms: i64) -> Option<String> {
        let value: Value = match serde_json::from_str(request_json) {
            Ok(value) => value,
            Err(_) => return Some(error_response(Value::Null, -32700, "Parse error")),
        };

        // A top-level array is a JSON-RPC batch. MCP 2025-06-18 removed batching,
        // so reject it with a single Invalid Request instead of letting the `id`
        // lookup below miss and silently drop it (which hangs a waiting client).
        if value.is_array() {
            return Some(error_response(Value::Null, -32600, "Invalid Request"));
        }

        // Requests carry an `id`; notifications do not, and receive no response.
        let id = match value.get("id") {
            Some(id) if !id.is_null() => id.clone(),
            _ => return None,
        };

        let method = match value.get("method").and_then(Value::as_str) {
            Some(method) => method,
            None => return Some(error_response(id, -32600, "Invalid Request")),
        };

        Some(match method {
            "initialize" => success_response(id, initialize_result(&value)),
            "ping" => success_response(id, json!({})),
            "tools/list" => success_response(id, json!({ "tools": tool_definitions() })),
            "tools/call" => self.tools_call(id, &value, now_ms),
            _ => error_response(id, -32601, "Method not found"),
        })
    }

    /// Dispatch a `tools/call`: tool errors become an `isError` result (not a
    /// JSON-RPC error), per the MCP contract.
    fn tools_call(&mut self, id: Value, request: &Value, now_ms: i64) -> String {
        let params = request.get("params");
        let name = match params.and_then(|p| p.get("name")).and_then(Value::as_str) {
            Some(name) => name.to_string(),
            None => return tool_error_response(id, "tools/call is missing the tool name"),
        };
        let args = params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
        match self.call_tool(&name, &args, now_ms) {
            Ok(value) => tool_result_response(id, &value),
            Err(message) => tool_error_response(id, &message),
        }
    }

    fn call_tool(&mut self, name: &str, args: &Value, now_ms: i64) -> Result<Value, String> {
        match name {
            "calendar_today" => self.calendar_today(now_ms),
            "calendar_next" => self.calendar_next(now_ms),
            "tasks_open" => self.tasks_open(),
            "issues_open" => self.issues_open(),
            "prs_status" => self.prs_status(),
            "agent_sessions" => self.agent_sessions(args, now_ms),
            "journal_append" => self.journal_append(args, now_ms),
            "collect_status" => self.collect_status(now_ms),
            other => Err(format!("unknown tool: {other}")),
        }
    }

    /// Events whose start falls within the local calendar day of `now_ms`.
    fn calendar_today(&self, now_ms: i64) -> Result<Value, String> {
        let (start, end) = local_day_bounds(now_ms);
        let events = self.store.events_between(start, end).map_err(|e| e.to_string())?;
        Ok(json!({ "events": events, "now": now_ms }))
    }

    /// The next event starting strictly after `now_ms`, with minutes-until.
    fn calendar_next(&self, now_ms: i64) -> Result<Value, String> {
        let after = now_ms.saturating_add(1);
        match self.store.next_event(after).map_err(|e| e.to_string())? {
            Some(event) => {
                let minutes_until = (event.start_ts - now_ms) / 60_000;
                Ok(json!({ "event": event, "minutesUntil": minutes_until, "now": now_ms }))
            }
            None => Ok(json!({ "event": Value::Null, "now": now_ms })),
        }
    }

    fn tasks_open(&self) -> Result<Value, String> {
        let tasks = self.store.open_tasks().map_err(|e| e.to_string())?;
        Ok(json!({ "tasks": tasks }))
    }

    fn issues_open(&self) -> Result<Value, String> {
        let issues = self.store.issues().map_err(|e| e.to_string())?;
        Ok(json!({ "issues": issues }))
    }

    fn prs_status(&self) -> Result<Value, String> {
        let prs = self.store.prs().map_err(|e| e.to_string())?;
        Ok(json!({ "prs": prs }))
    }

    /// Agentboard sessions from a one-shot engine scan, optionally filtered by
    /// `agentState.status`. Engine construction/scan is host- and env-dependent
    /// (reads `~/.claude`, shells out to `claude`); a panic there yields an empty
    /// list with a `message` rather than failing the call.
    fn agent_sessions(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let status_filter = args.get("status").and_then(Value::as_str).map(str::to_string);
        let scanned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut engine = tt_agentboard::engine::Engine::new();
            engine.scan_once(now_ms);
            engine.compute_payload(now_ms)
        }));
        let payload = match scanned {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(json!({ "sessions": [], "message": "agentboard engine unavailable" }));
            }
        };
        // Flatten the Repo → Folder → Session tree into one list, enriching each
        // session with its repo/folder/dir/branch context for callers.
        let mut sessions: Vec<Value> = Vec::new();
        for repo in &payload.repos {
            for folder in &repo.folders {
                for session in &folder.sessions {
                    let mut entry = serde_json::to_value(session).map_err(|e| e.to_string())?;
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("repo".into(), json!(repo.name));
                        obj.insert("folder".into(), json!(folder.name));
                        obj.insert("dir".into(), json!(folder.dir));
                        obj.insert("branch".into(), json!(folder.branch));
                    }
                    sessions.push(entry);
                }
            }
        }
        let filtered: Vec<Value> = match status_filter {
            Some(want) => sessions
                .into_iter()
                .filter(|session| {
                    session
                        .get("agentState")
                        .and_then(|state| state.get("status"))
                        .and_then(Value::as_str)
                        == Some(want.as_str())
                })
                .collect(),
            None => sessions,
        };
        Ok(json!({ "sessions": filtered }))
    }

    /// Append a timestamped line to today's daily note.
    fn journal_append(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required argument: text".to_string())?;
        let settings = match &self.journal_settings {
            Some(settings) => settings.clone(),
            None => tt_config::load().map_err(|e| e.to_string())?.journal_settings,
        };
        let (date, time_hhmm) = local_date_and_time(now_ms)
            .ok_or_else(|| "could not resolve local time".to_string())?;
        let path = tt_journal::entries::append_to_daily(&settings, date, &time_hhmm, text)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true, "path": path.to_string_lossy() }))
    }

    /// Collector run records, each annotated with `ageMs` (now minus `ranAt`).
    fn collect_status(&self, now_ms: i64) -> Result<Value, String> {
        let runs = self.store.runs().map_err(|e| e.to_string())?;
        let mut items = Vec::with_capacity(runs.len());
        for run in &runs {
            let mut value = serde_json::to_value(run).map_err(|e| e.to_string())?;
            if let Some(object) = value.as_object_mut() {
                object.insert("ageMs".to_string(), json!(now_ms - run.ran_at));
            }
            items.push(value);
        }
        Ok(json!({ "runs": items }))
    }
}

/// Run the stdio server: open the store (at `store_path` when given, else the
/// default location), then read/dispatch/write until EOF. Returns a process exit
/// code (0 on clean EOF, 1 on a fatal IO/store error).
pub fn serve(store_path: Option<&Path>) -> i32 {
    let store = match store_path {
        Some(path) => Store::open(path),
        None => Store::open_default(),
    };
    let store = match store {
        Ok(store) => store,
        Err(error) => {
            log::error!("tt-mcp: failed to open store: {error}");
            return 1;
        }
    };
    let mut dispatcher = Dispatcher::new(store);
    log::info!("tt-mcp: serving on stdio");

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // clean EOF
            Ok(_) => {
                let request = line.trim_end();
                if request.is_empty() {
                    continue;
                }
                if let Some(response) = dispatcher.handle(request)
                    && (writeln!(writer, "{response}").is_err() || writer.flush().is_err())
                {
                    break;
                }
            }
            Err(error) => {
                log::error!("tt-mcp: stdin read error: {error}");
                return 1;
            }
        }
    }
    0
}

// ---------------------------------------------------------------------------
// JSON-RPC / MCP response builders
// ---------------------------------------------------------------------------

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Build the `initialize` result, echoing the client's `protocolVersion` if any.
fn initialize_result(request: &Value) -> Value {
    let protocol_version = request
        .get("params")
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "towles-tool", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// Wrap a tool's JSON result as a `tools/call` success (pretty-printed text item).
fn tool_result_response(id: Value, value: &Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    success_response(id, json!({ "content": [{ "type": "text", "text": text }] }))
}

/// A `tools/call` result flagged `isError` with an explanatory text item.
fn tool_error_response(id: Value, message: &str) -> String {
    success_response(
        id,
        json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
    )
}

/// The `[start, end)` epoch-ms bounds of the local calendar day containing `now_ms`.
fn local_day_bounds(now_ms: i64) -> (i64, i64) {
    let Some(now) = Local.timestamp_millis_opt(now_ms).single() else {
        return (now_ms, now_ms);
    };
    let date = now.date_naive();
    let start = date.and_hms_opt(0, 0, 0).and_then(|dt| Local.from_local_datetime(&dt).single());
    let end = (date + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| Local.from_local_datetime(&dt).single());
    match (start, end) {
        (Some(start), Some(end)) => (start.timestamp_millis(), end.timestamp_millis()),
        _ => (now_ms, now_ms),
    }
}

/// The local date and `HH:MM` string for `now_ms`.
fn local_date_and_time(now_ms: i64) -> Option<(NaiveDate, String)> {
    let now = Local.timestamp_millis_opt(now_ms).single()?;
    Some((now.date_naive(), now.format("%H:%M").to_string()))
}

/// JSON Schema tool descriptors returned by `tools/list`.
fn tool_definitions() -> Value {
    let no_args = || json!({ "type": "object", "properties": {}, "required": [] });
    json!([
        {
            "name": "calendar_today",
            "description": "Calendar events starting within today's local calendar day.",
            "inputSchema": no_args(),
        },
        {
            "name": "calendar_next",
            "description": "The next upcoming calendar event and minutes until it starts.",
            "inputSchema": no_args(),
        },
        {
            "name": "tasks_open",
            "description": "Open (not-done) tasks, due-soonest first.",
            "inputSchema": no_args(),
        },
        {
            "name": "issues_open",
            "description": "Open GitHub issues assigned to me across tracked repos.",
            "inputSchema": no_args(),
        },
        {
            "name": "prs_status",
            "description": "Tracked pull-request status rows.",
            "inputSchema": no_args(),
        },
        {
            "name": "agent_sessions",
            "description": "Agentboard sessions, optionally filtered by agent status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Only return sessions whose agentState.status matches.",
                    },
                },
                "required": [],
            },
        },
        {
            "name": "journal_append",
            "description": "Append a timestamped line to today's daily note.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The line to append." },
                },
                "required": ["text"],
            },
        },
        {
            "name": "collect_status",
            "description": "Collector run records with the age of each run in ms.",
            "inputSchema": no_args(),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tt_store::{EventInput, IssueInput, PrInput};

    const NOW: i64 = 1_700_000_000_000; // fixed epoch ms for deterministic tests
    const HOUR_MS: i64 = 3_600_000;

    fn event(external_id: &str, start_ts: i64) -> EventInput {
        EventInput {
            external_id: external_id.to_string(),
            title: external_id.to_string(),
            start_ts,
            end_ts: Some(start_ts + 1000),
            attendees: vec![],
            location: None,
            join_url: None,
        }
    }

    fn issue(number: i64, title: &str) -> IssueInput {
        IssueInput {
            repo: "o/r".to_string(),
            number,
            title: title.to_string(),
            labels: vec!["bug".to_string()],
            state: "open".to_string(),
            url: format!("https://github.com/o/r/issues/{number}"),
            updated_ts: NOW,
        }
    }

    fn seeded_store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .replace_events(
                &[
                    event("today", NOW),                 // exactly now → within today's bounds
                    event("soon", NOW + HOUR_MS),        // next upcoming event
                    event("future", NOW + 25 * HOUR_MS), // different calendar day
                    event("past", NOW - 25 * HOUR_MS),   // different calendar day
                ],
                NOW,
            )
            .unwrap();
        store.add_task("open task", Some(NOW + HOUR_MS), NOW).unwrap();
        store.replace_issues(&[issue(390, "Refunds double-charge"), issue(391, "a11y")]).unwrap();
        store
            .replace_prs(&[PrInput {
                repo: "o/r".to_string(),
                number: 7,
                title: "Fix".to_string(),
                branch: "feat".to_string(),
                state: "open".to_string(),
                checks: "passing".to_string(),
                review_state: "approved".to_string(),
                url: "https://example.com/pr/7".to_string(),
                updated_ts: NOW,
            }])
            .unwrap();
        store.record_run("gcal", true, None, NOW - 60_000).unwrap();
        store
    }

    fn dispatcher() -> Dispatcher {
        Dispatcher::new(seeded_store())
    }

    /// Call a tool and return the parsed inner JSON result (the `text` payload).
    fn call_tool(dispatcher: &mut Dispatcher, name: &str, args: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
        .to_string();
        let response = dispatcher.handle_at(&request, NOW).expect("tool call returns a response");
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["isError"], Value::Null, "unexpected tool error");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn initialize_echoes_protocol_version_and_server_info() {
        let mut dispatcher = dispatcher();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" },
        })
        .to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(response["result"]["serverInfo"]["name"], "towles-tool");
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_defaults_protocol_version() {
        let mut dispatcher = dispatcher();
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }).to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["result"]["protocolVersion"], DEFAULT_PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_get_no_response() {
        let mut dispatcher = dispatcher();
        let initialized =
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
        assert!(dispatcher.handle_at(&initialized, NOW).is_none());
        let other = json!({ "jsonrpc": "2.0", "method": "some/notification" }).to_string();
        assert!(dispatcher.handle_at(&other, NOW).is_none());
    }

    #[test]
    fn ping_returns_empty_result() {
        let mut dispatcher = dispatcher();
        let request = json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }).to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["id"], 9);
        assert_eq!(response["result"], json!({}));
    }

    #[test]
    fn tools_list_contains_all_eight_tools() {
        let mut dispatcher = dispatcher();
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for expected in [
            "calendar_today",
            "calendar_next",
            "tasks_open",
            "issues_open",
            "prs_status",
            "agent_sessions",
            "journal_append",
            "collect_status",
        ] {
            assert!(names.contains(&expected), "missing tool {expected} in {names:?}");
        }
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn calendar_today_returns_only_todays_events() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "calendar_today", json!({}));
        assert_eq!(result["now"], NOW);
        let ids: Vec<&str> = result["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["externalId"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"today"), "today event missing: {ids:?}");
        assert!(!ids.contains(&"past"), "past event leaked: {ids:?}");
        assert!(!ids.contains(&"future"), "future event leaked: {ids:?}");
    }

    #[test]
    fn calendar_next_returns_soonest_with_minutes_until() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "calendar_next", json!({}));
        assert_eq!(result["event"]["externalId"], "soon");
        assert_eq!(result["minutesUntil"], 60);
    }

    #[test]
    fn tasks_open_returns_seeded_task() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "tasks_open", json!({}));
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["text"], "open task");
        assert_eq!(tasks[0]["dueTs"], NOW + HOUR_MS);
    }

    #[test]
    fn issues_open_returns_rows() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "issues_open", json!({}));
        let issues = result["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0]["repo"], "o/r");
        assert!(issues.iter().any(|i| i["number"] == 390));
    }

    #[test]
    fn prs_status_returns_rows() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "prs_status", json!({}));
        let prs = result["prs"].as_array().unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0]["number"], 7);
        assert_eq!(prs[0]["reviewState"], "approved");
    }

    #[test]
    fn collect_status_annotates_age() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "collect_status", json!({}));
        let runs = result["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["collector"], "gcal");
        assert_eq!(runs[0]["ageMs"], 60_000);
    }

    #[test]
    fn unknown_tool_returns_is_error_content() {
        let mut dispatcher = dispatcher();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "does_not_exist", "arguments": {} },
        })
        .to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"].as_str().unwrap().contains("does_not_exist")
        );
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let mut dispatcher = dispatcher();
        let request = json!({ "jsonrpc": "2.0", "id": 3, "method": "no/such" }).to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["id"], 3);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn broken_json_returns_parse_error_with_null_id() {
        let mut dispatcher = dispatcher();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at("{ not json", NOW).unwrap()).unwrap();
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["error"]["code"], -32700);
    }

    #[test]
    fn batch_array_returns_invalid_request() {
        // MCP 2025-06-18 dropped batching: a top-level array must get a single
        // Invalid Request response, not be silently dropped as a notification.
        let mut dispatcher = dispatcher();
        let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#;
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(batch, NOW).unwrap()).unwrap();
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn request_with_id_but_no_method_is_invalid_request() {
        let mut dispatcher = dispatcher();
        let request = json!({ "jsonrpc": "2.0", "id": 5 }).to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["id"], 5);
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn journal_append_writes_to_injected_settings() {
        let dir = TempDir::new().unwrap();
        let settings = JournalSettings {
            base_folder: dir.path().to_string_lossy().to_string(),
            template_dir: dir.path().join("templates").to_string_lossy().to_string(),
            ..Default::default()
        };
        let mut dispatcher = Dispatcher::with_journal_settings(seeded_store(), settings);
        let result = call_tool(&mut dispatcher, "journal_append", json!({ "text": "hello world" }));
        assert_eq!(result["ok"], true);
        let path = result["path"].as_str().unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("hello world"), "note missing appended line: {contents}");
    }

    #[test]
    fn journal_append_requires_text() {
        let dir = TempDir::new().unwrap();
        let settings = JournalSettings {
            base_folder: dir.path().to_string_lossy().to_string(),
            template_dir: dir.path().join("templates").to_string_lossy().to_string(),
            ..Default::default()
        };
        let mut dispatcher = Dispatcher::with_journal_settings(seeded_store(), settings);
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "journal_append", "arguments": {} },
        })
        .to_string();
        let response: Value =
            serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn agent_sessions_call_does_not_panic() {
        // Engine construction/scan is env-dependent; assert only that the call
        // returns a well-formed sessions payload (empty or otherwise).
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "agent_sessions", json!({}));
        assert!(result.get("sessions").is_some(), "expected a sessions field: {result}");
    }
}
