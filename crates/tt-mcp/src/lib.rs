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
//! whole machine. Neither "which directory is the caller in" nor a bearer
//! token in the environment can gate against that: both would be inherited
//! by (or visible to) the exact same hijacked session, so neither
//! distinguishes genuine user intent from injected intent.
//!
//! The boundary this server actually enforces: **read access to the
//! aggregate personal dashboard (calendar, PRs, issues, DMs, todos, collector
//! status) is available from any of the user's own Claude Code sessions by
//! design** — that's the whole point of "ask what's on my plate from
//! anywhere." But nothing that mutates the store/journal, shells out to `gh`
//! machine-wide, or exposes *other* sessions' live data is reachable until
//! the human opts in via [`tt_config::McpSettings`] — a settings-file edit
//! that no tool exposed here can perform, so no prompt injection can
//! self-approve it. See `Dispatcher::call_tool`'s gate and
//! [`tt_config::McpSettings`]'s doc comment for the mechanics. (This does not
//! defend against a session with unrestricted shell access being instructed
//! to edit the settings file directly and then call a gated tool — that
//! raises the bar a lot, since it's a visible, out-of-project, two-step
//! write rather than one opaque tool call, but it isn't airtight. A fully
//! airtight fix needs OS-level process isolation, which is out of scope
//! here.)

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{Duration, Local, NaiveDate, TimeZone};
use serde_json::{Value, json};
use tt_collect::SlackDmConfig;
use tt_config::JournalSettings;
use tt_store::{McpCallInput, Store};

pub mod needs_you;

/// How many doing/next todos `day_brief` surfaces.
const DAY_BRIEF_TODO_LIMIT: usize = 5;

/// Protocol version advertised when the client does not send one of its own.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Longest tool-args rendering kept in the call log; anything past this is
/// truncated with an ellipsis so a huge payload can't bloat tt.db.
const CALL_LOG_ARGS_MAX: usize = 400;

/// Tools gated behind [`tt_config::McpSettings::mutations_enabled`]: anything
/// that mutates the store/journal or shells out to `gh` machine-wide. See the
/// module doc-comment's "Trust boundary" section for why this exists.
const MUTATING_TOOLS: &[&str] = &[
    "todo_create",
    "todo_update",
    "todo_delete",
    "todo_clear_done",
    "todo_link_issue",
    "todo_set_status",
    "journal_append",
    "collect_refresh",
];

/// The one tool gated behind [`tt_config::McpSettings::agent_sessions_enabled`]:
/// it reports live data from every Claude Code session on the machine, not just
/// the caller's. A named const (not an inline literal) so the gate, the
/// description derivation in [`tool_definitions`], and the classification test
/// can never drift apart on a rename.
const AGENT_SESSIONS_TOOL: &str = "agent_sessions";

/// The stateful core of the server: owns the [`Store`] and dispatches JSON-RPC
/// requests to tool handlers. Kept free of stdio so it can be driven directly in
/// tests.
pub struct Dispatcher {
    store: Store,
    /// When set, `journal_append` uses these settings instead of loading from
    /// disk. Tests inject a tempdir here so they never touch the real `$HOME`;
    /// the stdio/CLI path leaves it `None` and loads the shared settings file.
    journal_settings: Option<JournalSettings>,
    /// When set, `collect_refresh` uses this instead of resolving the tracked
    /// repos + Slack config from disk. Tests inject an empty set so the manual
    /// sweep is a clean no-op that never shells out to `gh` or hits Slack; the
    /// stdio/CLI path leaves it `None` and resolves from the shared config.
    collect_config: Option<CollectConfig>,
    /// `clientInfo` from the session's `initialize` (e.g. `claude-code 2.1`),
    /// stamped onto call-log rows so the app's MCP screen can say who called.
    client: Option<String>,
    /// When set, gates mutating tools + `agent_sessions` against this instead
    /// of the shared settings file — see [`Dispatcher::with_mcp_capabilities`].
    /// The stdio/CLI path leaves it `None` and re-resolves from disk on every
    /// call, so a settings edit takes effect without restarting the server.
    mcp_settings: Option<tt_config::McpSettings>,
    /// When set, every on-demand settings read (the capability gate,
    /// `journal_append`, `collect_refresh`) loads this file instead of the
    /// shared default — the CLI's global `--config-dir` flag lands here, so
    /// `tt --config-dir <dir> mcp serve` really is isolated from the machine's
    /// settings (and tests can exercise the real disk-resolution path against
    /// a sandbox).
    settings_path: Option<PathBuf>,
}

/// The runtime inputs `collect_refresh` feeds to [`tt_collect::collect_manual`]:
/// the tracked repo checkouts and the Slack DM config (`None` when disabled).
struct CollectConfig {
    repo_dirs: Vec<PathBuf>,
    slack: Option<SlackDmConfig>,
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
    /// Build a dispatcher over `store`; `journal_append` and `collect_refresh`
    /// resolve their config from the shared settings file on demand.
    pub fn new(store: Store) -> Dispatcher {
        Dispatcher {
            store,
            journal_settings: None,
            collect_config: None,
            client: None,
            mcp_settings: None,
            settings_path: None,
        }
    }

    /// Build a dispatcher with fixed [`JournalSettings`] (test hook — keeps
    /// `journal_append` off the real `$HOME`).
    pub fn with_journal_settings(store: Store, journal_settings: JournalSettings) -> Dispatcher {
        Dispatcher { journal_settings: Some(journal_settings), ..Dispatcher::new(store) }
    }

