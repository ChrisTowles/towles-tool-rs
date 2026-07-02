#![allow(dead_code)]

use assert_cmd::Command;
use std::path::Path;

/// Build a `ttr` command pointed at an isolated config directory, so tests never
/// touch the real `~/.config/towles-tool`.
pub fn cli_cmd(config_dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ttr").expect("binary `ttr` should build");
    cmd.arg("--config-dir").arg(config_dir);
    cmd
}
