//! A Model Context Protocol (MCP) server for towles-tool, spoken over stdio.
//!
//! The transport is newline-delimited JSON-RPC 2.0: one request object per line
//! on stdin, one single-line response object per request on stdout (flushed
//! after each write), diagnostics on stderr via `log`. There is no async
//! runtime — [`serve`] is a hand-rolled blocking read loop over [`Dispatcher`],
//! which is the testable core: [`Dispatcher::handle_at`] takes the request line
//! plus an injected `now_ms` so tool logic never reads the clock itself.
//!
//! Exposed tools surface the towles-tool "personal dashboard" ([`tt_store`]).
//!
//! ## Trust boundary
//!
//! `tt` is registered with Claude Code at **user scope**
//! (`claude mcp add --scope user`), so this server is spawned fresh for
//! *every* Claude Code session on the machine, in any project, with no
//! awareness of which one. The threat that matters here isn't an
//! unauthenticated third party reaching the stdio pipe — it isn't
//! network-reachable, so every call already comes from a genuinely local,
//! genuinely-authenticated Claude Code process. It's that same legitimate
//! process's model getting its instructions hijacked by content it reads
//! mid-session (a hostile GitHub issue/PR body, a fetched web page) while
//! working on something unrelated, then calling a tool whose reach is the
//! whole machine.
//!
//! The posture is therefore **read-only by construction**: every exposed tool
//! reads the aggregate personal dashboard (PRs, issues, DMs, todos, collector
//! status) and nothing else — that's the whole point of "ask what's on my
//! plate from anywhere," and a read of the user's own dashboard is available
//! to any of the user's own sessions by design. There used to be a set of
//! mutating tools (`todo_*`, `journal_append`, `collect_refresh`) and a
//! cross-session `agent_sessions` tool behind an opt-in capability gate
//! (`mcp.mutationsEnabled` / `mcp.agentSessionsEnabled`); telemetry showed
//! zero real use, so the 2026-07 datamine removed the tools and the gate
//! machinery with them. A future mutating tool must bring the gate back —
//! see git history for the design (settings-file opt-in that no exposed tool
//! can flip, so prompt injection can't self-approve).

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::Instant;

use serde_json::{Value, json};
use tt_store::{McpCallInput, Store};

pub mod needs_you;

/// How many doing/next todos `day_brief` surfaces.
const DAY_BRIEF_TODO_LIMIT: usize = 5;

/// Protocol version advertised when the client does not send one of its own.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Longest tool-args rendering kept in the call log; anything past this is
/// truncated with an ellipsis so a huge payload can't bloat tt.db.
const CALL_LOG_ARGS_MAX: usize = 400;

/// The stateful core of the server: owns the [`Store`] and dispatches JSON-RPC
/// requests to tool handlers. Kept free of stdio so it can be driven directly in
/// tests.
pub struct Dispatcher {
    store: Store,
    /// `clientInfo` from the session's `initialize` (e.g. `claude-code 2.1`),
    /// stamped onto call-log rows so the app's MCP screen can say who called.
    client: Option<String>,
}

/// The result of dispatching one request: the response line to write back, plus
/// the bits the call log needs — the tool name and compacted args (only set for
/// `tools/call`) and an `error` message that is `Some` exactly when the call
/// failed (a JSON-RPC error or an `isError` tool result).
struct Outcome {
    response: String,
    tool: Option<String>,
    args: Option<String>,
    error: Option<String>,
}

impl Outcome {
    fn ok(response: String) -> Outcome {
        Outcome { response, tool: None, args: None, error: None }
    }

    fn err(response: String, error: String) -> Outcome {
        Outcome { response, tool: None, args: None, error: Some(error) }
    }

    fn with_tool(mut self, tool: String, args: String) -> Outcome {
        self.tool = Some(tool);
        self.args = Some(args);
        self
    }
}

