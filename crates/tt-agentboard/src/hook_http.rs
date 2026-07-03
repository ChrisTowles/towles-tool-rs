//! Parsing for the tmux-hook HTTP POST bodies (pure; the server layer owns
//! sockets). Ports `unquoteBody`/`parseContext`/`parseResizeContext` from
//! slot-1 `server/index.ts`.
//!
//! Hook bodies are pipe-delimited tmux format expansions, e.g.
//! `#{q:client_tty}|#{q:session_name}|#{q:window_id}` — `#{q:...}` may leave
//! surrounding quotes, hence the unquoting.

use crate::sidebar_width_sync::SidebarResizeContext;

/// tmux context carried by focus/ensure-sidebar/toggle/switch-index bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookContext {
    pub client_tty: Option<String>,
    pub session: String,
    pub window_id: String,
}

/// Strip surrounding whitespace and quote characters from a POST body.
pub fn unquote_body(body: &str) -> &str {
    body.trim().trim_matches('"').trim_matches('\'')
}

/// Parse `clientTty|session|windowId` (or legacy `session:windowId`).
pub fn parse_context(body: &str) -> Option<HookContext> {
    let trimmed = unquote_body(body);

    let pipe_parts: Vec<&str> = trimmed.split('|').collect();
    if pipe_parts.len() == 3 && !pipe_parts[1].is_empty() && !pipe_parts[2].is_empty() {
        return Some(HookContext {
            client_tty: (!pipe_parts[0].is_empty()).then(|| pipe_parts[0].to_string()),
            session: pipe_parts[1].to_string(),
            window_id: pipe_parts[2].to_string(),
        });
    }

    // Legacy format: "session:windowId".
    let colon_idx = trimmed.find(':')?;
    if colon_idx < 1 {
        return None;
    }
    let session = &trimmed[..colon_idx];
    let window_id = &trimmed[colon_idx + 1..];
    if session.is_empty() || window_id.is_empty() {
        return None;
    }
    Some(HookContext {
        client_tty: None,
        session: session.to_string(),
        window_id: window_id.to_string(),
    })
}

/// Parse `paneId|session|windowId|paneWidth|windowWidth` from an
/// `after-resize-pane` hook body.
pub fn parse_resize_context(body: &str) -> Option<SidebarResizeContext> {
    let trimmed = unquote_body(body);
    if trimmed.is_empty() {
        return None;
    }

    let mut parts = trimmed.split('|');
    let pane_id = parts.next().unwrap_or("");
    if pane_id.is_empty() {
        return None;
    }
    let session_name = parts.next().unwrap_or("");
    let window_id = parts.next().unwrap_or("");
    let width = parts.next().and_then(|s| s.parse().ok());
    let window_width = parts.next().and_then(|s| s.parse().ok());

    Some(SidebarResizeContext {
        pane_id: Some(pane_id.to_string()),
        session_name: (!session_name.is_empty()).then(|| session_name.to_string()),
        window_id: (!window_id.is_empty()).then(|| window_id.to_string()),
        width,
        window_width,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquotes_whitespace_and_quotes() {
        assert_eq!(unquote_body("  \"a|b|c\"  "), "a|b|c");
        assert_eq!(unquote_body("'x'"), "x");
        assert_eq!(unquote_body("plain"), "plain");
    }

    #[test]
    fn parses_pipe_context() {
        let ctx = parse_context("/dev/pts/3|main|@2").unwrap();
        assert_eq!(ctx.client_tty.as_deref(), Some("/dev/pts/3"));
        assert_eq!(ctx.session, "main");
        assert_eq!(ctx.window_id, "@2");
        // Empty tty slot → None.
        let ctx = parse_context("|main|@2").unwrap();
        assert_eq!(ctx.client_tty, None);
    }

    #[test]
    fn parses_legacy_colon_context() {
        let ctx = parse_context("main:@2").unwrap();
        assert_eq!(ctx.client_tty, None);
        assert_eq!(ctx.session, "main");
        assert_eq!(ctx.window_id, "@2");
        assert_eq!(parse_context(":@2"), None);
        assert_eq!(parse_context("main:"), None);
        assert_eq!(parse_context("garbage"), None);
    }

    #[test]
    fn missing_pipe_fields_reject() {
        assert_eq!(parse_context("tty||@2"), None);
        assert_eq!(parse_context("tty|main|"), None);
    }

    #[test]
    fn parses_resize_context() {
        let ctx = parse_resize_context("%5|main|@2|40|160").unwrap();
        assert_eq!(ctx.pane_id.as_deref(), Some("%5"));
        assert_eq!(ctx.session_name.as_deref(), Some("main"));
        assert_eq!(ctx.window_id.as_deref(), Some("@2"));
        assert_eq!(ctx.width, Some(40));
        assert_eq!(ctx.window_width, Some(160));
    }

    #[test]
    fn resize_context_tolerates_missing_tail() {
        let ctx = parse_resize_context("%5").unwrap();
        assert_eq!(ctx.pane_id.as_deref(), Some("%5"));
        assert_eq!(ctx.session_name, None);
        assert_eq!(ctx.width, None);
        // Non-numeric widths → None fields, not a parse failure.
        let ctx = parse_resize_context("%5|s|@1|x|y").unwrap();
        assert_eq!(ctx.width, None);
        assert_eq!(ctx.window_width, None);
        assert_eq!(parse_resize_context(""), None);
        assert_eq!(parse_resize_context("\"\""), None);
    }
}
