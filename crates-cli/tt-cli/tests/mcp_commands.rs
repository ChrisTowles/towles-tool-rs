//! Black-box tests for `tt mcp serve`: a real stdio MCP handshake against the
//! compiled binary, with the store pointed at a sandbox file. Tests that touch
//! the capability gate go through `common::cli_cmd` so the server's per-call
//! settings reads resolve inside the sandbox, never the real
//! `~/.config/towles-tool` (the repo rule: tests never touch real settings).

mod common;

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn mcp_serve_handshake_and_tool_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tt.db");

    // initialize, the initialized notification, then tools/list; closing stdin
    // ends the loop cleanly (exit 0).
    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
    );

    Command::cargo_bin("tt")
        .unwrap()
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("protocolVersion"))
        .stdout(contains("towles-tool"))
        .stdout(contains("calendar_today"))
        .stdout(contains("journal_append"))
        .stdout(contains("agent_sessions"));
}

#[test]
fn mcp_serve_tools_call_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tt.db");

    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tasks_open","arguments":{}}}"#,
        "\n",
    );

    Command::cargo_bin("tt")
        .unwrap()
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("tasks"));
}

#[test]
fn mcp_serve_gates_mutating_tools_from_the_config_dir_settings() {
    // The production capability-gate path: no injected override, the server
    // resolves the flags from the settings file per call — pointed into the
    // sandbox by the global --config-dir flag.
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tt.db");
    let config_dir = dir.path().join("config");

    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"task_create","arguments":{"title":"gated"}}}"#,
        "\n",
    );

    // Default posture (no settings file yet): the mutating tool is refused
    // with the actionable opt-in message, as an isError tool result.
    common::cli_cmd(&config_dir)
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("mutationsEnabled"))
        .stdout(contains("isError"));

    // Opting in via the settings file the gate reads takes effect on the next
    // serve run — no override involved, the real disk-resolution branch.
    let settings_path = config_dir.join(format!("{}.settings.json", tt_config::TOOL_NAME));
    std::fs::write(&settings_path, r#"{ "mcp": { "mutationsEnabled": true } }"#).unwrap();
    common::cli_cmd(&config_dir)
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains(r#"\"text\": \"gated\""#));
}