    /// Build a dispatcher with fixed `collect_refresh` inputs (test hook — keeps
    /// the manual sweep off `gh`/Slack). An empty `repo_dirs` is a clean no-op.
    pub fn with_collect_config(
        store: Store,
        repo_dirs: Vec<PathBuf>,
        slack: Option<SlackDmConfig>,
    ) -> Dispatcher {
        Dispatcher {
            collect_config: Some(CollectConfig { repo_dirs, slack }),
            ..Dispatcher::new(store)
        }
    }

    /// Override the [`tt_config::McpSettings`] capability gate (test hook —
    /// keeps the gate off the real settings file; also lets a test flip a
    /// tool on/off deterministically instead of depending on this machine's
    /// config). The stdio/CLI path never calls this, so `serve()` always
    /// re-resolves from disk per call.
    pub fn with_mcp_capabilities(mut self, mcp_settings: tt_config::McpSettings) -> Dispatcher {
        self.mcp_settings = Some(mcp_settings);
        self
    }

    /// Point every on-demand settings read (the capability gate,
    /// `journal_append`, `collect_refresh`) at an explicit settings file —
    /// how the CLI's global `--config-dir` flag reaches the server.
    pub fn with_settings_path(mut self, path: PathBuf) -> Dispatcher {
        self.settings_path = Some(path);
        self
    }

    /// Load the shared settings, honoring [`Dispatcher::with_settings_path`].
    /// Like every other consumer of `tt_config`, a missing file is created
    /// with defaults on first use.
    fn load_settings(&self) -> Result<tt_config::UserSettings, String> {
        match &self.settings_path {
            Some(path) => tt_config::load_from(path).map_err(|e| e.to_string()),
            None => tt_config::load().map_err(|e| e.to_string()),
        }
    }

    /// The capability gate to enforce for this call: the injected override if
    /// any, else re-resolved from the settings file every time (so a settings
    /// edit takes effect without restarting the server).
    fn mcp_capabilities(&self) -> Result<tt_config::McpSettings, String> {
        match self.mcp_settings {
            Some(settings) => Ok(settings),
            None => Ok(self.load_settings()?.mcp),
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
        // Only resolve the capability gate (a settings-file read) for the 9
        // gated tools — the 10 read-only dashboard tools never touch it. An
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
        if name == AGENT_SESSIONS_TOOL {
            match self.mcp_capabilities() {
                Ok(caps) if caps.agent_sessions_enabled => {}
                Ok(_) => return Err(agent_sessions_disabled_message(&self.settings_hint())),
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
            "calendar_today" => self.calendar_today(now_ms),
            "calendar_next" => self.calendar_next(now_ms),
            "tasks_open" => self.tasks_open(),
            "todo_create" => self.todo_create(args, now_ms),
            "todo_update" => self.todo_update(args),
            "todo_delete" => self.todo_delete(args),
            "todo_clear_done" => self.todo_clear_done(args, now_ms),
            "todo_link_issue" => self.todo_link_issue(args),
            "todo_set_status" => self.todo_set_status(args, now_ms),
            "issues_open" => self.issues_open(),
            "prs_status" => self.prs_status(),
            "dm_status" => self.dm_status(),
            "day_brief" => self.day_brief(now_ms),
            "needs_you" => self.needs_you(),
            "snapshot" => self.snapshot(),
            "agent_sessions" => self.agent_sessions(args, now_ms),
            "journal_append" => self.journal_append(args, now_ms),
            "collect_status" => self.collect_status(now_ms),
            "collect_refresh" => self.collect_refresh(now_ms),
            other => Err(format!("unknown tool: {other}")),
        }
    }

    /// Events whose start falls within the local calendar day of `now_ms`.
    fn calendar_today(&self, now_ms: i64) -> Result<Value, String> {
        let (start, end) = local_day_bounds(now_ms);
        let events = self.store.events_between(start, end).map_err(|e| e.to_string())?;
        Ok(json!({ "events": events, "now": now_ms }))
    }

    /// The meeting in progress at `now_ms`, or the next one still to start —
    /// with minutes-until (negative while a meeting is live) and a `live` flag.
    fn calendar_next(&self, now_ms: i64) -> Result<Value, String> {
        match self.store.current_or_next_event(now_ms).map_err(|e| e.to_string())? {
            Some(event) => {
                let minutes_until = (event.start_ts - now_ms) / 60_000;
                let live = event.start_ts <= now_ms && event.end_ts.is_some_and(|end| now_ms < end);
                Ok(json!({
                    "event": event,
                    "minutesUntil": minutes_until,
                    "live": live,
                    "now": now_ms,
                }))
            }
            None => Ok(json!({ "event": Value::Null, "now": now_ms })),
        }
    }

    fn tasks_open(&self) -> Result<Value, String> {
        let tasks = self.store.open_tasks().map_err(|e| e.to_string())?;
        Ok(json!({ "tasks": tasks }))
    }

    /// Create a kanban todo in the `backlog` column, optionally tagged with a
    /// repo and free-form notes.
    fn todo_create(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| "missing required argument: title".to_string())?;
        let repo = args.get("repo").and_then(Value::as_str);
        let notes = args.get("notes").and_then(Value::as_str);
        let todo =
            self.store.add_task(title, None, repo, notes, now_ms).map_err(|e| e.to_string())?;
        Ok(json!({ "todo": todo }))
    }

    /// Move a todo to another kanban column; returns the updated todo.
    fn todo_set_status(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: id".to_string())?;
        let status = args
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required argument: status".to_string())?;
        if !tt_store::TASK_STATUSES.contains(&status) {
            return Err(format!(
                "unknown status: {status} (expected one of {})",
                tt_store::TASK_STATUSES.join(", ")
            ));
        }
        if self.store.get_task(id).map_err(|e| e.to_string())?.is_none() {
            return Err(format!("no todo with id {id}"));
        }
        self.store.set_task_status(id, status, now_ms).map_err(|e| e.to_string())?;
        let todo = self
            .store
            .get_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no todo with id {id}"))?;
        Ok(json!({ "todo": todo }))
    }

    /// Edit a todo's free-form fields (title/notes/due). This is a full replace
    /// of those fields — omitting `notes` or `dueTs` clears them, matching
    /// [`tt_store::Store::update_task`]. Status, position, and any issue link are
    /// left untouched. Returns the updated todo, or an error when no todo has `id`.
    fn todo_update(&self, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: id".to_string())?;
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| "missing required argument: title".to_string())?;
        let notes = args.get("notes").and_then(Value::as_str);
        let due_ts = args.get("dueTs").and_then(Value::as_i64);
        let todo = self.store.update_task(id, title, notes, due_ts).map_err(|e| e.to_string())?;
        Ok(json!({ "todo": todo }))
    }

