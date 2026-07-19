//! Per-terminal Claude Code IDE servers (see `docs/CLAUDE-CODE-IDE.md`).
//!
//! Every embedded terminal gets its own localhost WebSocket MCP server, a
//! `~/.claude/ide/<port>.lock` advertisement, and a `CLAUDE_CODE_SSE_PORT`
//! stamp in its PTY env, so a `claude` started in a pane pairs with exactly
//! that pane — highlights made in the app's diff view for that folder become
//! the session's selection context. Protocol logic lives in the Tauri-free
//! `tt_ide` crate; this module owns sockets, tokens and lifecycle.
//!
//! Connections are served concurrently: Claude Code (>= 2.1.x) is
//! multi-process — the interactive TUI and its session daemon each hold their
//! own IDE connection, and both need the selection stream. The [`IdeServer`]
//! handle lives inside the terminal's `Session`; dropping it (kill,
//! replacement, window teardown) aborts the server task and removes the
//! lockfile.

use std::collections::{HashMap, HashSet};
use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;
use tt_agentboard::fs_notify::MultiFileNotifier;

use crate::terminal::TermState;

/// Emitted whenever a Claude Code CLI connects to / disconnects from a
/// terminal's IDE server, so the diff pane can show a live "claude" badge.
pub const STATUS_EVENT: &str = "ide://status";
const MAIN_WINDOW_LABEL: &str = "main";

/// The name Claude Code shows for this IDE (`/ide`, status line, lockfile).
pub const IDE_NAME: &str = "Towles Tool";

/// One `ide://status` edge, plus enough to seed initial state via [`ide_status`].
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdeStatus {
    pub term_id: String,
    pub dir: String,
    pub port: u16,
    pub connected: bool,
}

/// State shared between the server task and command handlers. Mutexes here
/// guard tiny in-memory reads/writes only — nothing IO-bound.
struct Shared {
    term_id: String,
    cwd: PathBuf,
    port: u16,
    auth_token: String,
    /// Latest diff-pane selection; serves `getCurrentSelection`/`getLatestSelection`.
    selection: Mutex<Option<tt_ide::Selection>>,
    /// The file open in the app's code viewer (path + dirty), if any — the
    /// Files tab has at most one at a time, so a set replaces the whole
    /// value (`set_open_file`).
    open_file: Mutex<Option<tt_ide::OpenFile>>,
    /// Paths the diff pane currently has unsaved edits in — unlike the Files
    /// tab, several of these can be dirty at once, so this is an upsert/
    /// remove set (`set_diff_file_dirty`), not a single replaced value.
    /// [`Shared::context`] merges both into `ServerContext::open_files` for
    /// `getOpenEditors` / `checkDocumentDirty`.
    diff_dirty_files: Mutex<HashSet<String>>,
    /// Compiler-diagnostics hub, queried per message for this folder's
    /// current `getDiagnostics` payload.
    diagnostics: Arc<crate::diagnostics::DiagHub>,
    /// Outbound frame senders, one per connected CLI process (Claude Code is
    /// multi-process: the TUI and its session daemon may both connect).
    out: Mutex<Vec<UnboundedSender<Message>>>,
}

impl Shared {
    fn context(&self) -> tt_ide::ServerContext {
        let mut open_files: Vec<tt_ide::OpenFile> =
            self.open_file.lock().unwrap().clone().into_iter().collect();
        open_files.extend(
            self.diff_dirty_files
                .lock()
                .unwrap()
                .iter()
                .map(|path| tt_ide::OpenFile { path: path.clone(), dirty: true }),
        );
        tt_ide::ServerContext {
            ide_name: IDE_NAME.to_string(),
            workspace_folder: self.cwd.clone(),
            selection: self.selection.lock().unwrap().clone(),
            open_files,
            diagnostics: self.diagnostics.wire_for(&self.cwd),
        }
    }

    /// Queue a notification frame for every connected CLI. Returns false when
    /// none is connected (the frame is dropped — selection state is still
    /// cached, so a later connection can pull it via `getLatestSelection`).
    fn push(&self, frame: String) -> bool {
        let mut guard = self.out.lock().unwrap();
        guard.retain(|tx| tx.send(Message::text(frame.clone())).is_ok());
        !guard.is_empty()
    }

    fn is_connected(&self) -> bool {
        !self.out.lock().unwrap().is_empty()
    }
}

/// A running IDE server for one terminal. Owned by the terminal's `Session`;
/// drop tears everything down.
pub struct IdeServer {
    port: u16,
    shared: Arc<Shared>,
    lock_dir: Option<PathBuf>,
    task: tauri::async_runtime::JoinHandle<()>,
}

