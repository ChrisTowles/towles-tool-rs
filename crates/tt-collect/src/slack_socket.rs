//! Slack Socket Mode: the pure protocol logic for real-time DM delivery.
//!
//! Socket Mode replaces the 60s poll with an event stream. An app-level token
//! (`xapp-…`) opens a connection via `apps.connections.open`, which returns a
//! short-lived `wss://` URL; the app connects that WebSocket and receives
//! *envelopes*:
//!
//! - `hello` — the connection is live (resets reconnect backoff).
//! - `events_api` — carries an Events API payload (we care about `message.im`);
//!   **must be acked** within a few seconds by echoing its `envelope_id`.
//! - `disconnect` — Slack is closing this socket (it does so every few minutes
//!   by design, or to shed load); reconnect with a *fresh*
//!   `apps.connections.open`.
//!
//! Everything here is pure and unit-tested: envelope parsing, ack construction,
//! the watched-message predicate, reconnect backoff, and the connection-URL
//! extraction. The actual WebSocket I/O and collector refresh live in the app
//! shell (`crates-tauri/tt-app/src/slack_socket.rs`), which drives these
//! functions — keeping this crate Tauri-free.

use std::time::Duration;

/// Per-call HTTP cap for `apps.connections.open` (mirrors the DM client's cap).
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// A decoded Socket Mode envelope, normalized to the cases the DM watcher acts
/// on. Anything unrecognized (or not requiring an ack) is [`Envelope::Ignore`].
#[derive(Debug, Clone, PartialEq)]
pub enum Envelope {
    /// Connection acknowledged — the socket is live.
    Hello,
    /// Slack is closing this connection; reconnect fresh.
    Disconnect { reason: String },
    /// An ack-required envelope (`events_api`/`slash_commands`/`interactive`),
    /// carrying a message event when it was a `message.*`.
    Event {
        envelope_id: String,
        message: Option<MessageEvent>,
    },
    /// Junk, or a type we neither ack nor act on.
    Ignore,
}

/// The fields of a `message` Events API event the DM watcher needs to decide
/// whether an incoming message belongs to the watched conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageEvent {
    /// The channel (a DM is a `D…` id).
    pub channel: String,
    /// `"im"` for a direct message.
    pub channel_type: String,
    /// Sender member id.
    pub user: String,
    /// Present on edits/deletes/etc; `file_share` on a shared image.
    pub subtype: Option<String>,
    /// Slack `ts` (`"seconds.micros"`).
    pub ts: String,
}

#[derive(serde::Deserialize)]
struct RawEnvelope {
    #[serde(rename = "type")]
    kind: String,
    envelope_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    payload: Option<RawPayload>,
}

#[derive(serde::Deserialize)]
struct RawPayload {
    #[serde(default)]
    event: Option<RawEvent>,
}

#[derive(serde::Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    channel_type: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

/// Decode a raw envelope frame. Malformed JSON and unknown/no-ack types map to
/// [`Envelope::Ignore`] so the socket loop can treat parsing as infallible.
pub fn parse_envelope(text: &str) -> Envelope {
    let Ok(raw) = serde_json::from_str::<RawEnvelope>(text) else {
        return Envelope::Ignore;
    };
    match raw.kind.as_str() {
        "hello" => Envelope::Hello,
        "disconnect" => Envelope::Disconnect { reason: raw.reason.unwrap_or_default() },
        // Every other typed envelope Slack sends over Socket Mode
        // (`events_api`, `slash_commands`, `interactive`) requires an ack keyed
        // by its `envelope_id`. We extract a message event when present and ack
        // the rest with no further action.
        _ => match raw.envelope_id {
            Some(envelope_id) => {
                let message =
                    raw.payload.and_then(|p| p.event).filter(|e| e.kind == "message").map(|e| {
                        MessageEvent {
                            channel: e.channel.unwrap_or_default(),
                            channel_type: e.channel_type.unwrap_or_default(),
                            user: e.user.unwrap_or_default(),
                            subtype: e.subtype,
                            ts: e.ts.unwrap_or_default(),
                        }
                    });
                Envelope::Event { envelope_id, message }
            }
            None => Envelope::Ignore,
        },
    }
}

