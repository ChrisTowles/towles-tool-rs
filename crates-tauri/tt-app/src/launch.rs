//! Dev servers from a checkout's Claude Desktop `.claude/launch.json` — the
//! Tauri glue over [`tt_agentboard::launch`]. `launch_configs` lists a
//! folder's configs with live status (port probe + which app session runs
//! each); `launch_register` records "config X now runs in session Y" after
//! the client typed the launch command into that session's PTY. The launch
//! itself is frontend-driven, exactly like starting `claude` in a pane —
//! there is no backend spawn path here (the PTY's shell spawn is already
//! recorded via `tt_exec::record_detached_spawn`), so the register event
//! below is what makes the launch gesture visible in the event log.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, State};

/// Which app session (PTY) each launched dev-server config runs in, keyed by
/// `(folder dir, config name)`. In-memory only: PTYs die with the app, so
/// the mapping does too (the terminal's own no-cross-restart-persistence
/// rule). Pruned against the live-PTY registry on every read, so a closed
/// pane stops claiming its config the moment it's gone.
#[derive(Default)]
pub struct LaunchState {
    running: Mutex<HashMap<(String, String), String>>,
}

/// One `launch.json` config plus what the client needs to render its row:
/// the config itself (flattened — `name`/`runtimeExecutable`/`runtimeArgs`/
/// `port`), whether anything is listening on its port, and the app session
/// it runs in when we launched it ourselves. `sessionId` set → "focus that
/// pane"; unset but `portListening` → running outside the app (don't offer
/// a second launch); neither → launchable.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchConfigStatus {
    #[serde(flatten)]
    pub config: tt_agentboard::LaunchConfig,
    /// Something accepts TCP connections on `port` right now. Always false
    /// for a config without a port — the client tells "unknown" from
    /// "stopped" by `port` itself.
    pub port_listening: bool,
    pub session_id: Option<String>,
}

/// A folder's dev-server configs with live status. Async over a blocking
/// task: file IO plus one connect probe per distinct port (instant on
/// loopback for the usual refused/accepted cases, but never on the main
/// thread — sync commands dispatch inline on the GTK thread on Linux).
#[tauri::command]
pub async fn launch_configs(
    app: AppHandle,
    dir: String,
) -> Result<Vec<LaunchConfigStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || launch_configs_blocking(&app, dir))
        .await
        .map_err(|e| format!("launch probe task failed: {e}"))?
}

fn launch_configs_blocking(
    app: &AppHandle,
    dir: String,
) -> Result<Vec<LaunchConfigStatus>, String> {
    let file = tt_agentboard::read_launch_file(Path::new(&dir))
        .map_err(|e| format!("launch.json: {e}"))?;
    let Some(file) = file else {
        return Ok(Vec::new());
    };

    // Resolve each config's owning session while pruning dead panes, then
    // drop the lock before the network probes.
    let live = app.state::<crate::terminal::TermState>().live_ids();
    let launch_state = app.state::<LaunchState>();
    let sessions: Vec<Option<String>> = {
        let mut running = launch_state.running.lock().unwrap();
        running.retain(|_, sid| live.contains(sid));
        file.configurations
            .iter()
            .map(|c| running.get(&(dir.clone(), c.name.clone())).cloned())
            .collect()
    };

    // One probe per distinct port — configs may share one (the blog fixture's
    // "all" covers the same server as "blog").
    let mut probed: HashMap<u16, bool> = HashMap::new();
    Ok(file
        .configurations
        .into_iter()
        .zip(sessions)
        .filter(|(config, _)| config.launchable())
        .map(|(config, session_id)| {
            let port_listening = config
                .port
                .map(|p| *probed.entry(p).or_insert_with(|| tt_agentboard::port_listening(p)))
                .unwrap_or(false);
            LaunchConfigStatus { config, port_listening, session_id }
        })
        .collect())
}

/// Record "config `name` from `dir`'s launch.json now runs in session
/// `session_id`" — called by the client right after typing the launch
/// command into that pane. Also the event-log record of the launch gesture
/// itself (root CLAUDE.md: every user-initiated action is logged): the PTY
/// only sees anonymous keystrokes, so without this event a dev-server
/// launch would be indistinguishable from any other typing.
#[tauri::command]
pub fn launch_register(
    state: State<'_, LaunchState>,
    dir: String,
    name: String,
    session_id: String,
    port: Option<u16>,
    command: String,
) {
    tracing::info!(
        dir = %dir,
        config = %name,
        session_id = %session_id,
        port = ?port,
        command = %command,
        "dev_server.launch"
    );
    state.running.lock().unwrap().insert((dir, name), session_id);
}
