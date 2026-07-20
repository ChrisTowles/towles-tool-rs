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
//! after a crash. Chris runs several worktree slots of this app at once, and a
//! machine-wide singleton is the point: a session anywhere on the machine
//! reaches one server holding one store, not whichever copy happened to start
//! last. A slot that loses the race is unreachable by design; to debug one,
//! point a client at it by hand.
//!
//! This is also why the port is a fixed default from settings rather than a
//! `${tt:port}` pool claim. The no-hardcoded-ports rule exists because parallel
//! slots collide over shared resources; here exactly one process ever holds the
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

/// Largest request body accepted, so a stray upload can't balloon memory. MCP
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
        }
    }
}

/// Decide whether a request may reach the dispatcher.
///
/// Pure and header-only on purpose: this is the whole security boundary (see
/// the module doc), so it is exercised directly by unit tests rather than only
/// through a live socket.
///
/// `origin` is the raw `Origin` header if the request carried one *at all*;
/// `Some("")` still counts as carrying it. `content_type` is the raw
/// `Content-Type` header.
pub fn check_admission(
    method: &str,
    path: &str,
    origin: Option<&str>,
    content_type: Option<&str>,
) -> Result<(), Refusal> {
    // Origin first: a browser-originated request is refused whatever else it
    // says, and answering it with a more specific error would leak which of the
    // other checks it passed.
    if origin.is_some() {
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

    Ok(serde_json::json!({
        "status": status,
        "body": String::from_utf8_lossy(&bytes),
        "durationMs": started.elapsed().as_millis() as u64,
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
            // Not a warning: with several slots open this is the normal path
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
    let dispatcher = Arc::new(Mutex::new(Dispatcher::new(store)));

    SERVING.store(true, Ordering::Relaxed);
    tracing::info!(%addr, "mcp.http: serving");
    tauri::async_runtime::spawn(async move {
        accept_loop(app, listener, dispatcher).await;
    });
}

/// Accept connections until the task is aborted. One failed connection recycles
/// the loop rather than taking the server down.
async fn accept_loop(app: AppHandle, listener: StdTcpListener, dispatcher: Arc<Mutex<Dispatcher>>) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        tracing::warn!("mcp.http: listener could not join the runtime; not serving");
        return;
    };
    loop {
        let Ok((stream, _peer)) = listener.accept().await else {
            return;
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
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;

    let io = TokioIo::new(stream);
    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
        let app = app.clone();
        let dispatcher = dispatcher.clone();
        async move {
            let method = req.method().as_str().to_string();
            let path = req.uri().path().to_string();
            let origin = req
                .headers()
                .get(hyper::header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let content_type = req
                .headers()
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);

            if let Err(refusal) =
                check_admission(&method, &path, origin.as_deref(), content_type.as_deref())
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
                dispatcher.handle(&body)
            })
            .await;

            match reply {
                // A notification: no response body. 202 is what MCP's
                // streamable-HTTP transport specifies for this case.
                Ok(None) => Ok(Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(String::new())
                    .unwrap_or_else(|_| Response::new(String::new()))),
                Ok(Some(reply)) => {
                    // Any successful call may have mutated the store, and the
                    // dispatcher writes through its own connection, so the UI
                    // would otherwise not notice until its next poll.
                    crate::store::emit_snapshot_from_app(&app);
                    Ok(json_response(reply))
                }
                Err(error) => {
                    tracing::error!(%error, "mcp.http: dispatch task failed");
                    Ok(text_response(500, "internal error"))
                }
            }
        }
    });

    if let Err(error) =
        hyper::server::conn::http1::Builder::new().serve_connection(io, service).await
    {
        // Client hangups are routine; log at debug so the event log keeps the
        // detail without the terminal turning into noise.
        tracing::debug!(%error, "mcp.http: connection ended");
    }
}

/// Read the whole request body, refusing anything past [`MAX_BODY_BYTES`].
async fn read_body(body: hyper::body::Incoming) -> Result<String, Refusal> {
    use http_body_util::BodyExt;

    let collected = body.collect().await.map_err(|_| Refusal::TooLarge)?.to_bytes();
    if collected.len() > MAX_BODY_BYTES {
        return Err(Refusal::TooLarge);
    }
    Ok(String::from_utf8_lossy(&collected).into_owned())
}