impl IdeServer {
    /// Bind `127.0.0.1:0` (OS-assigned port — never hardcoded, slots run
    /// concurrently), write the lockfile, and start the accept loop.
    pub fn start(
        app: AppHandle,
        term_id: String,
        cwd: PathBuf,
        diagnostics: Arc<crate::diagnostics::DiagHub>,
    ) -> Result<IdeServer, String> {
        let listener = StdTcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("failed to bind IDE server socket: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed to configure IDE server socket: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("failed to read IDE server port: {e}"))?
            .port();

        let auth_token = new_auth_token();
        let lock_dir = dirs::home_dir().map(|home| tt_ide::lockfile::lock_dir(&home));
        if let Some(dir) = &lock_dir {
            let lockfile = tt_ide::Lockfile::new(std::process::id(), &cwd, IDE_NAME, &auth_token);
            tt_ide::lockfile::write(dir, port, &lockfile)
                .map_err(|e| format!("failed to write IDE lockfile: {e}"))?;
        }

        let shared = Arc::new(Shared {
            term_id,
            cwd,
            port,
            auth_token,
            selection: Mutex::new(None),
            open_file: Mutex::new(None),
            diff_dirty_files: Mutex::new(HashSet::new()),
            diagnostics,
            out: Mutex::new(Vec::new()),
        });

        let task = tauri::async_runtime::spawn(accept_loop(app, listener, shared.clone()));
        Ok(IdeServer { port, shared, lock_dir, task })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn cwd(&self) -> &Path {
        &self.shared.cwd
    }

    pub fn connected(&self) -> bool {
        self.shared.is_connected()
    }

    pub fn status(&self) -> IdeStatus {
        IdeStatus {
            term_id: self.shared.term_id.clone(),
            dir: self.shared.cwd.to_string_lossy().into_owned(),
            port: self.port,
            connected: self.connected(),
        }
    }

    /// Cache `selection` and push a `selection_changed` to the connected CLI.
    pub fn set_selection(&self, selection: tt_ide::Selection) {
        let frame = tt_ide::selection_changed_frame(&selection);
        *self.shared.selection.lock().unwrap() = Some(selection);
        self.shared.push(frame);
    }

    /// Push an `at_mentioned` (explicit "send to Claude"). Returns whether a
    /// CLI was connected to receive it.
    pub fn at_mention(&self, file_path: &str, lines: Option<(u32, u32)>) -> bool {
        self.shared.push(tt_ide::at_mentioned_frame(file_path, lines))
    }

    /// Tell connected CLIs these files' diagnostics went stale (they re-pull
    /// via `getDiagnostics`).
    pub fn notify_diagnostics(&self, uris: &[String]) {
        self.shared.push(tt_ide::diagnostics::diagnostics_changed_frame(uris));
    }

    /// Record which file the app's code viewer has open (None = closed).
    pub fn set_open_file(&self, open: Option<tt_ide::OpenFile>) {
        *self.shared.open_file.lock().unwrap() = open;
    }

    /// Record (or clear) one diff-pane file's unsaved-edit state. Unlike
    /// [`Self::set_open_file`], this upserts a single path into a set rather
    /// than replacing the whole value — the diff pane can have several files
    /// dirty at once. A clean file (`dirty: false`) is simply removed: only
    /// dirty files need to be visible to `getOpenEditors`/`checkDocumentDirty`.
    pub fn set_diff_file_dirty(&self, path: String, dirty: bool) {
        let mut files = self.shared.diff_dirty_files.lock().unwrap();
        if dirty {
            files.insert(path);
        } else {
            files.remove(&path);
        }
    }
}

impl Drop for IdeServer {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(dir) = &self.lock_dir {
            tt_ide::lockfile::remove(dir, self.port);
        }
    }
}

/// Random bearer token for the lockfile — UUID-shaped like the extension's.
fn new_auth_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Accept connections forever (until the task is aborted), serving one client
/// at a time. Errors on an individual connection just recycle the loop.
async fn accept_loop(app: AppHandle, listener: StdTcpListener, shared: Arc<Shared>) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        eprintln!("warning: IDE server for terminal {} failed to start", shared.term_id);
        return;
    };
    loop {
        let Ok((stream, _addr)) = listener.accept().await else {
            return;
        };
        // Concurrent connections: Claude Code >= 2.1.x is multi-process (the
        // interactive TUI plus a session daemon), and each process may hold
        // its own IDE connection. Every authenticated client gets the same
        // dispatcher state and every notification frame.
        let app = app.clone();
        let shared = shared.clone();
        tauri::async_runtime::spawn(async move {
            serve_connection(&app, stream, &shared).await;
        });
    }
}

