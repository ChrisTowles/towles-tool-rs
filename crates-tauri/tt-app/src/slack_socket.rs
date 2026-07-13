//! Slack Socket Mode driver: the WebSocket loop that turns the 60s DM poll into
//! real-time delivery when an app-level token (`xapp-…`) is configured.
//!
//! All protocol decisions — envelope parsing, ack construction, the
//! watched-message predicate, reconnect backoff, connection-URL extraction —
//! live in the Tauri-free `tt_collect::slack_socket` module and are unit-tested
//! there. This module is just the I/O shell: open a connection with
//! `apps.connections.open`, connect the returned `wss://` URL, ack each events
//! envelope, and on a message in the watched DM re-run the `slack:dm` collector
//! and re-emit the snapshot (the same refresh `slack_dm_send` does). The poll
//! stays on as the fallback/backfill; the socket just makes it instant.
//!
//! Absence of an app token is a clean no-op: the task parks on the reload signal
//! and costs nothing until Slack is (re)configured.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tt_collect::{Backoff, Envelope, SlackDmConfig};

use crate::store::SNAPSHOT_EVENT;

/// The Slack config a socket connection needs: the user token (Web API DM
/// calls + file fetch), the app token (`apps.connections.open`), and the watched
/// user for event matching.
struct SocketConfig {
    dm: SlackDmConfig,
    app_token: String,
}

/// Read socket config from settings, or `None` when Socket Mode is off: the
/// slack collector disabled, or any of the user token / app token / watch id
/// missing. Absence keeps the task idle rather than erroring.
fn read_config() -> Option<SocketConfig> {
    let slack = tt_config::load().ok()?.collectors.slack;
    if !slack.enabled
        || slack.token.trim().is_empty()
        || slack.app_token.trim().is_empty()
        || slack.watch_user_id.trim().is_empty()
    {
        return None;
    }
    Some(SocketConfig {
        dm: SlackDmConfig {
            token: slack.token,
            watch_user_id: slack.watch_user_id,
            watch_name: slack.watch_name,
        },
        app_token: slack.app_token,
    })
}

/// How a connection attempt ended, deciding the loop's next move.
enum Outcome {
    /// Settings changed mid-connection — re-read config immediately.
    Reload,
    /// Slack closed the socket cleanly (it does this every few minutes by
    /// design) — reconnect promptly with a fresh `apps.connections.open`.
    Disconnected,
    /// The connection failed or dropped with an error — back off before retry.
    Error,
}

/// Spawn the Socket Mode loop. Re-reads settings whenever `reload` fires (a
/// `settings_set`), so enabling Socket Mode or changing tokens takes effect
/// without a relaunch.
pub fn spawn(app: AppHandle, reload: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        let mut backoff = Backoff::new();
        loop {
            let Some(config) = read_config() else {
                // Socket Mode off: park until settings change, then re-evaluate.
                reload.notified().await;
                backoff.reset();
                continue;
            };
            match run_connection(&app, &config, &reload, &mut backoff).await {
                Outcome::Reload | Outcome::Disconnected => backoff.reset(),
                Outcome::Error => {
                    let delay = backoff.next_delay();
                    // Sleep before reconnecting, but wake early if settings change.
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = reload.notified() => backoff.reset(),
                    }
                }
            }
        }
    });
}

