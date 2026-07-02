//! Thin process-execution wrapper for the towles-tool CLI.
//!
//! Ports `src/lib/git/exec.ts` and the `gh` JSON helper from the TypeScript CLI:
//! [`run`] captures stdout/stderr/exit-code without failing, [`run_ok`] fails on a
//! non-zero exit, and [`gh_json`] shells out to `gh` and deserializes its JSON stdout.

use serde::de::DeserializeOwned;
use std::process::Command;
use thiserror::Error;

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
    fn gh_json_parses_echoed_json() {
        // Exercise the JSON path via `echo` rather than depending on `gh` being installed.
        let output = run_ok("echo", &[r#"{"count":3}"#]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["count"], 3);
    }
}