fn json_response(body: String) -> hyper::Response<String> {
    hyper::Response::builder()
        .status(200)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap_or_else(|_| hyper::Response::new(String::new()))
}

fn text_response(status: u16, message: &str) -> hyper::Response<String> {
    hyper::Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.to_string())
        .unwrap_or_else(|_| hyper::Response::new(String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests are the security boundary's only direct coverage. With the
    // capability gate gone, nothing else stands between a web page and a write.

    #[test]
    fn a_normal_mcp_client_request_is_admitted() {
        assert_eq!(check_admission("POST", "/mcp", None, Some("application/json")), Ok(()));
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
                check_admission("POST", "/mcp", None, Some(value)),
                Ok(()),
                "should admit {value}"
            );
        }
    }

    /// The DNS-rebinding mitigation: refuse on the header's *presence*. A real
    /// MCP client never sends one, so there is nothing to allowlist — and an
    /// allowlist would mean trusting an attacker-controlled string.
    #[test]
    fn any_origin_header_is_refused() {
        for origin in ["https://evil.example", "null", "http://localhost:5173", ""] {
            assert_eq!(
                check_admission("POST", "/mcp", Some(origin), Some("application/json")),
                Err(Refusal::BrowserOrigin),
                "should refuse Origin: {origin:?}"
            );
        }
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
                check_admission("POST", "/mcp", None, Some(value)),
                Err(Refusal::NotJson),
                "should refuse {value}"
            );
        }
    }

    #[test]
    fn a_missing_content_type_is_refused() {
        assert_eq!(check_admission("POST", "/mcp", None, None), Err(Refusal::NotJson));
    }

    /// Both browser defenses have to hold together: a page that omits
    /// `Content-Type` still gets caught, and one that sets it still gets caught
    /// by Origin. Neither check is load-bearing alone.
    #[test]
    fn a_browser_request_is_refused_by_whichever_check_it_trips() {
        // Simple request: no preflight, but text/plain.
        assert_eq!(
            check_admission("POST", "/mcp", Some("https://evil.example"), Some("text/plain")),
            Err(Refusal::BrowserOrigin)
        );
        // Even if some future client somehow omitted Origin, the content type
        // a no-preflight page can send is still refused.
        assert_eq!(
            check_admission("POST", "/mcp", None, Some("text/plain")),
            Err(Refusal::NotJson)
        );
    }

    #[test]
    fn only_post_to_the_mcp_path_is_served() {
        assert_eq!(
            check_admission("GET", "/mcp", None, Some("application/json")),
            Err(Refusal::MethodNotAllowed)
        );
        assert_eq!(
            check_admission("POST", "/", None, Some("application/json")),
            Err(Refusal::NotFound)
        );
        assert_eq!(
            check_admission("POST", "/mcp/extra", None, Some("application/json")),
            Err(Refusal::NotFound)
        );
        // Method casing is not meaningful in HTTP routing here.
        assert_eq!(check_admission("post", "/mcp", None, Some("application/json")), Ok(()));
    }

    /// An OPTIONS preflight must not be answered with anything permissive —
    /// there are no CORS headers on any response, so a browser preflight fails
    /// closed. Pinned because "helpfully" adding CORS headers later would
    /// silently undo the whole defense.
    #[test]
    fn preflight_is_not_specially_accommodated() {
        assert_eq!(
            check_admission("OPTIONS", "/mcp", Some("https://evil.example"), None),
            Err(Refusal::BrowserOrigin)
        );
        assert_eq!(check_admission("OPTIONS", "/mcp", None, None), Err(Refusal::MethodNotAllowed));
    }

    #[test]
    fn refusal_statuses_are_distinct_and_sane() {
        assert_eq!(Refusal::BrowserOrigin.status(), 403);
        assert_eq!(Refusal::NotJson.status(), 415);
        assert_eq!(Refusal::NotFound.status(), 404);
        assert_eq!(Refusal::MethodNotAllowed.status(), 405);
        assert_eq!(Refusal::TooLarge.status(), 413);
    }
}
