//! Loopback HTTP transport for the MCP server ([`tt_mcp`]).
//!
//! [`tt_mcp`] is transport-free — it turns a JSON-RPC request string into a
//! response string and nothing else. This module is the other half: the socket,
//! the HTTP framing, and the request-admission rules. Same split as
//! [`crate::ide`] over [`tt_ide`].
//!
//! ## One server per machine
//!
//! The listener is taken **bind-or-skip**: whichever app instance binds the
//! port serves MCP, and every other instance silently serves none. The OS bind
//! *is* the mutex — there is no lockfile, no PID check, and nothing to clean up
//! after a crash. Chris runs several worktrees of this app at once, and a
//! machine-wide singleton is the point: a session anywhere on the machine
//! reaches one server holding one store, not whichever copy happened to start
//! last. A task that loses the race is unreachable by design; to debug one,
//! point a client at it by hand.
//!
//! This is also why the port is a fixed default from settings rather than a
//! `${tt:port}` pool claim. The no-hardcoded-ports rule exists because parallel
//! tasks collide over shared resources; here exactly one process ever holds the
//! port, so there is nothing to collide with — and a stable port is what lets
//! the `towles-tool-app` plugin ship a static, checked-in `.mcp.json`.
//!
//! ## Admission control is the only guard on writes
//!
//! There is no bearer token and no capability gate (both removed 2026-07-20 —
//! see [`tt_mcp`]'s module doc for why). [`check_admission`] is therefore the
//! entire security boundary, and it is a pure function precisely so it can be
//! tested directly rather than through a live socket.
//!
//! Binding to loopback keeps *remote hosts* out. It does not keep *web pages*
//! out: any site the user visits can POST to `127.0.0.1`, and although CORS
//! stops the page reading the reply, a blind write is the whole attack. The two
//! mitigations the MCP spec recommends for local HTTP servers close that:
//!
//! - **Reject any request carrying an `Origin` header.** Real MCP clients don't
//!   send one; a browser always does. This is the DNS-rebinding mitigation, and
//!   it rejects on *presence*, not on value — an allowlist would invite the
//!   mistake of trusting an attacker-controlled string.
//! - **Require `Content-Type: application/json`.** It is not a CORS-"simple"
//!   content type, so a page cannot send it without a preflight the browser
//!   will refuse. A page's only way through is `text/plain`, which this rejects.

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use tauri::AppHandle;
use tt_mcp::Dispatcher;
use tt_store::Store;

/// Whether *this* instance won the bind race and is serving MCP.
///
/// Process-global rather than managed Tauri state because it is written from
/// [`spawn`] during setup and read by a command; there is exactly one server
/// per process, so there is nothing to key it by.
static SERVING: AtomicBool = AtomicBool::new(false);
/// The port [`spawn`] attempted, whether or not the bind succeeded — the UI
/// needs it either way (to show the endpoint, or to say what's contended).
static PORT: AtomicU16 = AtomicU16::new(0);

/// Whether this instance is serving MCP, and on which port.
///
/// Reported to the app's MCP screen so the status it shows is the real bind
/// outcome rather than an inference from call recency — those differ exactly
/// when it matters: a healthy server nobody has called yet, and an instance
/// that lost the bind race and is serving nothing at all.
#[tauri::command]
pub fn mcp_status() -> serde_json::Value {
    serde_json::json!({
        "serving": SERVING.load(Ordering::Relaxed),
        "port": PORT.load(Ordering::Relaxed),
    })
}

/// The MCP endpoint path. A single route: this is not a REST API.
const MCP_PATH: &str = "/mcp";

