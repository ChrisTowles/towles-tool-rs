//! A Model Context Protocol (MCP) server for towles-tool, spoken over stdio.
//!
//! The transport is newline-delimited JSON-RPC 2.0: one request object per line
//! on stdin, one single-line response object per request on stdout (flushed
//! after each write), diagnostics on stderr via `log`. There is no async
//! runtime — [`serve`] is a hand-rolled blocking read loop over [`Dispatcher`],
//! which is the testable core: [`Dispatcher::handle_at`] takes the request line
//! plus an injected `now_ms` so tool logic never reads the clock itself.
//!
//! Exposed tools surface the towles-tool board ([`tt_store`]'s tasks — the
//! #339 unit of work): `task_list`, `task_status`, and the gated
//! `task_create`. The broader dashboard-read tools (`day_brief`, `needs_you`,
//! `snapshot`, `prs_status`, `issues_open`, `dm_status`, `collect_status`)
//! were pruned in the 2026-07 tool-surface review — the task family is the
//! surface a session actually needs; a calendar family may join it later.
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
//! The posture is therefore **read-only by default**: the ungated tools read
//! the user's own board, which is available to any of the user's own sessions
//! by design. The one mutating tool, `task_create`, sits behind the
//! capability gate ([`tt_config::McpSettings::mutations_enabled`]): a
//! settings-file opt-in, default off, that no tool exposed here can flip — so
//! prompt injection can't self-approve it — re-resolved from disk on every
//! call (a settings edit needs no server restart) and failing closed when the
//! file is unreadable. (This does not defend against a session with
//! unrestricted shell access being instructed to edit the settings file
//! directly and then call the gated tool — that session could just as well
//! run `sqlite3` against tt.db itself.) The previous generation of mutating
//! tools (`todo_*`, `journal_append`, `collect_refresh`) and the
//! cross-session `agent_sessions` tool showed zero real use in telemetry, so
//! the 2026-07 datamine removed them; `task_create` brought the gate
//! machinery back.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};
use tt_store::{McpCallInput, Store};

