mod common;

use common::cli_cmd;
use tempfile::TempDir;

#[test]
fn doctor_json_emits_parseable_json() {
    let temp_dir = TempDir::new().expect("temp dir");
    let assert = cli_cmd(temp_dir.path()).args(["doctor", "--json"]).assert().success();

    let stdout = &assert.get_output().stdout;
    let value: serde_json::Value =
        serde_json::from_slice(stdout).expect("doctor --json should emit valid JSON");

    // The report always lists the tools it probed, regardless of what's installed.
    let tools = value["tools"].as_array().expect("tools should be an array");
    assert!(!tools.is_empty());
    assert!(value.get("all_ok").is_some());

    // `cargo` is guaranteed present in a Rust test environment.
    let cargo = tools.iter().find(|t| t["name"] == "cargo").expect("cargo entry");
    assert_eq!(cargo["found"], true);
}

#[test]
fn doctor_text_runs() {
    let temp_dir = TempDir::new().expect("temp dir");
    cli_cmd(temp_dir.path()).args(["doctor"]).assert().success();
}