/// Largest request body accepted. Enforced incrementally by `Limited` in
/// [`read_body`] — the body is never buffered past this, so a stray upload
/// can't balloon memory rather than merely being rejected after the fact. MCP
/// requests are small; `calendar_set` pushing a full day of events is the
/// biggest realistic payload and is far under this.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Why a request was refused before it ever reached the dispatcher.
///
/// Each maps to an HTTP status and a short body. Kept as a type rather than
/// inline strings so [`check_admission`]'s tests assert on the *reason*, not on
/// prose that might be reworded later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Request carried an `Origin` header — i.e. it came from a web page.
    BrowserOrigin,
    /// Missing or non-JSON `Content-Type`.
    NotJson,
    /// Wrong path.
    NotFound,
    /// Wrong method (only POST is served).
    MethodNotAllowed,
    /// Body exceeded [`MAX_BODY_BYTES`].
    TooLarge,
    /// The body could not be read to the end (e.g. the client hung up).
    /// Distinct from [`Refusal::TooLarge`] so the logs don't report a hangup as
    /// an oversized upload.
    Unreadable,
}

impl Refusal {
    /// HTTP status to answer with.
    pub fn status(self) -> u16 {
        match self {
            Refusal::BrowserOrigin => 403,
            Refusal::NotJson => 415,
            Refusal::NotFound => 404,
            Refusal::MethodNotAllowed => 405,
            Refusal::TooLarge => 413,
            Refusal::Unreadable => 400,
        }
    }

    /// Short human-readable reason, returned as the response body.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::BrowserOrigin => {
                "requests carrying an Origin header are refused (browser-originated)"
            }
            Refusal::NotJson => "Content-Type must be application/json",
            Refusal::NotFound => "not found",
            Refusal::MethodNotAllowed => "method not allowed",
            Refusal::TooLarge => "request body too large",
            Refusal::Unreadable => "could not read request body",
        }
    }
}

/// Decide whether a request may reach the dispatcher.
///
/// Pure and header-only on purpose: this is the whole security boundary (see
/// the module doc), so it is exercised directly by unit tests rather than only
/// through a live socket.
///
/// `origin_present` is deliberately a **bool, not the header's value**. The
/// rule is presence, so the type says presence — passing the parsed value
/// invites the fail-open this signature now makes unrepresentable: reading the
/// header with `.to_str().ok()` yields `None` for a present-but-non-UTF8
/// `Origin`, which would have been admitted as though the header were absent.
/// Callers must ask `headers().contains_key(ORIGIN)` and nothing else.
///
/// `content_type` is the raw `Content-Type` header, where the value *does*
/// matter.
pub fn check_admission(
    method: &str,
    path: &str,
    origin_present: bool,
    content_type: Option<&str>,
) -> Result<(), Refusal> {
    // Origin first: a browser-originated request is refused whatever else it
    // says, and answering it with a more specific error would leak which of the
    // other checks it passed.
    if origin_present {
        return Err(Refusal::BrowserOrigin);
    }
    if path != MCP_PATH {
        return Err(Refusal::NotFound);
    }
    if !method.eq_ignore_ascii_case("POST") {
        return Err(Refusal::MethodNotAllowed);
    }
    if !is_json_content_type(content_type) {
        return Err(Refusal::NotJson);
    }
    Ok(())
}

/// Whether a `Content-Type` header names JSON.
///
/// Tolerates parameters (`application/json; charset=utf-8`) and case, since
/// those are legitimate and a real client may send them — but nothing else.
/// Notably `text/plain` is rejected, which is the point: it is the only content
/// type a web page can send without triggering a preflight.
fn is_json_content_type(value: Option<&str>) -> bool {
    let Some(value) = value else { return false };
    let essence = value.split(';').next().unwrap_or("").trim();
    essence.eq_ignore_ascii_case("application/json")
}