/// The ack frame for an `envelope_id` — Slack drops the connection if an
/// events envelope goes unacked, so the loop sends this before acting.
pub fn ack_json(envelope_id: &str) -> String {
    serde_json::json!({ "envelope_id": envelope_id }).to_string()
}

/// Whether a message event belongs to the watched DM and should trigger a
/// refresh. New messages (and shared files) count; edits/deletes don't. When a
/// resolved `watched_channel` is known it's matched exactly; otherwise the
/// watched sender is used (which misses my own outbound — already known
/// locally — but catches theirs).
pub fn is_watched_message(msg: &MessageEvent, watched_channel: &str, watch_user_id: &str) -> bool {
    if let Some(sub) = &msg.subtype
        && sub != "file_share"
    {
        return false;
    }
    if msg.channel_type != "im" {
        return false;
    }
    if !watched_channel.is_empty() {
        return msg.channel == watched_channel;
    }
    !watch_user_id.is_empty() && msg.user == watch_user_id
}

/// Extract the `wss://` URL from an `apps.connections.open` response, surfacing
/// Slack's `{ok:false, error}` shape as an error.
pub fn parse_connection_url(body: &serde_json::Value) -> Result<String, String> {
    if !body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let err = body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        return Err(format!("apps.connections.open failed: {err}"));
    }
    body.get("url")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "apps.connections.open: no url in response".to_string())
}

/// Open a Socket Mode connection with the app-level token and return the
/// `wss://` URL to connect. Live HTTP (the URL parsing is the tested seam).
pub fn open_socket_connection(app_token: &str) -> Result<String, String> {
    let response = crate::slack::agent()?
        .post("https://slack.com/api/apps.connections.open")
        .set("Authorization", &format!("Bearer {app_token}"))
        .timeout(HTTP_TIMEOUT)
        .send_form(&[])
        .map_err(|e| format!("apps.connections.open request failed: {e}"))?;
    let body: serde_json::Value = response
        .into_json()
        .map_err(|e| format!("apps.connections.open returned invalid JSON: {e}"))?;
    parse_connection_url(&body)
}

const BACKOFF_BASE_MS: u64 = 1_000;
const BACKOFF_CAP_MS: u64 = 30_000;
/// 2^5 · 1s = 32s already exceeds the 30s cap, so attempts past this add nothing.
const BACKOFF_MAX_SHIFT: u32 = 5;

