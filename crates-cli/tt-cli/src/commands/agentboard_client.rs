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
    /// Something else is bound to the port and completed the `GET /`
    /// round-trip with a body that isn't ours — a definitively different
    /// server.
    Foreign,
    /// A connection was accepted but the probe couldn't complete (the write
    /// failed, or the read timed out before a full response arrived) — this
    /// does NOT mean a foreign server; a healthy server that is merely slow
    /// to answer (e.g. still starting up) looks the same. Treat like
    /// `Closed`: worth retrying, not worth failing on.
    Unknown,
    /// Nothing accepted the connection.
    Closed,
}

/// Identify the listener via `GET /`: the agentboard server answers with
/// `{"name":"agentboard server",...}`; anything else does not. This route is
/// new to the Rust server (not carried over from the TS launcher's simpler
/// port-open check), so a non-matching response is expected whenever some
/// other process — not necessarily a former agentboard listener — holds the
/// port.
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
            return Listener::Unknown;
        }
        let mut response = String::new();
        if std::io::Read::read_to_string(&mut BufReader::new(stream), &mut response).is_err() {
            return Listener::Unknown;
        }
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
        "{host}:{port} is in use by a different server. Stop it or set \
         TT_AGENTBOARD_PORT to a free port."
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
    // A port can be open without being ours — only accept a verified server.
    match probe_listener(&host, port, Duration::from_millis(200)) {
        Listener::Agentboard => return Ok(()),
        Listener::Foreign | Listener::Unknown => return Err(foreign_listener_error(&host, port)),
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
            // A completed, mismatched response means something else won the
            // bind race — our server will have exited.
            Listener::Foreign => return Err(foreign_listener_error(&host, port)),
            // Nothing listening yet, or a connection that didn't finish the
            // probe in time (our server may just be slow to start) — keep
            // polling instead of failing on a single transient hiccup.
            Listener::Closed | Listener::Unknown => {}
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
            let status: u16 =
                status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn spawn_listener_that(handle: impl FnOnce(TcpStream) + Send + 'static) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle(stream);
            }
        });
        port
    }

    #[test]
    fn probe_listener_matches_agentboard_response() {
        let port = spawn_listener_that(|mut stream| {
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let body = "{\"name\":\"agentboard server\"}";
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            let _ = stream.write_all(resp.as_bytes());
        });
        assert!(
            probe_listener("127.0.0.1", port, Duration::from_millis(500)) == Listener::Agentboard
        );
    }

    #[test]
    fn probe_listener_treats_completed_mismatch_as_foreign() {
        let port = spawn_listener_that(|mut stream| {
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let body = "not agentboard";
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            let _ = stream.write_all(resp.as_bytes());
        });
        assert!(probe_listener("127.0.0.1", port, Duration::from_millis(500)) == Listener::Foreign);
    }

    #[test]
    fn probe_listener_treats_read_timeout_as_unknown_not_foreign() {
        // Accept the connection but never respond — the probe's read must
        // time out. A slow-to-answer server must not be misclassified as a
        // definitively different (`Foreign`) one.
        let port = spawn_listener_that(|stream| {
            std::thread::sleep(Duration::from_secs(2));
            drop(stream);
        });
        assert!(probe_listener("127.0.0.1", port, Duration::from_millis(100)) == Listener::Unknown);
    }

    #[test]
    fn probe_listener_reports_closed_when_nothing_listens() {
        // Bind and immediately drop to get a port nothing is listening on.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(probe_listener("127.0.0.1", port, Duration::from_millis(200)) == Listener::Closed);
    }
}
