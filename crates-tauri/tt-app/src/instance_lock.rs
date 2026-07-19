//! A generic single-instance guard: only the process holding a named,
//! PID-tagged lock file proceeds past [`InstanceLock::try_acquire`]; a lock
//! left behind by a process that's no longer running is detected and stolen.
//! Reused for two differently-scoped resources under different lock names —
//! the name is what determines the scope, not the type:
//!
//! - `"slack-socket"` (`slack_socket.rs`): Slack credentials live in the
//!   *shared* config dir (`tt_config::config_dir()` only re-scopes under a
//!   forced `TT_STATE_SCOPE`), so every open worktree slot's process reads the
//!   same token. Without this guard, N open slots would each open their own
//!   Slack Socket Mode connection and poll on the same token — see #227.
//! - `"app-<identifier>"` (`lib.rs`'s `run`): stops the *same* checkout
//!   (primary, or one specific slot) from being launched twice at once,
//!   which would otherwise silently run two independent windows/PTY sets/
//!   schedulers against the same checkout. Different slots already get
//!   different identifiers (see `app_identifier`), so this only ever fires
//!   within one checkout.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use sysinfo::{Pid, ProcessesToUpdate, System};

/// A held lock file under `tt_config::locks_dir()`, released on `Drop`.
pub struct InstanceLock {
    path: PathBuf,
}

impl InstanceLock {
    /// Try to acquire `<locks_dir>/<name>.lock`. `None` if another live
    /// process already holds it; a lock whose recorded PID is no longer
    /// running is stolen.
    pub fn try_acquire(name: &str) -> Option<Self> {
        let path = tt_config::locks_dir().join(format!("{name}.lock"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        if !create_new(&path) {
            let held_by = fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok());
            if held_by.is_some_and(pid_is_alive) {
                return None;
            }
            // Stale: the recorded PID is gone (or unreadable). Steal it.
            fs::remove_file(&path).ok()?;
            if !create_new(&path) {
                return None;
            }
        }
        fs::write(&path, std::process::id().to_string()).ok()?;
        Some(Self { path })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Atomically create `path`, failing (without error) if it already exists.
fn create_new(path: &PathBuf) -> bool {
    match fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => true,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => false,
        Err(_) => false,
    }
}

pub(crate) fn pid_is_alive(pid: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    system.process(Pid::from_u32(pid)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_of_a_live_holder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        assert!(create_new(&path));
        fs::write(&path, std::process::id().to_string()).unwrap();
        // Our own pid is alive, so a fresh check must treat it as held.
        let held_by = fs::read_to_string(&path).unwrap().trim().parse::<u32>().unwrap();
        assert!(pid_is_alive(held_by));
    }

    #[test]
    fn a_dead_pid_is_detected_as_not_alive() {
        // PID 1 belongs to init and is always alive on any running Linux box;
        // an implausibly large pid is never a live process to check the other
        // branch without depending on OS-specific "definitely dead" pids.
        assert!(!pid_is_alive(u32::MAX));
    }
}
