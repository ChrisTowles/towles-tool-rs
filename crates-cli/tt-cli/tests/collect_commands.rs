//! Black-box tests for `ttr collect` (assert_cmd).

mod common;

use common::cli_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn collect_help_lists_subcommands() {
    let dir = TempDir::new().unwrap();
    cli_cmd(dir.path()).args(["collect", "--help"]).assert().success().stdout(
        predicate::str::contains("calendar")
            .and(predicate::str::contains("email"))
            .and(predicate::str::contains("prs"))
            .and(predicate::str::contains("all")),
    );
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