/// One CLI connection: authenticated WebSocket handshake, then a frame loop
/// bridging incoming JSON-RPC to `tt_ide::handle_message` and outgoing
/// notifications from the diff pane.
// The handshake callback's Result<Response, ErrorResponse> shape is imposed
// by tungstenite's Callback trait — the Err size is not ours to shrink.
#[allow(clippy::result_large_err)]
async fn serve_connection(app: &AppHandle, stream: tokio::net::TcpStream, shared: &Arc<Shared>) {
    let auth = shared.auth_token.clone();
    let callback = move |req: &Request, mut resp: Response| {
        let presented = req
            .headers()
            .get("x-claude-code-ide-authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if presented != auth {
            let mut denied = ErrorResponse::new(Some("Unauthorized".to_string()));
            *denied.status_mut() = StatusCode::UNAUTHORIZED;
            return Err(denied);
        }
        // Echo the `mcp` subprotocol when the client requested it (the CLI
        // always does; some WS stacks drop the connection without the echo).
        let requested_mcp = req
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|p| p.trim() == "mcp"));
        if requested_mcp && let Ok(value) = "mcp".parse() {
            resp.headers_mut().insert("sec-websocket-protocol", value);
        }
        Ok(resp)
    };

    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
        return;
    };
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = unbounded_channel::<Message>();
    shared.out.lock().unwrap().push(tx.clone());
    emit_status(app, shared);
    // A fresh CLI wants fresh diagnostics — kick a (debounced) check run.
    shared.diagnostics.request(&shared.cwd);

    loop {
        tokio::select! {
            incoming = source.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        // Tools with app-side effects (openFile, openDiff and
                        // friends) are answered here so the webview can act;
                        // everything else goes through the pure dispatcher.
                        let reply = match intercept_app_tool(app, shared, &tx, text.as_str()) {
                            Intercept::Reply(reply) => Some(reply),
                            // Response rides the outbound channel once the
                            // user accepts/rejects in the review UI.
                            Intercept::Deferred => None,
                            Intercept::NotOurs => {
                                tt_ide::handle_message(text.as_str(), &shared.context())
                            }
                        };
                        if let Some(reply) = reply
                            && sink.send(Message::text(reply)).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
            outgoing = rx.recv() => {
                match outgoing {
                    Some(frame) => {
                        if sink.send(frame).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    shared.out.lock().unwrap().retain(|sender| !sender.same_channel(&tx));
    emit_status(app, shared);
}

/// Emitted when a CLI calls `openFile`: the frontend focuses the folder's
/// Files tab on that file (optionally selecting `startText`..`endText`).
pub const OPEN_FILE_EVENT: &str = "ide://open-file";
/// Emitted when a CLI calls `openDiff`: the frontend shows an accept/reject
/// review (Monaco DiffEditor) and resolves it via `ide_diff_resolve`.
pub const OPEN_DIFF_EVENT: &str = "ide://open-diff";
/// Emitted when the CLI closes diff tabs (`close_tab`/`closeAllDiffTabs`) so
/// the frontend can drop the matching review overlays.
pub const CLOSE_DIFF_EVENT: &str = "ide://close-diff";

/// One blocking `openDiff` waiting for the user's accept/reject. The wire
/// response is deferred: a task per request waits on `respond` and sends the
/// tool result down the *requesting* connection when the user decides.
struct PendingDiff {
    request_id: u64,
    dir: PathBuf,
    tab_name: String,
    new_file_path: PathBuf,
    respond: tokio::sync::oneshot::Sender<serde_json::Value>,
}

/// App-wide registry of unresolved `openDiff` requests (managed state — the
/// resolving Tauri command only knows the request id).
#[derive(Default)]
pub struct DiffRequests {
    pending: Mutex<Vec<PendingDiff>>,
    next_id: AtomicU64,
}

impl DiffRequests {
    /// Resolve one request: on accept, write `final_contents` to the target
    /// (the reviewer may have tweaked the proposed side). Returns the wire
    /// result to send, or an error when the id is unknown/already resolved.
    fn resolve(
        &self,
        request_id: u64,
        accepted: bool,
        final_contents: Option<String>,
    ) -> Result<(), String> {
        let entry = {
            let mut pending = self.pending.lock().unwrap();
            let index = pending
                .iter()
                .position(|p| p.request_id == request_id)
                .ok_or("diff review already resolved")?;
            pending.remove(index)
        };
        let result = if accepted {
            let contents = final_contents.unwrap_or_default();
            atomic_write(&entry.new_file_path, &contents)?;
            serde_json::json!({ "content": [
                { "type": "text", "text": "FILE_SAVED" },
                { "type": "text", "text": contents },
            ]})
        } else {
            rejected_result(&entry.tab_name)
        };
        let _ = entry.respond.send(result);
        Ok(())
    }

    /// Reject every pending review matching `dir` (and `tab_name`, when
    /// given). Returns how many were closed.
    fn reject_matching(&self, dir: &Path, tab_name: Option<&str>) -> usize {
        let drained: Vec<PendingDiff> = {
            let mut pending = self.pending.lock().unwrap();
            let (matching, rest): (Vec<_>, Vec<_>) = pending
                .drain(..)
                .partition(|p| p.dir == dir && tab_name.is_none_or(|t| t == p.tab_name));
            *pending = rest;
            matching
        };
        let count = drained.len();
        for entry in drained {
            let _ = entry.respond.send(rejected_result(&entry.tab_name));
        }
        count
    }
}

fn rejected_result(tab_name: &str) -> serde_json::Value {
    serde_json::json!({ "content": [
        { "type": "text", "text": "DIFF_REJECTED" },
        { "type": "text", "text": tab_name },
    ]})
}

/// Atomic write (tmp + rename), shared by the save command and diff accept.
/// Returns the written file's mtime, taken from the tmp file *before* the
/// rename (which preserves it): a stat of the destination path after the
/// rename could adopt a concurrent writer's mtime as our save token, which
/// would make the frontend treat that foreign write as its own echo and
/// silently overwrite it on the next save.
fn atomic_write(abs: &Path, content: &str) -> Result<i64, String> {
    let parent = abs.parent().ok_or_else(|| format!("no parent dir for {}", abs.display()))?;
    let tmp = parent.join(format!(
        ".{}.tt-tmp",
        abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    std::fs::write(&tmp, content).map_err(|e| format!("cannot write {}: {e}", abs.display()))?;
    let written_mtime = std::fs::metadata(&tmp)
        .map(|meta| mtime_ms(&meta))
        .map_err(|e| format!("cannot stat {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, abs).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot save {}: {e}", abs.display())
    })?;
    Ok(written_mtime)
}

/// Outcome of the app-side tool interception.
enum Intercept {
    /// Not an app-side tool — run the pure dispatcher.
    NotOurs,
    /// Immediate response.
    Reply(String),
    /// Response deferred to the review UI (a task holds this connection's
    /// outbound sender and answers on resolve).
    Deferred,
}

/// A raw JSON-RPC result response (the payload is already the full MCP
/// result, e.g. openDiff's two-block content).
fn raw_result_response(id: &serde_json::Value, result: &serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// Handle `tools/call`s that need the webview to act: `openFile` (focus a
/// file), `openDiff` (blocking accept/reject review), `close_tab` /
/// `closeAllDiffTabs` (reject + dismiss reviews).
fn intercept_app_tool(
    app: &AppHandle,
    shared: &Arc<Shared>,
    out: &UnboundedSender<Message>,
    message: &str,
) -> Intercept {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(message) else {
        return Intercept::NotOurs;
    };
    if value.get("method").and_then(serde_json::Value::as_str) != Some("tools/call") {
        return Intercept::NotOurs;
    }
    let Some(name) = value.pointer("/params/name").and_then(serde_json::Value::as_str) else {
        return Intercept::NotOurs;
    };
    let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let args = value.pointer("/params/arguments").cloned().unwrap_or_else(|| serde_json::json!({}));
    let arg_str = |key: &str| args.get(key).and_then(serde_json::Value::as_str).unwrap_or_default();

    match name {
        "openFile" => {
            let file_path = arg_str("filePath").to_string();
            let payload = serde_json::json!({
                "dir": shared.cwd.to_string_lossy(),
                "filePath": file_path,
                "startText": args.get("startText"),
                "endText": args.get("endText"),
                "selectToEndOfLine": args.get("selectToEndOfLine"),
            });
            let _ = app.emit_to(MAIN_WINDOW_LABEL, OPEN_FILE_EVENT, payload);
            let result = serde_json::json!({
                "success": true,
                "message": format!("Opening {file_path} in Towles Tool"),
            });
            Intercept::Reply(tt_ide::tool_result_response(id, &result))
        }
        "openDiff" => {
            let requests = app.state::<DiffRequests>();
            let request_id = requests.next_id.fetch_add(1, Ordering::Relaxed);
            let tab_name = arg_str("tab_name").to_string();
            let new_file_path = PathBuf::from(arg_str("new_file_path"));
            let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
            requests.pending.lock().unwrap().push(PendingDiff {
                request_id,
                dir: shared.cwd.clone(),
                tab_name: tab_name.clone(),
                new_file_path,
                respond: respond_tx,
            });
            let payload = serde_json::json!({
                "requestId": request_id,
                "dir": shared.cwd.to_string_lossy(),
                "oldFilePath": arg_str("old_file_path"),
                "newFilePath": arg_str("new_file_path"),
                "newFileContents": arg_str("new_file_contents"),
                "tabName": tab_name.clone(),
            });
            let _ = app.emit_to(MAIN_WINDOW_LABEL, OPEN_DIFF_EVENT, payload);
            // Answer on this connection when the user decides; a dropped
            // sender (teardown) degrades to a rejection so the CLI never
            // hangs forever.
            let out = out.clone();
            tauri::async_runtime::spawn(async move {
                let result = respond_rx.await.unwrap_or_else(|_| rejected_result(&tab_name));
                let _ = out.send(Message::text(raw_result_response(&id, &result)));
            });
            Intercept::Deferred
        }
        "close_tab" => {
            let tab_name = arg_str("tab_name").to_string();
            app.state::<DiffRequests>().reject_matching(&shared.cwd, Some(&tab_name));
            let _ = app.emit_to(
                MAIN_WINDOW_LABEL,
                CLOSE_DIFF_EVENT,
                serde_json::json!({ "dir": shared.cwd.to_string_lossy(), "tabName": tab_name }),
            );
            let result =
                serde_json::json!({ "content": [{ "type": "text", "text": "TAB_CLOSED" }] });
            Intercept::Reply(raw_result_response(&id, &result))
        }
        "closeAllDiffTabs" => {
            let closed = app.state::<DiffRequests>().reject_matching(&shared.cwd, None);
            let _ = app.emit_to(
                MAIN_WINDOW_LABEL,
                CLOSE_DIFF_EVENT,
                serde_json::json!({ "dir": shared.cwd.to_string_lossy(), "tabName": null }),
            );
            let result = serde_json::json!({ "content": [
                { "type": "text", "text": format!("CLOSED_{closed}_DIFF_TABS") },
            ]});
            Intercept::Reply(raw_result_response(&id, &result))
        }
        _ => Intercept::NotOurs,
    }
}

/// The review UI decided: accept (write `finalContents`, possibly tweaked in
/// the editor) or reject. Errors when the request is unknown/already gone.
#[tauri::command]
pub fn ide_diff_resolve(
    requests: State<DiffRequests>,
    request_id: u64,
    accepted: bool,
    final_contents: Option<String>,
) -> Result<(), String> {
    requests.resolve(request_id, accepted, final_contents)
}

fn emit_status(app: &AppHandle, shared: &Arc<Shared>) {
    let status = IdeStatus {
        term_id: shared.term_id.clone(),
        dir: shared.cwd.to_string_lossy().into_owned(),
        port: shared.port,
        connected: shared.is_connected(),
    };
    let _ = app.emit_to(MAIN_WINDOW_LABEL, STATUS_EVENT, status);
}

/// Remove lockfiles left behind by towles-tool processes that died without
/// cleanup (crash, SIGKILL). Other IDEs' lockfiles are never touched. Liveness
/// is only checkable via /proc on Linux; elsewhere we skip the sweep — the
/// CLI's own pid check ignores stale files anyway.
pub fn sweep_stale_lockfiles() {
    #[cfg(target_os = "linux")]
    if let Some(home) = dirs::home_dir() {
        let dir = tt_ide::lockfile::lock_dir(&home);
        let alive = |pid: u32| Path::new(&format!("/proc/{pid}")).exists();
        tt_ide::lockfile::sweep_stale(&dir, IDE_NAME, &alive);
    }
}

// ---------------------------------------------------------------------------
// Commands (invoked from the diff pane)

/// Resolve `file_path` (repo-relative) against `dir`, read the selected span
/// from disk, and build the wire selection (0-based). Lines are 1-based
/// inclusive; `start_char`/`end_char` are optional 0-based character columns
/// (the Monaco viewer sends them; the diff pane's gutter selects whole
/// lines). The text comes from the real file — the diff pane may only show
/// hunk excerpts.
fn build_selection(
    dir: &Path,
    file_path: &str,
    start_line: u32,
    end_line: u32,
    start_char: Option<u32>,
    end_char: Option<u32>,
) -> tt_ide::Selection {
    let abs = dir.join(file_path);
    let (start, end) = (start_line.min(end_line).max(1), start_line.max(end_line));
    let content = std::fs::read_to_string(&abs).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let from = (start as usize - 1).min(lines.len());
    let to = (end as usize).min(lines.len());

    let clip = |line: Option<&&str>, character: u32| -> usize {
        line.map(|l| (character as usize).min(l.chars().count())).unwrap_or(0)
    };
    let first_char = start_char.map(|c| clip(lines.get(from), c)).unwrap_or(0);
    let last_line_len = lines.get(to.saturating_sub(1)).map(|l| l.chars().count()).unwrap_or(0);
    let last_char =
        end_char.map(|c| clip(lines.get(to.saturating_sub(1)), c)).unwrap_or(last_line_len);

    let mut selected: Vec<String> = lines[from..to].iter().map(|l| l.to_string()).collect();
    if let Some(last) = selected.last_mut() {
        *last = last.chars().take(last_char).collect();
    }
    if let Some(first) = selected.first_mut() {
        *first = first.chars().skip(first_char).collect();
    }
    let text = selected.join("\n");

    let mut selection = tt_ide::Selection::range(&abs, start - 1, end - 1, last_char as u32, text);
    selection.selection.start.character = first_char as u32;
    selection
}

/// A highlight was made in the diff pane for `dir`: cache + push it to every
/// terminal IDE server rooted there. Returns whether any connected CLI got it
/// live (false = cached only, or no terminal in that folder).
#[tauri::command]
pub fn ide_set_selection(
    state: State<TermState>,
    dir: String,
    file_path: String,
    start_line: u32,
    end_line: u32,
    start_char: Option<u32>,
    end_char: Option<u32>,
) -> Result<bool, String> {
    let dir = PathBuf::from(dir);
    let selection = build_selection(&dir, &file_path, start_line, end_line, start_char, end_char);
    let mut delivered = false;
    state.for_ide_servers(&dir, |server| {
        let frame_selection = selection.clone();
        let connected = server.connected();
        server.set_selection(frame_selection);
        delivered |= connected;
    });
    Ok(delivered)
}

/// The highlight was dismissed — push an empty selection (what VS Code sends
/// when the cursor collapses) so stale context doesn't ride the next prompt.
#[tauri::command]
pub fn ide_clear_selection(
    state: State<TermState>,
    dir: String,
    file_path: String,
) -> Result<(), String> {
    let dir = PathBuf::from(dir);
    let cleared = tt_ide::Selection::cleared(&dir.join(&file_path));
    state.for_ide_servers(&dir, |server| server.set_selection(cleared.clone()));
    Ok(())
}

/// Explicit "send to Claude": emits `at_mentioned`, which becomes an
/// `@file#Lx-y` reference in the session's prompt. Errors when no CLI is
/// connected in that folder — the frontend surfaces that as a toast.
#[tauri::command]
pub fn ide_at_mention(
    state: State<TermState>,
    dir: String,
    file_path: String,
    start_line: Option<u32>,
    end_line: Option<u32>,
) -> Result<(), String> {
    let dir = PathBuf::from(dir);
    let abs = dir.join(&file_path);
    // Lines omitted = a whole-file mention (the Files tab); the wire drops
    // lineStart/lineEnd together in that case.
    let lines = match (start_line, end_line) {
        (Some(s), Some(e)) => Some((s.min(e).max(1) - 1, s.max(e).max(1) - 1)),
        _ => None,
    };
    let mut delivered = false;
    state.for_ide_servers(&dir, |server| {
        delivered |= server.at_mention(&abs.to_string_lossy(), lines);
    });
    if delivered {
        Ok(())
    } else {
        Err("No Claude Code session is connected in this folder — run `claude` in its terminal first".into())
    }
}

/// Snapshot of every terminal's IDE server, for the frontend's initial state
/// (live updates ride the `ide://status` event).
#[tauri::command]
pub fn ide_status(app: AppHandle) -> Vec<IdeStatus> {
    app.state::<TermState>().ide_statuses()
}

/// The code viewer opened (Some) or closed (None) a file in `dir`, or its
/// dirty state flipped — reflected to CLIs via `getOpenEditors` /
/// `checkDocumentDirty`.
#[tauri::command]
pub fn ide_set_open_file(
    state: State<TermState>,
    dir: String,
    file_path: Option<String>,
    dirty: Option<bool>,
) {
    let dir = PathBuf::from(dir);
    let open = file_path.map(|f| tt_ide::OpenFile {
        path: dir.join(f).to_string_lossy().into_owned(),
        dirty: dirty.unwrap_or(false),
    });
    state.for_ide_servers(&dir, |server| server.set_open_file(open.clone()));
}

/// One diff-pane file's unsaved-edit state flipped — reflected to CLIs via
/// `getOpenEditors`/`checkDocumentDirty` alongside whatever the Files tab has
/// open. Unlike `ide_set_open_file`, `dir`+`file_path` together are just one
/// entry in a set the diff pane maintains itself (several files can be dirty
/// there at once); `dirty: false` removes it rather than replacing the set.
#[tauri::command]
pub fn ide_set_diff_dirty(state: State<TermState>, dir: String, file_path: String, dirty: bool) {
    let path = PathBuf::from(&dir).join(file_path).to_string_lossy().into_owned();
    state
        .for_ide_servers(Path::new(&dir), |server| server.set_diff_file_dirty(path.clone(), dirty));
}

/// A viewer file read: the content plus the mtime the save path uses as its
/// conflict token.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRead {
    pub content: String,
    pub mtime_ms: i64,
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Substrings the editor bridge matches on to pick a
/// `FileSystemProviderErrorCode` (see `errorCodeFor` in `lib/monaco-fs.ts`).
/// VS Code behaves differently per code — `FileExists` offers an overwrite
/// prompt — so rewording these changes the UI. Pinned by a test below.
pub const ERR_ESCAPES_FOLDER: &str = "path escapes the folder";
pub const ERR_ALREADY_EXISTS: &str = "already exists";

/// The "that name is taken" error, spelled once so `ide_rename` and
/// `ide_create_dir` can't drift apart — the frontend matches on the substring
/// to pick `FileExists`, and only one of them drifting would be invisible.
fn already_exists(file_path: &str) -> String {
    format!("{file_path} {ERR_ALREADY_EXISTS}")
}

/// Guard against escapes — viewer paths must stay inside the folder.
///
/// Two ways out, and both have to be closed for the join to be safe. `..`
/// walks up; an *absolute* path is worse, because `Path::join` silently
/// discards the base and returns the argument whole, so `confined("/w/repo",
/// "/etc/passwd")` would hand back `/etc/passwd`. That was survivable while
/// these commands only read; `ide_delete` and `ide_rename` mutate.
///
/// With both rejected the join can't escape lexically, which is the right
/// depth here: `to`/`from` for a rename may not exist yet, so canonicalizing
/// isn't an option.
fn confined(dir: &Path, file_path: &str) -> Result<PathBuf, String> {
    let rel = Path::new(file_path);
    let escapes =
        rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir));
    if escapes {
        return Err(format!("{ERR_ESCAPES_FOLDER}: {file_path}"));
    }
    Ok(dir.join(file_path))
}

/// Read a repo file for the code viewer. Size-capped and text-only — the
/// viewer is for code, not assets.
#[tauri::command]
pub async fn ide_read_file(dir: String, file_path: String) -> Result<FileRead, String> {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    tauri::async_runtime::spawn_blocking(move || {
        let abs = confined(Path::new(&dir), &file_path)?;
        let meta = std::fs::metadata(&abs).map_err(|e| format!("cannot open {file_path}: {e}"))?;
        if meta.len() > MAX_BYTES {
            return Err(format!("{file_path} is too large to preview ({} KB)", meta.len() / 1024));
        }
        let bytes = std::fs::read(&abs).map_err(|e| format!("cannot read {file_path}: {e}"))?;
        if bytes.contains(&0) {
            return Err(format!("{file_path} looks like a binary file"));
        }
        Ok(FileRead {
            content: String::from_utf8_lossy(&bytes).into_owned(),
            mtime_ms: mtime_ms(&meta),
        })
    })
    .await
    .map_err(|e| format!("read task failed: {e}"))?
}

/// Save the viewer's buffer: atomic (tmp + rename into place) with an mtime
/// conflict token — if the file changed on disk since it was read (an agent
/// edited it), the save is refused rather than silently clobbering. Returns
/// the new mtime token.
#[tauri::command]
pub async fn ide_write_file(
    dir: String,
    file_path: String,
    content: String,
    expected_mtime_ms: Option<i64>,
) -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let abs = confined(Path::new(&dir), &file_path)?;
        if let (Some(expected), Ok(meta)) = (expected_mtime_ms, std::fs::metadata(&abs))
            && mtime_ms(&meta) != expected
        {
            tracing::debug!(dir = %dir, file = %file_path, "viewer save refused: changed on disk");
            return Err(format!(
                "{file_path} changed on disk since it was opened — reopen it to pick up the new contents"
            ));
        }
        let written_mtime = atomic_write(&abs, &content)?;
        // Recorded because saves are no longer always a user gesture — the
        // editors auto-save after a typing pause — and "when did the app
        // write this file?" must stay answerable from the event log alone.
        tracing::debug!(dir = %dir, file = %file_path, bytes = content.len(), "viewer file saved");
        Ok(written_mtime)
    })
    .await
    .map_err(|e| format!("write task failed: {e}"))?
}

/// Emitted when files open in the code viewer / diff pane change on disk
/// underneath it — an agent edit in the same checkout, a `git checkout`, any
/// external writer. One event per debounce batch, carrying every touched
/// path; the viewer re-checks each and either reloads in place (clean
/// buffer) or raises its conflict banner (unsaved edits).
pub const FILE_CHANGED_EVENT: &str = "ide://file-changed";

/// Live disk watchers behind the code viewer's and diff pane's open files:
/// **one** [`MultiFileNotifier`] per checkout dir (inotify instances are a
/// scarce per-user resource — see the notifier's own doc), with per-file
/// refcounts inside it, so a 50-file diff pane and a viewer on the same file
/// all share one OS watcher. The last `ide_unwatch_files` for a dir drops it.
#[derive(Default)]
pub struct ViewerWatches(Mutex<HashMap<String, MultiFileNotifier>>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FilesChangedPayload {
    dir: String,
    file_paths: Vec<String>,
}

/// Start watching open viewer/diff files for on-disk changes; changes arrive
/// as [`FILE_CHANGED_EVENT`]s (debounced + batched in [`MultiFileNotifier`]).
/// Takes the whole list in one call — a 50-file diff pane must not pay 50
/// sync-command round-trips on the GTK main thread. Pair with a matching
/// `ide_unwatch_files` when the files close. Per-path registration failures
/// are logged and skipped, not propagated — those files just degrade to the
/// callers' poll-driven refresh.
#[tauri::command]
pub fn ide_watch_files(
    app: AppHandle,
    watches: State<ViewerWatches>,
    dir: String,
    file_paths: Vec<String>,
) -> Result<(), String> {
    let mut map = watches.0.lock().unwrap();
    let notifier = match map.entry(dir.clone()) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(v) => {
            let app = app.clone();
            let event_dir = dir.clone();
            let notifier = MultiFileNotifier::new(move |paths| {
                let file_paths: Vec<String> = paths
                    .iter()
                    .filter_map(|abs| abs.strip_prefix(&event_dir).ok())
                    .map(|rel| rel.to_string_lossy().into_owned())
                    .collect();
                if file_paths.is_empty() {
                    return;
                }
                tracing::debug!(dir = %event_dir, files = ?file_paths, "viewer files changed on disk");
                let payload = FilesChangedPayload { dir: event_dir.clone(), file_paths };
                let _ = app.emit_to(MAIN_WINDOW_LABEL, FILE_CHANGED_EVENT, payload);
            })
            .map_err(|e| format!("cannot start watching {dir}: {e}"))?;
            tracing::debug!(dir = %dir, "viewer watch instance started");
            v.insert(notifier)
        }
    };
    // Per-path failures (a parent directory the agent just deleted, a bad
    // path) must not doom the rest of the batch — an unwatched file only
    // degrades to the poll-driven safety net, and the caller tears down
    // with the same full list either way (unmatched removes are no-ops).
    for file_path in &file_paths {
        match confined(Path::new(&dir), file_path).map(|abs| notifier.add(&abs)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::debug!(dir = %dir, file = %file_path, error = %e, "viewer watch skipped")
            }
            Err(e) => {
                tracing::debug!(dir = %dir, file = %file_path, error = %e, "viewer watch skipped")
            }
        }
    }
    Ok(())
}