    /// Delete a todo permanently; errors when no todo has `id`.
    fn todo_delete(&self, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: id".to_string())?;
        self.store.delete_task(id).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true, "id": id }))
    }

    /// Sweep `done` todos completed more than `olderThanDays` (default 7) before
    /// `now_ms`, returning how many were removed. The cutoff is derived from the
    /// injected `now_ms`, never a clock read in the store.
    fn todo_clear_done(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let older_than_days = args.get("olderThanDays").and_then(Value::as_i64).unwrap_or(7);
        if older_than_days < 0 {
            return Err("olderThanDays must be zero or positive".to_string());
        }
        let before_ms = now_ms - older_than_days * 24 * 60 * 60 * 1000;
        let deleted = self.store.clear_done_tasks(before_ms).map_err(|e| e.to_string())?;
        Ok(json!({ "deleted": deleted }))
    }

    /// Link a todo to a GitHub issue (repo + number + url). `link_task_issue`
    /// itself is a no-op update on a missing id, so existence is checked first to
    /// surface a clear error instead of silently succeeding. Returns the updated
    /// todo with its issue fields set.
    fn todo_link_issue(&self, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: id".to_string())?;
        let repo = args
            .get("repo")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
            .ok_or_else(|| "missing required argument: repo".to_string())?;
        let number = args
            .get("number")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: number".to_string())?;
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required argument: url".to_string())?;
        if self.store.get_task(id).map_err(|e| e.to_string())?.is_none() {
            return Err(format!("no todo with id {id}"));
        }
        self.store.link_task_issue(id, repo, number, url).map_err(|e| e.to_string())?;
        let todo = self
            .store
            .get_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no todo with id {id}"))?;
        Ok(json!({ "todo": todo }))
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

    /// Agentboard sessions from a one-shot engine scan, optionally filtered by
    /// `agentState.status`. Engine construction/scan is host- and env-dependent
    /// (reads `~/.claude`, shells out to `claude`); a panic there yields an empty
    /// list with a `message` rather than failing the call.
    fn agent_sessions(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let status_filter = args.get("status").and_then(Value::as_str).map(str::to_string);
        let scanned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // The MCP server has no PTYs of its own, so `InstanceScope::this_app()`
            // would always report zero sessions — `Any` is the only scope that
            // makes this tool work at all. Its cross-machine reach is exactly why
            // it's gated behind `mcp.agentSessionsEnabled` in `call_tool` (see the
            // module doc-comment's "Trust boundary" section); this scope itself
            // doesn't change.
            let mut engine =
                tt_agentboard::engine::Engine::new(tt_agentboard::procenv::InstanceScope::Any);
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
            None => self.load_settings()?.journal_settings,
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

    /// Run the manual "refresh now" sweep so an agent can freshen the store
    /// before answering "what needs me": issues, PRs, and — only when Slack is
    /// configured — the watched DM. Calendar is deliberately excluded (its
    /// per-run `claude` token cost keeps it on its scheduled cadence), matching
    /// [`tt_collect::collect_manual`]. Returns one entry per collector with its
    /// key, ok flag, item count, and message. The tracked repos + Slack config
    /// resolve from the shared settings on demand (tests inject an empty set so
    /// the sweep never shells out).
    fn collect_refresh(&self, now_ms: i64) -> Result<Value, String> {
        let (repo_dirs, slack) = match &self.collect_config {
            Some(config) => (config.repo_dirs.clone(), config.slack.clone()),
            None => {
                let collectors = self.load_settings()?.collectors;
                (tt_collect::tracked_repo_dirs(), slack_config(&collectors.slack))
            }
        };
        let summaries = tt_collect::collect_manual(&self.store, &repo_dirs, slack.as_ref(), now_ms);
        let summaries: Vec<Value> = summaries
            .iter()
            .map(|summary| {
                json!({
                    "collector": summary.collector,
                    "ok": summary.ok,
                    "count": summary.count,
                    "message": summary.message,
                })
            })
            .collect();
        Ok(json!({ "summaries": summaries }))
    }
}

/// The Slack collector's runtime config, or `None` when disabled/unconfigured —
/// mirrors the CLI's `collect` resolution so a manual refresh behaves the same
/// on both surfaces.
fn slack_config(slack: &tt_config::SlackDmCollector) -> Option<SlackDmConfig> {
    if !slack.enabled || slack.token.trim().is_empty() {
        return None;
    }
    Some(SlackDmConfig {
        token: slack.token.clone(),
        watch_user_id: slack.watch_user_id.clone(),
        watch_name: slack.watch_name.clone(),
    })
}

/// Path to the shared settings file, for use in the refusal messages below —
/// best-effort; falls back to a description if it can't be resolved.
fn settings_path_hint() -> String {
    tt_config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "your towles-tool.settings.json".to_string())
}