impl Dispatcher {
    /// Build a dispatcher over `store`.
    pub fn new(store: Store) -> Dispatcher {
        Dispatcher { store, client: None }
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

        // `initialize` carries the caller's identity; stamp it onto this and every
        // later call from the session so the app's MCP screen can say who called.
        if method == "initialize" {
            self.client = client_label(&value);
        }

        // Time the handler and capture the outcome so it can be logged. Instant is
        // a monotonic elapsed measurement at the transport boundary, not a
        // timestamp — the row's `ts` still comes from the injected `now_ms`.
        let started = Instant::now();
        let outcome = match method {
            "initialize" => Outcome::ok(success_response(id, initialize_result(&value))),
            "ping" => Outcome::ok(success_response(id, json!({}))),
            "tools/list" => {
                Outcome::ok(success_response(id, json!({ "tools": tool_definitions() })))
            }
            "tools/call" => self.tools_call(id, &value, now_ms),
            _ => Outcome::err(
                error_response(id, -32601, "Method not found"),
                "Method not found".to_string(),
            ),
        };

        let call = McpCallInput {
            method: method.to_string(),
            tool: outcome.tool,
            args: outcome.args,
            ok: outcome.error.is_none(),
            error: outcome.error,
            duration_ms: Some(started.elapsed().as_millis() as i64),
            client: self.client.clone(),
        };
        if let Err(error) = self.store.record_mcp_call(&call, now_ms) {
            log::warn!("tt-mcp: failed to record call log: {error}");
        }

        Some(outcome.response)
    }

    /// Dispatch a `tools/call`: tool errors become an `isError` result (not a
    /// JSON-RPC error), per the MCP contract. The returned [`Outcome`] also
    /// carries the tool name and compacted args for the call log.
    fn tools_call(&mut self, id: Value, request: &Value, now_ms: i64) -> Outcome {
        let params = request.get("params");
        let name = match params.and_then(|p| p.get("name")).and_then(Value::as_str) {
            Some(name) => name.to_string(),
            None => {
                return Outcome::err(
                    tool_error_response(id, "tools/call is missing the tool name"),
                    "tools/call is missing the tool name".to_string(),
                );
            }
        };
        let args = params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
        let logged_args = compact_args(&args);
        match self.call_tool(&name, &args, now_ms) {
            Ok(value) => Outcome::ok(tool_result_response(id, &value)).with_tool(name, logged_args),
            Err(message) => Outcome::err(tool_error_response(id, &message), message)
                .with_tool(name, logged_args),
        }
    }

