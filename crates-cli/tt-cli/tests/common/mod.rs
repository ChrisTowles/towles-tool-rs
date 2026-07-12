#![allow(dead_code)]

use assert_cmd::Command;
use std::path::Path;

/// Build a `ttr` command pointed at an isolated config directory, so tests never
/// touch the real `~/.config/towles-tool`.
///
/// Also forces `TT_STATE_SCOPE=` empty so state-path resolution stays *unscoped*
/// (the daily-driver defaults) even though the test binary runs from inside a
/// slot checkout, whose cwd would otherwise auto-derive a slot scope. These
/// black-box tests assert on the documented default paths.
pub fn cli_cmd(config_dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ttr").expect("binary `ttr` should build");
    cmd.arg("--config-dir").arg(config_dir);
    cmd.env(tt_config::STATE_SCOPE_ENV, "");
    cmd
}

/// Write a settings file into `config_dir` whose journal `baseFolder` and `templateDir`
/// point inside the sandbox, so journal tests never touch the real home directory.
pub fn write_journal_settings(config_dir: &Path, base_folder: &Path, template_dir: &Path) {
    std::fs::create_dir_all(config_dir).unwrap();
    let settings = serde_json::json!({
        "preferredEditor": "true",
        "journalSettings": {
            "baseFolder": base_folder.to_string_lossy(),
            "templateDir": template_dir.to_string_lossy(),
        },
    });
    let path = config_dir.join(format!("{}.settings.json", tt_config::TOOL_NAME));
    std::fs::write(path, serde_json::to_string_pretty(&settings).unwrap()).unwrap();
}