/// Drop one reference each to a batch of viewer file watches (see
/// [`ide_watch_files`]). Unmatched calls (a watch never started — browser
/// dev, an error path) are a no-op.
#[tauri::command]
pub fn ide_unwatch_files(watches: State<ViewerWatches>, dir: String, file_paths: Vec<String>) {
    let mut map = watches.0.lock().unwrap();
    let Some(notifier) = map.get_mut(&dir) else {
        return;
    };
    for file_path in &file_paths {
        if let Ok(abs) = confined(Path::new(&dir), file_path) {
            notifier.remove(&abs);
        }
    }
    if notifier.is_empty() {
        tracing::debug!(dir = %dir, "viewer watch instance stopped");
        map.remove(&dir);
    }
}

/// Minimal stat for the editor's filesystem-provider bridge (monaco-fs.ts).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsStat {
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: i64,
}

/// Stat one path for the VS Code filesystem provider. Same confinement rule
/// as [`ide_read_file`].
#[tauri::command]
pub async fn ide_stat(dir: String, file_path: String) -> Result<FsStat, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let abs = confined(Path::new(&dir), &file_path)?;
        let meta = std::fs::metadata(&abs).map_err(|e| format!("cannot stat {file_path}: {e}"))?;
        Ok(FsStat { is_dir: meta.is_dir(), size: meta.len(), mtime_ms: mtime_ms(&meta) })
    })
    .await
    .map_err(|e| format!("stat task failed: {e}"))?
}