/// Make a real HTTP request to this machine's MCP endpoint, for the app's
/// "test this tool" affordance.
///
/// **Why this lives in Rust rather than a `fetch` in the webview:** the webview
/// is a browser context, so any `fetch` it makes to `127.0.0.1` carries an
/// `Origin` header — and [`check_admission`] rejects exactly that. The app
/// genuinely cannot call its own MCP endpoint from the frontend, which is the
/// defense working as designed, not a bug to route around. Issuing the request
/// from here (no `Origin`, like a real MCP client) is the only way to exercise
/// the true path end to end: socket, admission checks, dispatcher, and back.
///
/// `simulate_browser_origin` deliberately attaches an `Origin` header so the UI
/// can *demonstrate* the rejection rather than merely claim it. That makes the
/// security boundary something the user can watch work.
///
/// Returns the HTTP status and raw body either way — a refusal is a result to
/// display, not an error, so the caller sees precisely what a real client would.
#[tauri::command]
pub async fn mcp_test_call(
    body: String,
    simulate_browser_origin: bool,
) -> Result<serde_json::Value, String> {
    use http_body_util::BodyExt;
    use hyper::Request;
    use hyper_util::rt::TokioIo;

    // Refuse unless *this* instance is the one serving. `PORT` is set before the
    // bind is attempted, so on an instance that lost the race it still names a
    // live socket — belonging to a different app instance, whose `Store` is a
    // different checkout's `tt.db`. Dialing it anyway would run a write tool
    // against a board this window will never display, under a dialog that says
    // the call succeeded.
    if !SERVING.load(Ordering::Relaxed) {
        return Err("this instance is not serving MCP (another instance holds the port), so \
                    there is nothing here to test"
            .to_string());
    }
    let port = PORT.load(Ordering::Relaxed);
    if port == 0 {
        return Err("MCP port is not configured".to_string());
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let started = std::time::Instant::now();

    let stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| format!("could not reach {addr}: {e}"))?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("HTTP handshake failed: {e}"))?;
    // The connection future must be driven for the request to make progress.
    tauri::async_runtime::spawn(async move {
        let _ = conn.await;
    });

    let mut request = Request::builder()
        .method("POST")
        .uri(format!("http://127.0.0.1:{port}{MCP_PATH}"))
        .header(hyper::header::CONTENT_TYPE, "application/json");
    if simulate_browser_origin {
        request = request.header(hyper::header::ORIGIN, "https://example.invalid");
    }
    let request = request.body(body).map_err(|e| format!("could not build request: {e}"))?;

    let response =
        sender.send_request(request).await.map_err(|e| format!("request failed: {e}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("could not read response: {e}"))?
        .to_bytes();

    let duration_ms = started.elapsed().as_millis() as u64;
    tracing::info!(status, duration_ms, sent_origin = simulate_browser_origin, "mcp.test_call");

    Ok(serde_json::json!({
        "status": status,
        "body": String::from_utf8_lossy(&bytes),
        "durationMs": duration_ms,
        "sentOrigin": simulate_browser_origin,
    }))
}

/// Bind the MCP port and serve until the app exits, or do nothing if another
/// instance already holds it.
///
/// Never returns an error to the caller: failing to serve MCP must not stop the
/// app from starting, and losing the bind race is the *expected* outcome for
/// every instance but one.
pub fn spawn(app: AppHandle, port: u16) {
    PORT.store(port, Ordering::Relaxed);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = match StdTcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(error) => {
            // Not a warning: with several tasks open this is the normal path
            // for all but the first, and logging it as a problem would train
            // the reader to ignore a message that sometimes matters.
            tracing::info!(
                %addr,
                %error,
                "mcp.http: port already held; this instance serves no MCP"
            );
            return;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        tracing::warn!(%error, "mcp.http: could not set non-blocking; not serving");
        return;
    }
    // Re-read the port from the socket rather than trusting the requested one.
    // `port: 0` is a legal `u16` that settings accepts, and binding it succeeds
    // on an OS-assigned ephemeral port — so without this the UI would advertise
    // `127.0.0.1:0` as the endpoint to paste into `.mcp.json`, and
    // `mcp_test_call`'s `port == 0` sentinel would refuse to reach a server that
    // is in fact listening.
    if let Ok(bound) = listener.local_addr() {
        PORT.store(bound.port(), Ordering::Relaxed);
    }

    // The dispatcher owns its own SQLite connection rather than sharing the
    // app's `StoreState` mutex: MCP calls and UI reads then never block each
    // other, and SQLite handles the concurrent connections. The cost is that a
    // write here doesn't automatically refresh the UI, which is why a mutating
    // call re-emits the snapshot through the app's own state below.
    let store = match Store::open_default() {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(%error, "mcp.http: store unavailable; not serving");
            return;
        }
    };
    // Age out stale calendar rows once at startup. Retention otherwise only runs
    // as a side effect of a calendar *write*, and the push model this server
    // exists to serve has no guaranteed writer: the pull collector is off by
    // default, so a machine whose events all arrive over `calendar_set` sweeps
    // nothing at all while nobody is pushing. Without this, an app reopened
    // after a quiet week counts down to a meeting from the last day anything
    // was written.
    let _ = store.sweep_old_events(crate::store::now_ms());

    let dispatcher = Arc::new(Mutex::new(
        Dispatcher::new(store).with_task_host(Box::new(AppTaskHost { app: app.clone() })),
    ));

    SERVING.store(true, Ordering::Relaxed);
    tracing::info!(%addr, "mcp.http: serving");
    tauri::async_runtime::spawn(async move {
        accept_loop(app, listener, dispatcher).await;
    });
}

