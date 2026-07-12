//! Black-box tests for `ttr collect` (assert_cmd).

mod common;

use common::cli_cmd;
use predicates::prelude::*;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn collect_help_lists_subcommands() {
    let dir = TempDir::new().unwrap();
    cli_cmd(dir.path()).args(["collect", "--help"]).assert().success().stdout(
        predicate::str::contains("calendar")
            .and(predicate::str::contains("issues"))
            .and(predicate::str::contains("prs"))
            .and(predicate::str::contains("all")),
    );
}

/// Seed a tt.db at the store's default location inside `home` (mirrors the
/// `XDG_DATA_HOME` layout the CLI subprocess resolves), then record two runs.
fn seed_store(home: &Path, now: i64) {
    let db = home.join("data").join(tt_config::TOOL_NAME).join("tt.db");
    let store = tt_store::Store::open(&db).unwrap();
    store.record_run("issues", true, Some("3 open"), now - 60_000).unwrap();
    store.record_run("prs", false, Some("gh failed"), now - 120_000).unwrap();
}

#[test]
fn collect_status_reports_health_and_never_run() {
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config").join("towles-tool");
    let now = 1_000_000_000_000_i64;
    seed_store(home.path(), now);

    cli_cmd(&config_dir)
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["collect", "status"])
        .assert()
        .success()
        // issues ran ok; prs failed; calendar/slack never ran.
        .stdout(
            predicate::str::contains("issues")
                .and(predicate::str::contains("ok"))
                .and(predicate::str::contains("prs"))
                .and(predicate::str::contains("FAIL"))
                .and(predicate::str::contains("gh failed"))
                .and(predicate::str::contains("claude:calendar"))
                .and(predicate::str::contains("never")),
        );
}

#[test]
fn collect_status_json_parses() {
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config").join("towles-tool");
    let now = 1_000_000_000_000_i64;
    seed_store(home.path(), now);

    let output = cli_cmd(&config_dir)
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["collect", "status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rows: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 4);

    let issues = rows.iter().find(|r| r["collector"] == "issues").unwrap();
    assert_eq!(issues["ok"], serde_json::json!(true));
    assert!(issues["ageMs"].is_number());

    // A collector that never ran omits ageMs/ok entirely.
    let calendar = rows.iter().find(|r| r["collector"] == "claude:calendar").unwrap();
    assert!(calendar["ageMs"].is_null());
    assert!(calendar["ok"].is_null());
}

#[test]
fn collect_prs_with_no_repos_exits_cleanly() {
    // Isolate HOME/XDG so the store and repos config resolve inside the sandbox.
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config").join("towles-tool");

    cli_cmd(&config_dir)
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .args(["collect", "prs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no repos configured"));
}
