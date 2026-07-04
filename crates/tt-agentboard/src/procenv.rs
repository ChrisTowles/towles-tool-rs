//! Read a live process's environment to link a detected agent back to the PTY
//! session it runs in. Every agentboard PTY is spawned with `TT_SESSION_ID` set
//! to its session id (see the app's `terminal.rs`); a Claude process launched
//! inside that shell inherits the variable, so reading it back from the agent's
//! process environment tells us exactly which session the agent occupies.
//!
//! Linux reads `/proc/<pid>/environ` (readable for our own uid). Other platforms
//! return `None` for now — a `ps eww` / libproc path is a documented follow-up;
//! the platform-specific surface is confined to [`read_session_id`].

/// The env var injected into every agentboard PTY at spawn, read back here to
/// attribute a detected agent to its session.
pub const TT_SESSION_ENV: &str = "TT_SESSION_ID";

/// The `TT_SESSION_ID` of the PTY `pid` runs in, if it was spawned by us.
#[cfg(target_os = "linux")]
pub fn read_session_id(pid: i32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    read_var_from_environ(&bytes, TT_SESSION_ENV)
}

#[cfg(not(target_os = "linux"))]
pub fn read_session_id(_pid: i32) -> Option<String> {
    // macOS/other: no `/proc`. Follow-up: `ps eww <pid>` or libproc.
    None
}

/// Extract a variable's value from NUL-separated `KEY=VALUE` environ bytes.
/// Pure and unit-tested (the platform-specific part is only the file read).
fn read_var_from_environ(bytes: &[u8], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    bytes.split(|&b| b == 0).find_map(|entry| {
        let s = std::str::from_utf8(entry).ok()?;
        s.strip_prefix(&prefix).map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_var_from_nul_separated_environ() {
        let environ = b"PATH=/usr/bin\0TT_SESSION_ID=s00abc\0SHELL=/bin/zsh\0";
        assert_eq!(read_var_from_environ(environ, TT_SESSION_ENV).as_deref(), Some("s00abc"));
        assert_eq!(read_var_from_environ(environ, "SHELL").as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn missing_var_is_none_and_no_prefix_false_match() {
        // A var whose name merely contains the key must not match.
        let environ = b"NOT_TT_SESSION_ID=x\0OTHER=1\0";
        assert_eq!(read_var_from_environ(environ, TT_SESSION_ENV), None);
    }

    #[test]
    fn empty_value_returns_empty_string() {
        let environ = b"TT_SESSION_ID=\0";
        assert_eq!(read_var_from_environ(environ, TT_SESSION_ENV).as_deref(), Some(""));
    }
}
