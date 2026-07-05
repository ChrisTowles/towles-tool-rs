//! Read a live process's environment to link a detected agent back to the PTY
//! session it runs in. Every agentboard PTY is spawned with `TT_SESSION_ID` set
//! to its session id (see the app's `terminal.rs`); a Claude process launched
//! inside that shell inherits the variable, so reading it back from the agent's
//! process environment tells us exactly which session the agent occupies.
//!
//! Linux reads `/proc/<pid>/environ` (readable for our own uid). Other platforms
//! return `None` for now — a `ps eww` / libproc path is a documented follow-up;
//! the platform-specific surface is confined to [`read_session_id`].

use std::path::PathBuf;

/// The env var injected into every agentboard PTY at spawn, read back here to
/// attribute a detected agent to its session.
pub const TT_SESSION_ENV: &str = "TT_SESSION_ID";

/// A live `claude` process discovered by scanning `/proc`, tagged with the
/// `TT_SESSION_ID` of the PTY it runs in and the transcript it has open. Lets
/// the engine surface an app-spawned agent even when `claude agents --all
/// --json` fails to enumerate it (e.g. a `--chrome` interactive session) — the
/// robust realization of the env-var linkage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAgentProc {
    pub session_id: String,
    pub pid: i32,
    pub transcript: Option<PathBuf>,
}

/// Scan `/proc` for live `claude` processes carrying `TT_SESSION_ID`, one entry
/// per matching process (the shell + MCP children carry the var too but are
/// filtered out by process name). Linux-only; empty elsewhere.
#[cfg(target_os = "linux")]
pub fn scan_session_agents() -> Vec<SessionAgentProc> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        // Only the Claude process itself, not the shell or the MCP children it
        // spawned (they all inherit the env var). `comm` is the truncated name.
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm.trim() != "claude" {
            continue;
        }
        match read_session_id(pid) {
            Some(sid) if !sid.is_empty() => {
                out.push(SessionAgentProc {
                    session_id: sid,
                    pid,
                    transcript: open_transcript(pid),
                });
            }
            _ => {}
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
pub fn scan_session_agents() -> Vec<SessionAgentProc> {
    Vec::new()
}

/// The Claude session transcript (`…/<uuid>.jsonl`) the process has open via
/// `/proc/<pid>/fd`, skipping subagent transcripts. Used to derive the task
/// name + status for an agent the CLI snapshot didn't report.
#[cfg(target_os = "linux")]
fn open_transcript(pid: i32) -> Option<PathBuf> {
    let fd_dir = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    fd_dir.flatten().find_map(|e| {
        let target = std::fs::read_link(e.path()).ok()?;
        let s = target.to_string_lossy();
        let ok =
            s.ends_with(".jsonl") && s.contains("/.claude/projects/") && !s.contains("/subagents/");
        ok.then_some(target)
    })
}

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
