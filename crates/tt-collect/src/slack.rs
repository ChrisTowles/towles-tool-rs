//! Slack DM watcher + chat bridge: one DM conversation via the Slack Web API.
//!
//! The *watcher* ([`fetch_dm`]) makes three calls per tick with a user OAuth
//! token (`xoxp-…`, scopes `im:history` and `im:read`): `auth.test` validates
//! the token and yields the team id for the `slack://` deep link;
//! `conversations.open` resolves the watched user's DM channel (idempotent,
//! returns the existing channel); and `conversations.history` fetches the
//! latest messages. The newest real message decides everything: if it was sent
//! by the watched user the DM is *unanswered*; if it was sent by anyone else
//! (i.e. me) it is answered.
//!
//! The *chat bridge* serves the app's DM panel on demand: [`fetch_dm_history`]
//! returns the conversation itself (oldest first), and [`send_dm`] posts a
//! reply as the user via `chat.postMessage` (additional scope: `chat:write`).
//!
//! HTTP plumbing is isolated in [`SlackHttp`]; all response interpretation is
//! pure functions over `serde_json::Value` so it unit-tests with inline
//! fixtures (same pattern as the `gh` collectors).

use std::time::Duration;

use tt_store::DmInput;

/// Per-call HTTP cap. Slack answers these in well under a second; without a
/// cap a dead network wedges the scheduler's blocking worker.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Slack settings the collector needs, decoupled from `tt-config` (callers map
/// their settings into this, mirroring how `CalendarProvider` is passed in).
#[derive(Debug, Clone, PartialEq)]
pub struct SlackDmConfig {
    /// User OAuth token (`xoxp-…`).
    pub token: String,
    /// Slack member ID being watched (e.g. `U0123ABCD`).
    pub watch_user_id: String,
    /// Display name for the banner; falls back to the member ID when empty.
    pub watch_name: String,
}

/// Minimal Slack Web API client (GET/POST with bearer token).
struct SlackHttp<'a> {
    token: &'a str,
}

impl SlackHttp<'_> {
    fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<serde_json::Value, String> {
        let mut request = ureq::post(&format!("https://slack.com/api/{method}"))
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(HTTP_TIMEOUT);
        for (k, v) in params {
            request = request.query(k, v);
        }
        let response = request.call().map_err(|e| format!("slack {method} request failed: {e}"))?;
        Self::parse_response(response, method)
    }

    /// POST with a form-encoded body — for calls carrying user-written text
    /// (`chat.postMessage`), where a query string would cap length and mangle
    /// newlines.
    fn call_form(&self, method: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, String> {
        let response = ureq::post(&format!("https://slack.com/api/{method}"))
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(HTTP_TIMEOUT)
            .send_form(form)
            .map_err(|e| format!("slack {method} request failed: {e}"))?;
        Self::parse_response(response, method)
    }

    fn parse_response(response: ureq::Response, method: &str) -> Result<serde_json::Value, String> {
        let body: serde_json::Value = response
            .into_json()
            .map_err(|e| format!("slack {method} returned invalid JSON: {e}"))?;
        check_ok(&body, method)?;
        Ok(body)
    }
}

/// One message of the watched DM conversation, as the app's chat panel renders
/// it. Serialized camelCase because it crosses the Tauri IPC boundary verbatim.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmMessage {
    pub text: String,
    /// Epoch ms.
    pub ts: i64,
    /// `true` when the message was sent by me (anyone but the watched user).
    pub from_me: bool,
    /// Files (images/attachments) shared on this message, if any. Empty for a
    /// plain text message.
    pub files: Vec<DmFile>,
}

/// One file attached to a DM message. The private URLs need the token's bearer
/// header, so the webview can't load them directly — the app fetches the bytes
/// through [`fetch_file`] (see the `slack_dm_file` command) and renders images
/// as a `data:` URI. Serialized camelCase for the IPC boundary.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmFile {
    pub id: String,
    /// Display name (`name`, falling back to `title`).
    pub name: String,
    pub mimetype: String,
    /// Full-size authenticated URL (`url_private`).
    pub url_private: String,
    /// A thumbnail URL when Slack provided one (images only), else empty.
    pub thumb_url: String,
    /// Human-facing web permalink, for "open in browser" on non-image chips.
    pub permalink: String,
    /// `true` when the mimetype is `image/*` — the panel renders these inline.
    pub is_image: bool,
}

