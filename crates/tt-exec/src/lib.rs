//! Thin process-execution wrapper for the towles-tool CLI.
//!
//! Ports `src/lib/git/exec.ts` and the `gh` JSON helper from the TypeScript CLI:
//! [`run`] captures stdout/stderr/exit-code without failing, [`run_ok`] fails on a
//! non-zero exit, and [`gh_json`] shells out to `gh` and deserializes its JSON stdout.

use serde::de::DeserializeOwned;
use std::process::Command;
use std::time::Duration;
use thiserror::Error;

/// Env-var name prefixes that identify the running app instance and must not
/// leak into a process spawned inside it: its dev-server port and session /
/// instance stamps (`TT_`, e.g. `TT_DEV_PORT`, `TT_SESSION_ID`,
/// `TT_APP_INSTANCE`), its Tauri build + automation config (`TAURI_`, e.g.
/// `TAURI_CONFIG`, `TAURI_ENV_TARGET_TRIPLE`, `TAURI_ANDROID_*`), and the npm
/// process that launched it (`npm_`, e.g. `npm_config_*`, `npm_lifecycle_*`).
///
/// A shell spawned inside the app that then starts a *nested* app instance
/// (`npm run dev`, `tt-app`) must re-derive its own port and session identity;
/// inheriting the parent's makes the nested instance collide on the parent's
/// port and mis-attribute to the parent's session (issue #39).
pub const APP_INSTANCE_ENV_PREFIXES: &[&str] = &["TT_", "TAURI_", "npm_"];

/// Env vars that stamp a process as living *inside* a Claude Code session.
/// When the app itself was launched from a Claude session (an agent running
/// `npm run dev`), these leak into every terminal the app spawns, and any
/// interactive `claude` started there inherits them. With
/// `CLAUDE_CODE_CHILD_SESSION=1` present, Claude Code treats the session as a
/// nested child and never writes its conversation transcript to
/// `~/.claude/projects/` — the session is unrecoverable after the window dies
/// (verified against Claude Code 2.1.207). The app's terminals host top-level
/// user sessions, never children, so the whole identity set is dropped.
pub const CLAUDE_SESSION_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "AI_AGENT",
];

/// Whether `key` names an env var a spawned process must not inherit: an
/// app-instance var (see [`APP_INSTANCE_ENV_PREFIXES`]) or a Claude-session
/// identity var (see [`CLAUDE_SESSION_ENV_VARS`]).
pub fn is_app_instance_env(key: &str) -> bool {
    APP_INSTANCE_ENV_PREFIXES.iter().any(|prefix| key.starts_with(prefix))
        || CLAUDE_SESSION_ENV_VARS.contains(&key)
}

