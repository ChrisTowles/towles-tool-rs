//! Tauri bridge for the watched Slack DM conversation: on-demand history for
//! the chat panel and sending a reply as the user. Reads the same `slack`
//! collector settings the scheduler uses, but deliberately ignores `enabled` —
//! the chat panel works whenever credentials exist, even with the background
//! watcher switched off. After a successful send the `slack:dm` collector runs
//! once and the snapshot re-emits, so the banner clears without waiting for
//! the next scheduled tick.

use tauri::{AppHandle, Emitter};

use crate::store::SNAPSHOT_EVENT;

/// How much of the DM conversation the chat panel pulls per fetch.
const HISTORY_LIMIT: u32 = 50;

/// The chat panel's view of the watched DM. `configured` is `false` when the
/// slack collector has no token/member id yet — the panel shows setup guidance
/// instead of a conversation.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackDmView {
    pub configured: bool,
    pub watch_name: String,
    /// The watched member id, so the panel can resolve `<@id>` mentions in
    /// message text to the watched user's name.
    pub watch_user_id: String,
    pub messages: Vec<tt_collect::DmMessage>,
}

/// A fetched Slack file, base64-encoded for the webview to render as a `data:`
/// URI (the private URL can't be loaded directly — it needs the bearer token).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackFileData {
    pub mimetype: String,
    pub data_base64: String,
}

/// The configured Slack DM settings, or `None` when token/member id are blank.
fn dm_config() -> Option<tt_collect::SlackDmConfig> {
    let slack = tt_config::load().ok()?.collectors.slack;
    if slack.token.trim().is_empty() || slack.watch_user_id.trim().is_empty() {
        return None;
    }
    Some(tt_collect::SlackDmConfig {
        token: slack.token,
        watch_user_id: slack.watch_user_id,
        watch_name: slack.watch_name,
    })
}

/// The banner/panel display name for the watched user (member id fallback,
/// matching the collector).
fn display_name(config: &tt_collect::SlackDmConfig) -> String {
    if config.watch_name.trim().is_empty() {
        config.watch_user_id.clone()
    } else {
        config.watch_name.clone()
    }
}

/// Fetch the watched DM conversation (oldest first) for the chat panel.
/// Unconfigured is a clean `configured: false` view, not an error.
#[tauri::command]
pub async fn slack_dm_history() -> Result<SlackDmView, String> {
    let Some(config) = dm_config() else {
        return Ok(SlackDmView {
            configured: false,
            watch_name: String::new(),
            watch_user_id: String::new(),
            messages: Vec::new(),
        });
    };
    let watch_name = display_name(&config);
    let watch_user_id = config.watch_user_id.clone();
    let messages = tauri::async_runtime::spawn_blocking(move || {
        tt_collect::fetch_dm_history(&config, HISTORY_LIMIT)
    })
    .await
    .map_err(|e| format!("slack history task failed: {e}"))??;
    Ok(SlackDmView { configured: true, watch_name, watch_user_id, messages })
}

/// Fetch a Slack file's bytes (base64) with the token's bearer header so the
/// panel can render it as a `data:` URI. `url` must be a `url_private`/thumb URL
/// from a [`tt_collect::DmFile`]; only `*.slack.com` URLs are honored. A missing
/// `files:read` scope surfaces as an error string the frontend maps to a subtle
/// placeholder rather than failing the whole panel.
#[tauri::command]
pub async fn slack_dm_file(url: String) -> Result<SlackFileData, String> {
    use base64::Engine;

    let config = dm_config().ok_or("Slack DM is not configured")?;
    let file =
        tauri::async_runtime::spawn_blocking(move || tt_collect::fetch_file(&config.token, &url))
            .await
            .map_err(|e| format!("slack file task failed: {e}"))??;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(&file.bytes);
    Ok(SlackFileData { mimetype: file.mimetype, data_base64 })
}

/// Send `text` to the watched DM as me, then refresh the stored DM state (the
/// newest message becomes mine, clearing the attention banner) and re-emit the
/// snapshot.
#[tauri::command]
pub async fn slack_dm_send(app: AppHandle, text: String) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("message text is required".into());
    }
    let config = dm_config()
        .ok_or("Slack DM is not configured — set the token and member id in Settings")?;

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        tt_collect::send_dm(&config, &text)?;
        // Best-effort refresh: the send already succeeded, so a store hiccup
        // here must not fail the command — the next scheduled tick catches up.
        if let Ok(store) = tt_store::Store::open_default() {
            let now = chrono::Local::now().timestamp_millis();
            let _ = tt_collect::collect_slack_dm(&store, &config, now);
            if let Ok(snapshot) = store.snapshot() {
                let _ = app.emit(SNAPSHOT_EVENT, snapshot);
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("slack send task failed: {e}"))?
}