/// Lets the `task_delete` MCP tool reach the same delete path the app's own UI
/// uses — see [`tt_mcp::TaskHost`] for why the tool can't do this itself.
///
/// Runs on the connection's `spawn_blocking` thread (the dispatcher call
/// already is one), which is right for work that is entirely git subprocesses.
/// It does hold the dispatcher's mutex for the duration, so a slow delete
/// serializes other MCP calls behind it — acceptable for a single-user local
/// server, and the alternative (releasing the lock mid-call) would let a
/// concurrent tool observe the store halfway through a delete.
struct AppTaskHost {
    app: AppHandle,
}

impl tt_mcp::TaskHost for AppTaskHost {
    fn delete_task(
        &self,
        id: i64,
        force: bool,
        outcome: Option<tt_store::TaskOutcome>,
    ) -> Result<tt_mcp::TaskDeletion, String> {
        use crate::task::{DeleteTarget, TaskDeleteOutcome};

        match crate::task::delete_task_blocking(
            &self.app,
            DeleteTarget::Board(id),
            force,
            outcome,
            false,
        )? {
            TaskDeleteOutcome::Deleted { name, messages } => {
                Ok(tt_mcp::TaskDeletion::Deleted { name, messages })
            }
            TaskDeleteOutcome::Blocked { name, blockers, messages } => {
                // Serialize through the same `Blocker` the frontend dialog
                // renders, so an agent reading the refusal and a human reading
                // the dialog are looking at the same fields. One conversion for
                // the whole list, so a serialization failure is one error here
                // rather than a `null` silently taking a blocker's place in the
                // refusal an agent acts on.
                let blockers = serde_json::to_value(&blockers)
                    .ok()
                    .and_then(|value| value.as_array().cloned())
                    .ok_or_else(|| format!("could not encode blockers for task {id}"))?;
                Ok(tt_mcp::TaskDeletion::Refused { name, blockers, messages })
            }
        }
    }
}

/// Accept connections until the task is aborted. One failed connection recycles
/// the loop rather than taking the server down.
///
/// That distinction is load-bearing here in a way it isn't for a typical server:
/// this process holds the port for the whole machine, so returning from this
/// loop takes MCP down for *every* Claude Code session with no other instance
/// able to take over — while the socket stays bound, so nothing can even notice.
/// `accept` fails for reasons that are per-connection and transient (the peer
/// RSTs between SYN and accept; the process is momentarily out of file
/// descriptors, plausible with a PTY per terminal plus `gh`/`git`/`claude`
/// subprocesses), so those are logged and retried. `SERVING` is cleared on the
/// paths that really do give up, so `mcp_status` stops claiming to serve.
async fn accept_loop(app: AppHandle, listener: StdTcpListener, dispatcher: Arc<Mutex<Dispatcher>>) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        tracing::warn!("mcp.http: listener could not join the runtime; not serving");
        SERVING.store(false, Ordering::Relaxed);
        return;
    };
    // Consecutive failures, reset by any success. A listener that is genuinely
    // broken (its fd closed under us) would otherwise spin this loop hot.
    let mut consecutive_errors = 0u32;
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(accepted) => {
                consecutive_errors = 0;
                accepted
            }
            Err(error) => {
                consecutive_errors += 1;
                tracing::warn!(%error, consecutive_errors, "mcp.http: accept failed");
                if consecutive_errors >= 64 {
                    tracing::error!("mcp.http: accept failing persistently; stopping");
                    SERVING.store(false, Ordering::Relaxed);
                    return;
                }
                // Yield before retrying so a hard-failing accept can't starve
                // the runtime.
                tokio::task::yield_now().await;
                continue;
            }
        };
        let app = app.clone();
        let dispatcher = dispatcher.clone();
        tauri::async_runtime::spawn(async move {
            serve_connection(app, stream, dispatcher).await;
        });
    }
}