/// Filter the app-instance env vars out of `env`, returning the pairs a nested
/// process should inherit. Pure and order-preserving; the caller applies the
/// result to the child (see the app's `terminal.rs`). Everything not matched by
/// [`is_app_instance_env`] survives (PATH, HOME, TERM, SHELL, …).
pub fn scrub_app_instance_env<I, K, V>(env: I) -> Vec<(K, V)>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
{
    env.into_iter().filter(|(key, _)| !is_app_instance_env(key.as_ref())).collect()
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to spawn `{cmd}`: {source}")]
    Spawn { cmd: String, source: std::io::Error },

    #[error("Command failed (exit {exit_code}): {cmd}\n{stderr}")]
    NonZeroExit {
        cmd: String,
        exit_code: i32,
        stderr: String,
    },

    #[error("Command timed out after {timeout:?}: {cmd}")]
    Timeout { cmd: String, timeout: Duration },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Captured output of a finished process.
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl Output {
    /// Whether the process exited with status 0.
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

fn display_cmd(cmd: &str, args: &[&str]) -> String {
    if args.is_empty() { cmd.to_string() } else { format!("{cmd} {}", args.join(" ")) }
}

/// Run a command, capturing output. Does not fail on a non-zero exit code.
pub fn run(cmd: &str, args: &[&str]) -> Result<Output> {
    log::debug!("exec: {}", display_cmd(cmd, args));
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|source| Error::Spawn { cmd: cmd.to_string(), source })?;

    Ok(Output {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Run a command with the given string piped to its stdin, capturing output.
/// Does not fail on a non-zero exit code.
pub fn run_with_stdin(cmd: &str, args: &[&str], stdin: &str) -> Result<Output> {
    use std::io::Write;
    use std::process::Stdio;

    log::debug!("exec (stdin {} bytes): {}", stdin.len(), display_cmd(cmd, args));
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn { cmd: cmd.to_string(), source })?;

    if let Some(mut handle) = child.stdin.take() {
        // A closed pipe (child exited early) is not fatal; we still collect output.
        let _ = handle.write_all(stdin.as_bytes());
    }
    let output =
        child.wait_with_output().map_err(|source| Error::Spawn { cmd: cmd.to_string(), source })?;

    Ok(Output {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Run a command, capturing output, but give up after `timeout`. On expiry the
/// child is killed (and reaped) and `Err(Error::Timeout)` is returned so a hung
/// subprocess can't block the caller forever. Does not fail on a non-zero exit.
///
/// stdout/stderr are drained on dedicated threads while the child runs, so a
/// chatty child can't deadlock by filling a pipe the parent isn't reading.
pub fn run_with_timeout(cmd: &str, args: &[&str], timeout: Duration) -> Result<Output> {
    run_with_timeout_in(cmd, args, None, timeout)
}

/// [`run_with_timeout`], but with the child's working directory set to `dir`.
/// For tools like `gh` that resolve their target repo from the cwd.
pub fn run_in_dir_with_timeout(
    cmd: &str,
    args: &[&str],
    dir: &std::path::Path,
    timeout: Duration,
) -> Result<Output> {
    run_with_timeout_in(cmd, args, Some(dir), timeout)
}

fn run_with_timeout_in(
    cmd: &str,
    args: &[&str],
    dir: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<Output> {
    use std::io::Read;
    use std::process::Stdio;
    use wait_timeout::ChildExt;

    log::debug!("exec (timeout {timeout:?}): {}", display_cmd(cmd, args));
    let mut command = Command::new(cmd);
    command.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child =
        command.spawn().map_err(|source| Error::Spawn { cmd: cmd.to_string(), source })?;

    fn drain(reader: Option<impl Read>) -> String {
        let mut buf = Vec::new();
        if let Some(mut reader) = reader {
            let _ = reader.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    let out = child.stdout.take();
    let err = child.stderr.take();
    let out_thread = std::thread::spawn(move || drain(out));
    let err_thread = std::thread::spawn(move || drain(err));

    let status = child
        .wait_timeout(timeout)
        .map_err(|source| Error::Spawn { cmd: cmd.to_string(), source })?;

    let Some(status) = status else {
        // Timed out: kill and reap so we don't leave a zombie. The drain threads
        // then observe EOF on the closed pipes and finish on their own.
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::Timeout { cmd: display_cmd(cmd, args), timeout });
    };

    // The child has exited, so its pipes are closed and the drain threads will
    // return promptly. `join` only errors if a thread panicked — treat that as
    // empty output rather than propagating a panic.
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();

    Ok(Output { stdout, stderr, exit_code: status.code().unwrap_or(-1) })
}

/// Run a command and fail if it exits with a non-zero status.
pub fn run_ok(cmd: &str, args: &[&str]) -> Result<Output> {
    let output = run(cmd, args)?;
    if !output.ok() {
        return Err(Error::NonZeroExit {
            cmd: display_cmd(cmd, args),
            exit_code: output.exit_code,
            stderr: output.stderr,
        });
    }
    Ok(output)
}

/// Shell out to `gh` and deserialize its JSON stdout into `T`.
pub fn gh_json<T: DeserializeOwned>(args: &[&str]) -> Result<T> {
    let output = run_ok("gh", args)?;
    Ok(serde_json::from_str(&output.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_captures_stdout_and_exit_code() {
        let output = run("echo", &["hello"]).unwrap();
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.exit_code, 0);
        assert!(output.ok());
    }

    #[test]
    fn run_ok_fails_on_nonzero_exit() {
        // `false` exits 1 on every unix system.
        let err = run_ok("false", &[]).unwrap_err();
        assert!(matches!(err, Error::NonZeroExit { exit_code: 1, .. }));
    }

    #[test]
    fn run_reports_spawn_failure_for_missing_binary() {
        let err = run("definitely-not-a-real-binary-xyz", &[]).unwrap_err();
        assert!(matches!(err, Error::Spawn { .. }));
    }

    #[test]
    fn run_with_stdin_pipes_input() {
        let output = run_with_stdin("cat", &[], "piped input").unwrap();
        assert_eq!(output.stdout, "piped input");
        assert!(output.ok());
    }

    #[test]
    fn run_with_stdin_survives_child_ignoring_stdin() {
        let output = run_with_stdin("echo", &["ok"], "ignored").unwrap();
        assert_eq!(output.stdout.trim(), "ok");
    }

    #[test]
    fn run_with_timeout_kills_a_slow_child() {
        let start = std::time::Instant::now();
        let err = run_with_timeout("sleep", &["5"], Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }));
        // The kill must happen near the timeout, not after the full 5s sleep.
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn run_with_timeout_returns_output_when_fast() {
        let output = run_with_timeout("echo", &["hi"], Duration::from_secs(5)).unwrap();
        assert_eq!(output.stdout.trim(), "hi");
        assert_eq!(output.exit_code, 0);
        assert!(output.ok());
    }

    #[test]
    fn run_with_timeout_reports_spawn_failure_for_missing_binary() {
        let err = run_with_timeout("definitely-not-a-real-binary-xyz", &[], Duration::from_secs(5))
            .unwrap_err();
        assert!(matches!(err, Error::Spawn { .. }));
    }

    #[test]
    fn run_in_dir_with_timeout_sets_cwd() {
        let dir = std::env::temp_dir();
        let output = run_in_dir_with_timeout("pwd", &[], &dir, Duration::from_secs(5)).unwrap();
        // Canonicalize both sides: temp_dir is often a symlink (e.g. /tmp → /private/tmp).
        let reported = std::fs::canonicalize(output.stdout.trim()).unwrap();
        assert_eq!(reported, std::fs::canonicalize(&dir).unwrap());
    }

    #[test]
    fn app_instance_env_prefixes_are_stripped() {
        // Every documented app-instance var (issue #39) must be recognized.
        for key in [
            "TT_DEV_PORT",
            "TT_SESSION_ID",
            "TT_APP_INSTANCE",
            "TT_E2E_WEBDRIVER_PORT",
            "TAURI_CONFIG",
            "TAURI_ENV_TARGET_TRIPLE",
            "TAURI_ANDROID_HOME",
            "TAURI_WEBVIEW_AUTOMATION",
            "npm_config_registry",
            "npm_lifecycle_event",
            "npm_package_name",
        ] {
            assert!(is_app_instance_env(key), "{key} should be stripped");
        }
    }

    #[test]
    fn ordinary_env_survives() {
        // Vars a shell needs, plus look-alikes that merely contain a prefix
        // (not at the start), must all survive.
        for key in [
            "PATH",
            "HOME",
            "TERM",
            "SHELL",
            "USER",
            "LANG",
            "PWD",
            "MY_TT_VAR",                    // prefix not at the start
            "NOTAURI",                      // prefix not at the start
            "SNAP_npm_x",                   // prefix not at the start
            "TTY",                          // "TT" without the underscore
            "TAURITE",                      // "TAURI" without the underscore
            "CLAUDE_CODE_ENABLE_TELEMETRY", // user config, not session identity
            "CLAUDE_EFFORT",                // user config, not session identity
        ] {
            assert!(!is_app_instance_env(key), "{key} should survive");
        }
    }

    #[test]
    fn claude_session_identity_vars_are_scrubbed() {
        for key in CLAUDE_SESSION_ENV_VARS {
            assert!(is_app_instance_env(key), "{key} should be scrubbed");
        }
    }

    #[test]
    fn scrub_keeps_survivors_and_order_and_drops_instance_vars() {
        let env = vec![
            ("PATH", "/usr/bin"),
            ("TT_DEV_PORT", "1440"),
            ("HOME", "/home/me"),
            ("TAURI_CONFIG", "{}"),
            ("TT_SESSION_ID", "s281e9dda73868f6f"),
            ("TERM", "xterm-256color"),
            ("npm_config_registry", "https://reg"),
        ];
        let scrubbed = scrub_app_instance_env(env);
        assert_eq!(
            scrubbed,
            vec![
                ("PATH", "/usr/bin"),
                ("HOME", "/home/me"),
                ("TERM", "xterm-256color")
            ]
        );
    }

    #[test]
    fn scrub_accepts_owned_pairs() {
        // The app passes owned Strings from the inherited env.
        let env = vec![
            ("TT_DEV_PORT".to_string(), "1440".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ];
        let scrubbed = scrub_app_instance_env(env);
        assert_eq!(scrubbed, vec![("PATH".to_string(), "/usr/bin".to_string())]);
    }

    #[test]
    fn gh_json_parses_echoed_json() {
        // Exercise the JSON path via `echo` rather than depending on `gh` being installed.
        let output = run_ok("echo", &[r#"{"count":3}"#]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["count"], 3);
    }
}
