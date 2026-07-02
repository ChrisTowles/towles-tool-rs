//! Pure request parsing + validation for the localhost metadata HTTP ingest
//! (agentboard phase 5). Ports the agent-facing metadata API of slot-1
//! `server/index.ts` per docs/AGENTBOARD-BRIDGE-SPEC.md §5.
//!
//! Transport-free: this parses raw request text and validates it into a
//! [`MetadataMutation`] + an HTTP status/body. The tt-app layer only binds the
//! socket, reads bytes, applies the mutation to the engine, and writes the
//! response — so all the §5 semantics are unit-testable here on raw strings.

use crate::metadata::{LogInput, ProgressInput};
use crate::types::MetadataTone;

/// A validated metadata mutation to apply to the store.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataMutation {
    SetStatus {
        session: String,
        text: Option<String>,
        tone: Option<MetadataTone>,
    },
    SetProgress {
        session: String,
        progress: Option<ProgressInput>,
    },
    AppendLog {
        session: String,
        log: LogInput,
    },
    ClearLogs {
        session: String,
    },
}

/// Parsed request line + `Content-Length`. Ports the head-parsing tt-app needs to
/// read the body off the socket.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub content_length: usize,
}

/// The result of handling a request: an HTTP status, a response body, and an
/// optional mutation for the caller to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestOutcome {
    pub status: u16,
    pub body: String,
    pub mutation: Option<MetadataMutation>,
}

/// Parse the request head (everything before the body). Returns `None` if the
/// request line is malformed. Case-insensitive `Content-Length`; query strings
/// are stripped from the path.
pub fn parse_request_head(head: &str) -> Option<RequestHead> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let raw_path = parts.next()?;
    let path = raw_path.split('?').next().unwrap_or(raw_path).to_string();

    let mut content_length = 0;
    for line in lines {
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    Some(RequestHead { method, path, content_length })
}

/// Map a tone string to [`MetadataTone`]; unknown/absent → `None` (§5: invalid
/// tone becomes undefined). Ports the tone whitelist.
pub fn parse_tone(tone: Option<&str>) -> Option<MetadataTone> {
    match tone {
        Some("neutral") => Some(MetadataTone::Neutral),
        Some("info") => Some(MetadataTone::Info),
        Some("success") => Some(MetadataTone::Success),
        Some("warn") => Some(MetadataTone::Warn),
        Some("error") => Some(MetadataTone::Error),
        _ => None,
    }
}

fn bad_request(msg: &str) -> IngestOutcome {
    IngestOutcome { status: 400, body: msg.to_string(), mutation: None }
}

fn not_found() -> IngestOutcome {
    IngestOutcome { status: 404, body: "not found".to_string(), mutation: None }
}

fn accepted(mutation: MetadataMutation) -> IngestOutcome {
    IngestOutcome { status: 204, body: String::new(), mutation: Some(mutation) }
}

/// The JSON route list returned by `GET /` (health check). Ports the TS health response.
pub fn route_list_json() -> String {
    r#"{"routes":["/set-status","/set-progress","/log","/clear-log"]}"#.to_string()
}

/// Handle a request: validate per §5 and return the status/body/mutation. Pure.
pub fn handle_request(method: &str, path: &str, body: &str) -> IngestOutcome {
    if method == "GET" && path == "/" {
        return IngestOutcome { status: 200, body: route_list_json(), mutation: None };
    }
    if method != "POST" {
        return not_found();
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return bad_request("invalid JSON");
    };

    // Every endpoint requires a non-empty string `session`.
    let session = match value.get("session").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return bad_request("session is required"),
    };

    match path {
        "/set-status" => match value.get("text") {
            None | Some(serde_json::Value::Null) => {
                accepted(MetadataMutation::SetStatus { session, text: None, tone: None })
            }
            Some(serde_json::Value::String(text)) => {
                let tone = parse_tone(value.get("tone").and_then(|v| v.as_str()));
                accepted(MetadataMutation::SetStatus { session, text: Some(text.clone()), tone })
            }
            Some(_) => bad_request("text must be a string or null"),
        },
        "/set-progress" => {
            if value.get("clear").and_then(|v| v.as_bool()) == Some(true) {
                accepted(MetadataMutation::SetProgress { session, progress: None })
            } else {
                let progress = ProgressInput {
                    current: value.get("current").and_then(|v| v.as_i64()),
                    total: value.get("total").and_then(|v| v.as_i64()),
                    percent: value.get("percent").and_then(|v| v.as_f64()),
                    label: value.get("label").and_then(|v| v.as_str()).map(String::from),
                };
                accepted(MetadataMutation::SetProgress { session, progress: Some(progress) })
            }
        }
        "/log" => {
            let message = match value.get("message").and_then(|v| v.as_str()) {
                Some(m) if !m.is_empty() => m.to_string(),
                _ => return bad_request("message is required"),
            };
            let tone = parse_tone(value.get("tone").and_then(|v| v.as_str()));
            let source = value.get("source").and_then(|v| v.as_str()).map(String::from);
            accepted(MetadataMutation::AppendLog {
                session,
                log: LogInput { message, tone, source },
            })
        }
        "/clear-log" => accepted(MetadataMutation::ClearLogs { session }),
        _ => not_found(),
    }
}