/// One directory entry for the VS Code filesystem provider.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsDirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// List one directory for the VS Code filesystem provider. `.git` is elided —
/// nothing in the editor stack should ever walk into it.
#[tauri::command]
pub async fn ide_read_dir(dir: String, file_path: String) -> Result<Vec<FsDirEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let abs = confined(Path::new(&dir), &file_path)?;
        let entries =
            std::fs::read_dir(&abs).map_err(|e| format!("cannot read {file_path}: {e}"))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            out.push(FsDirEntry { name, is_dir });
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("readdir task failed: {e}"))?
}

/// Create a directory (and any missing parents) for "New Folder".
///
/// Refuses a name that is already taken rather than letting `create_dir_all`
/// report success for a directory it didn't create — the frontend turns
/// [`ERR_ALREADY_EXISTS`] into `FileExists`, and VS Code's `mkdirp` swallows
/// exactly that code when it races another creator, so being strict here is
/// free. `symlink_metadata` so a dangling symlink still counts as taken.
#[tauri::command]
pub async fn ide_create_dir(dir: String, file_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let abs = confined(Path::new(&dir), &file_path)?;
        if std::fs::symlink_metadata(&abs).is_ok() {
            return Err(already_exists(&file_path));
        }
        std::fs::create_dir_all(&abs).map_err(|e| format!("cannot create {file_path}: {e}"))
    })
    .await
    .map_err(|e| format!("mkdir task failed: {e}"))?
}