    fn call_tool(&mut self, name: &str, args: &Value, now_ms: i64) -> Result<Value, String> {
        // Every tool here is a read of the user's own dashboard — see the
        // module doc-comment's "Trust boundary" section before adding one
        // that mutates anything.
        let _ = args;
        match name {
            "tasks_open" => self.tasks_open(),
            "issues_open" => self.issues_open(),
            "prs_status" => self.prs_status(),
            "dm_status" => self.dm_status(),
            "day_brief" => self.day_brief(now_ms),
            "needs_you" => self.needs_you(),
            "snapshot" => self.snapshot(),
            "collect_status" => self.collect_status(now_ms),
            other => Err(format!("unknown tool: {other}")),
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

    /// Watched Slack DMs, each annotated with `needsReply` — true when the newest
    /// message is not the user's own and is newer than the last dismissal
    /// (`!fromMe && dismissedTs < ts`), matching the app's DM banner predicate.
    fn dm_status(&self) -> Result<Value, String> {
        let dms = self.store.dms().map_err(|e| e.to_string())?;
        let mut items = Vec::with_capacity(dms.len());
        for dm in &dms {
            let mut value = serde_json::to_value(dm).map_err(|e| e.to_string())?;
            if let Some(object) = value.as_object_mut() {
                object.insert("needsReply".to_string(), json!(needs_you::dm_needs_reply(dm)));
            }
            items.push(value);
        }
        Ok(json!({ "dms": items }))
    }

    /// One aggregate answer to "what's on my plate": the next meeting (title +
    /// minutes-until only, never a full agenda), PR review/CI counts, open-issue
    /// count, unanswered-DM count, the top few doing/next todos, and collector
    /// freshness. Pure composition over the existing store reads.
    fn day_brief(&self, now_ms: i64) -> Result<Value, String> {
        let next_meeting =
            match self.store.current_or_next_event(now_ms).map_err(|e| e.to_string())? {
                Some(event) => {
                    let minutes_until = (event.start_ts - now_ms) / 60_000;
                    let live =
                        event.start_ts <= now_ms && event.end_ts.is_some_and(|end| now_ms < end);
                    json!({
                        "title": event.title,
                        "startTs": event.start_ts,
                        "minutesUntil": minutes_until,
                        "live": live,
                    })
                }
                None => Value::Null,
            };

        let prs = self.store.prs().map_err(|e| e.to_string())?;
        let review_requested =
            prs.iter().filter(|pr| pr.review_state == "review_requested").count();
        let failing_checks =
            prs.iter().filter(|pr| pr.state != "merged" && pr.checks == "failing").count();

        let open_issues = self
            .store
            .issues()
            .map_err(|e| e.to_string())?
            .iter()
            .filter(|i| i.state == "open")
            .count();

        let dms = self.store.dms().map_err(|e| e.to_string())?;
        let unanswered_dms = dms.iter().filter(|dm| needs_you::dm_needs_reply(dm)).count();

        // Top todos from the two active columns: doing first, then next, each in
        // the store's board order, capped at DAY_BRIEF_TODO_LIMIT.
        let tasks = self.store.open_tasks().map_err(|e| e.to_string())?;
        let top_todos: Vec<Value> = tasks
            .iter()
            .filter(|t| t.status == "doing")
            .chain(tasks.iter().filter(|t| t.status == "next"))
            .take(DAY_BRIEF_TODO_LIMIT)
            .map(|t| serde_json::to_value(t).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;

        // Collector freshness: each run with the age of its last attempt.
        let collectors: Vec<Value> = self
            .store
            .runs()
            .map_err(|e| e.to_string())?
            .iter()
            .map(|run| {
                json!({
                    "collector": run.collector,
                    "ok": run.ok,
                    "ageMs": now_ms - run.ran_at,
                    "message": run.message,
                })
            })
            .collect();

        Ok(json!({
            "now": now_ms,
            "nextMeeting": next_meeting,
            "prs": { "reviewRequested": review_requested, "failingChecks": failing_checks },
            "issues": { "open": open_issues },
            "dms": { "unanswered": unanswered_dms },
            "todos": top_todos,
            "collectors": collectors,
        }))
    }

    /// The itemized, ranked attention feed (failing CI + DMs first, then review
    /// requests), each entry carrying a stable id, kind, label, and any URL. See
    /// [`needs_you::attention_feed`] for the ranking rules.
    fn needs_you(&self) -> Result<Value, String> {
        let prs = self.store.prs().map_err(|e| e.to_string())?;
        let dms = self.store.dms().map_err(|e| e.to_string())?;
        let items = needs_you::attention_feed(&prs, &dms);
        Ok(json!({ "items": items }))
    }

    /// The whole store in one call (events, tasks, issues, PRs, runs, DMs) for
    /// "what's on my plate" summaries.
    fn snapshot(&self) -> Result<Value, String> {
        let snapshot = self.store.snapshot().map_err(|e| e.to_string())?;
        serde_json::to_value(snapshot).map_err(|e| e.to_string())
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
/// default location), then read/dispatch/write until EOF. Returns a process
/// exit code (0 on clean EOF, 1 on a fatal IO/store error).
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

/// `name version` (or just `name`) from an `initialize` request's `clientInfo`,
/// or `None` when the caller sent no usable identity. Stamped onto the call log
/// so the app can show which client is driving the server.
fn client_label(request: &Value) -> Option<String> {
    let info = request.get("params")?.get("clientInfo")?;
    let name = info.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty())?;
    match info.get("version").and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()) {
        Some(version) => Some(format!("{name} {version}")),
        None => Some(name.to_string()),
    }
}

/// A compact one-line rendering of a tool's arguments for the call log, capped at
/// [`CALL_LOG_ARGS_MAX`] chars (truncated on a char boundary with an ellipsis) so
/// a huge payload can't bloat tt.db.
fn compact_args(args: &Value) -> String {
    let rendered = args.to_string();
    if rendered.chars().count() <= CALL_LOG_ARGS_MAX {
        return rendered;
    }
    let mut truncated: String = rendered.chars().take(CALL_LOG_ARGS_MAX).collect();
    truncated.push('…');
    truncated
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

/// JSON Schema tool descriptors returned by `tools/list` — the MCP contract's
/// single source of truth. Also called directly by the app's `mcp_tool_docs`
/// command so the MCP screen's tool documentation can never drift from what
/// `tt mcp serve` actually exposes.
pub fn tool_definitions() -> Value {
    let no_args = || json!({ "type": "object", "properties": {}, "required": [] });
    json!([
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
            "name": "dm_status",
            "description": "Watched Slack DMs, each annotated with needsReply (true when the newest message is not yours and newer than the last dismissal).",
            "inputSchema": no_args(),
        },
        {
            "name": "day_brief",
            "description": "One aggregate 'what's on my plate' answer: next meeting (title + minutes-until), PR review-requested/failing-check counts, open-issue count, unanswered-DM count, top doing/next todos, and collector freshness.",
            "inputSchema": no_args(),
        },
        {
            "name": "needs_you",
            "description": "The ranked attention feed: failing-CI PRs and unanswered DMs first, then review-requested PRs, each with a stable id, kind, label, and URL where available.",
            "inputSchema": no_args(),
        },
        {
            "name": "snapshot",
            "description": "The whole store in one call (events, tasks, issues, prs, runs, dms) for a 'what's on my plate' summary.",
            "inputSchema": no_args(),
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
    use tt_store::{DmInput, EventInput, IssueInput, PrInput};

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
        store.add_task("open task", Some(NOW + HOUR_MS), None, None, NOW).unwrap();
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
        call_tool_at(dispatcher, name, args, NOW)
    }

    /// Like {@link call_tool} but at an explicit `now_ms`, so time-dependent
    /// tools (e.g. `calendar_next`) can be exercised across the meeting lifecycle.
    fn call_tool_at(dispatcher: &mut Dispatcher, name: &str, args: Value, now_ms: i64) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
        .to_string();
        let response =
            dispatcher.handle_at(&request, now_ms).expect("tool call returns a response");
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
    fn tools_list_contains_all_read_only_tools() {
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
            "tasks_open",
            "issues_open",
            "prs_status",
            "dm_status",
            "day_brief",
            "needs_you",
            "snapshot",
            "collect_status",
        ] {
            assert!(names.contains(&expected), "missing tool {expected} in {names:?}");
        }
        assert_eq!(names.len(), 8);
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
    fn dm_status_flags_needs_reply() {
        let store = seeded_store();
        // Unanswered: their message, not dismissed → needsReply.
        store
            .upsert_dm(
                &DmInput {
                    channel: "D_UNANSWERED".to_string(),
                    from_name: "Ada".to_string(),
                    text: "ping?".to_string(),
                    ts: NOW,
                    from_me: false,
                    url: None,
                },
                NOW,
            )
            .unwrap();
        // Answered: my own most-recent message → not needsReply.
        store
            .upsert_dm(
                &DmInput {
                    channel: "D_ANSWERED".to_string(),
                    from_name: "Bob".to_string(),
                    text: "on it".to_string(),
                    ts: NOW,
                    from_me: true,
                    url: None,
                },
                NOW,
            )
            .unwrap();
        // Dismissed: their message but already marked handled → not needsReply.
        store
            .upsert_dm(
                &DmInput {
                    channel: "D_DISMISSED".to_string(),
                    from_name: "Cy".to_string(),
                    text: "fyi".to_string(),
                    ts: NOW,
                    from_me: false,
                    url: None,
                },
                NOW,
            )
            .unwrap();
        store.dismiss_dm("D_DISMISSED", NOW).unwrap();

        let mut dispatcher = Dispatcher::new(store);
        let result = call_tool(&mut dispatcher, "dm_status", json!({}));
        let by_channel = |channel: &str| -> bool {
            result["dms"].as_array().unwrap().iter().find(|d| d["channel"] == channel).unwrap()
                    ["needsReply"]
                    .as_bool()
                    .unwrap()
        };
        assert!(by_channel("D_UNANSWERED"), "unanswered DM should need a reply");
        assert!(!by_channel("D_ANSWERED"), "answered DM should not need a reply");
        assert!(!by_channel("D_DISMISSED"), "dismissed DM should not need a reply");
    }

    #[test]
    fn snapshot_returns_the_whole_store() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "snapshot", json!({}));
        assert!(result["events"].is_array(), "snapshot missing events: {result}");
        assert!(result["tasks"].is_array(), "snapshot missing tasks: {result}");
        assert_eq!(result["issues"].as_array().unwrap().len(), 2);
        assert_eq!(result["prs"].as_array().unwrap().len(), 1);
        assert_eq!(result["runs"].as_array().unwrap().len(), 1);
        assert!(result["dms"].is_array(), "snapshot missing dms: {result}");
    }

    /// Build a PR input row with explicit checks/review states.
    fn pr_input(number: i64, checks: &str, review_state: &str) -> PrInput {
        PrInput {
            repo: "o/r".to_string(),
            number,
            title: format!("PR {number}"),
            branch: "feat".to_string(),
            state: "open".to_string(),
            checks: checks.to_string(),
            review_state: review_state.to_string(),
            url: format!("https://example.com/pr/{number}"),
            updated_ts: NOW,
        }
    }

    fn unanswered_dm(store: &Store, channel: &str) {
        store
            .upsert_dm(
                &DmInput {
                    channel: channel.to_string(),
                    from_name: "Ada".to_string(),
                    text: "ping?".to_string(),
                    ts: NOW,
                    from_me: false,
                    url: Some(format!("https://slack.example/{channel}")),
                },
                NOW,
            )
            .unwrap();
    }

    #[test]
    fn day_brief_reflects_each_signal() {
        let store = Store::open_in_memory().unwrap();
        // Next meeting 30 minutes out (not live).
        store.replace_events(&[event("standup", NOW + 30 * 60_000)], NOW).unwrap();
        // One failing PR, one review-requested, one clean → counts 1 and 1.
        store
            .replace_prs(&[
                pr_input(1, "failing", "none"),
                pr_input(2, "passing", "review_requested"),
                pr_input(3, "passing", "approved"),
            ])
            .unwrap();
        // Two open issues, one closed → open count 2.
        store
            .replace_issues(&[
                issue(10, "open one"),
                issue(11, "open two"),
                IssueInput { state: "closed".to_string(), ..issue(12, "closed") },
            ])
            .unwrap();
        // One unanswered DM, one already answered → unanswered count 1.
        unanswered_dm(&store, "D_UNANSWERED");
        store
            .upsert_dm(
                &DmInput {
                    channel: "D_ANSWERED".to_string(),
                    from_name: "Bob".to_string(),
                    text: "done".to_string(),
                    ts: NOW,
                    from_me: true,
                    url: None,
                },
                NOW,
            )
            .unwrap();
        // Todos across columns: two doing, one next, one backlog.
        let doing_a = store.add_task("doing a", None, None, None, NOW).unwrap();
        let doing_b = store.add_task("doing b", None, None, None, NOW).unwrap();
        let next_a = store.add_task("next a", None, None, None, NOW).unwrap();
        store.add_task("backlog a", None, None, None, NOW).unwrap();
        store.set_task_status(doing_a.id, "doing", NOW).unwrap();
        store.set_task_status(doing_b.id, "doing", NOW).unwrap();
        store.set_task_status(next_a.id, "next", NOW).unwrap();
        store.record_run("prs", true, None, NOW - 5_000).unwrap();

        let mut dispatcher = Dispatcher::new(store);
        let result = call_tool(&mut dispatcher, "day_brief", json!({}));

        assert_eq!(result["now"], NOW);
        assert_eq!(result["nextMeeting"]["title"], "standup");
        assert_eq!(result["nextMeeting"]["minutesUntil"], 30);
        assert_eq!(result["nextMeeting"]["live"], false);
        assert_eq!(result["prs"]["failingChecks"], 1);
        assert_eq!(result["prs"]["reviewRequested"], 1);
        assert_eq!(result["issues"]["open"], 2);
        assert_eq!(result["dms"]["unanswered"], 1);

        // Only doing/next todos surface; backlog is excluded, doing before next.
        let todo_texts: Vec<&str> = result["todos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["text"].as_str().unwrap())
            .collect();
        assert_eq!(todo_texts, vec!["doing a", "doing b", "next a"]);

        let collectors = result["collectors"].as_array().unwrap();
        assert_eq!(collectors.len(), 1);
        assert_eq!(collectors[0]["collector"], "prs");
        assert_eq!(collectors[0]["ageMs"], 5_000);
    }

    #[test]
    fn day_brief_caps_todos_at_the_limit() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..8 {
            let task = store.add_task(&format!("t{i}"), None, None, None, NOW).unwrap();
            store.set_task_status(task.id, "doing", NOW).unwrap();
        }
        let mut dispatcher = Dispatcher::new(store);
        let result = call_tool(&mut dispatcher, "day_brief", json!({}));
        assert_eq!(result["todos"].as_array().unwrap().len(), DAY_BRIEF_TODO_LIMIT);
    }

    #[test]
    fn day_brief_over_empty_store_is_clean() {
        let mut dispatcher = Dispatcher::new(Store::open_in_memory().unwrap());
        let result = call_tool(&mut dispatcher, "day_brief", json!({}));
        assert_eq!(result["nextMeeting"], Value::Null);
        assert_eq!(result["prs"]["failingChecks"], 0);
        assert_eq!(result["prs"]["reviewRequested"], 0);
        assert_eq!(result["issues"]["open"], 0);
        assert_eq!(result["dms"]["unanswered"], 0);
        assert!(result["todos"].as_array().unwrap().is_empty());
        assert!(result["collectors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn needs_you_ranks_and_ids_items() {
        let store = Store::open_in_memory().unwrap();
        store
            .replace_prs(&[
                pr_input(2, "passing", "review_requested"),
                pr_input(1, "failing", "none"),
            ])
            .unwrap();
        unanswered_dm(&store, "D_PING");
        let mut dispatcher = Dispatcher::new(store);
        let result = call_tool(&mut dispatcher, "needs_you", json!({}));
        let items = result["items"].as_array().unwrap();
        let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
        let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["failing_ci", "dm", "review_requested"]);
        assert_eq!(ids, vec!["pr:o/r#1", "dm:D_PING", "pr:o/r#2"]);
        assert_eq!(items[0]["url"], "https://example.com/pr/1");
    }

    #[test]
    fn needs_you_over_empty_store_is_empty() {
        let mut dispatcher = Dispatcher::new(Store::open_in_memory().unwrap());
        let result = call_tool(&mut dispatcher, "needs_you", json!({}));
        assert!(result["items"].as_array().unwrap().is_empty());
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
    fn removed_mutating_tools_are_unknown() {
        // The 2026-07 datamine removed the mutating/gated tool families
        // outright; a straggling client calling one gets a plain unknown-tool
        // refusal, not a capability hint.
        let mut dispatcher = dispatcher();
        for tool in [
            "todo_create",
            "journal_append",
            "collect_refresh",
            "agent_sessions",
        ] {
            let request = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": tool, "arguments": {} },
            })
            .to_string();
            let response: Value =
                serde_json::from_str(&dispatcher.handle_at(&request, NOW).unwrap()).unwrap();
            assert_eq!(response["result"]["isError"], true, "{tool} should be unknown");
            let text = response["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("unknown tool"), "{tool}: {text}");
        }
    }

    /// Drive a raw request line through the dispatcher, discarding its response.
    fn drive(dispatcher: &mut Dispatcher, request: Value) {
        dispatcher.handle_at(&request.to_string(), NOW);
    }

    #[test]
    fn dispatch_records_initialize_client_and_tool_calls() {
        let mut dispatcher = dispatcher();

        // The session's initialize carries the caller identity for the log.
        drive(
            &mut dispatcher,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "clientInfo": { "name": "claude-code", "version": "2.1" } },
            }),
        );
        // A successful tool call, then a failing one (unknown tool).
        drive(
            &mut dispatcher,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "tasks_open", "arguments": { "why": "ship it" } },
            }),
        );
        drive(
            &mut dispatcher,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "does_not_exist", "arguments": {} },
            }),
        );

        let calls = dispatcher.store.mcp_calls(10).unwrap();
        assert_eq!(calls.len(), 3, "one row per handled request: {calls:?}");

        // Newest first: the failing unknown-tool call.
        assert_eq!(calls[0].method, "tools/call");
        assert_eq!(calls[0].tool.as_deref(), Some("does_not_exist"));
        assert!(!calls[0].ok);
        assert!(calls[0].error.is_some(), "failed call records an error");
        assert!(calls[0].duration_ms.is_some());
        // The client identity from initialize rides along on every later row.
        assert_eq!(calls[0].client.as_deref(), Some("claude-code 2.1"));

        // The successful tasks_open call, with its compacted args and ts.
        assert_eq!(calls[1].tool.as_deref(), Some("tasks_open"));
        assert!(calls[1].ok);
        assert_eq!(calls[1].error, None);
        assert_eq!(calls[1].ts, NOW);
        assert!(
            calls[1].args.as_deref().is_some_and(|a| a.contains("ship it")),
            "args should carry the payload: {:?}",
            calls[1].args
        );

        // The initialize request itself: recorded, no tool, client stamped.
        assert_eq!(calls[2].method, "initialize");
        assert_eq!(calls[2].tool, None);
        assert!(calls[2].ok);
        assert_eq!(calls[2].client.as_deref(), Some("claude-code 2.1"));
    }

    #[test]
    fn dispatch_records_unknown_method_as_error() {
        let mut dispatcher = dispatcher();
        drive(&mut dispatcher, json!({ "jsonrpc": "2.0", "id": 1, "method": "bogus/method" }));
        let calls = dispatcher.store.mcp_calls(10).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "bogus/method");
        assert!(!calls[0].ok);
        assert_eq!(calls[0].error.as_deref(), Some("Method not found"));
    }

    #[test]
    fn notifications_are_not_recorded() {
        let mut dispatcher = dispatcher();
        // A notification (no id) gets no response and no call-log row.
        drive(&mut dispatcher, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        assert!(dispatcher.store.mcp_calls(10).unwrap().is_empty());
    }

    #[test]
    fn compact_args_truncates_on_char_boundary() {
        let short = json!({ "title": "x" });
        assert_eq!(compact_args(&short), r#"{"title":"x"}"#);

        let big = json!({ "notes": "é".repeat(CALL_LOG_ARGS_MAX) });
        let out = compact_args(&big);
        assert!(out.ends_with('…'), "oversized args end with an ellipsis: {out}");
        assert_eq!(out.chars().count(), CALL_LOG_ARGS_MAX + 1);
    }
}
