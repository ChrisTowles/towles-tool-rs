mod common;

use common::cli_cmd;
use tempfile::TempDir;

/// A `tt` command with HOME and XDG_CONFIG_HOME redirected into the sandbox, so
/// doctor's history file and agentboard checks never touch the real home dir.
fn doctor_cmd(temp: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = cli_cmd(temp);
    cmd.env("HOME", temp).env("XDG_CONFIG_HOME", temp.join("config"));
    cmd
}

#[test]
fn doctor_json_emits_ts_run_result_shape() {
    let temp_dir = TempDir::new().expect("temp dir");
    let assert = doctor_cmd(temp_dir.path()).args(["doctor", "--json"]).assert().success();

    let stdout = &assert.get_output().stdout;
    let value: serde_json::Value =
        serde_json::from_slice(stdout).expect("doctor --json should emit valid JSON");

    // Matches the TS `DoctorRunResult` shape (camelCase).
    assert!(value.get("timestamp").is_some());
    assert!(value.get("ghAuth").is_some());
    assert!(value["plugins"].is_array());
    assert!(value["agentboard"].is_array());

    let tools = value["tools"].as_array().expect("tools should be an array");
    assert!(!tools.is_empty());

    // `cargo` is guaranteed present in a Rust test environment.
    let cargo = tools.iter().find(|t| t["name"] == "cargo").expect("cargo entry");
    assert_eq!(cargo["ok"], true);
    assert!(cargo["version"].is_string());
}

#[test]
fn doctor_text_runs() {
    let temp_dir = TempDir::new().expect("temp dir");
    doctor_cmd(temp_dir.path()).args(["doctor"]).assert().success();
}

/// The shared settings file is resolved from `$HOME/.config/towles-tool` (unscoped,
/// which `cli_cmd` forces). Writing corrupt JSON there must surface as a doctor row.
#[test]
fn doctor_flags_a_corrupt_settings_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    let settings_path =
        temp_dir.path().join(".config").join("towles-tool").join("towles-tool.settings.json");
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    std::fs::write(&settings_path, "{ not valid json").unwrap();

    doctor_cmd(temp_dir.path())
        .args(["doctor"])
        .assert()
        .success()
        .stdout(predicates::str::contains("settings: failed to parse"));
}

/// Write an executable `claude` stub into a PATH dir and return that dir. The stub
/// prints `list_output` for `claude mcp list` and exits 0 for anything else, so
/// doctor's MCP-registration probe sees exactly what we choose without touching a
/// real Claude install.
#[cfg(unix)]
fn stub_claude(dir: &std::path::Path, mcp_list_output: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    // Use the `echo` builtin (not `cat`): the test restricts PATH to this stub
    // dir, so no external binary is reachable.
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = mcp ] && [ \"$2\" = list ]; then\n  echo '{mcp_list_output}'\nfi\nexit 0\n"
    );
    let path = dir.join("claude");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn doctor_reports_registered_mcp_server() {
    let temp_dir = TempDir::new().expect("temp dir");
    let bin = temp_dir.path().join("bin");
    stub_claude(&bin, "tt: tt mcp serve - Connected");

    doctor_cmd(temp_dir.path())
        .env("PATH", &bin)
        .args(["doctor"])
        .assert()
        .success()
        .stdout(predicates::str::contains("tt mcp server: registered"));
}

#[cfg(unix)]
#[test]
fn doctor_reports_unregistered_mcp_server() {
    let temp_dir = TempDir::new().expect("temp dir");
    let bin = temp_dir.path().join("bin");
    stub_claude(&bin, "chrome-devtools: npx chrome-devtools-mcp - Connected");

    doctor_cmd(temp_dir.path())
        .env("PATH", &bin)
        .args(["doctor"])
        .assert()
        .success()
        .stdout(predicates::str::contains("tt mcp server: not registered"));
}

#[test]
fn doctor_diff_without_history_warns() {
    let temp_dir = TempDir::new().expect("temp dir");
    doctor_cmd(temp_dir.path())
        .args(["doctor", "--diff"])
        .assert()
        .success()
        .stdout(predicates::str::contains("No previous runs tracked"));
}

#[test]
fn doctor_json_track_writes_history_and_stays_valid_json() {
    let temp_dir = TempDir::new().expect("temp dir");

    // --track must be honored in JSON mode, and stdout must remain valid JSON.
    let assert =
        doctor_cmd(temp_dir.path()).args(["doctor", "--json", "--track"]).assert().success();
    let stdout = &assert.get_output().stdout;
    serde_json::from_slice::<serde_json::Value>(stdout)
        .expect("doctor --json --track should still emit valid JSON on stdout");

    let history_path = temp_dir.path().join("config").join("tt").join("doctor-history.json");
    assert!(history_path.exists(), "--track should write history even in JSON mode");
    let runs: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&history_path).unwrap()).unwrap();
    assert_eq!(runs.as_array().unwrap().len(), 1);
}

#[test]
fn doctor_json_diff_is_rejected() {
    let temp_dir = TempDir::new().expect("temp dir");
    // --diff output is human-format, so it can't be combined with --json.
    doctor_cmd(temp_dir.path()).args(["doctor", "--json", "--diff"]).assert().failure();
}

#[test]
fn doctor_track_then_diff_round_trips() {
    let temp_dir = TempDir::new().expect("temp dir");

    // First tracked run writes the shared history file.
    doctor_cmd(temp_dir.path())
        .args(["doctor", "--track"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Results saved to history."));

    let history_path = temp_dir.path().join("config").join("tt").join("doctor-history.json");
    assert!(history_path.exists(), "history file should be created under XDG_CONFIG_HOME/tt");

    // The history file is a JSON array of DoctorRunResult records.
    let content = std::fs::read_to_string(&history_path).unwrap();
    let runs: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(runs.as_array().unwrap().len(), 1);

    // A subsequent diff against that run succeeds and reports the comparison header.
    doctor_cmd(temp_dir.path())
        .args(["doctor", "--diff"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Changes since last tracked run"));
}
