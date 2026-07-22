//! A Model Context Protocol (MCP) server for towles-tool.
//!
//! **This crate is the transport-free half.** It speaks JSON-RPC 2.0 as
//! strings — [`Dispatcher::dispatch_at`] takes one request and returns one
//! response (or `None` for a notification, which gets no reply) — and knows
//! nothing about sockets, HTTP, ports, or the wall clock, all of which are
//! passed in per call. The same split as [`tt_ide`]: the transport lives in the
//! app shell (`crates-tauri/tt-app`), which serves this over loopback HTTP.
//! That keeps the whole tool surface unit-testable by driving `dispatch_at`
//! directly, with no server to stand up.
//!
//! Exposed tools surface the towles-tool board ([`tt_store`]'s tasks — the #339
//! unit of work) and the calendar: `task_list`, `task_status`, `task_create`,
//! `calendar_today`, `calendar_next`, `calendar_set`. The broader
//! dashboard-read tools (`day_brief`, `needs_you`, `snapshot`, `prs_status`,
//! `issues_open`, `dm_status`, `collect_status`) were pruned in the 2026-07
//! tool-surface review and have not come back.
//!
//! ## Trust boundary
//!
//! The server is registered with Claude Code at **user scope**, so it is
//! reachable from *every* Claude Code session on the machine, in any project,
//! with no awareness of which one. Two distinct threats, guarded in two
//! different places:
//!
//! 1. **A hijacked session.** A legitimate local Claude Code process reads
//!    hostile content mid-session (a GitHub issue body, a fetched page) and is
//!    instructed to call a tool. There is deliberately **no capability gate**
//!    against this any more (removed 2026-07-20). The gate it replaced was
//!    off by default, which meant the one mutating tool had effectively never
//!    worked, and it never defended much: the writes reachable here are local,
//!    low-stakes and reversible (a board-task row, a calendar cache row), and
//!    any session with shell access could run `sqlite3` against tt.db directly
//!    regardless. The `mcp_calls` log is the audit trail instead.
//!
//! 2. **A web page in the user's browser.** This is the threat the transport
//!    actually has to stop, and it grew teeth when the gate went away. Binding
//!    to loopback does *not* keep pages out: any site the user visits can POST
//!    to `127.0.0.1`, and while CORS stops it reading the response, a blind
//!    write is the whole attack. A bearer token was considered and rejected —
//!    it only ever addressed this one case, and any local process could read it
//!    out of the settings file anyway. The transport in `tt-app` instead
//!    applies the two mitigations the MCP spec recommends for local HTTP
//!    servers: **reject any request carrying an `Origin` header** (real MCP
//!    clients don't send one; browsers always do — this is the DNS-rebinding
//!    mitigation) and **require `Content-Type: application/json`** (not a
//!    CORS-simple type, so a page can't dodge a preflight). Those two checks
//!    are now the only guard on writes and are unit-tested as such.

use std::time::Instant;

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::{Value, json};
use tt_store::{EventInput, McpCallInput, Store};

/// Protocol version advertised when the client does not send one of its own.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Longest tool-args rendering kept in the call log; anything past this is
/// truncated with an ellipsis so a huge payload can't bloat tt.db.
const CALL_LOG_ARGS_MAX: usize = 400;

/// The app-side half of `task_delete`.
///
/// Deleting a task means removing its live panes and its worktree on disk as
/// well as its board row, and neither of the first two is visible from this
/// Tauri-free crate — the PTYs live in the app's terminal state, and the
/// guarded worktree teardown needs the agentboard engine to close the folder's
/// sessions. So the serving transport injects an implementation
/// (`tt-app`'s `task::delete_task_blocking`) and this crate keeps only the
/// tool's shape.
///
/// A dispatcher with no host **refuses `task_delete` outright** rather than
/// falling back to deleting the row: a row-only delete is precisely the
/// half-delete this tool exists to stop, and doing it silently would strand
/// the worktree on disk with nothing left on the board pointing at it.
pub trait TaskHost: Send {
    /// Delete the board task `id` and everything bound to it. `force` skips
    /// the work-preserving guards. `Ok(Refused)` is a guarded refusal, not a
    /// failure — see [`TaskDeletion`].
    fn delete_task(&self, id: i64, force: bool) -> Result<TaskDeletion, String>;
}

/// What a [`TaskHost::delete_task`] attempt produced.
///
/// The refusal is an `Ok` variant for the same reason it is one in
/// `tt_tasks::ops::RemoveOutcome`: "your worktree still has uncommitted work"
/// is an answer with a next step attached, not a malfunction. Reporting it as
/// an error would tell a calling agent the delete *failed* — inviting a retry
/// with force — when the truth is that it was declined on purpose.
///
/// Both variants carry `name` — what the host called the thing it acted on —
/// so the dispatcher never has to read the row itself just to name it in the
/// reply. That read would be a second trip over a second SQLite connection for
/// a string the host already had in hand.
pub enum TaskDeletion {
    /// The task is gone: panes, worktree, and board row.
    Deleted { name: String, messages: Vec<String> },
    /// Nothing was deleted. Each blocker carries `losesWork` so a caller can
    /// tell "stop your dev server and retry" apart from "forcing this destroys
    /// commits that exist nowhere else".
    Refused {
        name: String,
        blockers: Vec<Value>,
        messages: Vec<String>,
    },
}

/// The stateful core of the server: owns the [`Store`] and dispatches JSON-RPC
/// requests to tool handlers. Kept free of any transport so it can be driven
/// directly in tests.
pub struct Dispatcher {
    store: Store,
    /// Injected by the serving transport — see [`TaskHost`]. `None` in tests
    /// and any Tauri-free driver, where `task_delete` refuses.
    task_host: Option<Box<dyn TaskHost>>,
    /// `clientInfo` from the session's `initialize` (e.g. `claude-code 2.1`),
    /// stamped onto call-log rows so the app's MCP screen can say who called.
    client: Option<String>,
    /// Injected tracked-repo dirs (test hook). The serving path leaves it
    /// `None` and re-reads the shared agentboard `repos.json` on every
    /// `task_create`, so newly tracked repos are creatable without a restart.
    tracked_repos: Option<Vec<String>>,
    /// Injected calendar-source ids (test hook), keeping `calendar_set`'s lane
    /// validation off the real settings file. `None` re-reads settings per call.
    calendar_sources: Option<Vec<String>>,
}

