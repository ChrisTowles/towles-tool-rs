//! Tauri bridge for the shared user settings (`tt_config`). The Settings window
//! reads the typed model with `settings_get` and writes it back with
//! `settings_set`, which uses `tt_config::save_merge` so keys the shared
//! TypeScript CLI owns (but this model doesn't capture) survive the round-trip.
//!
//! Settings live in a file, so each command loads/saves fresh. The one piece of
//! state is `SettingsSignal`: a `Notify` the scheduler waits on so a `settings_set`
//! re-reads the `collectors` block live (cadence/enable/provider) without a
//! relaunch.

use std::sync::Arc;

use tauri::State;
use tokio::sync::Notify;

use tt_config::UserSettings;

/// Managed signal fired after a settings write so the scheduler re-reads config.
pub struct SettingsSignal(pub Arc<Notify>);

/// Load the current settings (defaults written to disk if the file is missing).
#[tauri::command]
pub fn settings_get() -> Result<UserSettings, String> {
    tt_config::load().map_err(|e| format!("failed to load settings: {e}"))
}

/// Persist edited settings, preserving any unknown keys already on disk, then
/// signal the scheduler to re-read collector cadence.
#[tauri::command]
pub fn settings_set(settings: UserSettings, signal: State<SettingsSignal>) -> Result<(), String> {
    tt_config::save_merge(&settings).map_err(|e| format!("failed to save settings: {e}"))?;
    signal.0.notify_one();
    Ok(())
}