/// Format a full HTTP/1.1 response with `Connection: close`.
pub fn response_bytes(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_head_with_content_length() {
        let head = "POST /log HTTP/1.1\r\nHost: x\r\nContent-Length: 42\r\n";
        let h = parse_request_head(head).unwrap();
        assert_eq!(h.method, "POST");
        assert_eq!(h.path, "/log");
        assert_eq!(h.content_length, 42);
    }

    #[test]
    fn head_strips_query_and_defaults_length_zero() {
        let h = parse_request_head("GET /?x=1 HTTP/1.1\r\n").unwrap();
        assert_eq!(h.path, "/");
        assert_eq!(h.content_length, 0);
    }

    #[test]
    fn get_root_returns_route_list() {
        let out = handle_request("GET", "/", "");
        assert_eq!(out.status, 200);
        assert!(out.body.contains("/set-status"));
        assert!(out.mutation.is_none());
    }

    #[test]
    fn invalid_json_is_400() {
        let out = handle_request("POST", "/set-status", "not json");
        assert_eq!(out.status, 400);
    }

    #[test]
    fn missing_or_empty_session_is_400() {
        assert_eq!(handle_request("POST", "/log", r#"{"message":"hi"}"#).status, 400);
        assert_eq!(handle_request("POST", "/log", r#"{"session":"","message":"hi"}"#).status, 400);
    }

    #[test]
    fn set_status_string_and_null_and_bad_type() {
        // string → set
        let out =
            handle_request("POST", "/set-status", r#"{"session":"s","text":"hi","tone":"info"}"#);
        assert_eq!(out.status, 204);
        assert_eq!(
            out.mutation,
            Some(MetadataMutation::SetStatus {
                session: "s".into(),
                text: Some("hi".into()),
                tone: Some(MetadataTone::Info),
            })
        );
        // null → clear
        let out = handle_request("POST", "/set-status", r#"{"session":"s","text":null}"#);
        assert_eq!(
            out.mutation,
            Some(MetadataMutation::SetStatus { session: "s".into(), text: None, tone: None })
        );
        // absent text → clear
        let out = handle_request("POST", "/set-status", r#"{"session":"s"}"#);
        assert!(matches!(out.mutation, Some(MetadataMutation::SetStatus { text: None, .. })));
        // non-string text → 400
        assert_eq!(
            handle_request("POST", "/set-status", r#"{"session":"s","text":5}"#).status,
            400
        );
    }

    #[test]
    fn invalid_tone_becomes_none() {
        let out =
            handle_request("POST", "/set-status", r#"{"session":"s","text":"x","tone":"bogus"}"#);
        assert_eq!(
            out.mutation,
            Some(MetadataMutation::SetStatus {
                session: "s".into(),
                text: Some("x".into()),
                tone: None
            })
        );
    }

    #[test]
    fn set_progress_and_clear() {
        let out = handle_request(
            "POST",
            "/set-progress",
            r#"{"session":"s","current":3,"total":10,"percent":30.0,"label":"build"}"#,
        );
        assert_eq!(out.status, 204);
        match out.mutation {
            Some(MetadataMutation::SetProgress { progress: Some(p), .. }) => {
                assert_eq!(p.current, Some(3));
                assert_eq!(p.total, Some(10));
                assert_eq!(p.percent, Some(30.0));
                assert_eq!(p.label.as_deref(), Some("build"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        let out = handle_request("POST", "/set-progress", r#"{"session":"s","clear":true}"#);
        assert_eq!(
            out.mutation,
            Some(MetadataMutation::SetProgress { session: "s".into(), progress: None })
        );
    }

    #[test]
    fn log_requires_message() {
        assert_eq!(handle_request("POST", "/log", r#"{"session":"s"}"#).status, 400);
        assert_eq!(handle_request("POST", "/log", r#"{"session":"s","message":""}"#).status, 400);
        let out = handle_request("POST", "/log", r#"{"session":"s","message":"hi","source":"ci"}"#);
        assert_eq!(out.status, 204);
        assert!(matches!(out.mutation, Some(MetadataMutation::AppendLog { .. })));
    }

    #[test]
    fn clear_log_ok() {
        let out = handle_request("POST", "/clear-log", r#"{"session":"s"}"#);
        assert_eq!(out.status, 204);
        assert_eq!(out.mutation, Some(MetadataMutation::ClearLogs { session: "s".into() }));
    }

    #[test]
    fn unknown_route_and_method() {
        assert_eq!(handle_request("POST", "/bogus", r#"{"session":"s"}"#).status, 404);
        assert_eq!(handle_request("PUT", "/set-status", "").status, 404);
    }

    #[test]
    fn response_formatting() {
        let r = response_bytes(204, "");
        assert!(r.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(r.contains("Content-Length: 0\r\n"));
        assert!(r.ends_with("\r\n\r\n"));
        let r = response_bytes(200, "{}");
        assert!(r.contains("Content-Length: 2\r\n"));
        assert!(r.ends_with("\r\n\r\n{}"));
    }
}