/// Tools gated behind [`tt_config::McpSettings::mutations_enabled`] — anything
/// that writes the store. Everything else is a read of the user's own board
/// and never touches the gate (or the settings file).
const MUTATING_TOOLS: &[&str] = &["task_create"];

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
    /// Injected capability gate (test hook). The stdio/CLI path leaves it
    /// `None` and re-resolves from disk on every gated call, so a settings
    /// edit takes effect without restarting the server.
    mcp_settings: Option<tt_config::McpSettings>,
    /// When set, the capability gate loads this settings file instead of the
    /// shared default — the CLI's global `--config-dir` flag lands here, so
    /// `tt --config-dir <dir> mcp serve` really is isolated from the machine's
    /// settings (and tests can exercise the real disk-resolution path against
    /// a sandbox).
    settings_path: Option<PathBuf>,
    /// Injected tracked-repo dirs (test hook). The stdio/CLI path leaves it
    /// `None` and re-reads the shared agentboard `repos.json` on every
    /// `task_create`, so newly tracked repos are creatable without a restart.
    tracked_repos: Option<Vec<String>>,
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
        Dispatcher {
            store,
            client: None,
            mcp_settings: None,
            settings_path: None,
            tracked_repos: None,
        }
    }

    /// Build a dispatcher with a fixed capability gate (test hook — keeps the
    /// gate off the real settings file).
    pub fn with_mcp_settings(mut self, settings: tt_config::McpSettings) -> Dispatcher {
        self.mcp_settings = Some(settings);
        self
    }

    /// Point the capability gate at an explicit settings file — how the CLI's
    /// global `--config-dir` flag reaches the server.
    pub fn with_settings_path(mut self, path: PathBuf) -> Dispatcher {
        self.settings_path = Some(path);
        self
    }

    /// Build a dispatcher with a fixed tracked-repo list (test hook — keeps
    /// `task_create`'s repo validation off the real agentboard `repos.json`).
    pub fn with_tracked_repos(mut self, repos: Vec<String>) -> Dispatcher {
        self.tracked_repos = Some(repos);
        self
    }

    /// The capability gate to enforce for this call: the injected override if
    /// any, else re-resolved from the settings file every time (so a settings
    /// edit takes effect without restarting the server). Like every other
    /// consumer of `tt_config`, a missing file is created with defaults on
    /// first use.
    fn mcp_capabilities(&self) -> Result<tt_config::McpSettings, String> {
        match self.mcp_settings {
            Some(settings) => Ok(settings),
            None => match &self.settings_path {
                Some(path) => Ok(tt_config::load_from(path).map_err(|e| e.to_string())?.mcp),
                None => Ok(tt_config::load().map_err(|e| e.to_string())?.mcp),
            },
        }
    }

    /// The settings-file path named in the gate's refusal messages, honoring
    /// [`Dispatcher::with_settings_path`] so the hint points at the file the
    /// gate actually read.
    fn settings_hint(&self) -> String {
        match &self.settings_path {
            Some(path) => path.display().to_string(),
            None => settings_path_hint(),
        }
    }

    /// The tracked-repo dirs `task_create` validates against: the injected
    /// override if any, else re-read from the shared agentboard `repos.json`
    /// every time (so a newly tracked repo is creatable without a restart).
    fn tracked_repo_dirs(&self) -> Vec<String> {
        match &self.tracked_repos {
            Some(repos) => repos.clone(),
            None => tt_agentboard::repos::load_repos(&tt_agentboard::repos::default_repos_path()),
        }
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
        // Only resolve the capability gate (a settings-file read) for the
        // mutating tools — the read-only board tools never touch it. An
        // unreadable settings file fails closed with an actionable refusal
        // rather than leaking a raw parse error that reads as transient.
        if MUTATING_TOOLS.contains(&name) {
            match self.mcp_capabilities() {
                Ok(caps) if caps.mutations_enabled => {}
                Ok(_) => return Err(mutations_disabled_message(name, &self.settings_hint())),
                Err(error) => {
                    return Err(capabilities_unreadable_message(
                        name,
                        &error,
                        &self.settings_hint(),
                    ));
                }
            }
        }
        match name {
            "task_list" => self.task_list(),
            "task_status" => self.task_status(args),
            "task_create" => self.task_create(args, now_ms),
            other => Err(format!("unknown tool: {other}")),
        }
    }

    /// Open (not-done) board tasks in board order, each with its issue/PR
    /// links and repo/slot binding.
    fn task_list(&self) -> Result<Value, String> {
        let tasks = self.store.open_tasks().map_err(|e| e.to_string())?;
        Ok(json!({ "tasks": tasks }))
    }

    /// One task by id — the full row including done tasks, which `task_list`
    /// excludes.
    fn task_status(&self, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: id".to_string())?;
        let task = self.store.task_by_id(id).map_err(|_| format!("no task with id {id}"))?;
        Ok(json!({ "task": task }))
    }

    /// Create a board task in a tracked repo — the same store path as the
    /// app's Agentboard `+` flow: [`Store::add_task`] then a repo-only
    /// [`Store::set_task_slot`], so the task lands in that repo's Board
    /// swimlane immediately (no worktree yet).
    fn task_create(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| "missing required argument: title".to_string())?;
        let repo_arg = args
            .get("repo")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
            .ok_or_else(|| "missing required argument: repo".to_string())?;
        let status = args.get("status").and_then(Value::as_str).unwrap_or("backlog");
        let notes = args.get("notes").and_then(Value::as_str);

        let repos = self.tracked_repo_dirs();
        let entries = tt_agentboard::repos::repo_entries(&repos);
        let entry = entries
            .iter()
            .find(|entry| entry.dir == repo_arg || entry.name == repo_arg)
            .ok_or_else(|| unknown_repo_message(repo_arg, &entries))?;

        let task = self.store.add_task(title, status, notes, now_ms).map_err(|e| e.to_string())?;
        self.store
            .set_task_slot(task.id, &entry.dir, None, None, None)
            .map_err(|e| e.to_string())?;
        let task = self.store.task_by_id(task.id).map_err(|e| e.to_string())?;
        tracing::info!(task_id = task.id, repo = %entry.dir, %status, "task.created");
        Ok(json!({ "task": task }))
    }
}

/// Run the stdio server: open the store (at `store_path` when given, else the
/// default location), then read/dispatch/write until EOF. `settings_path`
/// overrides where the capability gate reads settings (the CLI's global
/// `--config-dir`). Returns a process exit code (0 on clean EOF, 1 on a fatal
/// IO/store error).
pub fn serve(store_path: Option<&Path>, settings_path: Option<&Path>) -> i32 {
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
    if let Some(path) = settings_path {
        dispatcher = dispatcher.with_settings_path(path.to_path_buf());
    }
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

/// Path to the shared settings file, for use in the refusal messages below —
/// best-effort; falls back to a description if it can't be resolved.
fn settings_path_hint() -> String {
    tt_config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "your towles-tool.settings.json".to_string())
}

