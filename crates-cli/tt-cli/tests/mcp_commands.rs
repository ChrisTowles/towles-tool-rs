//! Black-box tests for `tt mcp serve`: a real stdio MCP handshake against the
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

    Command::cargo_bin("tt")
        .unwrap()
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("protocolVersion"))
        .stdout(contains("towles-tool"))
        .stdout(contains("task_list"))
        .stdout(contains("task_status"))
        .stdout(contains("task_create"));
}

#[test]
fn mcp_serve_tools_call_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tt.db");

    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"task_list","arguments":{}}}"#,
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
fn mcp_serve_gates_task_create_off_by_default() {
    // A sandboxed `--config-dir` with no opt-in: the gate refuses with the
    // settings hint before any repo validation or store write happens.
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tt.db");

    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"task_create","arguments":{"repo":"demo","title":"nope"}}}"#,
        "\n",
    );

    Command::cargo_bin("tt")
        .unwrap()
        .args(["--config-dir"])
        .arg(dir.path())
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("mutationsEnabled"))
        .stdout(contains("isError"));
}

#[test]
fn mcp_serve_refuses_removed_tools() {
    // The mutating tool families (2026-07 datamine) and the broad dashboard
    // reads (2026-07 tool-surface review) were removed outright; a straggling
    // client gets a plain unknown-tool refusal.
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tt.db");

    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"day_brief","arguments":{}}}"#,
        "\n",
    );

    Command::cargo_bin("tt")
        .unwrap()
        .args(["mcp", "serve", "--store"])
        .arg(&db)
        .write_stdin(requests)
        .assert()
        .success()
        .stdout(contains("unknown tool"))
        .stdout(contains("isError"));
}
