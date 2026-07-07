mod common;

use common::cli_cmd;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn schema_emits_valid_json() {
    let temp_dir = TempDir::new().expect("temp dir");
    let assert = cli_cmd(temp_dir.path()).args(["config", "schema"]).assert().success();

    let stdout = &assert.get_output().stdout;
    let value: serde_json::Value =
        serde_json::from_slice(stdout).expect("config schema should emit valid JSON");
    assert!(value["properties"].get("preferredEditor").is_some());
    assert!(value["properties"].get("journalSettings").is_some());
}

#[test]
fn show_works_with_tempdir_config() {
    let temp_dir = TempDir::new().expect("temp dir");

    // First run creates the settings file from defaults, then prints it.
    let assert = cli_cmd(temp_dir.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(contains("towles-tool.settings.json"))
        .stdout(contains("\"preferredEditor\""));

    // The file the CLI reported should now exist on disk.
    let settings_path = temp_dir.path().join("towles-tool.settings.json");
    assert!(settings_path.exists(), "config show should create the settings file");

    // Its contents should parse as our settings model.
    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["preferredEditor"], "code");

    let _ = assert;
}

#[test]
fn validate_reports_missing_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    // Point at a config dir with no settings file yet.
    cli_cmd(temp_dir.path())
        .args(["config", "validate"])
        .assert()
        .failure()
        .stderr(contains("not found"));
}

#[test]
fn reset_requires_confirm() {
    let temp_dir = TempDir::new().expect("temp dir");
    let settings_path = temp_dir.path().join("towles-tool.settings.json");
    std::fs::write(&settings_path, r#"{"preferredEditor":"vim"}"#).unwrap();

    // Without --confirm, reset should refuse and exit non-zero.
    cli_cmd(temp_dir.path())
        .args(["config", "reset"])
        .assert()
        .failure()
        .stdout(contains("--confirm"));

    // With --confirm, it rewrites defaults.
    cli_cmd(temp_dir.path()).args(["config", "reset", "--confirm"]).assert().success();

    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["preferredEditor"], "code");
}

#[test]
fn reset_preserves_unknown_keys() {
    let temp_dir = TempDir::new().expect("temp dir");
    let settings_path = temp_dir.path().join("towles-tool.settings.json");
    // The settings file is shared with the TypeScript CLI, which owns keys this
    // model doesn't capture. Reset must not nuke them.
    std::fs::write(
        &settings_path,
        r#"{"preferredEditor":"vim","tsOnlyFlag":{"a":1},"anotherTsKey":true}"#,
    )
    .unwrap();

    cli_cmd(temp_dir.path()).args(["config", "reset", "--confirm"]).assert().success();

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    // Known fields are reset to defaults.
    assert_eq!(value["preferredEditor"], "code");
    // Unknown keys owned by the other tool survive.
    assert_eq!(value["tsOnlyFlag"], serde_json::json!({ "a": 1 }));
    assert_eq!(value["anotherTsKey"], true);
}
