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

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;

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
    /// Compiler-diagnostics hub, queried per message for this folder's
    /// current `getDiagnostics` payload.
    diagnostics: Arc<crate::diagnostics::DiagHub>,
    /// Outbound frame senders, one per connected CLI process (Claude Code is
    /// multi-process: the TUI and its session daemon may both connect).
    out: Mutex<Vec<UnboundedSender<Message>>>,
}

impl Shared {
    fn context(&self) -> tt_ide::ServerContext {
        tt_ide::ServerContext {
            ide_name: IDE_NAME.to_string(),
            workspace_folder: self.cwd.clone(),
            selection: self.selection.lock().unwrap().clone(),
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
                        if let Some(reply) = tt_ide::handle_message(text.as_str(), &shared.context())
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

/// Resolve `file_path` (repo-relative) against `dir`, read the selected lines
/// from disk (1-based inclusive), and build the wire selection (0-based). The
/// text comes from the real file — the diff pane may only show hunk excerpts.
fn build_selection(
    dir: &Path,
    file_path: &str,
    start_line: u32,
    end_line: u32,
) -> tt_ide::Selection {
    let abs = dir.join(file_path);
    let (start, end) = (start_line.min(end_line).max(1), start_line.max(end_line));
    let content = std::fs::read_to_string(&abs).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let from = (start as usize - 1).min(lines.len());
    let to = (end as usize).min(lines.len());
    let text = lines[from..to].join("\n");
    let end_character =
        lines.get(to.saturating_sub(1)).map(|l| l.chars().count() as u32).unwrap_or(0);
    tt_ide::Selection::range(&abs, start - 1, end - 1, end_character, text)
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
) -> Result<bool, String> {
    let dir = PathBuf::from(dir);
    let selection = build_selection(&dir, &file_path, start_line, end_line);
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