/// What one dispatched request produced: the reply to send back, and whether
/// the call actually wrote to the store.
///
/// `wrote` is deliberately narrow, and it answers "**must the transport
/// repaint?**" — not "did this mutate?". A refusal, a failed write, and every
/// read all report false, so a transport can skip work only a real mutation
/// justifies; so does a tool in [`SELF_REFRESHING_TOOLS`], which already
/// repainted on its own and would otherwise be repainted twice.
pub struct Handled {
    /// The response line, or `None` for a notification (which gets no reply).
    pub response: Option<String>,
    /// Whether this call committed a change to the store.
    pub wrote: bool,
}

impl Handled {
    /// A reply that changed nothing.
    fn read(response: Option<String>) -> Handled {
        Handled { response, wrote: false }
    }
}

/// The tools that write to the store. One list, read two ways: [`tool_writes`]
/// answers the transport's "must the UI refresh?" question from it, and
/// [`tool_definitions`] stamps `annotations.readOnlyHint: false` from it — so
/// the wire contract and the internal decision cannot disagree, because there
/// is only one fact.
///
/// The direction matters. Deriving the *internal* answer by reading the
/// *external* JSON back out would route control flow through an advisory client
/// hint (and rebuild the whole tool contract per request to do it); a tool added
/// to [`Dispatcher::call_tool`] but missed here is one edit away from a stale
/// board either way, but here the fix is a single obvious list.
const WRITING_TOOLS: &[&str] = &["task_create", "task_delete", "calendar_set"];

/// Whether a tool writes to the store — see [`WRITING_TOOLS`].
pub fn tool_writes(name: &str) -> bool {
    WRITING_TOOLS.contains(&name)
}