/// Fetch the newest `limit` messages of the watched DM, oldest first. Serves
/// the app's chat panel on demand (no store involved).
pub fn fetch_dm_history(config: &SlackDmConfig, limit: u32) -> Result<Vec<DmMessage>, String> {
    let http = SlackHttp { token: &config.token };
    let open = http.call("conversations.open", &[("users", config.watch_user_id.as_str())])?;
    let channel = parse_open_channel(&open)?;
    let history = http.call(
        "conversations.history",
        &[("channel", channel.as_str()), ("limit", &limit.to_string())],
    )?;
    Ok(parse_history(&history, config))
}

/// Resolve the watched user's DM channel id (`D…`) via `conversations.open`
/// (idempotent — returns the existing channel). The socket loop uses it to match
/// incoming `message.im` events to the watched conversation exactly.
pub fn dm_channel_id(config: &SlackDmConfig) -> Result<String, String> {
    let http = SlackHttp { token: &config.token };
    let open = http.call("conversations.open", &[("users", config.watch_user_id.as_str())])?;
    parse_open_channel(&open)
}

/// Send `text` to the watched DM as the token's user (`chat:write` scope).
pub fn send_dm(config: &SlackDmConfig, text: &str) -> Result<(), String> {
    let http = SlackHttp { token: &config.token };
    let open = http.call("conversations.open", &[("users", config.watch_user_id.as_str())])?;
    let channel = parse_open_channel(&open)?;
    http.call_form("chat.postMessage", &[("channel", channel.as_str()), ("text", text)])?;
    Ok(())
}

/// Hard cap on a fetched file (bytes). Thumbnails are tiny and full images a few
/// MB; the cap keeps a surprise large upload from ballooning the base64 payload
/// that crosses the IPC boundary.
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// A fetched Slack file: its declared content type and raw bytes.
pub struct SlackFile {
    pub mimetype: String,
    pub bytes: Vec<u8>,
}

/// Fetch a private Slack file's bytes with the user token's bearer header — the
/// webview can't, since those URLs 302 to a sign-in page without it. Only
/// `*.slack.com` URLs are honored (the token must never ride along to an
/// arbitrary host). A missing `files:read` scope surfaces as a distinct
/// unauthorized error the caller can render as a placeholder rather than a hard
/// failure.
pub fn fetch_file(token: &str, url: &str) -> Result<SlackFile, String> {
    use std::io::Read;

    if !is_slack_file_url(url) {
        return Err(format!("refusing to fetch non-Slack file URL: {url}"));
    }
    let response = ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401 | 403, _) => file_unauthorized(),
            other => format!("slack file request failed: {other}"),
        })?;
    let mimetype = response.content_type().to_string();
    if mimetype.starts_with("text/html") {
        // A token without `files:read` gets Slack's sign-in HTML at HTTP 200
        // instead of the bytes; treat that as the scope error, not an image.
        return Err(file_unauthorized());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("slack file read failed: {e}"))?;
    Ok(SlackFile { mimetype, bytes })
}

/// The one unauthorized message, carrying the stable `files:read` marker the
/// frontend matches to show its "re-auth for images" placeholder.
fn file_unauthorized() -> String {
    "slack file unauthorized (files:read scope missing)".to_string()
}

/// Whether `url` is an `https` URL on a `*.slack.com` host — the guard that
/// keeps the bearer token from being attached to any other origin.
pub(crate) fn is_slack_file_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Strip any userinfo/port so only the hostname is matched.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host == "slack.com" || host.ends_with(".slack.com")
}

/// Map a `conversations.history` response to chronological (oldest-first)
/// messages. Skips senderless entries and edit/delete tombstones, but *keeps*
/// `file_share` messages (a photo or attachment with no or little text) so the
/// chat panel can render them — unlike [`latest_message`], which only cares
/// about the answered/unanswered edge.
pub(crate) fn parse_history(history: &serde_json::Value, config: &SlackDmConfig) -> Vec<DmMessage> {
    let Some(messages) = history.get("messages").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<DmMessage> = messages
        .iter()
        .filter(|m| is_renderable(m))
        .filter_map(|m| {
            let sender = m.get("user").and_then(|v| v.as_str())?;
            Some(DmMessage {
                text: str_at(m, "text"),
                ts: slack_ts_ms(str_at(m, "ts").as_str()),
                from_me: sender != config.watch_user_id,
                files: parse_files(m),
            })
        })
        .collect();
    // Slack returns history newest-first; the chat view reads top-down in time.
    out.reverse();
    out
}

