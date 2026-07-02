//! Configuration model for the towles-tool CLI.
//!
//! This mirrors the zod settings schema used by the TypeScript CLI and reads/writes
//! the *same* file (`~/.config/towles-tool/towles-tool.settings.json`). Because the
//! file is shared, the model deliberately tolerates unknown fields and fills missing
//! ones from defaults via `#[serde(default)]` — never `deny_unknown_fields`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Tool name used for the config directory and file name.
pub const TOOL_NAME: &str = "towles-tool";

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Could not determine home directory")]
    NoHomeDir,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Position of the AgentBoard sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition {
    Left,
    Right,
}

/// Journal path templates and base folders.
///
/// Path template tokens follow Luxon formatting, e.g. `{yyyy}`, `{MM}`, `{dd}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct JournalSettings {
    /// Base folder where all journal files are stored.
    pub base_folder: String,

    /// Template for daily-note file paths (relative to `base_folder`).
    pub daily_path_template: String,

    /// Template for meeting-note file paths (relative to `base_folder`).
    pub meeting_path_template: String,

    /// Template for note file paths (relative to `base_folder`).
    pub note_path_template: String,

    /// Directory holding external templates (falls back to built-ins when absent).
    pub template_dir: String,
}

impl Default for JournalSettings {
    fn default() -> Self {
        Self {
            base_folder: home_dir_string(),
            daily_path_template:
                "journal/{monday:yyyy}/{monday:MM}/daily-notes/{monday:yyyy}-{monday:MM}-{monday:dd}-daily-notes.md"
                    .to_string(),
            meeting_path_template: "journal/{yyyy}/{MM}/meetings/{yyyy}-{MM}-{dd}-{title}.md"
                .to_string(),
            note_path_template: "journal/{yyyy}/{MM}/notes/{yyyy}-{MM}-{dd}-{title}.md".to_string(),
            template_dir: default_template_dir(),
        }
    }
}

/// AgentBoard UI preferences. Every field is optional; the TS CLI owns most of them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentboardSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mux: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Theme name or an inline theme object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_position: Option<SidebarPosition>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub keybinding: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_panel_heights: Option<HashMap<String, f64>>,
}

/// Top-level user settings, mirroring `UserSettingsSchema` in the TS CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct UserSettings {
    /// Preferred editor command (e.g. `code`).
    pub preferred_editor: String,

    pub journal_settings: JournalSettings,

    pub agentboard: AgentboardSettings,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            preferred_editor: "code".to_string(),
            journal_settings: JournalSettings::default(),
            agentboard: AgentboardSettings::default(),
        }
    }
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(Error::NoHomeDir)
}

/// Home directory as a string, falling back to `.` if it can't be resolved.
/// Used only for defaults, which must be infallible.
fn home_dir_string() -> String {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).to_string_lossy().to_string()
}

fn default_template_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join(TOOL_NAME)
        .join("templates")
        .to_string_lossy()
        .to_string()
}

/// Config directory: `~/.config/towles-tool` (matches the TS CLI on every platform).
pub fn config_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config").join(TOOL_NAME))
}

/// Full path to the settings file: `~/.config/towles-tool/towles-tool.settings.json`.
pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(format!("{TOOL_NAME}.settings.json")))
}

/// Load settings from the standard location, creating defaults if the file is missing.
pub fn load() -> Result<UserSettings> {
    load_from(&config_path()?)
}

/// Load settings from an explicit path, creating defaults if the file is missing.
///
/// Accepting a path keeps this testable without touching the real home directory.
pub fn load_from(path: &Path) -> Result<UserSettings> {
    if !path.exists() {
        let settings = UserSettings::default();
        save_to(path, &settings)?;
        return Ok(settings);
    }
    let raw = std::fs::read_to_string(path)?;
    let settings = serde_json::from_str(&raw)?;
    Ok(settings)
}

/// Save settings to the standard location.
pub fn save(settings: &UserSettings) -> Result<()> {
    save_to(&config_path()?, settings)
}

/// Save settings to an explicit path, creating parent directories as needed.
pub fn save_to(path: &Path, settings: &UserSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// JSON Schema for the settings file, as a `serde_json::Value`.
pub fn json_schema() -> serde_json::Value {
    let schema = schemars::schema_for!(UserSettings);
    serde_json::to_value(schema).expect("settings JSON schema should serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_match_ts_cli() {
        let settings = UserSettings::default();
        assert_eq!(settings.preferred_editor, "code");
        assert!(settings.journal_settings.daily_path_template.contains("daily-notes"));
        assert!(settings.agentboard.mux.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");

        let settings = UserSettings { preferred_editor: "nvim".to_string(), ..Default::default() };
        save_to(&path, &settings).unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn load_from_missing_creates_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("towles-tool.settings.json");

        assert!(!path.exists());
        let loaded = load_from(&path).unwrap();
        assert!(path.exists());
        assert_eq!(loaded, UserSettings::default());
    }

    #[test]
    fn tolerates_unknown_fields_and_fills_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("towles-tool.settings.json");
        // Unknown top-level key + a partial journalSettings object.
        std::fs::write(
            &path,
            r#"{"preferredEditor":"vim","futureFlag":true,"journalSettings":{"baseFolder":"/tmp/j"}}"#,
        )
        .unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.preferred_editor, "vim");
        assert_eq!(loaded.journal_settings.base_folder, "/tmp/j");
        // Missing journal fields fall back to defaults.
        assert!(loaded.journal_settings.daily_path_template.contains("daily-notes"));
    }

    #[test]
    fn serializes_camel_case() {
        let json = serde_json::to_string(&UserSettings::default()).unwrap();
        assert!(json.contains("\"preferredEditor\""));
        assert!(json.contains("\"journalSettings\""));
        assert!(json.contains("\"dailyPathTemplate\""));
    }

    #[test]
    fn json_schema_has_properties() {
        let schema = json_schema();
        let props = &schema["properties"];
        assert!(props.get("preferredEditor").is_some());
        assert!(props.get("journalSettings").is_some());
        assert!(props.get("agentboard").is_some());
    }
}
