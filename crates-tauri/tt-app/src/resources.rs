//! Self resource usage for the status bar (#78): CPU and RAM of the `tt-app`
//! process itself, so a long Agentboard session shows at a glance whether the
//! app is chewing resources. Passive readout only — polled by the frontend on
//! an interval, never pushed. First cut is the main process only (not the PTY
//! children or WebKit's separate web processes).

use std::sync::Mutex;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Managed sampler state. CPU usage is a delta between two refreshes, so the
/// `System` must live across command calls — the first poll reports 0% and
/// every later one covers the interval since the previous poll.
pub struct ResourceState(Mutex<System>);

impl Default for ResourceState {
    fn default() -> Self {
        Self(Mutex::new(System::new()))
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUsage {
    /// Percent of the whole machine's CPU (all cores) used by the process.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

/// CPU/RAM of the app process. `None` only if the process can't be inspected.
#[tauri::command]
pub fn app_resource_usage(state: tauri::State<'_, ResourceState>) -> Option<ResourceUsage> {
    let mut sys = state.0.lock().ok()?;
    let pid = Pid::from_u32(std::process::id());
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    // `cpu_usage()` is percent of ONE core; scale to share of the machine so
    // the readout matches what a user expects from a system monitor.
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
    sys.process(pid)
        .map(|p| ResourceUsage { cpu_percent: p.cpu_usage() / cores, memory_bytes: p.memory() })
}
