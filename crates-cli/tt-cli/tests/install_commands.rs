mod common;

use common::cli_cmd;
use predicates::prelude::PredicateBooleanExt;
use tempfile::TempDir;

/// A `ttr install` command sandboxed so it can't touch the real `~/.claude` or run
/// real `claude` subcommands: HOME points into a tempdir, and PATH is emptied so the
/// `claude` binary isn't found (list/marketplace fail gracefully, and the non-TTY
/// guard skips the interactive install).
fn install_cmd(temp: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = cli_cmd(temp);
    cmd.env("HOME", temp).env("PATH", temp.join("no-bins"));
    cmd
}

fn settings_path(temp: &std::path::Path) -> std::path::PathBuf {
    temp.join(".claude").join("settings.json")
}

#[test]
fn install_writes_recommended_settings() {
    let temp_dir = TempDir::new().expect("temp dir");

    install_cmd(temp_dir.path())
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Set cleanupPeriodDays: 99999"))
        .stdout(predicates::str::contains("Set alwaysThinkingEnabled: true"))
        .stdout(predicates::str::contains("Could not list Claude plugins"))
        .stdout(predicates::str::contains("skipped (non-interactive)"))
        // The MCP server is registered the same way: non-interactive => skipped,
        // and with PATH emptied `claude mcp list` can't run so it can't be found.
        .stdout(predicates::str::contains("Could not list Claude MCP servers"))
        .stdout(predicates::str::contains("tt MCP server skipped (non-interactive)"));

    let content =
        std::fs::read_to_string(settings_path(temp_dir.path())).expect("settings written");
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["cleanupPeriodDays"], serde_json::json!(99999));
    assert_eq!(value["alwaysThinkingEnabled"], serde_json::json!(true));
}

#[test]
fn install_is_idempotent_and_preserves_unknown_fields() {
    let temp_dir = TempDir::new().expect("temp dir");
    let path = settings_path(temp_dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"cleanupPeriodDays":99999,"alwaysThinkingEnabled":true,"customField":"keep me"}"#,
    )
    .unwrap();

    install_cmd(temp_dir.path())
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("cleanupPeriodDays already set to 99999"))
        .stdout(predicates::str::contains("alwaysThinkingEnabled already set to true"))
        .stdout(predicates::str::contains("Saved Claude settings").not());

    // Unknown field survives the round trip.
    let content = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["customField"], serde_json::json!("keep me"));
}

#[test]
fn install_observability_shows_otel_instructions() {
    let temp_dir = TempDir::new().expect("temp dir");
    install_cmd(temp_dir.path())
        .args(["install", "--observability"])
        .assert()
        .success()
        .stdout(predicates::str::contains("CLAUDE_CODE_ENABLE_TELEMETRY=1"))
        .stdout(predicates::str::contains("ccusage"));
}