/// Open one Socket Mode connection and pump envelopes until it ends. Returns how
/// it ended so the caller can decide the reconnect cadence. Resets `backoff` on
/// `hello` so a connection that proved healthy — then dropped with an error —
/// reconnects at the base delay rather than an inflated one.
async fn run_connection(
    app: &AppHandle,
    config: &SocketConfig,
    reload: &Notify,
    backoff: &mut Backoff,
) -> Outcome {
    // `apps.connections.open` is a blocking HTTP call (ureq).
    let app_token = config.app_token.clone();
    let url = match tauri::async_runtime::spawn_blocking(move || {
        tt_collect::open_socket_connection(&app_token)
    })
    .await
    {
        Ok(Ok(url)) => url,
        Ok(Err(e)) => {
            eprintln!("slack socket: open connection failed: {e}");
            return Outcome::Error;
        }
        Err(e) => {
            eprintln!("slack socket: open task failed: {e}");
            return Outcome::Error;
        }
    };

    // native-tls verifies against the OS trust store, matching the ureq client
    // in tt-collect — required for networks that TLS-inspect with a corporate
    // root CA (Zscaler and similar), which rustls's bundled webpki-roots (the
    // `connect_async` default) doesn't trust.
    let connector = match native_tls::TlsConnector::new() {
        Ok(c) => tokio_tungstenite::Connector::NativeTls(c),
        Err(e) => {
            eprintln!("slack socket: failed to initialize native TLS: {e}");
            return Outcome::Error;
        }
    };
    let mut ws =
        match tokio_tungstenite::connect_async_tls_with_config(&url, None, false, Some(connector))
            .await
        {
            Ok((ws, _resp)) => ws,
            Err(e) => {
                eprintln!("slack socket: websocket connect failed: {e}");
                return Outcome::Error;
            }
        };

    // Resolve the watched DM channel once so events match the exact conversation
    // (mine and theirs). If it fails we fall back to sender-matching inside
    // `is_watched_message`, which still catches the watched user's messages.
    let dm = config.dm.clone();
    let watched_channel =
        tauri::async_runtime::spawn_blocking(move || tt_collect::dm_channel_id(&dm))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();

    loop {
        tokio::select! {
            _ = reload.notified() => return Outcome::Reload,
            frame = ws.next() => {
                let Some(frame) = frame else {
                    // Stream ended without a disconnect envelope.
                    return Outcome::Error;
                };
                let text = match frame {
                    Ok(Message::Text(t)) => t.as_str().to_string(),
                    Ok(Message::Close(_)) => return Outcome::Error,
                    Ok(_) => continue, // ping/pong/binary — tungstenite auto-pongs.
                    Err(e) => {
                        eprintln!("slack socket: read error: {e}");
                        return Outcome::Error;
                    }
                };
                match tt_collect::parse_envelope(&text) {
                    // The connection is live: drop back to the base reconnect
                    // delay so a later error doesn't inherit an inflated backoff.
                    Envelope::Hello => backoff.reset(),
                    Envelope::Disconnect { reason } => {
                        eprintln!("slack socket: server disconnect ({reason}); reconnecting");
                        return Outcome::Disconnected;
                    }
                    Envelope::Event { envelope_id, message } => {
                        // Ack first — Slack drops the socket if an events
                        // envelope goes unacked within a few seconds.
                        if let Err(e) =
                            ws.send(Message::Text(tt_collect::ack_json(&envelope_id).into())).await
                        {
                            eprintln!("slack socket: ack failed: {e}");
                            return Outcome::Error;
                        }
                        if let Some(msg) = message
                            && tt_collect::is_watched_message(
                                &msg,
                                &watched_channel,
                                &config.dm.watch_user_id,
                            )
                        {
                            refresh_dm(app, &config.dm).await;
                        }
                    }
                    Envelope::Ignore => {}
                }
            }
        }
    }
}

/// Re-run the `slack:dm` collector and re-emit the snapshot so the attention
/// banner and any open chat panel reflect the new message immediately — the
/// same refresh path `slack_dm_send` uses. Best-effort: a store hiccup just
/// leaves the next poll to catch up.
async fn refresh_dm(app: &AppHandle, config: &SlackDmConfig) {
    if main_window_minimized(app) {
        return;
    }
    let app = app.clone();
    let config = config.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Ok(store) = tt_store::Store::open_default() {
            let now = chrono::Local::now().timestamp_millis();
            let _ = tt_collect::collect_slack_dm(&store, &config, now);
            if let Ok(snapshot) = store.snapshot() {
                let _ = app.emit(SNAPSHOT_EVENT, snapshot);
            }
        }
    })
    .await;
}

/// Whether the main window is minimized — mirrors the scheduler's guard so a
/// backgrounded app doesn't churn the store on every incoming message. Unknown
/// states count as visible.
fn main_window_minimized(app: &AppHandle) -> bool {
    app.get_webview_window("main").map(|w| w.is_minimized().unwrap_or(false)).unwrap_or(false)
}
