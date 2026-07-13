mod common;

use common::cli_cmd;
use std::path::Path;
use tempfile::TempDir;

/// A `tt claude-sessions` command with HOME redirected into the sandbox, so it
/// reads the fixture `~/.claude/projects` instead of the real one. stdout is not
/// a TTY under assert_cmd, so auto-open is already suppressed.
fn claude_sessions_cmd(temp: &Path) -> assert_cmd::Command {
    let mut cmd = cli_cmd(temp);
    cmd.env("HOME", temp);
    cmd
}

/// Write a fixture session JSONL under `$HOME/.claude/projects/<project>/<id>.jsonl`.
fn write_session(home: &Path, project: &str, session_id: &str, input: i64, output: i64) {
    let dir = home.join(".claude").join("projects").join(project);
    std::fs::create_dir_all(&dir).unwrap();
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": session_id,
        "timestamp": "2026-07-01T10:00:00.000Z",
        "message": {
            "role": "assistant",
            "model": "claude-opus-4-1",
            "usage": { "input_tokens": input, "output_tokens": output }
        }
    });
    std::fs::write(dir.join(format!("{session_id}.jsonl")), format!("{line}\n")).unwrap();
}

#[test]
fn claude_sessions_json_emits_rows() {
    let temp = TempDir::new().unwrap();
    write_session(
        temp.path(),
        "-home-user-proj",
        "aaaaaaaa-1111-2222-3333-444444444444",
        1000,
        500,
    );

    let assert = claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "--format", "json", "--days", "0"])
        .assert()
        .success();

    let stdout = &assert.get_output().stdout;
    let value: serde_json::Value = serde_json::from_slice(stdout).expect("valid JSON array");
    let rows = value.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["inputTokens"], 1000);
    assert_eq!(rows[0]["outputTokens"], 500);
    assert_eq!(rows[0]["totalTokens"], 1500);
}

#[test]
fn claude_sessions_csv_emits_header_and_row() {
    let temp = TempDir::new().unwrap();
    write_session(temp.path(), "-home-user-proj", "bbbbbbbb-1111-2222-3333-444444444444", 200, 100);

    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "csv", "--days", "0"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "session_path,project,model,input_tokens,output_tokens,total_tokens,cost,date",
        ))
        .stdout(predicates::str::contains("300"));
}

#[test]
fn claude_sessions_md_emits_table_header_and_totals() {
    let temp = TempDir::new().unwrap();
    write_session(temp.path(), "-home-user-proj", "ffffffff-1111-2222-3333-444444444444", 200, 100);

    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "md", "--days", "0"])
        .assert()
        .success()
        .stdout(predicates::str::contains("| Project | Model | Tokens | Est. cost |"))
        .stdout(predicates::str::contains("| **Total** |"));
}

#[test]
fn claude_sessions_md_empty_exits_zero_with_header_and_no_report() {
    let temp = TempDir::new().unwrap();
    // Projects dir exists but is empty: markdown still prints the header and a
    // zeroed totals row, exits 0, and writes no report file.
    std::fs::create_dir_all(temp.path().join(".claude").join("projects")).unwrap();

    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "md"])
        .assert()
        .success()
        .code(0)
        .stdout(predicates::str::contains("| Project | Model | Tokens | Est. cost |"))
        .stdout(predicates::str::contains("| **Total** | | **0** | **$0.00** |"));

    assert!(
        !temp.path().join(".claude").join("reports").exists(),
        "markdown output must not create a reports directory"
    );
}

#[test]
fn claude_sessions_session_not_found_errors() {
    let temp = TempDir::new().unwrap();
    write_session(temp.path(), "-home-user-proj", "cccccccc-1111-2222-3333-444444444444", 10, 5);

    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "json", "-s", "does-not-exist"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("Session does-not-exist not found"));
}

#[test]
fn claude_sessions_no_projects_dir_errors() {
    let temp = TempDir::new().unwrap();
    // No ~/.claude/projects created.
    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "json"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("No Claude projects directory found"));
}

#[test]
fn claude_sessions_no_sessions_errors() {
    let temp = TempDir::new().unwrap();
    // Projects dir exists but is empty.
    std::fs::create_dir_all(temp.path().join(".claude").join("projects")).unwrap();
    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "json"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("No sessions found"));
}

#[test]
fn claude_sessions_html_writes_report_under_reports_dir() {
    let temp = TempDir::new().unwrap();
    write_session(
        temp.path(),
        "-home-user-proj",
        "dddddddd-1111-2222-3333-444444444444",
        1000,
        500,
    );

    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "--days", "0", "--no-open"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Saved to"));

    let reports_dir = temp.path().join(".claude").join("reports");
    let html: Vec<_> = std::fs::read_dir(&reports_dir)
        .expect("reports dir created")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("treemap-all-") && name.ends_with(".html")
        })
        .collect();
    assert_eq!(html.len(), 1, "exactly one treemap-all-*.html report should be written");
}

#[test]
fn claude_sessions_html_single_session_filename_uses_short_id() {
    let temp = TempDir::new().unwrap();
    let id = "eeeeeeee-1111-2222-3333-444444444444";
    write_session(temp.path(), "-home-user-proj", id, 1000, 500);

    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-s", id, "--no-open"])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!("Generating treemap for session {id}")));

    let reports_dir = temp.path().join(".claude").join("reports");
    let found = std::fs::read_dir(&reports_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with("treemap-eeeeeeee-"));
    assert!(found, "report filename should use the 8-char session prefix");
}

#[test]
fn claude_sessions_invalid_format_errors() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join(".claude").join("projects")).unwrap();
    claude_sessions_cmd(temp.path())
        .args(["claude-sessions", "-f", "xml"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("Invalid format \"xml\""));
}