/// The refusal a mutating tool returns while `mcp.mutationsEnabled` is off.
fn mutations_disabled_message(tool: &str, path_hint: &str) -> String {
    format!(
        "{tool} is disabled: tt-mcp's mutating tools (todo_*, journal_append, collect_refresh) \
         are off until you opt in. Set \"mcp\": {{\"mutationsEnabled\": true}} in {path_hint} — \
         tt-mcp deliberately exposes no tool that can change that setting."
    )
}

/// The refusal `agent_sessions` returns while `mcp.agentSessionsEnabled` is off.
fn agent_sessions_disabled_message(path_hint: &str) -> String {
    format!(
        "agent_sessions is disabled: it reports live session data (task names, transcript paths, \
         working directories) from every Claude Code session on this machine, not just this one. \
         Set \"mcp\": {{\"agentSessionsEnabled\": true}} in {path_hint} to enable it."
    )
}

/// The refusal any gated tool returns when the settings file exists but can't
/// be read or parsed: the gate fails closed, and says so instead of leaking a
/// raw serde/IO error that reads as a transient tool failure.
fn capabilities_unreadable_message(tool: &str, error: &str, path_hint: &str) -> String {
    format!(
        "{tool} is disabled (failing closed): the settings file gating it could not be read \
         ({error}). Fix {path_hint} and retry."
    )
}

/// Run the stdio server: open the store (at `store_path` when given, else the
/// default location), then read/dispatch/write until EOF. `settings_path`
/// points the per-call settings reads (capability gate, `journal_append`,
/// `collect_refresh`) at an explicit file — the CLI's `--config-dir` flag —
/// falling back to the shared default when `None`. Returns a process exit
/// code (0 on clean EOF, 1 on a fatal IO/store error).
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

