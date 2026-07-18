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

/// Env vars that make `git` fail fast instead of blocking on an interactive
/// credential/SSH prompt. Without these, a missing credential-helper entry or
/// an SSH key needing a passphrase can pop a GUI prompt (e.g. macOS Keychain
/// "Allow Access") behind the app window; the git process then blocks reading
/// stdin until [`run_with_timeout`]'s kill fires, stalling the caller for the
/// full timeout instead of failing immediately with a clear error.
pub const GIT_NON_INTERACTIVE_ENV: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=10"),
];

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

/// Open the telemetry span covering one subprocess.
///
/// Every process this crate spawns goes through here, which makes the event
/// log a complete record of what the tool shelled out to — the question
/// "what ran, from where, how often, and how long did it take?" is answerable
/// without adding instrumentation at each call site. `outcome` and `exit_code`
/// start empty and are filled in before the span closes, so a single record
/// carries both the command and its result.
///
/// Field names follow OpenTelemetry's `process.*` semantic conventions.
///
/// Only argv is recorded, never stdin or captured output: stdin carries things
/// like PR bodies, and stdout is unbounded. Argv for the tools this crate
/// spawns (`gh`, `git`, `claude`) holds no credentials — tokens travel via
/// settings and env, not flags.
fn spawn_span(
    cmd: &str,
    args: &[&str],
    dir: Option<&std::path::Path>,
    timeout: Option<Duration>,
) -> tracing::Span {
    tracing::debug_span!(
        "process.spawn",
        "process.executable.name" = cmd,
        "process.command_args" = args.join(" "),
        "process.working_directory" = dir.map(|d| d.display().to_string()).unwrap_or_default(),
        timeout_ms = timeout.map(|t| t.as_millis() as u64),
        exit_code = tracing::field::Empty,
        outcome = tracing::field::Empty,
        stdin_bytes = tracing::field::Empty,
    )
}

/// Close out a span for a process that ran to completion.
fn record_exit(span: &tracing::Span, exit_code: i32) {
    span.record("exit_code", exit_code);
    span.record("outcome", if exit_code == 0 { "ok" } else { "non_zero_exit" });
}

/// Stamp `outcome` on a span whose process never produced an exit code, and
/// build the matching error. The single home for the failure vocabulary, so
/// adding an outcome or renaming the field is one edit rather than one per
/// spawn site.
fn spawn_error(span: &tracing::Span, outcome: &str, cmd: &str, source: std::io::Error) -> Error {
    span.record("outcome", outcome);
    Error::Spawn { cmd: cmd.to_string(), source }
}

/// Record a process this crate does *not* run to completion: a PTY shell, a
/// long-lived language server, a detached editor. Those have a different
/// lifecycle than [`run`] and friends — there is no exit code to wait for —
/// so they can't use the span-per-call shape, but they still belong in the
/// event log, which is what makes "what did the app launch?" answerable.
///
/// Emits a single event rather than a span, since there is no duration to
/// close over. `kind` names the launch shape (`"pty"`, `"lsp"`, `"editor"`).
pub fn record_detached_spawn(cmd: &str, args: &[&str], kind: &str) {
    tracing::debug!(
        "process.executable.name" = cmd,
        "process.command_args" = args.join(" "),
        outcome = "detached",
        launch_kind = kind,
        "spawned detached process"
    );
}

/// Run a command, capturing output. Does not fail on a non-zero exit code.
pub fn run(cmd: &str, args: &[&str]) -> Result<Output> {
    let span = spawn_span(cmd, args, None, None);
    let _entered = span.enter();

    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|source| spawn_error(&span, "spawn_failed", cmd, source))?;

    let exit_code = output.status.code().unwrap_or(-1);
    record_exit(&span, exit_code);
    Ok(Output {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code,
    })
}

/// Run a command with the given string piped to its stdin, capturing output.
/// Does not fail on a non-zero exit code.
pub fn run_with_stdin(cmd: &str, args: &[&str], stdin: &str) -> Result<Output> {
    use std::io::Write;
    use std::process::Stdio;

    let span = spawn_span(cmd, args, None, None);
    span.record("stdin_bytes", stdin.len() as u64);
    let _entered = span.enter();

    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| spawn_error(&span, "spawn_failed", cmd, source))?;

    if let Some(mut handle) = child.stdin.take() {
        // A closed pipe (child exited early) is not fatal; we still collect output.
        let _ = handle.write_all(stdin.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|source| spawn_error(&span, "wait_failed", cmd, source))?;

    let exit_code = output.status.code().unwrap_or(-1);
    record_exit(&span, exit_code);
    Ok(Output {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code,
    })
}

/// Run a command, capturing output, but give up after `timeout`. On expiry the
/// child is killed (and reaped) and `Err(Error::Timeout)` is returned so a hung
/// subprocess can't block the caller forever. Does not fail on a non-zero exit.
///
/// stdout/stderr are drained on dedicated threads while the child runs, so a
/// chatty child can't deadlock by filling a pipe the parent isn't reading.
pub fn run_with_timeout(cmd: &str, args: &[&str], timeout: Duration) -> Result<Output> {
    run_with_timeout_in(cmd, args, None, &[], timeout)
}