/// Reconnect backoff for the socket loop: exponential from 1s, doubling, capped
/// at 30s. Reset on a `hello` (a healthy connection); advanced on each failed
/// connect so a persistent outage (revoked token, Slack down) stops hammering.
#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self { attempt: 0 }
    }

    /// Drop back to the base delay after a successful connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// The delay to wait before the next reconnect, advancing the attempt.
    pub fn next_delay(&mut self) -> Duration {
        let shift = self.attempt.min(BACKOFF_MAX_SHIFT);
        let ms = BACKOFF_BASE_MS.saturating_mul(1u64 << shift).min(BACKOFF_CAP_MS);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_hello_and_disconnect() {
        assert_eq!(parse_envelope(r#"{"type":"hello","num_connections":1}"#), Envelope::Hello);
        assert_eq!(
            parse_envelope(r#"{"type":"disconnect","reason":"warning"}"#),
            Envelope::Disconnect { reason: "warning".to_string() }
        );
        // Missing reason degrades to empty, still a disconnect.
        assert_eq!(
            parse_envelope(r#"{"type":"disconnect"}"#),
            Envelope::Disconnect { reason: String::new() }
        );
    }

    #[test]
    fn parse_events_api_message_event() {
        let frame = json!({
            "type": "events_api",
            "envelope_id": "env-123",
            "accepts_response_payload": false,
            "payload": {
                "event": {
                    "type": "message",
                    "channel": "D07ABC",
                    "channel_type": "im",
                    "user": "U_WIFE",
                    "text": "dinner at 7?",
                    "ts": "1720000100.000200"
                }
            }
        })
        .to_string();
        let Envelope::Event { envelope_id, message } = parse_envelope(&frame) else {
            panic!("expected an Event envelope");
        };
        assert_eq!(envelope_id, "env-123");
        let msg = message.expect("a message event");
        assert_eq!(msg.channel, "D07ABC");
        assert_eq!(msg.channel_type, "im");
        assert_eq!(msg.user, "U_WIFE");
        assert_eq!(msg.subtype, None);
        assert_eq!(msg.ts, "1720000100.000200");
    }

    #[test]
    fn parse_events_api_non_message_still_acks_without_a_message() {
        let frame = json!({
            "type": "events_api",
            "envelope_id": "env-9",
            "payload": { "event": { "type": "reaction_added", "user": "U_WIFE" } }
        })
        .to_string();
        assert_eq!(
            parse_envelope(&frame),
            Envelope::Event { envelope_id: "env-9".to_string(), message: None }
        );
    }

    #[test]
    fn slash_and_interactive_envelopes_ack_with_no_message() {
        for kind in ["slash_commands", "interactive"] {
            let frame = json!({ "type": kind, "envelope_id": "e1" }).to_string();
            assert_eq!(
                parse_envelope(&frame),
                Envelope::Event { envelope_id: "e1".to_string(), message: None }
            );
        }
    }

    #[test]
    fn malformed_or_ackless_frames_are_ignored() {
        assert_eq!(parse_envelope("not json"), Envelope::Ignore);
        // An unknown type with no envelope_id can't be acked, so ignore it.
        assert_eq!(parse_envelope(r#"{"type":"mystery"}"#), Envelope::Ignore);
    }

    #[test]
    fn ack_json_echoes_the_envelope_id() {
        assert_eq!(ack_json("abc-1"), r#"{"envelope_id":"abc-1"}"#);
    }

    fn msg(channel: &str, channel_type: &str, user: &str, subtype: Option<&str>) -> MessageEvent {
        MessageEvent {
            channel: channel.to_string(),
            channel_type: channel_type.to_string(),
            user: user.to_string(),
            subtype: subtype.map(str::to_string),
            ts: "1.0".to_string(),
        }
    }

    #[test]
    fn watched_message_matches_the_resolved_channel() {
        let m = msg("D1", "im", "U_WIFE", None);
        assert!(is_watched_message(&m, "D1", "U_WIFE"));
        // A different DM (someone else) must not trigger a refresh.
        assert!(!is_watched_message(&msg("D2", "im", "U_OTHER", None), "D1", "U_WIFE"));
    }

    #[test]
    fn watched_message_falls_back_to_sender_without_a_channel() {
        // No resolved channel: match on the watched sender instead.
        assert!(is_watched_message(&msg("D9", "im", "U_WIFE", None), "", "U_WIFE"));
        assert!(!is_watched_message(&msg("D9", "im", "U_ME", None), "", "U_WIFE"));
    }

    #[test]
    fn watched_message_ignores_non_im_and_edits_but_keeps_file_shares() {
        assert!(!is_watched_message(&msg("C1", "channel", "U_WIFE", None), "", "U_WIFE"));
        assert!(!is_watched_message(&msg("D1", "im", "U_WIFE", Some("message_changed")), "D1", ""));
        // A shared photo is a real new message.
        assert!(is_watched_message(&msg("D1", "im", "U_WIFE", Some("file_share")), "D1", ""));
    }

    #[test]
    fn parse_connection_url_reads_the_wss_url() {
        let body = json!({ "ok": true, "url": "wss://wss-primary.slack.com/link/?ticket=x" });
        assert_eq!(
            parse_connection_url(&body).unwrap(),
            "wss://wss-primary.slack.com/link/?ticket=x"
        );
    }

    #[test]
    fn parse_connection_url_surfaces_slack_errors() {
        let err = parse_connection_url(&json!({ "ok": false, "error": "invalid_auth" }));
        assert_eq!(err.unwrap_err(), "apps.connections.open failed: invalid_auth");
        assert!(parse_connection_url(&json!({ "ok": true })).is_err());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let mut b = Backoff::new();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
        assert_eq!(b.next_delay(), Duration::from_secs(4));
        assert_eq!(b.next_delay(), Duration::from_secs(8));
        assert_eq!(b.next_delay(), Duration::from_secs(16));
        assert_eq!(b.next_delay(), Duration::from_secs(30), "capped at 30s");
        assert_eq!(b.next_delay(), Duration::from_secs(30), "stays capped");
        // A healthy hello resets to the base delay.
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }
}