/// JSON Schema tool descriptors returned by `tools/list` — the MCP contract's
/// single source of truth. Also called directly by the app's `mcp_tool_docs`
/// command so the MCP screen's tool documentation can never drift from what
/// `tt mcp serve` actually exposes.
pub fn tool_definitions() -> Value {
    let no_args = || json!({ "type": "object", "properties": {}, "required": [] });
    let mut tools = json!([
        {
            "name": "calendar_today",
            "description": "Calendar events starting within today's local calendar day.",
            "inputSchema": no_args(),
        },
        {
            "name": "calendar_next",
            "description": "The meeting in progress now, or the next one to start, with minutes until it starts (negative while live) and a `live` flag.",
            "inputSchema": no_args(),
        },
        {
            "name": "tasks_open",
            "description": "Open (not-done) tasks, due-soonest first.",
            "inputSchema": no_args(),
        },
        {
            "name": "todo_create",
            "description": "Create a kanban todo (lands in the backlog column).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "The todo's title." },
                    "repo": {
                        "type": "string",
                        "description": "Repository the todo relates to, as owner/name.",
                    },
                    "notes": { "type": "string", "description": "Free-form context notes." },
                },
                "required": ["title"],
            },
        },
        {
            "name": "todo_update",
            "description": "Edit a todo's title, notes, and due date (full replace: omitting notes/dueTs clears them). Status and any issue link are untouched.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The todo's id." },
                    "title": { "type": "string", "description": "The todo's title." },
                    "notes": { "type": "string", "description": "Free-form context notes (omit to clear)." },
                    "dueTs": {
                        "type": "integer",
                        "description": "Due date as epoch milliseconds (omit to clear).",
                    },
                },
                "required": ["id", "title"],
            },
        },
        {
            "name": "todo_delete",
            "description": "Delete a kanban todo permanently.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The todo's id." },
                },
                "required": ["id"],
            },
        },
        {
            "name": "todo_clear_done",
            "description": "Delete done todos completed more than olderThanDays (default 7) ago.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "olderThanDays": {
                        "type": "integer",
                        "description": "Only sweep todos completed at least this many days ago (default 7).",
                    },
                },
            },
        },
        {
            "name": "todo_link_issue",
            "description": "Link a todo to a GitHub issue (sets its repo, number, and url).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The todo's id." },
                    "repo": { "type": "string", "description": "The issue's repository, as owner/name." },
                    "number": { "type": "integer", "description": "The GitHub issue number." },
                    "url": { "type": "string", "description": "The issue's URL." },
                },
                "required": ["id", "repo", "number", "url"],
            },
        },
        {
            "name": "todo_set_status",
            "description": "Move a kanban todo to another column.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The todo's id." },
                    "status": {
                        "type": "string",
                        "enum": tt_store::TASK_STATUSES,
                        "description": "The kanban column to move the todo to.",
                    },
                },
                "required": ["id", "status"],
            },
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
            "name": "agent_sessions",
            "description": "Agentboard sessions, optionally filtered by agent status. Reports live session data from every Claude Code session on this machine, not just the caller's.",
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
        {
            "name": "collect_refresh",
            "description": "Refresh the store now by running the issues + PRs sweep (and the watched Slack DM when configured), deliberately never calendar. Returns one summary per collector (key, ok, count, message). Use before answering 'what needs me' so the data isn't stale.",
            "inputSchema": no_args(),
        },
    ]);
    // Append the "disabled by default" advertisement from the same constants
    // `call_tool` enforces, so a description can never claim a gate the server
    // doesn't apply (or hide one it does).
    let Some(list) = tools.as_array_mut() else {
        return tools;
    };
    for tool in list {
        let note = match tool.get("name").and_then(Value::as_str) {
            Some(name) if MUTATING_TOOLS.contains(&name) => {
                " Disabled by default until \"mcp\":{\"mutationsEnabled\":true} is set in settings."
            }
            Some(AGENT_SESSIONS_TOOL) => {
                " Disabled by default until \"mcp\":{\"agentSessionsEnabled\":true} is set in settings."
            }
            _ => continue,
        };
        let described = match tool.get("description").and_then(Value::as_str) {
            Some(description) => format!("{description}{note}"),
            None => continue,
        };
        tool["description"] = Value::String(described);
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tt_store::{DmInput, EventInput, IssueInput, PrInput};

    const NOW: i64 = 1_700_000_000_000; // fixed epoch ms for deterministic tests
    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;

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

    /// All existing tests predate the mutations/agent_sessions capability
    /// gate and assume every tool works, so the shared helper opts every
    /// tool in; the gate itself is exercised by its own dedicated tests below.
    fn allow_all_mcp_capabilities() -> tt_config::McpSettings {
        tt_config::McpSettings { mutations_enabled: true, agent_sessions_enabled: true }
    }

    fn dispatcher() -> Dispatcher {
        Dispatcher::new(seeded_store()).with_mcp_capabilities(allow_all_mcp_capabilities())
    }

    /// A dispatcher with the capability gate at its (all-off) defaults — the
    /// posture every fresh machine has.
    fn gate_disabled_dispatcher() -> Dispatcher {
        Dispatcher::new(seeded_store()).with_mcp_capabilities(tt_config::McpSettings::default())
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
    fn tools_list_contains_all_nineteen_tools() {
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
            "todo_create",
            "todo_update",
            "todo_delete",
            "todo_clear_done",
            "todo_link_issue",
            "todo_set_status",
            "issues_open",
            "prs_status",
            "dm_status",
            "day_brief",
            "needs_you",
            "snapshot",
            "agent_sessions",
            "journal_append",
            "collect_status",
            "collect_refresh",
        ] {
            assert!(names.contains(&expected), "missing tool {expected} in {names:?}");
        }
        assert_eq!(names.len(), 19);
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
    fn calendar_next_surfaces_an_in_progress_meeting() {
        // The "today" event runs [NOW, NOW + 1000), so it is live at NOW and
        // must not be skipped in favor of the later "soon" event.
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "calendar_next", json!({}));
        assert_eq!(result["event"]["externalId"], "today");
        assert_eq!(result["minutesUntil"], 0);
        assert_eq!(result["live"], true);
    }

    #[test]
    fn calendar_next_moves_on_once_the_live_meeting_ends() {
        // After "today" has ended (now past NOW + 1000) the next meeting "soon"
        // (NOW + 1h) becomes the answer, and it is not live.
        let mut dispatcher = dispatcher();
        let result = call_tool_at(&mut dispatcher, "calendar_next", json!({}), NOW + 2000);
        assert_eq!(result["event"]["externalId"], "soon");
        assert_eq!(result["live"], false);
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

    /// Call a tool expecting an `isError` result; returns the error text.
    fn call_tool_err(dispatcher: &mut Dispatcher, name: &str, args: Value) -> String {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
        .to_string();
        let response = dispatcher.handle_at(&request, NOW).expect("tool call returns a response");
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["isError"], true, "expected a tool error: {response}");
        response["result"]["content"][0]["text"].as_str().unwrap().to_string()
    }

    #[test]
    fn todo_create_lands_in_backlog_with_repo_and_notes() {
        let mut dispatcher = dispatcher();
        let result = call_tool(
            &mut dispatcher,
            "todo_create",
            json!({ "title": "port the CLI", "repo": "o/r", "notes": "start with doctor" }),
        );
        assert_eq!(result["todo"]["text"], "port the CLI");
        assert_eq!(result["todo"]["status"], "backlog");
        assert_eq!(result["todo"]["repo"], "o/r");
        assert_eq!(result["todo"]["notes"], "start with doctor");
        assert_eq!(result["todo"]["createdAt"], NOW);

        // The new todo shows up in the tasks_open read tool.
        let open = call_tool(&mut dispatcher, "tasks_open", json!({}));
        let texts: Vec<&str> =
            open["tasks"].as_array().unwrap().iter().map(|t| t["text"].as_str().unwrap()).collect();
        assert!(texts.contains(&"port the CLI"), "created todo missing: {texts:?}");
    }

    #[test]
    fn todo_create_without_repo_or_notes() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "todo_create", json!({ "title": "bare" }));
        assert_eq!(result["todo"]["text"], "bare");
        assert_eq!(result["todo"]["repo"], Value::Null);
        assert_eq!(result["todo"]["notes"], Value::Null);
    }

    #[test]
    fn todo_create_requires_title() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "todo_create", json!({}));
        assert!(message.contains("title"), "error should name the missing arg: {message}");
        let message = call_tool_err(&mut dispatcher, "todo_create", json!({ "title": "  " }));
        assert!(message.contains("title"), "blank title should be rejected: {message}");
    }

    #[test]
    fn todo_set_status_moves_and_stamps_done() {
        let mut dispatcher = dispatcher();
        let created = call_tool(&mut dispatcher, "todo_create", json!({ "title": "ship it" }));
        let id = created["todo"]["id"].as_i64().unwrap();

        let result =
            call_tool(&mut dispatcher, "todo_set_status", json!({ "id": id, "status": "doing" }));
        assert_eq!(result["todo"]["status"], "doing");
        assert_eq!(result["todo"]["completedAt"], Value::Null);

        let result =
            call_tool(&mut dispatcher, "todo_set_status", json!({ "id": id, "status": "done" }));
        assert_eq!(result["todo"]["status"], "done");
        assert_eq!(result["todo"]["completedAt"], NOW);
    }

    #[test]
    fn todo_set_status_rejects_unknown_status() {
        let mut dispatcher = dispatcher();
        let created = call_tool(&mut dispatcher, "todo_create", json!({ "title": "x" }));
        let id = created["todo"]["id"].as_i64().unwrap();
        let message = call_tool_err(
            &mut dispatcher,
            "todo_set_status",
            json!({ "id": id, "status": "bogus" }),
        );
        assert!(message.contains("bogus"), "error should echo the bad status: {message}");
        assert!(message.contains("backlog"), "error should list valid statuses: {message}");
    }

    #[test]
    fn todo_set_status_rejects_unknown_id() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(
            &mut dispatcher,
            "todo_set_status",
            json!({ "id": 9999, "status": "doing" }),
        );
        assert!(message.contains("9999"), "error should name the missing id: {message}");
    }

    #[test]
    fn todo_set_status_requires_id_and_status() {
        let mut dispatcher = dispatcher();
        let message =
            call_tool_err(&mut dispatcher, "todo_set_status", json!({ "status": "doing" }));
        assert!(message.contains("id"), "error should name the missing arg: {message}");
        let message = call_tool_err(&mut dispatcher, "todo_set_status", json!({ "id": 1 }));
        assert!(message.contains("status"), "error should name the missing arg: {message}");
    }

    #[test]
    fn todo_update_replaces_title_notes_and_due() {
        let mut dispatcher = dispatcher();
        let created = call_tool(
            &mut dispatcher,
            "todo_create",
            json!({ "title": "draft", "notes": "old notes" }),
        );
        let id = created["todo"]["id"].as_i64().unwrap();

        let result = call_tool(
            &mut dispatcher,
            "todo_update",
            json!({ "id": id, "title": "final", "notes": "new notes", "dueTs": NOW + HOUR_MS }),
        );
        assert_eq!(result["todo"]["text"], "final");
        assert_eq!(result["todo"]["notes"], "new notes");
        assert_eq!(result["todo"]["dueTs"], NOW + HOUR_MS);

        // Omitting notes/dueTs is a full replace that clears them.
        let cleared =
            call_tool(&mut dispatcher, "todo_update", json!({ "id": id, "title": "final" }));
        assert_eq!(cleared["todo"]["notes"], Value::Null);
        assert_eq!(cleared["todo"]["dueTs"], Value::Null);
    }

    #[test]
    fn todo_update_rejects_missing_args_and_unknown_id() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "todo_update", json!({ "title": "x" }));
        assert!(message.contains("id"), "error should name the missing arg: {message}");
        let message = call_tool_err(&mut dispatcher, "todo_update", json!({ "id": 1 }));
        assert!(message.contains("title"), "error should name the missing arg: {message}");
        let message =
            call_tool_err(&mut dispatcher, "todo_update", json!({ "id": 9999, "title": "x" }));
        assert!(message.contains("9999"), "error should name the missing id: {message}");
    }

    #[test]
    fn todo_delete_removes_the_todo() {
        let mut dispatcher = dispatcher();
        let created = call_tool(&mut dispatcher, "todo_create", json!({ "title": "temp" }));
        let id = created["todo"]["id"].as_i64().unwrap();

        let result = call_tool(&mut dispatcher, "todo_delete", json!({ "id": id }));
        assert_eq!(result["ok"], true);
        assert_eq!(result["id"], id);

        // The todo is gone: set_status on it now errors with its id.
        let message = call_tool_err(
            &mut dispatcher,
            "todo_set_status",
            json!({ "id": id, "status": "doing" }),
        );
        assert!(message.contains(&id.to_string()), "deleted id should be gone: {message}");
    }

    #[test]
    fn todo_delete_rejects_missing_and_unknown_id() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "todo_delete", json!({}));
        assert!(message.contains("id"), "error should name the missing arg: {message}");
        let message = call_tool_err(&mut dispatcher, "todo_delete", json!({ "id": 9999 }));
        assert!(message.contains("9999"), "error should name the missing id: {message}");
    }

    #[test]
    fn todo_clear_done_sweeps_only_old_done() {
        let mut dispatcher = dispatcher();
        let old = call_tool(&mut dispatcher, "todo_create", json!({ "title": "old" }));
        let old_id = old["todo"]["id"].as_i64().unwrap();
        let recent = call_tool(&mut dispatcher, "todo_create", json!({ "title": "recent" }));
        let recent_id = recent["todo"]["id"].as_i64().unwrap();

        // Complete "old" at NOW and "recent" ten days later.
        call_tool_at(
            &mut dispatcher,
            "todo_set_status",
            json!({ "id": old_id, "status": "done" }),
            NOW,
        );
        call_tool_at(
            &mut dispatcher,
            "todo_set_status",
            json!({ "id": recent_id, "status": "done" }),
            NOW + 10 * DAY_MS,
        );

        // Sweep eight days out with the default 7-day window: only "old" qualifies.
        let result = call_tool_at(&mut dispatcher, "todo_clear_done", json!({}), NOW + 8 * DAY_MS);
        assert_eq!(result["deleted"], 1);

        // "old" is gone; "recent" survives (still errors nothing on set_status).
        let message = call_tool_err(
            &mut dispatcher,
            "todo_set_status",
            json!({ "id": old_id, "status": "doing" }),
        );
        assert!(message.contains(&old_id.to_string()), "old id should be gone: {message}");
        let survived =
            call_tool_at(&mut dispatcher, "todo_clear_done", json!({}), NOW + 8 * DAY_MS);
        assert_eq!(survived["deleted"], 0);
    }

    #[test]
    fn todo_link_issue_sets_issue_fields() {
        let mut dispatcher = dispatcher();
        let created = call_tool(&mut dispatcher, "todo_create", json!({ "title": "promote me" }));
        let id = created["todo"]["id"].as_i64().unwrap();

        let result = call_tool(
            &mut dispatcher,
            "todo_link_issue",
            json!({
                "id": id,
                "repo": "o/r",
                "number": 42,
                "url": "https://github.com/o/r/issues/42",
            }),
        );
        assert_eq!(result["todo"]["repo"], "o/r");
        assert_eq!(result["todo"]["issueNumber"], 42);
        assert_eq!(result["todo"]["issueUrl"], "https://github.com/o/r/issues/42");
    }

    #[test]
    fn todo_link_issue_rejects_unknown_id_and_missing_args() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(
            &mut dispatcher,
            "todo_link_issue",
            json!({ "id": 9999, "repo": "o/r", "number": 1, "url": "u" }),
        );
        assert!(message.contains("9999"), "error should name the missing id: {message}");

        let created = call_tool(&mut dispatcher, "todo_create", json!({ "title": "x" }));
        let id = created["todo"]["id"].as_i64().unwrap();
        let message =
            call_tool_err(&mut dispatcher, "todo_link_issue", json!({ "id": id, "repo": "o/r" }));
        assert!(message.contains("number"), "error should name the missing arg: {message}");
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
    fn collect_refresh_over_no_repos_sweeps_issues_and_prs() {
        // Empty repo dirs + no Slack config → the manual sweep is a clean no-op
        // that never shells out: it still runs issues then prs (never calendar)
        // and reports each as ok with a "no repos configured" note.
        let mut dispatcher = Dispatcher::with_collect_config(seeded_store(), vec![], None)
            .with_mcp_capabilities(allow_all_mcp_capabilities());
        let result = call_tool(&mut dispatcher, "collect_refresh", json!({}));
        let summaries = result["summaries"].as_array().unwrap();
        let keys: Vec<&str> = summaries.iter().map(|s| s["collector"].as_str().unwrap()).collect();
        assert_eq!(keys, ["issues", "prs"], "manual sweep runs issues + PRs, never calendar");
        for summary in summaries {
            assert_eq!(summary["ok"], true, "no-repos sweep is a clean success: {summary}");
            assert_eq!(summary["count"], 0);
            assert_eq!(summary["message"], "no repos configured");
        }
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
        let mut dispatcher = Dispatcher::with_journal_settings(seeded_store(), settings)
            .with_mcp_capabilities(allow_all_mcp_capabilities());
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
        let mut dispatcher = Dispatcher::with_journal_settings(seeded_store(), settings)
            .with_mcp_capabilities(allow_all_mcp_capabilities());
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

    #[test]
    fn mutating_tools_are_refused_when_disabled() {
        // The default (all-false) gate: mutating tools across every family
        // (todo_*, journal_append, collect_refresh) are refused, and the
        // message names the exact tool plus the setting to flip.
        let mut dispatcher = gate_disabled_dispatcher();
        for (tool, args) in [
            ("todo_create", json!({ "title": "x" })),
            ("todo_update", json!({ "id": 1, "title": "x" })),
            ("todo_delete", json!({ "id": 1 })),
            ("todo_clear_done", json!({})),
            ("todo_link_issue", json!({ "id": 1, "repo": "o/r", "number": 1, "url": "u" })),
            ("todo_set_status", json!({ "id": 1, "status": "doing" })),
            ("journal_append", json!({ "text": "hi" })),
            ("collect_refresh", json!({})),
        ] {
            let message = call_tool_err(&mut dispatcher, tool, args);
            assert!(message.contains(tool), "message should name the tool {tool}: {message}");
            assert!(
                message.contains("mutationsEnabled"),
                "message should name the setting for {tool}: {message}"
            );
        }
    }

    #[test]
    fn agent_sessions_is_refused_when_disabled() {
        let mut dispatcher = gate_disabled_dispatcher();
        let message = call_tool_err(&mut dispatcher, "agent_sessions", json!({}));
        assert!(
            message.contains("agentSessionsEnabled"),
            "message should name the setting: {message}"
        );
    }

    #[test]
    fn read_only_tools_are_unaffected_by_a_disabled_gate() {
        // The gate only applies to mutating tools + agent_sessions — the
        // "personal dashboard" reads are the product's cross-project purpose
        // and stay available regardless of the capability settings.
        let mut dispatcher = gate_disabled_dispatcher();
        let result = call_tool(&mut dispatcher, "prs_status", json!({}));
        assert_eq!(result["prs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn mutating_tool_succeeds_once_capability_is_enabled() {
        let mut dispatcher =
            Dispatcher::new(seeded_store()).with_mcp_capabilities(tt_config::McpSettings {
                mutations_enabled: true,
                agent_sessions_enabled: false,
            });
        let result = call_tool(&mut dispatcher, "todo_create", json!({ "title": "ship it" }));
        assert_eq!(result["todo"]["text"], "ship it");

        // agent_sessions stays gated independently of mutations_enabled.
        let message = call_tool_err(&mut dispatcher, "agent_sessions", json!({}));
        assert!(message.contains("agentSessionsEnabled"), "still gated: {message}");
    }

    #[test]
    fn every_tool_is_explicitly_classified_for_the_capability_gate() {
        // The gate is enforced by name lists (MUTATING_TOOLS + AGENT_SESSIONS_TOOL),
        // so a new tool that nobody classifies would ship UNGATED (fail open).
        // This test turns that silent drift into a failure: every tool in
        // tool_definitions() must be exactly one of read-only / mutating /
        // agent_sessions, and its description must advertise exactly the gate
        // call_tool enforces.
        const READ_ONLY_TOOLS: &[&str] = &[
            "calendar_today",
            "calendar_next",
            "tasks_open",
            "issues_open",
            "prs_status",
            "dm_status",
            "day_brief",
            "needs_you",
            "snapshot",
            "collect_status",
        ];
        let tools = tool_definitions();
        let tools = tools.as_array().unwrap();
        assert_eq!(tools.len(), READ_ONLY_TOOLS.len() + MUTATING_TOOLS.len() + 1);
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            let classifications = usize::from(READ_ONLY_TOOLS.contains(&name))
                + usize::from(MUTATING_TOOLS.contains(&name))
                + usize::from(name == AGENT_SESSIONS_TOOL);
            assert_eq!(
                classifications, 1,
                "tool {name} must be classified exactly once (read-only, mutating, or \
                 agent_sessions) — an unclassified tool would ship ungated"
            );
            let description = tool["description"].as_str().unwrap();
            assert_eq!(
                description.contains("Disabled by default"),
                !READ_ONLY_TOOLS.contains(&name),
                "tool {name}'s description must advertise exactly the gate call_tool enforces: \
                 {description}"
            );
        }
    }

    #[test]
    fn gate_reads_the_settings_file_when_no_override_is_injected() {
        // The production path: no with_mcp_capabilities override, the gate
        // resolves from disk per call (via with_settings_path, so the test
        // stays off the real machine settings).
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        let mut dispatcher = Dispatcher::new(seeded_store()).with_settings_path(path.clone());

        // No file yet → created with defaults → both gates refuse.
        let message = call_tool_err(&mut dispatcher, "todo_create", json!({ "title": "x" }));
        assert!(message.contains("mutationsEnabled"), "default-off from disk: {message}");
        assert!(path.exists(), "first load creates the settings file with defaults");

        // Flip the flag in the file — the very next call sees it, no restart.
        std::fs::write(&path, r#"{ "mcp": { "mutationsEnabled": true } }"#).unwrap();
        let result = call_tool(&mut dispatcher, "todo_create", json!({ "title": "ship it" }));
        assert_eq!(result["todo"]["text"], "ship it");

        // agent_sessions is still off in that file.
        let message = call_tool_err(&mut dispatcher, "agent_sessions", json!({}));
        assert!(message.contains("agentSessionsEnabled"), "still gated: {message}");
    }

    #[test]
    fn gated_tools_fail_closed_with_a_clear_refusal_when_settings_are_unreadable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let mut dispatcher = Dispatcher::new(seeded_store()).with_settings_path(path);

        // Gated tools refuse with an actionable message, not a raw serde error.
        let message = call_tool_err(&mut dispatcher, "todo_create", json!({ "title": "x" }));
        assert!(message.contains("failing closed"), "names the posture: {message}");
        assert!(message.contains("todo_create"), "names the tool: {message}");
        assert!(message.contains("towles-tool.settings.json"), "names the file: {message}");

        // Read-only tools never consult the gate and keep working.
        let result = call_tool(&mut dispatcher, "prs_status", json!({}));
        assert_eq!(result["prs"].as_array().unwrap().len(), 1);
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
        // A successful tool call, then a failing one (unknown status).
        drive(
            &mut dispatcher,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "todo_create", "arguments": { "title": "ship it" } },
            }),
        );
        drive(
            &mut dispatcher,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "todo_set_status", "arguments": { "id": 9999, "status": "bad" } },
            }),
        );

        let calls = dispatcher.store.mcp_calls(10).unwrap();
        assert_eq!(calls.len(), 3, "one row per handled request: {calls:?}");

        // Newest first: the failing set-status call.
        assert_eq!(calls[0].method, "tools/call");
        assert_eq!(calls[0].tool.as_deref(), Some("todo_set_status"));
        assert!(!calls[0].ok);
        assert!(calls[0].error.is_some(), "failed call records an error");
        assert!(calls[0].duration_ms.is_some());
        // The client identity from initialize rides along on every later row.
        assert_eq!(calls[0].client.as_deref(), Some("claude-code 2.1"));

        // The successful todo_create call, with its compacted args and ts.
        assert_eq!(calls[1].tool.as_deref(), Some("todo_create"));
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
