//! Read/write Claude Code's real `~/.claude/settings.json`.
//!
//! Ports `src/commands/claude-settings.ts`. This is Claude Code's own settings
//! file, not towles-tool's — so we must preserve every unknown field. The model
//! is a `serde_json::Map<String, Value>`, never a closed struct: clobbering keys
//! we don't recognize is a hard failure.
//!
//! Pure logic only (path-parameterized) so tests never touch the real `~/.claude`.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Claude settings modeled as an open JSON object so unknown keys survive a round trip.
pub type ClaudeSettings = Map<String, Value>;

/// Default path to Claude Code's settings file: `~/.claude/settings.json`.
pub fn claude_settings_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("settings.json")
}

/// Load Claude settings from `path`.
///
/// Returns an empty map when the file is missing OR contains invalid JSON — the
/// TS version swallows parse errors the same way (`JSON.parse` in a try/catch).
pub fn load_claude_settings(path: &Path) -> ClaudeSettings {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// Apply recommended defaults, returning the updated settings plus a list of
/// human-readable change descriptions (empty when nothing changed).
///
/// Mirrors `applyRecommendedSettings`: only touches a key when it isn't already
/// the recommended value, and pushes the exact TS change strings.
pub fn apply_recommended_settings(mut settings: ClaudeSettings) -> (ClaudeSettings, Vec<String>) {
    let mut changes = Vec::new();

    if settings.get("cleanupPeriodDays").and_then(Value::as_f64) != Some(99999.0) {
        settings.insert("cleanupPeriodDays".to_string(), Value::from(99999));
        changes.push("Set cleanupPeriodDays: 99999 (prevent log deletion)".to_string());
    }

    if settings.get("alwaysThinkingEnabled").and_then(Value::as_bool) != Some(true) {
        settings.insert("alwaysThinkingEnabled".to_string(), Value::Bool(true));
        changes.push("Set alwaysThinkingEnabled: true".to_string());
    }

    (settings, changes)
}

/// Write Claude settings as pretty (2-space) JSON, creating parent dirs as needed.
pub fn save_claude_settings(path: &Path, settings: &ClaudeSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let settings = load_claude_settings(&dir.path().join("nope.json"));
        assert!(settings.is_empty());
    }

    #[test]
    fn load_invalid_json_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        assert!(load_claude_settings(&path).is_empty());
    }

    #[test]
    fn apply_on_empty_sets_both_and_reports_changes() {
        let (settings, changes) = apply_recommended_settings(Map::new());
        assert_eq!(settings["cleanupPeriodDays"], json!(99999));
        assert_eq!(settings["alwaysThinkingEnabled"], json!(true));
        assert_eq!(
            changes,
            vec![
                "Set cleanupPeriodDays: 99999 (prevent log deletion)".to_string(),
                "Set alwaysThinkingEnabled: true".to_string(),
            ]
        );
    }

    #[test]
    fn apply_is_noop_when_already_set() {
        let mut existing = Map::new();
        existing.insert("cleanupPeriodDays".to_string(), json!(99999));
        existing.insert("alwaysThinkingEnabled".to_string(), json!(true));
        let (_settings, changes) = apply_recommended_settings(existing);
        assert!(changes.is_empty());
    }

    #[test]
    fn apply_preserves_unknown_fields() {
        let mut existing = Map::new();
        existing.insert("hooks".to_string(), json!({ "PreToolUse": [] }));
        existing.insert("someFutureFlag".to_string(), json!("keep me"));
        let (settings, _changes) = apply_recommended_settings(existing);
        assert_eq!(settings["someFutureFlag"], json!("keep me"));
        assert_eq!(settings["hooks"], json!({ "PreToolUse": [] }));
    }

    #[test]
    fn apply_only_sets_missing_key() {
        let mut existing = Map::new();
        existing.insert("cleanupPeriodDays".to_string(), json!(99999));
        let (settings, changes) = apply_recommended_settings(existing);
        assert_eq!(settings["alwaysThinkingEnabled"], json!(true));
        assert_eq!(changes, vec!["Set alwaysThinkingEnabled: true".to_string()]);
    }

    #[test]
    fn save_then_load_round_trips_and_creates_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let (settings, _) = apply_recommended_settings(Map::new());
        save_claude_settings(&path, &settings).unwrap();
        assert!(path.exists());
        let loaded = load_claude_settings(&path);
        assert_eq!(loaded["cleanupPeriodDays"], json!(99999));
    }
}