/// The gate's refusal when mutations are off (the default).
fn mutations_disabled_message(tool: &str, path_hint: &str) -> String {
    format!(
        "{tool} is disabled: tt-mcp's mutating tools are off until you opt in. \
         Set \"mcp\": {{\"mutationsEnabled\": true}} in {path_hint} — tt-mcp deliberately \
         exposes no tool that can change that setting."
    )
}

/// The gate's refusal when the settings file can't be read: fail closed with a
/// hint at the file, rather than surfacing a raw parse error that reads as a
/// transient failure worth retrying.
fn capabilities_unreadable_message(tool: &str, error: &str, path_hint: &str) -> String {
    format!(
        "{tool} is disabled: the capability gate could not read {path_hint} ({error}). \
         Mutations stay off until the settings file is readable."
    )
}

/// The repo-validation refusal: names the argument and lists what *is*
/// tracked, so a caller can self-correct without another round trip.
fn unknown_repo_message(repo: &str, entries: &[tt_agentboard::repos::RepoEntry]) -> String {
    if entries.is_empty() {
        return format!(
            "unknown repo: {repo} — no repos are tracked yet (add one on the app's Agentboard)"
        );
    }
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    format!("unknown repo: {repo} — tracked repos: {}", names.join(", "))
}

/// JSON Schema tool descriptors returned by `tools/list` — the MCP contract's
/// single source of truth. Also called directly by the app's `mcp_tool_docs`
/// command so the MCP screen's tool documentation can never drift from what
/// `tt mcp serve` actually exposes.
pub fn tool_definitions() -> Value {
    let no_args = || json!({ "type": "object", "properties": {}, "required": [] });
    json!([
        {
            "name": "task_list",
            "description": "Open (not-done) board tasks in board order, each with its issue/PR links and repo/slot binding.",
            "inputSchema": no_args(),
        },
        {
            "name": "task_status",
            "description": "One board task by id — the full row (status, links, repo/slot binding), including done tasks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The task's id (from task_list or task_create)." },
                },
                "required": ["id"],
            },
        },
        {
            "name": "task_create",
            "description": "Create a board task in a tracked repo's swimlane. Gated: requires \"mcp\": {\"mutationsEnabled\": true} in the settings file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "The tracked repo — its Agentboard name (dir basename) or absolute path." },
                    "title": { "type": "string", "description": "The task's title." },
                    "notes": { "type": "string", "description": "Optional free-form context." },
                    "status": { "type": "string", "enum": ["backlog", "next", "doing", "review", "done"], "description": "Column to land in (default backlog)." },
                },
                "required": ["repo", "title"],
            },
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000; // fixed epoch ms for deterministic tests
    const HOUR_MS: i64 = 3_600_000;

    /// The tracked-repo dir every test dispatcher knows about.
    const REPO_DIR: &str = "/home/u/code/demo";

    fn seeded_store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store.add_task("open task", "backlog", None, NOW).unwrap();
        store
    }

    /// A dispatcher with the gate ON and one tracked repo — both injected, so
    /// no test touches the real settings file or agentboard repos.json.
    fn dispatcher() -> Dispatcher {
        Dispatcher::new(seeded_store())
            .with_mcp_settings(tt_config::McpSettings { mutations_enabled: true })
            .with_tracked_repos(vec![REPO_DIR.to_string()])
    }

    /// Call a tool and return the parsed inner JSON result (the `text` payload).
    fn call_tool(dispatcher: &mut Dispatcher, name: &str, args: Value) -> Value {
        let response = call_tool_raw(dispatcher, name, args);
        assert_eq!(response["result"]["isError"], Value::Null, "unexpected tool error");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    /// Call a tool expecting an `isError` result; returns the error text.
    fn call_tool_err(dispatcher: &mut Dispatcher, name: &str, args: Value) -> String {
        let response = call_tool_raw(dispatcher, name, args);
        assert_eq!(response["result"]["isError"], true, "expected a tool error: {response}");
        response["result"]["content"][0]["text"].as_str().unwrap().to_string()
    }

    fn call_tool_raw(dispatcher: &mut Dispatcher, name: &str, args: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
        .to_string();
        let response = dispatcher.handle_at(&request, NOW).expect("tool call returns a response");
        serde_json::from_str(&response).unwrap()
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
    fn tools_list_is_exactly_the_task_family() {
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
        assert_eq!(names, vec!["task_list", "task_status", "task_create"]);
    }

    #[test]
    fn every_mutating_tool_is_defined_and_flagged_gated() {
        // MUTATING_TOOLS and tool_definitions can't drift: every gated name
        // must be a real tool whose description says it is gated.
        let tools = tool_definitions();
        for name in MUTATING_TOOLS {
            let tool = tools
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == *name)
                .unwrap_or_else(|| panic!("{name} is gated but not defined"));
            assert!(
                tool["description"].as_str().unwrap().contains("Gated"),
                "{name}'s description should say it is gated"
            );
        }
    }

    #[test]
    fn task_list_returns_seeded_task() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "task_list", json!({}));
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["text"], "open task");
    }

    #[test]
    fn task_status_returns_one_task_including_done() {
        let store = seeded_store();
        let done = store.add_task("shipped", "done", None, NOW).unwrap();
        let mut dispatcher = Dispatcher::new(store);
        let result = call_tool(&mut dispatcher, "task_status", json!({ "id": done.id }));
        assert_eq!(result["task"]["text"], "shipped");
        assert_eq!(result["task"]["status"], "done");
    }

    #[test]
    fn task_status_requires_a_known_id() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "task_status", json!({}));
        assert!(message.contains("id"), "error should name the missing arg: {message}");
        let message = call_tool_err(&mut dispatcher, "task_status", json!({ "id": 9999 }));
        assert!(message.contains("9999"), "error should name the unknown id: {message}");
    }

    #[test]
    fn task_create_lands_in_the_repo_swimlane() {
        let mut dispatcher = dispatcher();
        let result = call_tool(
            &mut dispatcher,
            "task_create",
            json!({ "repo": "demo", "title": "port the CLI", "notes": "start with doctor" }),
        );
        assert_eq!(result["task"]["text"], "port the CLI");
        assert_eq!(result["task"]["status"], "backlog");
        assert_eq!(result["task"]["notes"], "start with doctor");
        assert_eq!(result["task"]["createdAt"], NOW);
        // The repo binding is what puts the task in a Board swimlane.
        assert_eq!(result["task"]["slot"]["repoRoot"], REPO_DIR);

        // The new task shows up in the task_list read tool.
        let open = call_tool(&mut dispatcher, "task_list", json!({}));
        let texts: Vec<&str> =
            open["tasks"].as_array().unwrap().iter().map(|t| t["text"].as_str().unwrap()).collect();
        assert!(texts.contains(&"port the CLI"), "created task missing: {texts:?}");
    }

    #[test]
    fn task_create_accepts_the_repo_dir_and_a_status() {
        let mut dispatcher = dispatcher();
        let result = call_tool(
            &mut dispatcher,
            "task_create",
            json!({ "repo": REPO_DIR, "title": "already underway", "status": "doing" }),
        );
        assert_eq!(result["task"]["status"], "doing");
        assert_eq!(result["task"]["slot"]["repoRoot"], REPO_DIR);
    }

    #[test]
    fn task_create_rejects_an_untracked_repo() {
        let mut dispatcher = dispatcher();
        let message =
            call_tool_err(&mut dispatcher, "task_create", json!({ "repo": "nope", "title": "x" }));
        assert!(message.contains("unknown repo: nope"), "{message}");
        assert!(message.contains("demo"), "error should list tracked repos: {message}");

        let mut empty = Dispatcher::new(seeded_store())
            .with_mcp_settings(tt_config::McpSettings { mutations_enabled: true })
            .with_tracked_repos(vec![]);
        let message =
            call_tool_err(&mut empty, "task_create", json!({ "repo": "demo", "title": "x" }));
        assert!(message.contains("no repos are tracked"), "{message}");
    }

    #[test]
    fn task_create_requires_title_and_repo() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "task_create", json!({ "repo": "demo" }));
        assert!(message.contains("title"), "error should name the missing arg: {message}");
        let message =
            call_tool_err(&mut dispatcher, "task_create", json!({ "repo": "demo", "title": " " }));
        assert!(message.contains("title"), "blank title should be rejected: {message}");
        let message = call_tool_err(&mut dispatcher, "task_create", json!({ "title": "x" }));
        assert!(message.contains("repo"), "error should name the missing arg: {message}");
    }

    #[test]
    fn task_create_rejects_a_bogus_status() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(
            &mut dispatcher,
            "task_create",
            json!({ "repo": "demo", "title": "x", "status": "bogus" }),
        );
        assert!(message.contains("bogus"), "{message}");
        // Nothing was created.
        let open = call_tool(&mut dispatcher, "task_list", json!({}));
        assert_eq!(open["tasks"].as_array().unwrap().len(), 1, "only the seeded task remains");
    }

    #[test]
    fn task_create_is_gated_off_by_default() {
        // A settings file in a sandbox dir with no mcp block: the gate reads
        // the real disk-resolution path and refuses with the opt-in hint.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        let mut dispatcher = Dispatcher::new(seeded_store())
            .with_settings_path(path.clone())
            .with_tracked_repos(vec![REPO_DIR.to_string()]);
        let message =
            call_tool_err(&mut dispatcher, "task_create", json!({ "repo": "demo", "title": "x" }));
        assert!(message.contains("mutationsEnabled"), "refusal carries the opt-in hint: {message}");
        assert!(
            message.contains(&path.display().to_string()),
            "refusal names the settings file the gate read: {message}"
        );
        // Nothing was created, and the read tools still work ungated.
        let open = call_tool(&mut dispatcher, "task_list", json!({}));
        assert_eq!(open["tasks"].as_array().unwrap().len(), 1, "only the seeded task remains");
    }

    #[test]
    fn task_create_honors_a_settings_file_opt_in() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        std::fs::write(&path, r#"{"mcp":{"mutationsEnabled":true}}"#).unwrap();
        let mut dispatcher = Dispatcher::new(seeded_store())
            .with_settings_path(path)
            .with_tracked_repos(vec![REPO_DIR.to_string()]);
        let result =
            call_tool(&mut dispatcher, "task_create", json!({ "repo": "demo", "title": "opted" }));
        assert_eq!(result["task"]["text"], "opted");
    }

    #[test]
    fn task_create_fails_closed_when_settings_are_unreadable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let mut dispatcher = Dispatcher::new(seeded_store())
            .with_settings_path(path)
            .with_tracked_repos(vec![REPO_DIR.to_string()]);
        let message =
            call_tool_err(&mut dispatcher, "task_create", json!({ "repo": "demo", "title": "x" }));
        assert!(message.contains("could not read"), "fails closed, actionably: {message}");
    }

    #[test]
    fn removed_tools_are_unknown() {
        // The 2026-07 datamine (mutating tools) and tool-surface review (the
        // broad dashboard reads) removed these outright; a straggling client
        // gets a plain unknown-tool refusal, not a capability hint.
        let mut dispatcher = dispatcher();
        for tool in [
            "todo_create",
            "journal_append",
            "collect_refresh",
            "agent_sessions",
            "tasks_open",
            "issues_open",
            "prs_status",
            "dm_status",
            "day_brief",
            "needs_you",
            "snapshot",
            "collect_status",
        ] {
            let message = call_tool_err(&mut dispatcher, tool, json!({}));
            assert!(message.contains("unknown tool"), "{tool}: {message}");
        }
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
                "params": { "name": "task_list", "arguments": { "why": "ship it" } },
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

        // The successful task_list call, with its compacted args and ts.
        assert_eq!(calls[1].tool.as_deref(), Some("task_list"));
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
    fn gate_refusals_land_in_the_call_log() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        let mut dispatcher = Dispatcher::new(seeded_store())
            .with_settings_path(path)
            .with_tracked_repos(vec![REPO_DIR.to_string()]);
        drive(
            &mut dispatcher,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "task_create", "arguments": { "repo": "demo", "title": "x" } },
            }),
        );
        let calls = dispatcher.store.mcp_calls(10).unwrap();
        assert_eq!(calls[0].tool.as_deref(), Some("task_create"));
        assert!(!calls[0].ok);
        assert!(
            calls[0].error.as_deref().is_some_and(|e| e.contains("mutationsEnabled")),
            "the refusal is the recorded error: {:?}",
            calls[0].error
        );
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