/// Writing tools that refresh the UI themselves, so the transport must not do
/// it again.
///
/// `task_delete` runs through the app's own delete path, which emits a snapshot
/// the moment the row goes. Letting [`Handled::wrote`] stay true here would
/// rebuild the whole snapshot a second time — under the `StoreState` mutex the
/// transport opened a separate connection specifically to avoid — and would do
/// it even for a *refused* delete, which changed nothing at all.
///
/// Deliberately separate from [`WRITING_TOOLS`] rather than removing the tool
/// from it: `readOnlyHint` must still say the tool writes, because it does.
/// The two questions ("does this mutate?" and "who repaints?") only looked like
/// one question while every writer went through the dispatcher's own store.
const SELF_REFRESHING_TOOLS: &[&str] = &["task_delete"];

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
            task_host: None,
            client: None,
            tracked_repos: None,
            calendar_sources: None,
        }
    }

    /// Inject the app-side deletion host — see [`TaskHost`]. The serving
    /// transport in `tt-app` is the only caller; without it `task_delete`
    /// refuses.
    pub fn with_task_host(mut self, host: Box<dyn TaskHost>) -> Dispatcher {
        self.task_host = Some(host);
        self
    }

    /// Build a dispatcher with a fixed tracked-repo list (test hook — keeps
    /// `task_create`'s repo validation off the real agentboard `repos.json`).
    pub fn with_tracked_repos(mut self, repos: Vec<String>) -> Dispatcher {
        self.tracked_repos = Some(repos);
        self
    }

    /// Build a dispatcher with a fixed calendar-source list (test hook — keeps
    /// `calendar_set`'s lane validation off the real settings file).
    pub fn with_calendar_sources(mut self, sources: Vec<String>) -> Dispatcher {
        self.calendar_sources = Some(sources);
        self
    }

    /// The calendar-source ids `calendar_set` validates against: the injected
    /// override if any, else re-read from the settings file every time, so a
    /// calendar added in Settings is writable without restarting the server.
    /// An unreadable settings file yields no ids, which fails closed — every
    /// `calendar_set` is refused rather than allowed to mint a lane.
    fn calendar_source_ids(&self) -> Vec<String> {
        if let Some(ids) = &self.calendar_sources {
            return ids.clone();
        }
        tt_config::load()
            .map(|settings| {
                settings
                    .collectors
                    .calendar
                    .sources
                    .into_iter()
                    // Trimmed to match `calendar_set`, which trims the incoming
                    // `source` before comparing: without this a settings id with
                    // stray whitespace is listed as configured yet can never be
                    // matched, so that lane is permanently unwritable.
                    .map(|source| source.id.trim().to_string())
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
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

    /// [`Dispatcher::dispatch_at`] keeping only the response line — the shape
    /// most tests assert on.
    ///
    /// Test-only: the transport needs [`Handled::wrote`], so discarding it is
    /// never right in production. Gating it here is what keeps that from
    /// becoming a second, lossy entry point someone reaches for by accident.
    #[cfg(test)]
    fn handle_at(&mut self, request_json: &str, now_ms: i64) -> Option<String> {
        self.dispatch_at(request_json, now_ms).response
    }

    /// Handle one request line and report what it did, not just what to send
    /// back.
    ///
    /// A transport needs [`Handled::wrote`] to decide whether anything
    /// downstream must be refreshed. Only the dispatcher can answer that
    /// honestly — it knows which tool ran — and guessing costs real work: the
    /// app rebuilds and broadcasts an entire store snapshot per refresh, so
    /// treating every `ping` and `task_list` as a possible write means a full
    /// snapshot per read, taken against the very lock the transport opened a
    /// second SQLite connection to avoid contending with.
    pub fn dispatch(&mut self, request_json: &str) -> Handled {
        self.dispatch_at(request_json, now_ms())
    }

    /// [`Dispatcher::dispatch`] with an injected `now_ms` (deterministic tests).
    pub fn dispatch_at(&mut self, request_json: &str, now_ms: i64) -> Handled {
        let value: Value = match serde_json::from_str(request_json) {
            Ok(value) => value,
            Err(_) => {
                return Handled::read(Some(error_response(Value::Null, -32700, "Parse error")));
            }
        };

        // A top-level array is a JSON-RPC batch. MCP 2025-06-18 removed batching,
        // so reject it with a single Invalid Request instead of letting the `id`
        // lookup below miss and silently drop it (which hangs a waiting client).
        if value.is_array() {
            return Handled::read(Some(error_response(Value::Null, -32600, "Invalid Request")));
        }

        // Requests carry an `id`; notifications do not, and receive no response.
        let id = match value.get("id") {
            Some(id) if !id.is_null() => id.clone(),
            _ => return Handled::read(None),
        };

        let method = match value.get("method").and_then(Value::as_str) {
            Some(method) => method,
            None => return Handled::read(Some(error_response(id, -32600, "Invalid Request"))),
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

        // A refused or failed call changed nothing, so it needs no refresh —
        // and neither does one that already repainted itself
        // ([`SELF_REFRESHING_TOOLS`]).
        let wrote = call.ok
            && call
                .tool
                .as_deref()
                .is_some_and(|tool| tool_writes(tool) && !SELF_REFRESHING_TOOLS.contains(&tool));
        Handled { response: Some(outcome.response), wrote }
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
        match name {
            "task_list" => self.task_list(),
            "task_status" => self.task_status(args),
            "task_create" => self.task_create(args, now_ms),
            "task_delete" => self.task_delete(args),
            "calendar_today" => self.calendar_today(now_ms),
            "calendar_next" => self.calendar_next(now_ms),
            "calendar_set" => self.calendar_set(args, now_ms),
            other => Err(format!("unknown tool: {other}")),
        }
    }

    /// Events whose start falls within the local calendar day of `now_ms` —
    /// the shape of the day, for deciding where the focus blocks are.
    fn calendar_today(&self, now_ms: i64) -> Result<Value, String> {
        let (start, end) = Store::local_day_bounds(now_ms);
        let events = self.store.events_between(start, end).map_err(|e| e.to_string())?;
        Ok(json!({ "events": events, "now": now_ms }))
    }

    /// The meeting in progress at `now_ms`, or the next one still to start —
    /// with minutes-until (negative while a meeting is live) and a `live` flag.
    fn calendar_next(&self, now_ms: i64) -> Result<Value, String> {
        match self.store.current_or_next_event(now_ms).map_err(|e| e.to_string())? {
            Some(event) => {
                // Floor, not truncate-toward-zero. Plain `/` would report `0`
                // for the whole first minute of a live meeting, and the
                // contract promises `minutesUntil` is *negative* while one is
                // running — so a consumer distinguishing "starts now" from
                // "already started" would be wrong for exactly the 59 seconds
                // that distinction matters most.
                let minutes_until = (event.start_ms() - now_ms).div_euclid(60_000);
                let live =
                    event.start_ms() <= now_ms && event.end_ms().is_some_and(|end| now_ms < end);
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

    /// The push-model write path: replace one calendar's events for one local
    /// day, leaving every other calendar and every other day untouched (see
    /// [`Store::replace_events_for_source`]).
    ///
    /// **The day window is derived here, never taken from the payload.** The
    /// caller may name a day (`day`, `YYYY-MM-DD`, local); with no `day` it is
    /// the local calendar day containing `now_ms`. Either way the actual
    /// `[start, end)` bounds come from [`Store::local_day_bounds`], so a client cannot
    /// widen the delete beyond one day — a mis-stated window is the one way
    /// this tool could destroy calendar rows it was not asked to touch, and the
    /// events themselves are the least trustworthy part of the request.
    ///
    /// **`source` must name a calendar the user actually configured**, and is
    /// checked against `collectors.calendar.sources` in the settings file the
    /// same way `task_create` checks `repo` against tracked repos. Two distinct
    /// things go wrong without that check, and neither is hypothetical:
    ///
    /// - A typo or hallucinated id (`"gcal"`, `"Google"`, `"personal"`) mints a
    ///   lane nothing will ever write again. Its rows still feed
    ///   `calendar_next`, and no sweep will ever remove them — precisely the
    ///   orphan-lane failure the v9 migration destroyed data to avoid. Enforcing
    ///   it only at migration time would leave the runtime free to recreate it.
    /// - It bounds the blast radius of a hijacked session. This tool replaces a
    ///   day of a lane, so `{source, events: []}` *clears* that day; restricting
    ///   `source` to configured calendars at least means it can only affect
    ///   calendars the user opted into, and the refusal names what exists.
    ///
    /// [`EventInput`] additionally has no `source` field, so a model-authored
    /// event array cannot smuggle a different lane in per-event.
    fn calendar_set(&self, args: &Value, now_ms: i64) -> Result<Value, String> {
        let source = args
            .get("source")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|source| !source.is_empty())
            .ok_or_else(|| "missing required argument: source".to_string())?;
        let configured = self.calendar_source_ids();
        if !configured.iter().any(|id| id == source) {
            return Err(unknown_calendar_source_message(source, &configured));
        }
        let events_arg = args
            .get("events")
            .filter(|events| events.is_array())
            .ok_or_else(|| "missing required argument: events (an array)".to_string())?;
        let events: Vec<EventInput> = serde_json::from_value(events_arg.clone())
            .map_err(|e| format!("invalid events payload: {e}"))?;
        // An event that ends before it starts is accepted by the schema and
        // then read inconsistently: `calendar_today` windows on `start_ts` and
        // lists it, while `current_or_next_event` matches on `end_ts > now` and
        // drops it — so the meeting shows up in the day's shape but never in
        // the countdown or the meeting-start notification. Swapped or
        // timezone-slipped fields are exactly what a model gets wrong, so this
        // is a refusal, not a silent repair.
        if let Some(bad) = events.iter().find(|e| e.end.is_some_and(|end| end < e.start)) {
            return Err(format!(
                "event {} ends before it starts (start {}, end {}) — check the field order",
                bad.external_id,
                bad.start.to_rfc3339(),
                bad.end.map(|e| e.to_rfc3339()).unwrap_or_default(),
            ));
        }

        let reference_ms = match args.get("day").and_then(Value::as_str) {
            Some(day) => day_reference_ms(day)
                .ok_or_else(|| format!("invalid day: {day} (expected YYYY-MM-DD)"))?,
            None => now_ms,
        };
        let (day_start, day_end) = Store::local_day_bounds(reference_ms);

        // Refuse a day the store's retention sweep would immediately reclaim.
        // Without this the tool accepts the write, reports `written: N`, and the
        // very next calendar write from any source deletes those rows — a
        // success return value with a one-tick shelf life. Better to say no than
        // to be briefly, confidently wrong.
        if day_end <= now_ms.saturating_sub(tt_store::EVENT_RETAIN_MS) {
            return Err(format!(
                "day {} is past the {}-day retention window — events that old are swept, so \
                 writing them would report success and then silently drop them",
                args.get("day").and_then(Value::as_str).unwrap_or("(derived)"),
                tt_store::EVENT_RETAIN_MS / (24 * 60 * 60 * 1000),
            ));
        }

        // Every event must fall inside the day it is being pushed for.
        // `replace_events_for_source` only deletes within `[day_start, day_end)`,
        // and retention only sweeps the *past*, so a row landing outside the
        // window is reachable by neither: no later push to this lane covers it,
        // no sweep ages it out, and it feeds `calendar_next` as a phantom
        // meeting until an event with the identical `externalId` happens to
        // replace it. A model that mis-dates one entry (wrong day, wrong year,
        // a timezone slip) is the likely author, so name the offender.
        if let Some(stray) =
            events.iter().find(|e| e.start_ms() < day_start || e.start_ms() >= day_end)
        {
            return Err(format!(
                "event {} starts at {}, outside the day being written [{}, {}) — push it with \
                 that day's `day` argument instead; an event outside the window would be stored \
                 where nothing can ever replace or sweep it",
                stray.external_id,
                stray.start.to_rfc3339(),
                local_iso(day_start),
                local_iso(day_end),
            ));
        }

        let written = self
            .store
            .replace_events_for_source(source, day_start, day_end, &events, now_ms)
            .map_err(|e| e.to_string())?;
        tracing::info!(
            %source,
            written,
            day_start = %local_iso(day_start),
            day_end = %local_iso(day_end),
            "calendar.set"
        );
        Ok(json!({
            "source": source,
            "written": written,
            "dayStart": local_iso(day_start),
            "dayEnd": local_iso(day_end),
        }))
    }

    /// Open (not-done) board tasks in board order, each with its issue/PR
    /// links and repo/worktree binding.
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
        // Only a genuinely absent row is "no such task". Collapsing every store
        // error into that message tells a caller the task does not exist when
        // the truth is a busy-timeout, a disk error, or a corrupt page — and a
        // session that believes its task vanished may go create a duplicate.
        let task = self.store.task_by_id(id).map_err(|error| match error {
            tt_store::Error::TaskNotFound(id) => format!("no task with id {id}"),
            other => format!("could not read task {id}: {other}"),
        })?;
        Ok(json!({ "task": task }))
    }

    /// Create a board task in a tracked repo — the same store path as the
    /// app's Agentboard `+` flow: [`Store::add_task`] then a repo-only
    /// [`Store::set_task_worktree`], so the task lands in that repo's Board
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
            .set_task_worktree(task.id, &entry.dir, None, None, None)
            .map_err(|e| e.to_string())?;
        let task = self.store.task_by_id(task.id).map_err(|e| e.to_string())?;
        tracing::info!(task_id = task.id, repo = %entry.dir, %status, "task.created");
        Ok(json!({ "task": task }))
    }

    /// Delete a board task and everything bound to it — its live panes and its
    /// worktree on disk as well as its row — through the injected
    /// [`TaskHost`].
    ///
    /// The refusal path is the point of this tool. A worktree holding
    /// uncommitted changes, commits that reached no branch or remote, or a
    /// foreign process on its claimed ports comes back as `status: "refused"`
    /// with the reasons, having deleted **nothing** — so an agent that calls
    /// this on a task the user still has work in cannot destroy it by
    /// accident. `force: true` is the deliberate override and is reported in
    /// the app's event log as such.
    fn task_delete(&mut self, args: &Value) -> Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "missing required argument: id".to_string())?;
        let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
        let host = self
            .task_host
            .as_ref()
            .ok_or_else(|| "task_delete is unavailable: no task host is attached".to_string())?;

        // No pre-read to name the task: the host has to resolve the row anyway
        // before it can delete anything, so it returns the name it found — and
        // an unknown id comes back as its error rather than being diagnosed
        // twice over two connections.
        match host.delete_task(id, force)? {
            TaskDeletion::Deleted { name, messages } => {
                Ok(json!({ "status": "deleted", "id": id, "text": name, "messages": messages }))
            }
            TaskDeletion::Refused { name, blockers, messages } => Ok(json!({
                "status": "refused",
                "id": id,
                "text": name,
                "blockers": blockers,
                "messages": messages,
            })),
        }
    }
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

/// An epoch-ms instant rendered in the machine's local zone, RFC 3339.
///
/// Used for the day-window bounds this tool reports and refuses on. They are
/// computed as epoch ms (that is what `local_day_bounds` returns), but a caller
/// reading `1784707200000` in a refusal cannot tell which day it was handed —
/// which is the whole reason this tool speaks ISO.
fn local_iso(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| ms.to_string())
}

/// Local midnight of a `YYYY-MM-DD` date as epoch ms, or `None` when the date
/// does not parse or has no unambiguous local midnight. Only used to pick the
/// reference instant handed to [`Store::local_day_bounds`].
fn day_reference_ms(day: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d").ok()?;
    // Local **noon**, not midnight. This value is only a reference instant —
    // `Store::local_day_bounds` re-derives the real `[start, end)` edges from
    // it — and midnight is the one time of day that can fail to exist or be
    // ambiguous. Resolving it with `.single()` returned `None` on exactly the
    // DST-transition dates in zones that switch at midnight (Havana, Santiago,
    // São Paulo), so a perfectly valid date came back as "invalid day" and that
    // day's calendar could never be pushed. Noon is unambiguous in every zone.
    let noon = date.and_hms_opt(12, 0, 0)?;
    Some(Local.from_local_datetime(&noon).earliest()?.timestamp_millis())
}

/// The calendar-lane refusal: names the rejected id and lists the configured
/// ones, so a caller can self-correct without another round trip. An empty
/// configured set is called out separately — "no calendars are configured" is a
/// settings problem, not a bad argument, and the two want different fixes.
fn unknown_calendar_source_message(source: &str, configured: &[String]) -> String {
    if configured.is_empty() {
        return format!(
            "unknown calendar source: {source} — no calendars are configured. Add one under \
             Settings → Collectors → Calendar before pushing events."
        );
    }
    format!(
        "unknown calendar source: {source} — configured calendars are: {}. Writing to an \
         unconfigured lane would strand rows nothing ever sweeps.",
        configured.join(", ")
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
/// the server actually exposes.
pub fn tool_definitions() -> Value {
    let no_args = || json!({ "type": "object", "properties": {}, "required": [] });
    let mut tools = json!([
        {
            "name": "task_list",
            "description": "Open (not-done) board tasks in board order, each with its issue/PR links and repo/worktree binding.",
            "inputSchema": no_args(),
        },
        {
            "name": "task_status",
            "description": "One board task by id — the full row (status, links, repo/worktree binding), including done tasks.",
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
            "description": "Create a board task in a tracked repo's swimlane. Writes to the board.",
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
        {
            "name": "task_delete",
            "description": "Delete a board task and everything bound to it — its terminal panes and its git worktree on disk as well as its board row. Guarded: if the worktree has uncommitted changes, commits that reached no branch or remote, or a foreign process on its claimed ports, nothing is deleted and the reasons come back as `status: \"refused\"`. Report those to the user and let them decide; only pass force after they have said so explicitly, since it destroys that work permanently.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "The task's id (from task_list or task_status)." },
                    "force": { "type": "boolean", "description": "Skip the guards and delete anyway, discarding uncommitted changes and unreachable commits for good. Default false." },
                },
                "required": ["id"],
            },
        },
        {
            "name": "calendar_today",
            "description": "The shape of today: every meeting starting in today's local calendar day, in order. Use it to see where the uninterrupted stretches are before committing to deep work.",
            "inputSchema": no_args(),
        },
        {
            "name": "calendar_next",
            "description": "How much focus time is left: the meeting in progress now, or the next one to start, with `minutesUntil` (negative while a meeting is live) and a `live` flag. The one calendar read that matters mid-task — nothing scheduled means keep working.",
            "inputSchema": no_args(),
        },
        {
            "name": "calendar_set",
            "description": "Push one calendar's meetings for one local day into the local cache, replacing whatever that calendar previously had for that day. This is how the calendar gets filled — a scheduled pull writes here; nothing else reads your real calendar. Other calendars and other days are left untouched.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "Which configured calendar this pull represents (e.g. \"google\", \"outlook\"). Only this calendar's rows for the day are replaced." },
                    "day": { "type": "string", "description": "Local calendar day being pushed, YYYY-MM-DD. Defaults to today." },
                    "events": {
                        "type": "array",
                        "description": "The meetings for that day. An empty array clears the day for this calendar.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "externalId": { "type": "string", "description": "The calendar provider's stable id for the event." },
                                "title": { "type": "string", "description": "Meeting title." },
                                "start": { "type": "string", "description": "Start time, RFC 3339 with the calendar's UTC offset, e.g. \"2026-07-20T15:00:00+01:00\" (or \"...Z\"). Keep the offset the calendar reports — it records that the meeting was booked as 3pm there, which a UTC-only time cannot." },
                                "end": { "type": "string", "description": "End time, same format. Omit for a point-in-time entry." },
                                "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee names or addresses." },
                                "location": { "type": "string", "description": "Room or place, if any." },
                                "joinUrl": { "type": "string", "description": "Video-call link, if any." },
                            },
                            "required": ["externalId", "title", "start"],
                        },
                    },
                },
                "required": ["source", "events"],
            },
        },
    ]);

    // Stamp `readOnlyHint` from [`WRITING_TOOLS`] rather than hand-writing it
    // per descriptor, so the flag a client sees and the flag the transport acts
    // on are the same fact rather than two that agree today.
    if let Some(entries) = tools.as_array_mut() {
        for entry in entries {
            let writes = entry["name"].as_str().is_some_and(tool_writes);
            entry["annotations"] = json!({ "readOnlyHint": !writes });
        }
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const NOW: i64 = 1_700_000_000_000; // fixed epoch ms for deterministic tests

    /// The tracked-repo dir every test dispatcher knows about.
    const REPO_DIR: &str = "/home/u/code/demo";

    fn seeded_store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store.add_task("open task", "backlog", None, NOW).unwrap();
        store
    }

    /// A dispatcher with one injected tracked repo, so no test touches the real
    /// agentboard repos.json.
    fn dispatcher() -> Dispatcher {
        Dispatcher::new(seeded_store())
            .with_tracked_repos(vec![REPO_DIR.to_string()])
            .with_calendar_sources(vec!["google".to_string(), "outlook".to_string()])
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
    fn tools_list_is_exactly_the_task_and_calendar_families() {
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
        assert_eq!(
            names,
            vec![
                "task_list",
                "task_status",
                "task_create",
                "task_delete",
                "calendar_today",
                "calendar_next",
                "calendar_set",
            ]
        );
    }

    /// An epoch-ms instant as the RFC 3339 the tool now speaks, in local time
    /// (which is what a real calendar would report).
    fn iso(ms: i64) -> String {
        Local.timestamp_millis_opt(ms).single().unwrap().to_rfc3339()
    }

    /// A `calendar_set` event payload, in the tool's wire shape.
    fn event_json(external_id: &str, start_ts: i64, end_ts: i64) -> Value {
        json!({
            "externalId": external_id,
            "title": external_id,
            "start": iso(start_ts),
            "end": iso(end_ts),
        })
    }

    /// `calendar_set` one source's day, returning the tool result.
    fn set_calendar(dispatcher: &mut Dispatcher, source: &str, events: Value) -> Value {
        call_tool(dispatcher, "calendar_set", json!({ "source": source, "events": events }))
    }

    #[test]
    fn calendar_today_returns_only_the_local_day() {
        let (day_start, day_end) = Store::local_day_bounds(NOW);
        let mut dispatcher = dispatcher();
        // Neighbouring days are pushed as their own days — the only way to
        // store them, since a day's push refuses events outside its window.
        let day_of = |ms: i64| {
            Local.timestamp_millis_opt(ms).single().unwrap().format("%Y-%m-%d").to_string()
        };
        for (day_ms, event) in [
            (day_start - 3_600_000, event_json("yesterday", day_start - 3_600_000, day_start - 1)),
            (NOW, event_json("standup", day_start + 3_600_000, day_start + 5_400_000)),
            (day_end + 3_600_000, event_json("tomorrow", day_end + 3_600_000, day_end + 5_400_000)),
        ] {
            call_tool(
                &mut dispatcher,
                "calendar_set",
                json!({ "source": "google", "day": day_of(day_ms), "events": [event] }),
            );
        }

        let result = call_tool(&mut dispatcher, "calendar_today", json!({}));
        assert_eq!(result["now"], NOW);
        let ids: Vec<&str> = result["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["externalId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["standup"], "only today's event is in the window");
    }

    /// A row outside the day it is pushed for is reachable by nothing: the
    /// lane's next write only deletes inside that day's window, and retention
    /// only sweeps the past. It would feed the countdown as a phantom meeting
    /// forever, so the push is refused rather than half-accepted.
    #[test]
    fn calendar_set_refuses_an_event_outside_the_day_being_written() {
        let (_, day_end) = Store::local_day_bounds(NOW);
        let mut dispatcher = dispatcher();
        let message = call_tool_err(
            &mut dispatcher,
            "calendar_set",
            json!({
                "source": "google",
                "events": [event_json("next-week", day_end + 6 * 86_400_000, day_end + 6 * 86_400_000 + 1_800_000)],
            }),
        );
        assert!(message.contains("next-week"), "names the offending event: {message}");
        assert!(message.contains("outside the day"), "says why: {message}");
    }

    /// `end < start` parses fine and then reads two different ways: it shows up
    /// in `calendar_today` (which windows on `start_ts`) but never in
    /// `calendar_next` (which matches `end_ts > now`), so the meeting is in the
    /// day's shape and missing from the countdown.
    #[test]
    fn calendar_set_refuses_an_event_that_ends_before_it_starts() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(
            &mut dispatcher,
            "calendar_set",
            json!({
                "source": "google",
                "events": [event_json("backwards", NOW + 3_600_000, NOW - 3_600_000)],
            }),
        );
        assert!(message.contains("backwards"), "names the offending event: {message}");
        assert!(message.contains("ends before it starts"), "says why: {message}");
    }

    /// The contract promises `minutesUntil` is negative while a meeting runs.
    /// Truncating division would report `0` for the first 59 seconds — the
    /// window where "starting now" vs "already started" matters most.
    #[test]
    fn calendar_next_minutes_until_is_negative_from_the_first_second() {
        let mut dispatcher = dispatcher();
        set_calendar(
            &mut dispatcher,
            "google",
            json!([event_json("live", NOW - 30_000, NOW + 1_800_000)]),
        );
        let result = call_tool(&mut dispatcher, "calendar_next", json!({}));
        assert_eq!(result["live"], true);
        assert_eq!(result["minutesUntil"], -1, "30s in is already negative, not 0");
    }

    #[test]
    fn calendar_next_flags_a_meeting_in_progress() {
        let mut dispatcher = dispatcher();
        // Started 10 minutes ago, runs for another 20.
        set_calendar(
            &mut dispatcher,
            "google",
            json!([event_json("in-progress", NOW - 600_000, NOW + 1_200_000)]),
        );

        let result = call_tool(&mut dispatcher, "calendar_next", json!({}));
        assert_eq!(result["event"]["externalId"], "in-progress");
        assert_eq!(result["live"], true);
        assert_eq!(result["minutesUntil"], -10, "minutes go negative while live");
        assert_eq!(result["now"], NOW);
    }

    #[test]
    fn calendar_next_counts_down_to_the_next_meeting() {
        let mut dispatcher = dispatcher();
        set_calendar(
            &mut dispatcher,
            "google",
            json!([event_json("upcoming", NOW + 1_800_000, NOW + 3_600_000)]),
        );

        let result = call_tool(&mut dispatcher, "calendar_next", json!({}));
        assert_eq!(result["event"]["externalId"], "upcoming");
        assert_eq!(result["live"], false);
        assert_eq!(result["minutesUntil"], 30);
    }

    #[test]
    fn calendar_next_on_an_empty_calendar_is_null() {
        let mut dispatcher = dispatcher();
        let result = call_tool(&mut dispatcher, "calendar_next", json!({}));
        assert_eq!(result["event"], Value::Null);
    }

    #[test]
    fn calendar_set_replaces_one_source_and_leaves_the_others() {
        let (day_start, _) = Store::local_day_bounds(NOW);
        let mut dispatcher = dispatcher();
        set_calendar(
            &mut dispatcher,
            "outlook",
            json!([event_json(
                "work-sync",
                day_start + 3_600_000,
                day_start + 5_400_000
            )]),
        );
        set_calendar(
            &mut dispatcher,
            "google",
            json!([event_json(
                "dentist",
                day_start + 7_200_000,
                day_start + 9_000_000
            )]),
        );

        // Re-pushing google replaces only google's rows for the day.
        let result = set_calendar(
            &mut dispatcher,
            "google",
            json!([event_json(
                "school-run",
                day_start + 10_800_000,
                day_start + 12_600_000
            )]),
        );
        assert_eq!(result["source"], "google");
        assert_eq!(result["written"], 1);
        assert_eq!(result["dayStart"], iso(day_start));

        let today = call_tool(&mut dispatcher, "calendar_today", json!({}));
        let ids: Vec<&str> = today["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["externalId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["work-sync", "school-run"], "outlook's row survived");
    }

    #[test]
    fn calendar_set_accepts_an_explicit_day() {
        let mut dispatcher = dispatcher();
        let day = Local.timestamp_millis_opt(NOW).single().unwrap().date_naive();
        let tomorrow = (day + Duration::days(1)).format("%Y-%m-%d").to_string();
        let tomorrow_start = Store::local_day_bounds(day_reference_ms(&tomorrow).unwrap()).0;

        let result = call_tool(
            &mut dispatcher,
            "calendar_set",
            json!({
                "source": "google",
                "day": tomorrow,
                "events": [event_json("offsite", tomorrow_start + 3_600_000, tomorrow_start + 7_200_000)],
            }),
        );
        assert_eq!(result["dayStart"], iso(tomorrow_start));

        // It is tomorrow's, so today's read does not see it.
        let today = call_tool(&mut dispatcher, "calendar_today", json!({}));
        assert!(today["events"].as_array().unwrap().is_empty(), "{today}");
    }

    #[test]
    fn calendar_set_clears_a_day_with_an_empty_array() {
        let (day_start, _) = Store::local_day_bounds(NOW);
        let mut dispatcher = dispatcher();
        set_calendar(
            &mut dispatcher,
            "google",
            json!([event_json(
                "cancelled",
                day_start + 3_600_000,
                day_start + 5_400_000
            )]),
        );
        let result = set_calendar(&mut dispatcher, "google", json!([]));
        assert_eq!(result["written"], 0);
        let today = call_tool(&mut dispatcher, "calendar_today", json!({}));
        assert!(today["events"].as_array().unwrap().is_empty(), "{today}");
    }

    #[test]
    fn calendar_set_validates_its_arguments() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "calendar_set", json!({ "events": [] }));
        assert!(message.contains("source"), "{message}");
        let message = call_tool_err(&mut dispatcher, "calendar_set", json!({ "source": "google" }));
        assert!(message.contains("events"), "{message}");
        let message = call_tool_err(
            &mut dispatcher,
            "calendar_set",
            json!({ "source": "google", "events": [{ "title": "no id" }] }),
        );
        assert!(message.contains("invalid events payload"), "{message}");
        let message = call_tool_err(
            &mut dispatcher,
            "calendar_set",
            json!({ "source": "google", "day": "not-a-date", "events": [] }),
        );
        assert!(message.contains("invalid day"), "{message}");
    }

    #[test]
    fn calendar_set_ignores_a_source_field_smuggled_into_an_event() {
        // `source` is caller-assigned: an event carrying its own `source` must
        // not be able to write into a different lane.
        let (day_start, _) = Store::local_day_bounds(NOW);
        let mut dispatcher = dispatcher();
        set_calendar(
            &mut dispatcher,
            "outlook",
            json!([event_json(
                "work-sync",
                day_start + 3_600_000,
                day_start + 5_400_000
            )]),
        );
        call_tool(
            &mut dispatcher,
            "calendar_set",
            json!({
                "source": "google",
                "events": [{
                    "source": "outlook",
                    "externalId": "work-sync",
                    "title": "hijacked",
                    "start": iso(day_start + 3_600_000),
                }],
            }),
        );

        let today = call_tool(&mut dispatcher, "calendar_today", json!({}));
        let events = today["events"].as_array().unwrap();
        assert_eq!(events.len(), 2, "the outlook row was not overwritten: {today}");
        let outlook = events.iter().find(|e| e["source"] == "outlook").unwrap();
        assert_eq!(outlook["title"], "work-sync");
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
        assert_eq!(result["task"]["worktree"]["repoRoot"], REPO_DIR);

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
        assert_eq!(result["task"]["worktree"]["repoRoot"], REPO_DIR);
    }

    /// A host that records what it was asked to do and answers with whatever
    /// the test wants — the app's real one tears down worktrees, which has no
    /// place in a unit test of the tool's shape.
    /// Each `(id, force)` the host was called with, shared with the test.
    type HostCalls = std::sync::Arc<std::sync::Mutex<Vec<(i64, bool)>>>;

    struct FakeHost {
        answer: std::sync::Mutex<Option<Result<TaskDeletion, String>>>,
        calls: HostCalls,
    }

    impl TaskHost for FakeHost {
        fn delete_task(&self, id: i64, force: bool) -> Result<TaskDeletion, String> {
            self.calls.lock().unwrap().push((id, force));
            self.answer.lock().unwrap().take().expect("one delete per test")
        }
    }

    fn with_host(answer: Result<TaskDeletion, String>) -> (Dispatcher, HostCalls) {
        let calls: HostCalls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = FakeHost {
            answer: std::sync::Mutex::new(Some(answer)),
            calls: std::sync::Arc::clone(&calls),
        };
        (dispatcher().with_task_host(Box::new(host)), calls)
    }

    fn deleted(name: &str, messages: Vec<String>) -> Result<TaskDeletion, String> {
        Ok(TaskDeletion::Deleted { name: name.to_string(), messages })
    }

    /// The whole point of routing deletion through a host: a dispatcher with
    /// none must refuse rather than quietly fall back to deleting the row,
    /// which would strand the worktree on disk.
    #[test]
    fn task_delete_without_a_host_refuses() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(&mut dispatcher, "task_delete", json!({ "id": 1 }));
        assert!(message.contains("no task host"), "{message}");
        // And the row it declined to delete is still there.
        let open = call_tool(&mut dispatcher, "task_list", json!({}));
        assert_eq!(open["tasks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn task_delete_passes_the_id_and_force_through_and_names_what_it_deleted() {
        let (mut dispatcher, calls) =
            with_host(deleted("open task", vec!["removed the worktree".into()]));
        let result = call_tool(&mut dispatcher, "task_delete", json!({ "id": 1, "force": true }));
        assert_eq!(result["status"], "deleted");
        assert_eq!(result["id"], 1);
        // The name comes back from the host, which had to resolve the row
        // anyway — the dispatcher never reads it a second time.
        assert_eq!(result["text"], "open task");
        assert_eq!(result["messages"][0], "removed the worktree");
        assert_eq!(*calls.lock().unwrap(), vec![(1, true)]);
    }

    /// A guarded refusal is a normal result, not a tool error: reporting it as
    /// an error would tell a calling agent the delete *failed* and invite a
    /// retry with force, when it was declined on purpose.
    #[test]
    fn task_delete_reports_a_refusal_as_a_normal_result() {
        let (mut dispatcher, calls) = with_host(Ok(TaskDeletion::Refused {
            name: "open task".to_string(),
            blockers: vec![json!({
                "kind": "dirtyTree",
                "message": "2 uncommitted files",
                "remedy": "commit or stash them",
                "losesWork": true,
            })],
            messages: vec![],
        }));
        let result = call_tool(&mut dispatcher, "task_delete", json!({ "id": 1 }));
        assert_eq!(result["status"], "refused");
        assert_eq!(result["blockers"][0]["kind"], "dirtyTree");
        assert_eq!(result["blockers"][0]["losesWork"], true);
        // Force defaults off — a refusal must be reachable without asking for one.
        assert_eq!(*calls.lock().unwrap(), vec![(1, false)]);
    }

    #[test]
    fn task_delete_requires_an_id() {
        let (mut dispatcher, calls) = with_host(deleted("open task", vec![]));
        let message = call_tool_err(&mut dispatcher, "task_delete", json!({}));
        assert!(message.contains("missing required argument: id"), "{message}");
        // Rejected before the host could touch anything.
        assert!(calls.lock().unwrap().is_empty());
    }

    /// An unknown id is the host's answer, not a pre-flight check here — it has
    /// to resolve the row to delete it, so diagnosing it twice would be a second
    /// read for a string the host already produces.
    #[test]
    fn task_delete_surfaces_the_hosts_unknown_id_error() {
        let (mut dispatcher, calls) = with_host(Err("no board task #9999".to_string()));
        let message = call_tool_err(&mut dispatcher, "task_delete", json!({ "id": 9999 }));
        assert!(message.contains("no board task #9999"), "{message}");
        assert_eq!(*calls.lock().unwrap(), vec![(9999, false)]);
    }

    #[test]
    fn task_create_rejects_an_untracked_repo() {
        let mut dispatcher = dispatcher();
        let message =
            call_tool_err(&mut dispatcher, "task_create", json!({ "repo": "nope", "title": "x" }));
        assert!(message.contains("unknown repo: nope"), "{message}");
        assert!(message.contains("demo"), "error should list tracked repos: {message}");

        let mut empty = Dispatcher::new(seeded_store()).with_tracked_repos(vec![]);
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

    /// The lane check is what stops a typo minting a calendar nothing will ever
    /// write again — rows that still feed `calendar_next` and that no sweep
    /// removes. Exactly the orphan-lane failure the v9 migration destroyed data
    /// to avoid, so it has to hold at runtime too, not only at migration time.
    #[test]
    fn calendar_set_refuses_an_unconfigured_source() {
        let mut dispatcher = dispatcher();
        let message = call_tool_err(
            &mut dispatcher,
            "calendar_set",
            json!({ "source": "gcal", "events": [] }),
        );
        assert!(message.contains("unknown calendar source: gcal"), "{message}");
        assert!(message.contains("google"), "refusal lists configured lanes: {message}");
    }

    /// Fails closed: with nothing configured, every push is refused rather than
    /// allowed to create the first lane implicitly.
    #[test]
    fn calendar_set_refuses_everything_when_no_calendars_are_configured() {
        let mut dispatcher = Dispatcher::new(seeded_store()).with_calendar_sources(vec![]);
        let message = call_tool_err(
            &mut dispatcher,
            "calendar_set",
            json!({ "source": "google", "events": [] }),
        );
        assert!(message.contains("no calendars are configured"), "{message}");
    }

    /// A configured lane still works, including the empty-array clear.
    #[test]
    fn calendar_set_accepts_a_configured_source() {
        let mut dispatcher = dispatcher();
        let result =
            call_tool(&mut dispatcher, "calendar_set", json!({ "source": "google", "events": [] }));
        assert_eq!(result["source"], "google");
        assert_eq!(result["written"], 0);
    }

    /// Writes are flagged in the contract, not inferred from the wording of a
    /// description — the UI's write warning is the only signal a human gets
    /// before a mutation now that the capability gate is gone, so it must not
    /// depend on an adjective someone might reword.
    #[test]
    fn mutating_tools_are_flagged_read_only_false() {
        let tools = tool_definitions();
        let tools = tools.as_array().unwrap();
        let flag = |name: &str| {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .and_then(|t| t["annotations"]["readOnlyHint"].as_bool())
        };
        assert_eq!(flag("task_create"), Some(false), "task_create writes");
        assert_eq!(flag("calendar_set"), Some(false), "calendar_set writes");
        // Reads say so explicitly. The annotation is stamped from
        // `WRITING_TOOLS`, so every tool carries the flag and a client never has
        // to read "absent" as either answer.
        assert_eq!(flag("task_list"), Some(true));
        assert_eq!(flag("calendar_next"), Some(true));

        // The wire flag and the transport's refresh decision come from one
        // list, so they cannot disagree — this is the assertion that pins it.
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            let read_only = tool["annotations"]["readOnlyHint"].as_bool().unwrap();
            assert_eq!(read_only, !tool_writes(name), "{name}'s hint must match tool_writes");
        }
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
