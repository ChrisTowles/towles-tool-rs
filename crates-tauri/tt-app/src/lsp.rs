//! Language-server process host for the editor's LSP bridge (spike).
//!
//! The webview's `monaco-languageclient` speaks LSP over Tauri IPC instead of
//! a WebSocket: `lsp_start` spawns a server (rust-analyzer) for a workspace
//! dir, a reader thread reframes its stdout into `lsp://msg` events, and
//! `lsp_send` writes framed messages to its stdin. No ports involved — slot
//! port claims stay untouched.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
pub struct Lsp {
    next_id: AtomicU32,
    servers: Mutex<HashMap<u32, ServerHandle>>,
}

struct ServerHandle {
    child: Child,
    stdin: ChildStdin,
}

impl Drop for Lsp {
    fn drop(&mut self) {
        if let Ok(mut servers) = self.servers.lock() {
            for (_, mut server) in servers.drain() {
                let _ = server.child.kill();
            }
        }
    }
}

/// One LSP message leaving the server, relayed to the webview.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LspMsg {
    id: u32,
    message: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LspExit {
    id: u32,
}

/// Spawn a language server for `dir` and stream its stdout as `lsp://msg`
/// events (an `lsp://exit` follows EOF). Returns the server id used by
/// `lsp_send`/`lsp_stop`. rust-analyzer only for now — the binary comes from
/// PATH, never bundled.
#[tauri::command]
pub fn lsp_start(app: AppHandle, state: State<Lsp>, dir: String) -> Result<u32, String> {
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    tt_exec::record_detached_spawn("rust-analyzer", &[], "lsp");
    let mut child = Command::new("rust-analyzer")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot spawn rust-analyzer: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    state.servers.lock().unwrap().insert(id, ServerHandle { child, stdin });

    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Ok(Some(message)) = read_lsp_frame(&mut reader) {
            let _ = app.emit("lsp://msg", LspMsg { id, message });
        }
        // EOF: reap the child so a crashed/dead server never lingers as a
        // zombie, then tell the webview.
        {
            use tauri::Manager;
            let state = app.state::<Lsp>();
            if let Some(mut server) = state.servers.lock().unwrap().remove(&id) {
                let _ = server.child.kill();
                let _ = server.child.wait();
            }
        }
        let _ = app.emit("lsp://exit", LspExit { id });
    });
    Ok(id)
}

/// Write one client→server LSP message (unframed JSON; framing added here).
#[tauri::command]
pub fn lsp_send(state: State<Lsp>, id: u32, message: String) -> Result<(), String> {
    let mut servers = state.servers.lock().unwrap();
    let server = servers.get_mut(&id).ok_or("unknown lsp server")?;
    server
        .stdin
        .write_all(format!("Content-Length: {}\r\n\r\n{message}", message.len()).as_bytes())
        .and_then(|()| server.stdin.flush())
        .map_err(|e| format!("lsp write failed: {e}"))
}

/// Kill every server. The webview calls this once as its LSP module loads —
/// a page reload loses all client state, so whatever servers the previous
/// page started are orphans by definition.
#[tauri::command]
pub fn lsp_stop_all(state: State<Lsp>) {
    let mut servers = state.servers.lock().unwrap();
    for (_, mut server) in servers.drain() {
        let _ = server.child.kill();
        let _ = server.child.wait();
    }
}

/// Kill a server and forget it. Safe to call twice.
#[tauri::command]
pub fn lsp_stop(state: State<Lsp>, id: u32) {
    if let Some(mut server) = state.servers.lock().unwrap().remove(&id) {
        let _ = server.child.kill();
        let _ = server.child.wait();
    }
}

/// Read one `Content-Length`-framed LSP message. `Ok(None)` on clean EOF.
fn read_lsp_frame(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::read_lsp_frame;

    #[test]
    fn reads_a_framed_message() {
        let raw = b"Content-Length: 13\r\n\r\n{\"jsonrpc\":2}";
        let mut reader = std::io::BufReader::new(&raw[..]);
        assert_eq!(read_lsp_frame(&mut reader).unwrap().as_deref(), Some("{\"jsonrpc\":2}"));
        assert_eq!(read_lsp_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn tolerates_extra_headers() {
        let raw = b"Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        let mut reader = std::io::BufReader::new(&raw[..]);
        assert_eq!(read_lsp_frame(&mut reader).unwrap().as_deref(), Some("{}"));
    }
}
