//! Task Explorer: a live process view of what this app itself is running —
//! the `tt-app` process plus each embedded terminal's shell and everything
//! that shell has spawned. Passive readout only, polled by the frontend on
//! an interval; this module never signals a process (kill/stop lives on
//! `terminal.rs`'s `term_kill`).
//!
//! A terminal's process set is resolved the same way `terminal::
//! kill_session_stragglers` resolves what a kill would reach: on Unix,
//! every process sharing the shell's POSIX session id, which also catches a
//! backgrounded subshell (`(cmd &)`) reparented to init after its immediate
//! parent exits. Windows has no session-id equivalent for this (see that
//! function's doc), so there we fall back to a parent-child tree walk from
//! the shell pid — less exact (misses a `setsid`-style detach, which barely
//! exists on Windows anyway) but never pulls in unrelated processes the way
//! Windows' login-session id would.

#[cfg(not(unix))]
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::State;

use crate::terminal::TermState;

/// Managed sampler state: CPU usage is a delta between refreshes, so the
/// `System` must live across command calls or every poll would report 0%.
pub struct ExplorerState(Mutex<System>);

impl Default for ExplorerState {
    fn default() -> Self {
        Self(Mutex::new(System::new()))
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRow {
    pid: u32,
    parent_pid: Option<u32>,
    name: String,
    /// Percent of the whole machine's CPU (all cores).
    cpu_percent: f32,
    memory_bytes: u64,
    status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessGroup {
    /// `None` for the app's own process; `Some(term_id)` for a terminal's
    /// shell and its descendants.
    term_id: Option<String>,
    label: String,
    rows: Vec<ProcessRow>,
    total_cpu_percent: f32,
    total_memory_bytes: u64,
}

/// One snapshot: the app process, then one group per live terminal. Groups
/// with no inspectable processes are omitted (a shell that exited between
/// the terminal list and the process refresh).
#[tauri::command]
pub fn task_explorer_snapshot(
    explorer: State<'_, ExplorerState>,
    term_state: State<'_, TermState>,
) -> Vec<ProcessGroup> {
    let Ok(mut sys) = explorer.0.lock() else {
        return Vec::new();
    };
    // Every live process, not just known pids: a session-id sweep (Unix) or
    // a child-tree walk (Windows) can't be targeted with `ProcessesToUpdate::
    // Some` up front since the member set isn't known until processes() is
    // walked.
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;

    let mut groups = Vec::new();

    let app_pid = Pid::from_u32(std::process::id());
    if let Some(row) = process_row(&sys, app_pid, cores) {
        groups.push(ProcessGroup {
            term_id: None,
            label: "tt-app".to_string(),
            total_cpu_percent: row.cpu_percent,
            total_memory_bytes: row.memory_bytes,
            rows: vec![row],
        });
    }

    for (term_id, shell_pid, label) in term_state.shell_pid_labels() {
        let shell_pid = Pid::from_u32(shell_pid);
        let mut rows: Vec<ProcessRow> = related_pids(&sys, shell_pid)
            .into_iter()
            .filter_map(|pid| process_row(&sys, pid, cores))
            .collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|r| r.pid);
        let total_cpu_percent = rows.iter().map(|r| r.cpu_percent).sum();
        let total_memory_bytes = rows.iter().map(|r| r.memory_bytes).sum();
        groups.push(ProcessGroup {
            term_id: Some(term_id),
            label,
            rows,
            total_cpu_percent,
            total_memory_bytes,
        });
    }

    groups
}

/// `None` for a pid sysinfo can't resolve, or one that resolves to a thread
/// rather than a real process — see `related_pids`'s doc for why threads
/// must never reach a `ProcessRow`.
fn process_row(sys: &System, pid: Pid, cores: f32) -> Option<ProcessRow> {
    let p = sys.process(pid)?;
    if p.thread_kind().is_some() {
        return None;
    }
    Some(ProcessRow {
        pid: pid.as_u32(),
        parent_pid: p.parent().map(|pp| pp.as_u32()),
        name: p.name().to_string_lossy().into_owned(),
        cpu_percent: p.cpu_usage() / cores,
        memory_bytes: p.memory(),
        status: p.status().to_string(),
    })
}

/// Every pid belonging to `shell_pid`'s process set, including the shell
/// itself. Threads never qualify: on Linux, `sysinfo::System::processes()`
/// also surfaces every thread of every process (tagged via
/// `Process::thread_kind`, `Some` for a thread and `None` for a real
/// process) — a thread shares its process's session id, so an unfiltered
/// sweep pulled in every thread of every process in the shell's session and
/// double-, triple-, N-counted that one process's memory once per thread
/// (a `claude`/Bun session with dozens of worker threads inflated the
/// group's total from tens of MB to gigabytes). Filtering here, at the
/// source, is cheaper than filtering downstream since it also shrinks the
/// set `process_row` has to resolve.
#[cfg(unix)]
fn related_pids(sys: &System, shell_pid: Pid) -> HashSet<Pid> {
    sys.processes()
        .iter()
        .filter(|(pid, p)| {
            p.thread_kind().is_none() && (**pid == shell_pid || p.session_id() == Some(shell_pid))
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// No POSIX session id on Windows, so descend the parent-child tree instead
/// — still catches ordinary subprocesses, just not a fully detached one.
#[cfg(not(unix))]
fn related_pids(sys: &System, shell_pid: Pid) -> HashSet<Pid> {
    let mut children_by_parent: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, p) in sys.processes() {
        // See the Unix `related_pids`' doc for why threads (`thread_kind()
        // .is_some()`) must never enter this walk.
        if p.thread_kind().is_some() {
            continue;
        }
        if let Some(parent) = p.parent() {
            children_by_parent.entry(parent).or_default().push(*pid);
        }
    }

    let mut related = HashSet::new();
    let mut stack = vec![shell_pid];
    while let Some(pid) = stack.pop() {
        if !related.insert(pid) {
            continue;
        }
        if let Some(children) = children_by_parent.get(&pid) {
            stack.extend(children.iter().copied());
        }
    }
    related
}
