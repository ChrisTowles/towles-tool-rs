//! Zellij web-client integration. Zellij (>= 0.43) ships a built-in web
//! server (`zellij web`) that serves terminal sessions over HTTP/WebSocket;
//! we host that client in its own webview window instead of embedding a
//! terminal emulator ourselves.

use std::net::SocketAddr;
use std::process::{Command, Output};
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const WEB_URL: &str = "http://127.0.0.1:8082";
const WINDOW_LABEL: &str = "zellij";

/// Open (or focus) the terminal window, starting the local `zellij web`
/// server first if it isn't running. Returns a login token when this call
/// created the first one — zellij shows tokens exactly once, so the UI must
/// surface it for the login form; on later opens the webview's stored session
/// cookie (or a token the user kept) signs in.
#[tauri::command]
pub async fn zellij_open(app: AppHandle) -> Result<Option<String>, String> {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_focus();
        return Ok(None);
    }

    let token = tauri::async_runtime::spawn_blocking(ensure_server_and_token)
        .await
        .map_err(|e| e.to_string())??;

    let url = WEB_URL.parse().map_err(|e| format!("{WEB_URL}: {e}"))?;
    WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::External(url))
        .title("Terminal — zellij")
        .inner_size(1000.0, 700.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(token)
}

/// Make sure the web server is listening and at least one login token exists.
/// Returns the token value only when one was created by this call.
fn ensure_server_and_token() -> Result<Option<String>, String> {
    let token = ensure_token()?;
    if !port_open() {
        let out = zellij(&["web", "--daemonize"])?;
        if !out.status.success() {
            return Err(format!(
                "zellij web failed to start: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        // The daemon reports success before it binds; poll until it's up.
        let deadline = 25;
        if !(0..deadline).any(|_| {
            std::thread::sleep(Duration::from_millis(200));
            port_open()
        }) {
            return Err(format!(
                "zellij web server did not come up on {WEB_URL} — check that \
                 ~/.config/zellij/config.kdl exists (zellij setup --dump-config)"
            ));
        }
    }
    Ok(token)
}

/// Create a login token if none exist yet (`zellij web --list-tokens` can
/// only reveal names, not values, so an existing token is left alone).
fn ensure_token() -> Result<Option<String>, String> {
    let list = zellij(&["web", "--list-tokens"])?;
    if !String::from_utf8_lossy(&list.stdout).trim().is_empty() {
        return Ok(None);
    }
    let out = zellij(&["web", "--create-token"])?;
    if !out.status.success() {
        return Err(format!(
            "failed to create zellij login token: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_token(&String::from_utf8_lossy(&out.stdout))
        .map(Some)
        .ok_or_else(|| "could not parse `zellij web --create-token` output".to_string())
}

/// Extract the token value from create-token output ("token_1: <uuid>").
fn parse_token(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().split_once(": ").map(|(_, t)| t.trim().to_string()))
        .filter(|t| !t.is_empty())
}

fn port_open() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], 8082));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

fn zellij(args: &[&str]) -> Result<Output, String> {
    Command::new("zellij").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "zellij is not installed — see https://zellij.dev/documentation/installation".into()
        } else {
            format!("failed to run zellij: {e}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::parse_token;

    #[test]
    fn parses_create_token_output() {
        let out = "Created token successfully\n\ntoken_1: 6975bbaf-5e0e-403a-ad0e-b127834b7066\n";
        assert_eq!(parse_token(out).as_deref(), Some("6975bbaf-5e0e-403a-ad0e-b127834b7066"));
    }

    #[test]
    fn rejects_unexpected_output() {
        assert_eq!(parse_token("Created token successfully"), None);
        assert_eq!(parse_token(""), None);
    }
}
