//! Client-side plumbing for talking to the agentboard server: the launcher
//! (`ensure_server`, ports slot-1 `runtime/server/launcher.ts`), a minimal
//! blocking HTTP POST, and the SSE subscription used by the TUI. All std —
//! the TUI event loop is synchronous and the launcher runs before any runtime
//! exists.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use tt_agentboard::engine::ingest_addr;
use tt_agentboard::types::ServerMessage;

use crate::commands::agentboard_server::PID_FILE;

const SERVER_ERR_LOG: &str = "/tmp/agentboard-server-err.log";

fn is_process_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// What is actually listening on the configured address.
#[derive(PartialEq)]
enum Listener {
    /// The agentboard server (identified by its `GET /` name field).
    Agentboard,
    /// Something else — e.g. the desktop app's metadata listener, which shares
    /// the default port but speaks a different protocol.
    Foreign,
    /// Nothing accepted the connection.
    Closed,
}

/// Identify the listener via `GET /`: the agentboard server answers with
/// `{"name":"agentboard server",...}`; the metadata listener (and anything
/// else) does not.
fn probe_listener(host: &str, port: u16, timeout: Duration) -> Listener {
    let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(host, port)) else {
        return Listener::Closed;
    };
    for addr in addrs {
        let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
            continue;
        };
        stream.set_read_timeout(Some(timeout)).ok();
        let req = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        if stream.write_all(req.as_bytes()).is_err() {
            return Listener::Foreign;
        }
        let mut response = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stream), &mut response);
        return if response.contains("\"name\":\"agentboard server\"") {
            Listener::Agentboard
        } else {
            Listener::Foreign
        };
    }
    Listener::Closed
}

fn foreign_listener_error(host: &str, port: u16) -> String {
    format!(
        "{host}:{port} is in use by a different server (likely the desktop app's \
         metadata listener). Stop it or set TT_AGENTBOARD_PORT to a free port."
    )
}

/// Make sure an agentboard server is running on the configured address,
/// spawning `current_exe() agentboard server` detached if needed. Returns an
/// error message on failure.
pub fn ensure_server() -> Result<(), String> {
    let (host, port) = ingest_addr();

    if let Ok(content) = std::fs::read_to_string(PID_FILE) {
        if let Ok(pid) = content.trim().parse::<i32>()
            && is_process_alive(pid)
            && probe_listener(&host, port, Duration::from_millis(200)) == Listener::Agentboard
        {
            return Ok(());
        }
        // Stale PID file — remove before spawning a new server.
        let _ = std::fs::remove_file(PID_FILE);
    }
    // A port can be open without being ours (e.g. the desktop app's metadata
    // listener shares the default 4201) — only accept a verified server.
    match probe_listener(&host, port, Duration::from_millis(200)) {
        Listener::Agentboard => return Ok(()), // e.g. the TS server — use it.
        Listener::Foreign => return Err(foreign_listener_error(&host, port)),
        Listener::Closed => {}
    }

    let exe = std::env::current_exe().map_err(|e| format!("cannot resolve own binary: {e}"))?;
    let err_log = std::fs::File::create(SERVER_ERR_LOG)
        .map_err(|e| format!("cannot open {SERVER_ERR_LOG}: {e}"))?;
    std::process::Command::new(&exe)
        .args(["agentboard", "server"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(err_log)
        .spawn()
        .map_err(|e| format!("failed to spawn agentboard server: {e}"))?;

    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(50));
        match probe_listener(&host, port, Duration::from_millis(100)) {
            Listener::Agentboard => return Ok(()),
            // Something else won the bind race — our server will have exited.
            Listener::Foreign => return Err(foreign_listener_error(&host, port)),
            Listener::Closed => {}
        }
    }

    let log = std::fs::read_to_string(SERVER_ERR_LOG).unwrap_or_default();
    let detail = if log.trim().is_empty() {
        format!("No error output. Check {SERVER_ERR_LOG}")
    } else {
        log.trim().to_string()
    };
    Err(format!("agentboard server failed to start:\n{detail}"))
}

/// Blocking HTTP/1.1 POST; returns the status code.
pub fn http_post(path: &str, body: &str) -> Result<u16, String> {
    let (host, port) = ingest_addr();
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(&mut stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(status)
}

/// Subscribe to `GET /events` and forward each `data:` frame's parsed
/// [`ServerMessage`] into `tx`. Blocks until the connection drops; run it on
/// a dedicated thread. Returns Err if the connection could not be opened.
pub fn sse_subscribe(tx: Sender<ServerMessage>) -> Result<(), String> {
    let (host, port) = ingest_addr();
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
    let req = format!("GET /events HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\n\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);

    // Check the status line: a non-200 means whatever is on this port is not
    // the agentboard server's SSE endpoint (e.g. the metadata listener's 404).
    let mut status_line = String::new();
    match reader.read_line(&mut status_line) {
        Ok(0) => return Err("server closed the SSE stream during headers".into()),
        Ok(_) => {
            let status: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if status != 200 {
                return Err(format!(
                    "GET /events returned HTTP {status} — {host}:{port} is not an \
                     agentboard server"
                ));
            }
        }
        Err(e) => return Err(e.to_string()),
    }

    // Skip the remaining response headers.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("server closed the SSE stream during headers".into()),
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }

    // Frames: `data: <json>\n\n`.
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()), // connection closed
            Ok(_) => {
                let trimmed = line.trim_end();
                if let Some(json) = trimmed.strip_prefix("data: ")
                    && let Ok(msg) = serde_json::from_str::<ServerMessage>(json)
                    && tx.send(msg).is_err()
                {
                    return Ok(()); // receiver hung up — TUI is exiting
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                {
                    continue;
                }
                return Ok(());
            }
        }
    }
}