/// Delete a path for the Explorer. Defaults to the OS trash: a checkout is
/// full of untracked files (.env, scratch notes, build output) that git cannot
/// bring back, so a stray Delete must stay recoverable. `use_trash: false` is
/// the permanent path (VS Code's shift-delete).
#[tauri::command]
pub async fn ide_delete(
    dir: String,
    file_path: String,
    recursive: bool,
    use_trash: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let abs = confined(Path::new(&dir), &file_path)?;
        if use_trash {
            return trash::delete(&abs).map_err(|e| format!("cannot trash {file_path}: {e}"));
        }
        let meta =
            std::fs::symlink_metadata(&abs).map_err(|e| format!("cannot stat {file_path}: {e}"))?;
        if meta.is_dir() {
            if recursive { std::fs::remove_dir_all(&abs) } else { std::fs::remove_dir(&abs) }
        } else {
            std::fs::remove_file(&abs)
        }
        .map_err(|e| format!("cannot delete {file_path}: {e}"))
    })
    .await
    .map_err(|e| format!("delete task failed: {e}"))?
}

/// Move/rename within the folder. Both ends are confined, so a rename can
/// never be used to write outside the checkout.
#[tauri::command]
pub async fn ide_rename(
    dir: String,
    from_path: String,
    to_path: String,
    overwrite: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&dir);
        let from = confined(root, &from_path)?;
        let to = confined(root, &to_path)?;
        if to.exists() && !overwrite {
            return Err(already_exists(&to_path));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {to_path}: {e}"))?;
        }
        std::fs::rename(&from, &to)
            .map_err(|e| format!("cannot rename {from_path} to {to_path}: {e}"))
    })
    .await
    .map_err(|e| format!("rename task failed: {e}"))?
}

