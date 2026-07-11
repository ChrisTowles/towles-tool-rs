//! Slack DM watcher: polls one DM conversation via the Slack Web API.
//!
//! Three calls per tick with a user OAuth token (`xoxp-…`, scopes `im:history`
//! and `im:read`): `auth.test` validates the token and yields the team id for
//! the `slack://` deep link; `conversations.open` resolves the watched user's
//! DM channel (idempotent, returns the existing channel); and
//! `conversations.history` fetches the latest messages. The newest real
//! message decides everything: if it was sent by the watched user the DM is
//! *unanswered*; if it was sent by anyone else (i.e. me) it is answered.
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
        let body: serde_json::Value = response
            .into_json()
            .map_err(|e| format!("slack {method} returned invalid JSON: {e}"))?;
        check_ok(&body, method)?;
        Ok(body)
    }
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
    fn latest_message_falls_back_to_member_id_without_a_name() {
        let mut cfg = config();
        cfg.watch_name = "  ".to_string();
        let history = json!({"ok": true, "messages": [
            {"user": "U_WIFE", "text": "hi", "ts": "1.0"}
        ]});
        assert_eq!(latest_message(&history, &cfg, "D1", "T1").unwrap().from_name, "U_WIFE");
    }
}