/// Whether a `conversations.history` entry is a real message the panel renders:
/// it has a sender, and it is either a plain message (no subtype) or a
/// `file_share` (a shared image/attachment). Edits, deletes, joins and other
/// subtypes are noise.
fn is_renderable(m: &serde_json::Value) -> bool {
    if m.get("user").and_then(|v| v.as_str()).is_none() {
        return false;
    }
    match m.get("subtype").and_then(|v| v.as_str()) {
        None => true,
        Some("file_share") => true,
        Some(_) => false,
    }
}

/// Parse a message's `files` array into [`DmFile`]s, skipping tombstoned
/// (deleted) entries and any without an id.
fn parse_files(m: &serde_json::Value) -> Vec<DmFile> {
    let Some(files) = m.get("files").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    files.iter().filter_map(parse_file).collect()
}

/// Parse one Slack file object. Returns `None` for tombstones and entries
/// missing an id.
fn parse_file(f: &serde_json::Value) -> Option<DmFile> {
    if f.get("mode").and_then(|v| v.as_str()) == Some("tombstone") {
        return None;
    }
    let id = f.get("id").and_then(|v| v.as_str())?.to_string();
    let name = f
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| f.get("title").and_then(|v| v.as_str()))
        .unwrap_or("file")
        .to_string();
    let mimetype = str_at(f, "mimetype");
    let is_image = mimetype.starts_with("image/");
    Some(DmFile {
        id,
        name,
        mimetype,
        url_private: str_at(f, "url_private"),
        thumb_url: pick_thumb(f),
        permalink: str_at(f, "permalink"),
        is_image,
    })
}

/// Choose a reasonably-sized thumbnail URL from a Slack file object, preferring
/// a mid-size render and falling back through what's available. Empty when the
/// file has no thumbnails (non-image attachments).
fn pick_thumb(f: &serde_json::Value) -> String {
    for key in [
        "thumb_360",
        "thumb_480",
        "thumb_720",
        "thumb_160",
        "thumb_80",
    ] {
        if let Some(url) = f.get(key).and_then(|v| v.as_str()) {
            return url.to_string();
        }
    }
    String::new()
}

/// Fetch the watched DM's latest state. Returns `Ok(None)` when the DM exists
/// but holds no visible messages yet.
pub(crate) fn fetch_dm(config: &SlackDmConfig) -> Result<Option<DmInput>, String> {
    let http = SlackHttp { token: &config.token };

    let auth = http.call("auth.test", &[])?;
    let team_id = str_at(&auth, "team_id");

    let open = http.call("conversations.open", &[("users", config.watch_user_id.as_str())])?;
    let channel = parse_open_channel(&open)?;

    let history =
        http.call("conversations.history", &[("channel", channel.as_str()), ("limit", "10")])?;
    Ok(latest_message(&history, config, &channel, &team_id))
}

/// Slack wraps errors in `{"ok": false, "error": "..."}` with HTTP 200.
fn check_ok(body: &serde_json::Value, method: &str) -> Result<(), String> {
    if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Ok(());
    }
    let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
    Err(format!("slack {method} failed: {error}"))
}