/// [`run_with_timeout`], but with the child's working directory set to `dir`.
/// For tools like `gh` that resolve their target repo from the cwd.
pub fn run_in_dir_with_timeout(
    cmd: &str,
    args: &[&str],
    dir: &std::path::Path,
    timeout: Duration,
) -> Result<Output> {
    run_with_timeout_in(cmd, args, Some(dir), &[], timeout)
}

/// [`run_with_timeout`], with extra env vars set on the child (e.g.
/// [`GIT_NON_INTERACTIVE_ENV`]).
pub fn run_with_timeout_env(
    cmd: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output> {
    run_with_timeout_in(cmd, args, None, env, timeout)
}

/// [`run_in_dir_with_timeout`], with extra env vars set on the child (e.g.
/// [`GIT_NON_INTERACTIVE_ENV`]).
pub fn run_in_dir_with_timeout_env(
    cmd: &str,
    args: &[&str],
    dir: &std::path::Path,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output> {
    run_with_timeout_in(cmd, args, Some(dir), env, timeout)
}

fn run_with_timeout_in(
    cmd: &str,
    args: &[&str],
    dir: Option<&std::path::Path>,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output> {
    use std::io::Read;
    use std::process::Stdio;
    use wait_timeout::ChildExt;

    let span = spawn_span(cmd, args, dir, Some(timeout));
    let _entered = span.enter();

    let mut command = Command::new(cmd);
    command.args(args).envs(env.iter().copied()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child =
        command.spawn().map_err(|source| spawn_error(&span, "spawn_failed", cmd, source))?;

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
        .map_err(|source| spawn_error(&span, "wait_failed", cmd, source))?;

    let Some(status) = status else {
        // Timed out: kill and reap so we don't leave a zombie. The drain threads
        // then observe EOF on the closed pipes and finish on their own.
        let _ = child.kill();
        let _ = child.wait();
        span.record("outcome", "timed_out");
        return Err(Error::Timeout { cmd: display_cmd(cmd, args), timeout });
    };

    // The child has exited, so its pipes are closed and the drain threads will
    // return promptly. `join` only errors if a thread panicked — treat that as
    // empty output rather than propagating a panic.
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();

    let exit_code = status.code().unwrap_or(-1);
    record_exit(&span, exit_code);
    Ok(Output { stdout, stderr, exit_code })
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
    fn run_with_timeout_env_sets_env_vars() {
        let output = run_with_timeout_env(
            "sh",
            &["-c", "echo $FOO"],
            &[("FOO", "bar")],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.stdout.trim(), "bar");
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

    /// Run `body` with an event-log-backed subscriber installed, returning the
    /// `process.spawn` records it produced. A local subscriber, so these tests
    /// don't fight over the global one.
    fn spawn_records(body: impl FnOnce()) -> Vec<serde_json::Value> {
        use tracing_subscriber::prelude::*;

        let dir = tempfile::tempdir().unwrap();
        let layer = tt_otel::EventLogLayer::new(
            tt_otel::EventLog::new(dir.path(), 7),
            serde_json::Map::new(),
        );
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), body);

        let mut records = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            for line in text.lines() {
                let record: serde_json::Value = serde_json::from_str(line).unwrap();
                if record["name"] == "process.spawn" {
                    records.push(record);
                }
            }
        }
        records
    }

    #[test]
    fn every_spawned_command_reaches_the_event_log() {
        let records = spawn_records(|| {
            run("echo", &["hello"]).unwrap();
        });

        assert_eq!(records.len(), 1, "one record per spawn");
        assert_eq!(records[0]["process.executable.name"], "echo");
        assert_eq!(records[0]["process.command_args"], "hello");
        assert_eq!(records[0]["exit_code"], 0);
        assert_eq!(records[0]["outcome"], "ok");
        assert!(records[0]["duration_ms"].is_u64());
    }

    #[test]
    fn the_log_records_a_non_zero_exit() {
        let records = spawn_records(|| {
            run("sh", &["-c", "exit 3"]).unwrap();
        });

        assert_eq!(records[0]["exit_code"], 3);
        assert_eq!(records[0]["outcome"], "non_zero_exit");
    }

    #[test]
    fn the_log_records_the_working_directory_for_dir_scoped_calls() {
        let dir = tempfile::tempdir().unwrap();
        let records = spawn_records(|| {
            run_in_dir_with_timeout("pwd", &[], dir.path(), Duration::from_secs(10)).unwrap();
        });

        // The cwd is what attributes a `gh` call to a specific checkout, which
        // is the whole point of recording it.
        assert_eq!(records[0]["process.working_directory"], dir.path().display().to_string());
        assert_eq!(records[0]["timeout_ms"], 10_000);
    }

    #[test]
    fn a_timeout_is_recorded_as_its_own_outcome() {
        let records = spawn_records(|| {
            let result = run_with_timeout("sleep", &["5"], Duration::from_millis(50));
            assert!(matches!(result, Err(Error::Timeout { .. })));
        });

        assert_eq!(records[0]["outcome"], "timed_out");
        assert!(records[0]["exit_code"].is_null(), "a killed process has no exit code");
    }

    #[test]
    fn a_failed_spawn_is_recorded_rather_than_going_dark() {
        let records = spawn_records(|| {
            let result = run("tt-no-such-binary-exists", &[]);
            assert!(matches!(result, Err(Error::Spawn { .. })));
        });

        assert_eq!(records[0]["outcome"], "spawn_failed");
    }
}
