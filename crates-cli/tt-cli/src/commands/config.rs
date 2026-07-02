//! `tt config` subcommands: show, validate, schema, reset.
//!
//! Library crates (`tt-config`) return typed errors; this CLI boundary flattens
//! them to process exit codes and human/JSON output (the yaak pattern).

use crate::cli::ConfigCommands;
use crate::ui;
use std::path::{Path, PathBuf};

/// Resolve the settings file path, honoring a `--config-dir` override.
fn resolve_config_path(config_dir: Option<&Path>) -> Result<PathBuf, tt_config::Error> {
    match config_dir {
        Some(dir) => Ok(dir.join(format!("{}.settings.json", tt_config::TOOL_NAME))),
        None => tt_config::config_path(),
    }
}

pub fn run(command: ConfigCommands, config_dir: Option<&Path>) -> i32 {
    match command {
        ConfigCommands::Show => show(config_dir),
        ConfigCommands::Validate => validate(config_dir),
        ConfigCommands::Schema => schema(),
        ConfigCommands::Reset { confirm } => reset(config_dir, confirm),
    }
}

fn show(config_dir: Option<&Path>) -> i32 {
    let path = match resolve_config_path(config_dir) {
        Ok(path) => path,
        Err(e) => {
            ui::error(&format!("Could not resolve config path: {e}"));
            return 1;
        }
    };

    let settings = match tt_config::load_from(&path) {
        Ok(settings) => settings,
        Err(e) => {
            ui::error(&format!("Failed to load settings: {e}"));
            return 1;
        }
    };

    ui::info(&format!("Settings file: {}", path.display()));
    println!();
    match serde_json::to_string_pretty(&settings) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(e) => {
            ui::error(&format!("Failed to serialize settings: {e}"));
            1
        }
    }
}

fn validate(config_dir: Option<&Path>) -> i32 {
    let path = match resolve_config_path(config_dir) {
        Ok(path) => path,
        Err(e) => {
            ui::error(&format!("Could not resolve config path: {e}"));
            return 1;
        }
    };

    if !path.exists() {
        ui::error(&format!("Settings file not found: {}", path.display()));
        return 1;
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            ui::error(&format!("Failed to read {}: {e}", path.display()));
            return 1;
        }
    };

    match serde_json::from_str::<tt_config::UserSettings>(&raw) {
        Ok(_) => {
            ui::success(&format!("{} is valid", path.display()));
            0
        }
        Err(e) => {
            ui::error(&format!("{} has validation errors: {e}", path.display()));
            1
        }
    }
}

fn schema() -> i32 {
    let schema = tt_config::json_schema();
    match serde_json::to_string_pretty(&schema) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(e) => {
            ui::error(&format!("Failed to serialize schema: {e}"));
            1
        }
    }
}

fn reset(config_dir: Option<&Path>, confirm: bool) -> i32 {
    let path = match resolve_config_path(config_dir) {
        Ok(path) => path,
        Err(e) => {
            ui::error(&format!("Could not resolve config path: {e}"));
            return 1;
        }
    };

    let defaults = tt_config::UserSettings::default();

    if !confirm {
        // Show a diff of current vs defaults, then require --confirm.
        let current = if path.exists() {
            tt_config::load_from(&path).unwrap_or_else(|_| defaults.clone())
        } else {
            defaults.clone()
        };

        if current == defaults {
            ui::success("Settings already match defaults. Nothing to reset.");
            return 0;
        }

        let current_json = serde_json::to_string_pretty(&current).unwrap_or_default();
        let default_json = serde_json::to_string_pretty(&defaults).unwrap_or_default();
        println!("Current:\n{current_json}\n");
        println!("Default:\n{default_json}\n");
        ui::warning("Run with --confirm to reset settings to defaults.");
        return 1;
    }

    match tt_config::save_to(&path, &defaults) {
        Ok(()) => {
            ui::success(&format!("Settings reset to defaults: {}", path.display()));
            0
        }
        Err(e) => {
            ui::error(&format!("Failed to write settings: {e}"));
            1
        }
    }
}