/// Serve one connection: parse requests, admit or refuse them, and hand the
/// admitted bodies to the dispatcher.
async fn serve_connection(
    app: AppHandle,
    stream: tokio::net::TcpStream,
    dispatcher: Arc<Mutex<Dispatcher>>,
) {
    use hyper::service::service_fn;
    use hyper::{Request, StatusCode};
    use hyper_util::rt::TokioIo;

    let io = TokioIo::new(stream);
    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
        let app = app.clone();
        let dispatcher = dispatcher.clone();
        async move {
            let method = req.method().as_str().to_string();
            let path = req.uri().path().to_string();
            // Presence only — a header we can't decode is still a header, and
            // treating it as absent would admit the request.
            let origin_present = req.headers().contains_key(hyper::header::ORIGIN);
            let content_type = req
                .headers()
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);

            if let Err(refusal) =
                check_admission(&method, &path, origin_present, content_type.as_deref())
            {
                tracing::warn!(
                    %method, %path, refusal = ?refusal,
                    "mcp.http: request refused"
                );
                return Ok::<_, std::convert::Infallible>(text_response(
                    refusal.status(),
                    refusal.message(),
                ));
            }

            let body = match read_body(req.into_body()).await {
                Ok(body) => body,
                Err(refusal) => {
                    return Ok(text_response(refusal.status(), refusal.message()));
                }
            };

            // The dispatcher is blocking (SQLite) and its guard must not be
            // held across an await, so the whole call runs on a blocking thread.
            let reply = tokio::task::spawn_blocking(move || {
                let mut dispatcher = match dispatcher.lock() {
                    Ok(guard) => guard,
                    // A panic in a previous call poisoned the lock. Recover the
                    // guard rather than propagating: the dispatcher holds no
                    // invariant that a panic could have half-broken (every tool
                    // is a self-contained store call), and taking the server
                    // down for the rest of the session is the worse failure.
                    Err(poisoned) => poisoned.into_inner(),
                };
                dispatcher.dispatch(&body)
            })
            .await;

            match reply {
                // A notification: no response body. 202 is what MCP's
                // streamable-HTTP transport specifies for this case.
                Ok(handled) if handled.response.is_none() => {
                    Ok(status_response(StatusCode::ACCEPTED, String::new()))
                }
                Ok(handled) => {
                    // Refresh the UI only for a call that actually wrote. The
                    // dispatcher writes through its own connection, so the app
                    // wouldn't otherwise notice — but `emit_snapshot_from_app`
                    // rebuilds the *entire* snapshot and takes `StoreState`'s
                    // lock, which is the lock this transport opened a second
                    // connection to stay off. Doing that per read would hand
                    // back the contention the separate connection bought, and a
                    // session's opening `initialize` + `tools/list` alone would
                    // pay for two full rebuilds that changed nothing.
                    //
                    // Detached onto the blocking pool, and deliberately not
                    // awaited: the rebuild is blocking SQLite work behind a
                    // `std::sync::Mutex` that sync Tauri commands also hold, so
                    // running it inline would park a tokio worker for the whole
                    // contended hold — the very thing the `spawn_blocking` above
                    // exists to avoid — and would make the caller wait on a UI
                    // refresh it has no stake in.
                    if handled.wrote {
                        let app = app.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            crate::store::emit_snapshot_from_app(&app);
                        });
                    }
                    Ok(json_response(handled.response.unwrap_or_default()))
                }
                Err(error) => {
                    tracing::error!(%error, "mcp.http: dispatch task failed");
                    Ok(text_response(500, "internal error"))
                }
            }
        }
    });

    // `.timer(...)` is not optional decoration: hyper's default 30s
    // header-read timeout is silently dropped (with only an internal warning)
    // when no timer is installed, and without it a peer that opens a socket and
    // never finishes its request headers holds a task and an fd forever. On a
    // machine-wide singleton that is enough to take MCP down for every session
    // on the box, so the timeout is what bounds a half-open connection.
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(30));
    if let Err(error) = builder.serve_connection(io, service).await {
        // Client hangups are routine; log at debug so the event log keeps the
        // detail without the terminal turning into noise.
        tracing::debug!(%error, "mcp.http: connection ended");
    }
}