/// Extract the DM channel id from a `conversations.open` response.
pub(crate) fn parse_open_channel(body: &serde_json::Value) -> Result<String, String> {
    body.pointer("/channel/id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "slack conversations.open: no channel id in response".to_string())
}

/// Map a `conversations.history` response to the DM's latest state.
///
/// Skips messages with a `subtype` (joins, edits-tombstones, bot chatter) and
/// messages without a sender; the first remaining entry is the newest real
/// message (Slack returns history newest-first).
pub(crate) fn latest_message(
    history: &serde_json::Value,
    config: &SlackDmConfig,
    channel: &str,
    team_id: &str,
) -> Option<DmInput> {
    let messages = history.get("messages").and_then(|v| v.as_array())?;
    let msg = messages
        .iter()
        .find(|m| m.get("subtype").is_none() && m.get("user").and_then(|v| v.as_str()).is_some())?;

    let sender = msg.get("user").and_then(|v| v.as_str()).unwrap_or_default();
    let from_me = sender != config.watch_user_id;
    let from_name = if config.watch_name.trim().is_empty() {
        config.watch_user_id.clone()
    } else {
        config.watch_name.clone()
    };
    let url = if team_id.is_empty() {
        None
    } else {
        Some(format!("slack://channel?team={team_id}&id={channel}"))
    };

    Some(DmInput {
        channel: channel.to_string(),
        from_name,
        text: str_at(msg, "text"),
        ts: slack_ts_ms(str_at(msg, "ts").as_str()),
        from_me,
        url,
    })
}

/// Slack `ts` is `"seconds.micros"` as a string; convert to epoch ms (0 on
/// unparseable input, matching the other collectors' lenient timestamps).
pub(crate) fn slack_ts_ms(ts: &str) -> i64 {
    ts.parse::<f64>().map(|s| (s * 1000.0) as i64).unwrap_or(0)
}

fn str_at(value: &serde_json::Value, key: &str) -> String {
    value.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> SlackDmConfig {
        SlackDmConfig {
            token: "xoxp-test".to_string(),
            watch_user_id: "U_WIFE".to_string(),
            watch_name: "Sarah".to_string(),
        }
    }

    #[test]
    fn check_ok_accepts_ok_and_surfaces_slack_errors() {
        assert!(check_ok(&json!({"ok": true}), "auth.test").is_ok());
        let err = check_ok(&json!({"ok": false, "error": "invalid_auth"}), "auth.test");
        assert_eq!(err.unwrap_err(), "slack auth.test failed: invalid_auth");
        assert!(check_ok(&json!({}), "auth.test").is_err());
    }

    #[test]
    fn parse_open_channel_reads_the_dm_id() {
        let body = json!({"ok": true, "channel": {"id": "D07ABC123"}});
        assert_eq!(parse_open_channel(&body).unwrap(), "D07ABC123");
        assert!(parse_open_channel(&json!({"ok": true})).is_err());
    }

    #[test]
    fn slack_ts_ms_converts_seconds_to_millis() {
        assert_eq!(slack_ts_ms("1720000000.123456"), 1720000000123);
        assert_eq!(slack_ts_ms("not-a-ts"), 0);
    }

    #[test]
    fn latest_message_from_watched_user_is_unanswered() {
        let history = json!({"ok": true, "messages": [
            {"user": "U_WIFE", "text": "can you grab the kids at 4?", "ts": "1720000100.000200"},
            {"user": "U_ME", "text": "heading out now", "ts": "1720000000.000100"}
        ]});
        let dm = latest_message(&history, &config(), "D1", "T1").unwrap();
        assert!(!dm.from_me);
        assert_eq!(dm.from_name, "Sarah");
        assert_eq!(dm.text, "can you grab the kids at 4?");
        assert_eq!(dm.ts, 1720000100000);
        assert_eq!(dm.url.as_deref(), Some("slack://channel?team=T1&id=D1"));
    }

    #[test]
    fn latest_message_from_me_is_answered() {
        let history = json!({"ok": true, "messages": [
            {"user": "U_ME", "text": "on it", "ts": "1720000200.0"},
            {"user": "U_WIFE", "text": "pickup at 4?", "ts": "1720000100.0"}
        ]});
        let dm = latest_message(&history, &config(), "D1", "T1").unwrap();
        assert!(dm.from_me);
    }

    #[test]
    fn latest_message_skips_subtypes_and_userless_entries() {
        let history = json!({"ok": true, "messages": [
            {"subtype": "message_changed", "user": "U_WIFE", "text": "edited", "ts": "1720000300.0"},
            {"text": "no sender", "ts": "1720000250.0"},
            {"user": "U_WIFE", "text": "real one", "ts": "1720000200.0"}
        ]});
        let dm = latest_message(&history, &config(), "D1", "T1").unwrap();
        assert_eq!(dm.text, "real one");
    }

    #[test]
    fn latest_message_handles_empty_history_and_missing_team() {
        assert!(
            latest_message(&json!({"ok": true, "messages": []}), &config(), "D1", "T1").is_none()
        );
        let history = json!({"ok": true, "messages": [
            {"user": "U_WIFE", "text": "hi", "ts": "1.0"}
        ]});
        let dm = latest_message(&history, &config(), "D1", "").unwrap();
        assert_eq!(dm.url, None);
    }

    #[test]
    fn parse_history_is_chronological_and_skips_noise() {
        let history = json!({"ok": true, "messages": [
            {"user": "U_WIFE", "text": "newest", "ts": "1720000300.0"},
            {"subtype": "message_changed", "user": "U_WIFE", "text": "edited", "ts": "1720000250.0"},
            {"text": "no sender", "ts": "1720000225.0"},
            {"user": "U_ME", "text": "mine", "ts": "1720000200.0"},
            {"user": "U_WIFE", "text": "oldest", "ts": "1720000100.0"}
        ]});
        let msgs = parse_history(&history, &config());
        let texts: Vec<&str> = msgs.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["oldest", "mine", "newest"], "oldest first, noise dropped");
        assert!(!msgs[0].from_me);
        assert!(msgs[1].from_me);
        assert_eq!(msgs[2].ts, 1720000300000);
    }

    #[test]
    fn parse_history_of_empty_or_malformed_response_is_empty() {
        assert!(parse_history(&json!({"ok": true, "messages": []}), &config()).is_empty());
        assert!(parse_history(&json!({"ok": true}), &config()).is_empty());
    }

    #[test]
    fn parse_history_reads_files_and_keeps_file_shares() {
        let history = json!({"ok": true, "messages": [
            {
                "subtype": "file_share",
                "user": "U_WIFE",
                "text": "look at this",
                "ts": "1720000300.0",
                "files": [
                    {
                        "id": "F123",
                        "name": "beach.jpg",
                        "mimetype": "image/jpeg",
                        "url_private": "https://files.slack.com/files-pri/T1-F123/beach.jpg",
                        "thumb_360": "https://files.slack.com/files-tmb/T1-F123/beach_360.jpg",
                        "permalink": "https://team.slack.com/files/U_WIFE/F123/beach.jpg"
                    }
                ]
            },
            {"user": "U_ME", "text": "nice", "ts": "1720000200.0"}
        ]});
        let msgs = parse_history(&history, &config());
        assert_eq!(msgs.len(), 2, "the file_share message is kept, not filtered");
        // Oldest-first: the plain reply (ts 200) precedes the newer share (ts 300).
        assert!(msgs[0].files.is_empty(), "the plain text message carries no files");
        let shared = &msgs[1];
        assert_eq!(shared.text, "look at this");
        assert_eq!(shared.files.len(), 1);
        let file = &shared.files[0];
        assert_eq!(file.id, "F123");
        assert_eq!(file.name, "beach.jpg");
        assert_eq!(file.mimetype, "image/jpeg");
        assert!(file.is_image);
        assert_eq!(file.url_private, "https://files.slack.com/files-pri/T1-F123/beach.jpg");
        assert_eq!(file.thumb_url, "https://files.slack.com/files-tmb/T1-F123/beach_360.jpg");
        assert_eq!(file.permalink, "https://team.slack.com/files/U_WIFE/F123/beach.jpg");
    }

    #[test]
    fn parse_history_handles_non_image_files_and_tombstones() {
        let history = json!({"ok": true, "messages": [
            {
                "subtype": "file_share",
                "user": "U_WIFE",
                "text": "",
                "ts": "1720000300.0",
                "files": [
                    {"id": "F1", "title": "budget.pdf", "mimetype": "application/pdf",
                     "url_private": "https://files.slack.com/files-pri/T1-F1/budget.pdf",
                     "permalink": "https://team.slack.com/files/U_WIFE/F1/budget.pdf"},
                    {"id": "F2", "mode": "tombstone"}
                ]
            }
        ]});
        let msgs = parse_history(&history, &config());
        assert_eq!(msgs.len(), 1);
        let files = &msgs[0].files;
        assert_eq!(files.len(), 1, "the tombstoned file is dropped");
        assert_eq!(files[0].name, "budget.pdf", "falls back to title when name is absent");
        assert!(!files[0].is_image);
        assert_eq!(files[0].thumb_url, "", "no thumbnail for a non-image");
    }

    #[test]
    fn is_slack_file_url_only_accepts_slack_https_hosts() {
        assert!(is_slack_file_url("https://files.slack.com/files-pri/T1-F1/x.png"));
        assert!(is_slack_file_url("https://slack.com/api/files.info"));
        assert!(is_slack_file_url("https://files-edge.slack.com/x"));
        // Wrong scheme, wrong host, or a look-alike domain must be refused so
        // the bearer token never leaks.
        assert!(!is_slack_file_url("http://files.slack.com/x"));
        assert!(!is_slack_file_url("https://evil.com/x"));
        assert!(!is_slack_file_url("https://files.slack.com.evil.com/x"));
        assert!(!is_slack_file_url("https://notslack.com/x"));
    }

    #[test]
    fn latest_message_falls_back_to_member_id_without_a_name() {
        let mut cfg = config();
        cfg.watch_name = "  ".to_string();
        let history = json!({"ok": true, "messages": [
            {"user": "U_WIFE", "text": "hi", "ts": "1.0"}
        ]});
        assert_eq!(latest_message(&history, &cfg, "D1", "T1").unwrap().from_name, "U_WIFE");
    }
}
