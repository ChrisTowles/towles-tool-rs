//! Tauri bridge for the shared user settings (`tt_config`). The Settings window
//! reads the typed model with `settings_get` and writes it back with
//! `settings_set`, which uses `tt_config::save_merge` so keys the shared
//! TypeScript CLI owns (but this model doesn't capture) survive the round-trip.
//!
//! No managed state: settings live in a file, so each command loads/saves fresh.
//! Collector cadence is read once when the scheduler spawns, so changes to the
//! `collectors` block take effect on the next app launch — the UI says so.

use tt_config::UserSettings;

/// Load the current settings (defaults written to disk if the file is missing).
#[tauri::command]
pub fn settings_get() -> Result<UserSettings, String> {
    tt_config::load().map_err(|e| format!("failed to load settings: {e}"))
}

/// Persist edited settings, preserving any unknown keys already on disk.
#[tauri::command]
pub fn settings_set(settings: UserSettings) -> Result<(), String> {
    tt_config::save_merge(&settings).map_err(|e| format!("failed to save settings: {e}"))
}