/// Read the whole request body, refusing anything past [`MAX_BODY_BYTES`]
/// **without buffering it first**.
///
/// The cap is enforced by `Limited`, which stops reading once the budget is
/// exhausted, rather than by checking the length after `collect()` — collecting
/// first would materialize an oversized upload in full and only then reject it,
/// which is the opposite of what the cap is for.
///
/// A read that fails for any other reason (a client hanging up mid-body) is
/// reported as [`Refusal::Unreadable`], not as "too large" — mislabelling a
/// hangup 413 would make the refusal logs actively misleading.
async fn read_body(body: hyper::body::Incoming) -> Result<String, Refusal> {
    use http_body_util::{BodyExt, Limited};

    let limited = Limited::new(body, MAX_BODY_BYTES);
    match limited.collect().await {
        Ok(collected) => Ok(String::from_utf8_lossy(&collected.to_bytes()).into_owned()),
        // `Limited` surfaces the overflow as its own boxed error; anything else
        // is a genuine transport failure.
        Err(error) if error.is::<http_body_util::LengthLimitError>() => Err(Refusal::TooLarge),
        Err(_) => Err(Refusal::Unreadable),
    }
}

/// A response with an explicit status, built without a fallible step.
///
/// `Response::builder()` defers every error to `.body()`, and the natural
/// `.unwrap_or_else(|_| Response::new(body))` fallback yields a **200 OK** —
/// which would quietly turn a security refusal into an acceptance. Setting the
/// parts on an already-constructed response has no failure mode, so there is no
/// error left to mishandle.
fn status_response(status: hyper::StatusCode, body: String) -> hyper::Response<String> {
    let mut response = hyper::Response::new(body);
    *response.status_mut() = status;
    response
}

