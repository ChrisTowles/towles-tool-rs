//! Black-box tests for `ttr mcp serve`: a real stdio MCP handshake against the
//! compiled binary, with the store pointed at a sandbox file.

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

    Command::cargo_bin("ttr")
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

    Command::cargo_bin("ttr")
        .unwrap()
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("tasks"));
}