/// Every file in the folder's checkout (tracked + untracked-but-not-ignored),
/// for the diff pane's Files tab — so any file, changed or not, can be
/// @-mentioned to the folder's Claude session. Async: git subprocesses.
#[tauri::command]
pub async fn ide_list_files(dir: String) -> Vec<String> {
    const FILE_LIST_CAP: usize = 20_000;
    tauri::async_runtime::spawn_blocking(move || {
        tt_agentboard::git_info::list_files(&dir, FILE_LIST_CAP)
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod fs_command_tests {
    use super::*;

    #[test]
    fn confined_rejects_parent_traversal() {
        let err = confined(Path::new("/w"), "../etc/passwd").unwrap_err();
        assert!(err.contains(ERR_ESCAPES_FOLDER), "{err}");
    }

    #[test]
    fn confined_rejects_traversal_in_the_middle_of_a_path() {
        assert!(confined(Path::new("/w"), "src/../../etc/passwd").is_err());
    }

    /// `Path::join` throws the base away when the argument is absolute, so
    /// without this check `confined` hands back a path outside the folder and
    /// `ide_delete` would happily trash it.
    #[test]
    fn confined_rejects_an_absolute_path() {
        let err = confined(Path::new("/w"), "/etc/passwd").unwrap_err();
        assert!(err.contains(ERR_ESCAPES_FOLDER), "{err}");
        assert_ne!(
            confined(Path::new("/w"), "/etc/passwd").ok(),
            Some(PathBuf::from("/etc/passwd"))
        );
    }

    #[test]
    fn confined_joins_a_plain_relative_path() {
        assert_eq!(confined(Path::new("/w"), "src/main.rs").unwrap(), Path::new("/w/src/main.rs"));
    }

    /// The editor bridge picks a `FileSystemProviderErrorCode` by matching
    /// these substrings (`errorCodeFor` in `apps/client/src/lib/monaco-fs.ts`).
    /// Rewording them silently downgrades the code to `Unknown`, which changes
    /// what VS Code offers the user — so assert the produced messages contain
    /// what the frontend looks for.
    #[test]
    fn error_messages_carry_the_substrings_the_frontend_matches() {
        let escape = confined(Path::new("/w"), "../x").unwrap_err();
        assert!(escape.contains(ERR_ESCAPES_FOLDER), "{escape}");

        let exists = already_exists("dest.txt");
        assert!(exists.contains(ERR_ALREADY_EXISTS), "{exists}");
        assert!(exists.starts_with("dest.txt"), "{exists}");
    }
}
