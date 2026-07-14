//! Update-available check (`tt-update`): compares the running app's version
//! against the latest GitHub release and, on a newer release, pushes an
//! `update://available` event the frontend's banner listens for.

use tauri::{AppHandle, Emitter};

/// The GitHub repo releases are published to.
const REPO: &str = "ChrisTowles/towles-tool-rs";

/// Event the frontend listens for (`update-banner.tsx`). Fired only when a
/// newer release exists — a no-update result is never pushed, so the frontend
/// doesn't need to distinguish "checked, nothing newer" from "hasn't checked yet".
pub const UPDATE_AVAILABLE_EVENT: &str = "update://available";

/// Run the check on a blocking worker (it makes a network call) and return the
/// result to the frontend. Also used by the startup check in `lib.rs`.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<tt_update::UpdateCheck, String> {
    let current_version = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        tt_update::check_for_update(REPO, &current_version)
    })
    .await
    .map_err(|e| format!("update check task failed: {e}"))?
    .map_err(|e| e.to_string())
}

/// Fire the check once on startup and, only when a newer release is
/// available, emit [`UPDATE_AVAILABLE_EVENT`] and post an OS notification.
/// Network failures (offline, GitHub down) are swallowed — a background
/// version check must never interrupt startup or nag the user with an error.
pub fn check_on_startup(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let current_version = app.package_info().version.to_string();
        let check = tauri::async_runtime::spawn_blocking(move || {
            tt_update::check_for_update(REPO, &current_version)
        })
        .await;

        let Ok(Ok(check)) = check else { return };
        if !check.update_available {
            return;
        }

        let _ = app.emit(UPDATE_AVAILABLE_EVENT, &check);

        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title("Towles Tool update available")
            .body(format!(
                "{} is available — you're on {}",
                check.latest_version, check.current_version
            ))
            .show();
    });
}