fn json_response(body: String) -> hyper::Response<String> {
    let mut response = status_response(hyper::StatusCode::OK, body);
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

fn text_response(status: u16, message: &str) -> hyper::Response<String> {
    // Fails *closed* if a status ever fails to convert: 500 says something went
    // wrong, where the builder's default 200 would have said "admitted".
    let status =
        hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = status_response(status, message.to_string());
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests are the security boundary's only direct coverage. With the
    // capability gate gone, nothing else stands between a web page and a write.

    #[test]
    fn a_normal_mcp_client_request_is_admitted() {
        assert_eq!(check_admission("POST", "/mcp", false, Some("application/json")), Ok(()));
    }

    #[test]
    fn content_type_parameters_and_casing_are_tolerated() {
        for value in [
            "application/json; charset=utf-8",
            "Application/JSON",
            "application/json ",
            "application/json;charset=UTF-8",
        ] {
            assert_eq!(
                check_admission("POST", "/mcp", false, Some(value)),
                Ok(()),
                "should admit {value}"
            );
        }
    }

    /// The DNS-rebinding mitigation: refuse on the header's *presence*. A real
    /// MCP client never sends one, so there is nothing to allowlist — and an
    /// allowlist would mean trusting an attacker-controlled string.
    /// The DNS-rebinding mitigation refuses on the header's *presence*, so
    /// there is nothing to allowlist — and an allowlist would mean trusting an
    /// attacker-controlled string.
    ///
    /// The signature takes a bool rather than the header value precisely so the
    /// fail-open this used to have is unrepresentable: reading the header with
    /// `.to_str().ok()` yields `None` for a present-but-non-UTF8 `Origin`, and
    /// passing that through would have admitted the request as though no header
    /// were sent. Presence is the rule, so presence is the parameter.
    #[test]
    fn a_present_origin_header_is_refused_whatever_its_value() {
        assert_eq!(
            check_admission("POST", "/mcp", true, Some("application/json")),
            Err(Refusal::BrowserOrigin)
        );
        // …and an absent one is the only way through.
        assert_eq!(check_admission("POST", "/mcp", false, Some("application/json")), Ok(()));
    }

    /// `text/plain` is the one content type a page can POST without a preflight,
    /// so rejecting it is what forces a browser into a preflight it will fail.
    #[test]
    fn non_json_content_types_are_refused() {
        for value in [
            "text/plain",
            "text/plain;charset=UTF-8",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "application/json-patch+json",
        ] {
            assert_eq!(
                check_admission("POST", "/mcp", false, Some(value)),
                Err(Refusal::NotJson),
                "should refuse {value}"
            );
        }
    }

    #[test]
    fn a_missing_content_type_is_refused() {
        assert_eq!(check_admission("POST", "/mcp", false, None), Err(Refusal::NotJson));
    }

    /// Both browser defenses have to hold together: a page that omits
    /// `Content-Type` still gets caught, and one that sets it still gets caught
    /// by Origin. Neither check is load-bearing alone.
    #[test]
    fn a_browser_request_is_refused_by_whichever_check_it_trips() {
        // Simple request: no preflight, but text/plain.
        assert_eq!(
            check_admission("POST", "/mcp", true, Some("text/plain")),
            Err(Refusal::BrowserOrigin)
        );
        // Even if some future client somehow omitted Origin, the content type
        // a no-preflight page can send is still refused.
        assert_eq!(
            check_admission("POST", "/mcp", false, Some("text/plain")),
            Err(Refusal::NotJson)
        );
    }

    #[test]
    fn only_post_to_the_mcp_path_is_served() {
        assert_eq!(
            check_admission("GET", "/mcp", false, Some("application/json")),
            Err(Refusal::MethodNotAllowed)
        );
        assert_eq!(
            check_admission("POST", "/", false, Some("application/json")),
            Err(Refusal::NotFound)
        );
        assert_eq!(
            check_admission("POST", "/mcp/extra", false, Some("application/json")),
            Err(Refusal::NotFound)
        );
        // Method casing is not meaningful in HTTP routing here.
        assert_eq!(check_admission("post", "/mcp", false, Some("application/json")), Ok(()));
    }

    /// An OPTIONS preflight must not be answered with anything permissive —
    /// there are no CORS headers on any response, so a browser preflight fails
    /// closed. Pinned because "helpfully" adding CORS headers later would
    /// silently undo the whole defense.
    #[test]
    fn preflight_is_not_specially_accommodated() {
        assert_eq!(check_admission("OPTIONS", "/mcp", true, None), Err(Refusal::BrowserOrigin));
        assert_eq!(check_admission("OPTIONS", "/mcp", false, None), Err(Refusal::MethodNotAllowed));
    }

    #[test]
    fn refusal_statuses_are_distinct_and_sane() {
        assert_eq!(Refusal::BrowserOrigin.status(), 403);
        assert_eq!(Refusal::NotJson.status(), 415);
        assert_eq!(Refusal::NotFound.status(), 404);
        assert_eq!(Refusal::MethodNotAllowed.status(), 405);
        assert_eq!(Refusal::TooLarge.status(), 413);
        assert_eq!(Refusal::Unreadable.status(), 400);
    }
}
