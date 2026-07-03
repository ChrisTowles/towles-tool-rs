//! Tauri bridge for agentboard (phases 3+4). The engine itself lives in
//! `tt_agentboard::engine` (shared with the tmux-mode server, phase T3); this
//! module owns the Tauri glue: the managed state, the `agentboard://state`
//! event, the `ab_*` commands, and the localhost metadata listener.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use tt_agentboard::engine::{apply_mutation, parse_tone};
use tt_agentboard::fs_notify::DirNotifier;
use tt_agentboard::metadata::{LogInput, ProgressInput, StatusInput};
use tt_agentboard::session_order::ReorderDelta;
use tt_agentboard::{StatePayload, handle_request, parse_request_head, response_bytes};

pub use tt_agentboard::engine::{Engine, ingest_addr, now_ms};

/// Tauri event carrying the state snapshot.
pub const STATE_EVENT: &str = "agentboard://state";

/// Managed Tauri state: the engine plus the task-signal handles.
pub struct Ab {
    pub engine: Arc<Mutex<Engine>>,
    /// Signals the debounced emitter to rebuild + emit.
    pub emit: Arc<Notify>,
    /// Signals the scan task to run an eager scan (fs-notify accelerant).
    pub scan: Arc<Notify>,
    /// Keeps the fs watcher alive.
    pub _notifier: Mutex<Option<DirNotifier>>,
}

// --- Tauri commands ---

/// Pull the current snapshot (initial mount).
#[tauri::command]
pub fn ab_get_state(state: State<Ab>) -> StatePayload {
    let mut engine = state.engine.lock().unwrap();
    engine.compute_payload(now_ms())
}

/// Clear unseen for a session (fast-path: patch + re-emit, no full rebuild).
#[tauri::command]
pub fn ab_mark_seen(state: State<Ab>, app: AppHandle, name: String) {
    let patched = {
        let mut engine = state.engine.lock().unwrap();
        engine.mark_seen_patch(&name)
    };
    if let Some(payload) = patched {
        let _ = app.emit(STATE_EVENT, payload);
    }
}

#[tauri::command]
pub fn ab_dismiss_agent(
    state: State<Ab>,
    session: String,
    agent: String,
    thread_id: Option<String>,
) {
    let changed = {
        let mut engine = state.engine.lock().unwrap();
        engine.dismiss(&session, &agent, thread_id.as_deref())
    };
    if changed {
        state.emit.notify_one();
    }
}

#[tauri::command]
pub fn ab_reorder_session(state: State<Ab>, name: String, delta: ReorderDelta) {
    state.engine.lock().unwrap().reorder(&name, delta);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_set_theme(state: State<Ab>, theme: String) {
    state.engine.lock().unwrap().set_theme(theme);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_add_repo(state: State<Ab>, path: String) {
    state.engine.lock().unwrap().add_repo(&path);
    state.scan.notify_one(); // discover the new repo's sessions
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_remove_repo(state: State<Ab>, name: String) {
    state.engine.lock().unwrap().remove_repo(&name);
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_refresh(state: State<Ab>) {
    state.emit.notify_one();
}

#[tauri::command]
pub fn ab_set_status(
    state: State<Ab>,
    session: String,
    text: Option<String>,
    tone: Option<String>,
) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    let input = text.map(|t| StatusInput { text: t, tone: parse_tone(tone) });
    state.engine.lock().unwrap().set_status(&session, input, now_ms());
    state.emit.notify_one();
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn ab_set_progress(
    state: State<Ab>,
    session: String,
    current: Option<i64>,
    total: Option<i64>,
    percent: Option<f64>,
    label: Option<String>,
    clear: Option<bool>,
) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    let input = if clear == Some(true) {
        None
    } else {
        Some(ProgressInput { current, total, percent, label })
    };
    state.engine.lock().unwrap().set_progress(&session, input, now_ms());
    state.emit.notify_one();
    Ok(())
}

#[tauri::command]
pub fn ab_log(
    state: State<Ab>,
    session: String,
    message: String,
    tone: Option<String>,
    source: Option<String>,
) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    if message.is_empty() {
        return Err("message is required".into());
    }
    let input = LogInput { message, tone: parse_tone(tone), source };
    state.engine.lock().unwrap().append_log(&session, input, now_ms());
    state.emit.notify_one();
    Ok(())
}

#[tauri::command]
pub fn ab_clear_log(state: State<Ab>, session: String) -> Result<(), String> {
    if session.trim().is_empty() {
        return Err("session is required".into());
    }
    state.engine.lock().unwrap().clear_logs(&session);
    state.emit.notify_one();
    Ok(())
}

/// Open a session's repo directory in the preferred editor. Ports the TS
/// open-in-editor (spawns `<preferredEditor> <dir>`; the TS TMUX-env stripping is
/// desktop-irrelevant and skipped).
#[tauri::command]
pub fn ab_open_in_editor(state: State<Ab>, name: String) -> Result<(), String> {
    let (editor, dir) = {
        let mut engine = state.engine.lock().unwrap();
        (engine.preferred_editor(), engine.repo_dir_for(&name))
    };
    let Some(dir) = dir else {
        return Err(format!("No repo named {name}"));
    };
    if editor.trim().is_empty() {
        return Err("No preferred editor configured".into());
    }
    std::process::Command::new(&editor)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("Failed to launch {editor}: {e}"))?;
    Ok(())
}

// --- Localhost metadata HTTP ingest (phase 5) ---

/// Run the localhost metadata listener. Binds `TT_AGENTBOARD_HOST:PORT`; if the
/// port is in use (e.g. the tmux-mode server owns it), logs a warning and
/// returns without a listener — the app must not crash. Ports the agent-facing
/// HTTP API (§5); request parsing/validation is pure in `tt-agentboard`.
pub async fn serve_metadata(
    engine: Arc<Mutex<Engine>>,
    emit: Arc<Notify>,
    host: String,
    port: u16,
) {
    let listener = match TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agentboard: metadata listener disabled on {host}:{port} ({e})");
            return;
        }
    };
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            continue;
        };
        let engine = engine.clone();
        let emit = emit.clone();
        tauri::async_runtime::spawn(async move {
            handle_connection(socket, engine, emit).await;
        });
    }
}

/// Read one HTTP/1.1 request (headers + Content-Length body), apply any mutation,
/// and write the response. Best-effort: gives up quietly on malformed input.
async fn handle_connection(mut socket: TcpStream, engine: Arc<Mutex<Engine>>, emit: Arc<Notify>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];

    // Read until the end of headers.
    let head_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        match socket.read(&mut tmp).await {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
        if buf.len() > 64 * 1024 {
            return; // oversized header — bail
        }
    };

    let head_str = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let Some(head) = parse_request_head(&head_str) else {
        let _ = socket.write_all(response_bytes(400, "bad request").as_bytes()).await;
        return;
    };

    // Read the body up to Content-Length.
    while buf.len() - head_end < head.content_length {
        match socket.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
        if buf.len() > 8 * 1024 * 1024 {
            break; // cap body size
        }
    }
    let body_end = (head_end + head.content_length).min(buf.len());
    let body = String::from_utf8_lossy(&buf[head_end..body_end]).to_string();

    let outcome = handle_request(&head.method, &head.path, &body);
    if let Some(mutation) = outcome.mutation {
        {
            let mut engine = engine.lock().unwrap();
            apply_mutation(&mut engine, mutation, now_ms());
        }
        emit.notify_one();
    }
    let _ = socket.write_all(response_bytes(outcome.status, &outcome.body).as_bytes()).await;
}

/// First index of `needle` in `haystack`, if present.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}
