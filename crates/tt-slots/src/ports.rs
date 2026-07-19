//! Who holds a claimed port, and how to stop them.
//!
//! The removal guard ([`crate::guards::RmBlocked::ForeignPortListener`]) can
//! only learn "something is listening" from [`crate::ops::port_occupied`]'s
//! bind probe, which is enough to refuse a removal and not enough to act on
//! one: "port 4424 is in use" leaves the user hunting for the process by
//! hand. Asking the OS who holds it turns that into a name to recognize and a
//! process to stop.
//!
//! Mirrors `scripts/slot-port.mjs`'s `killPort`, which solves the identical
//! problem for the dev-server launcher, and for the same reasons:
//! - `lsof` for the listeners — it's what the launcher already relies on, and
//!   its absence is indistinguishable from "nothing listening" for our
//!   purposes (both mean we have no pid to offer).
//! - Signal the listener's POSIX **process group**, not its pid: a
//!   `npm run dev` tree leaves vite/esbuild orphaned and still bound to the
//!   port when you signal only the pid `lsof` reports.
//! - SIGTERM, wait, then SIGKILL — a dev server gets its chance to shut down
//!   cleanly first.
//!
//! Every probe here is best-effort by construction: a missing `lsof`/`ps`, a
//! process that exits between the listing and the signal, and a platform with
//! no POSIX process groups all degrade to "no holder known", never an error
//! that blocks the caller.

use std::thread::sleep;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::guards::PortHolder;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PortError {
    #[error("stopping a port's process needs POSIX process groups (unsupported on this platform)")]
    Unsupported,

    #[error(
        "couldn't identify what is listening on port {0} — `lsof` found no process to stop (is it running as another user?)"
    )]
    NoListenerFound(u16),

    #[error("port {port} is still in use after SIGKILL — stop the process by hand")]
    StillInUse { port: u16 },
}

/// What [`stop_listeners`] did, for the caller's user-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    /// Process groups signaled. Empty = the port was already free and nothing
    /// needed signaling.
    pub pgids: Vec<i32>,
    /// Whether SIGTERM alone was enough (`false` = it took a SIGKILL).
    pub graceful: bool,
}

/// Identify what is listening on `port`, if we can name it.
pub fn holder(port: u16) -> Option<PortHolder> {
    let pid = *listening_pids(port).first()?;
    let command = probe_processes(&[pid])
        .into_iter()
        .next()
        .map_or_else(|| "unknown".to_string(), |p| p.command);
    Some(PortHolder { pid, command })
}

/// Stop whatever is listening on `port`: SIGTERM its process group(s), then
/// SIGKILL if the port is still held. Returns once the port is actually free
/// — the caller's next act is usually to retry an operation the listener was
/// blocking, and a "stopped" that returns before the socket is released just
/// moves the race.
///
/// `pub(crate)` on purpose: this is a "kill whatever holds this port"
/// primitive, and the only thing keeping it from killing a sibling slot's
/// legitimate dev server is the claim check in [`crate::ops::stop_slot_port`]
/// — its sole caller, and the only door into it from outside the crate. Every
/// port on the machine that this slot did not claim belongs to somebody else.
pub(crate) fn stop_listeners(port: u16) -> Result<Stopped, PortError> {
    if !cfg!(unix) {
        return Err(PortError::Unsupported);
    }
    // Already free — the caller asked for the port to be clear and it is, so
    // this is success with nothing to do, not a failure. It happens for real:
    // the user reads "a dev server may be running", quits it in their own
    // terminal, *then* clicks the button. Returning an error there would
    // dead-end the very flow this function exists to unblock.
    if !crate::ops::port_occupied(port) {
        return Ok(Stopped { pgids: Vec::new(), graceful: true });
    }
    let pgids = listening_pgids(port);
    if pgids.is_empty() {
        return Err(PortError::NoListenerFound(port));
    }

    signal_groups(&pgids, TERM);
    if wait_until_free(port, Duration::from_secs(3)) {
        return Ok(Stopped { pgids, graceful: true });
    }
    signal_groups(&pgids, KILL);
    if wait_until_free(port, Duration::from_secs(2)) {
        return Ok(Stopped { pgids, graceful: false });
    }
    Err(PortError::StillInUse { port })
}

/// Distinct process groups of everything listening on `port`. Deduplicated:
/// a dev-server tree usually puts several listeners in one group, and
/// signaling it twice is pointless noise.
fn listening_pgids(port: u16) -> Vec<i32> {
    let mut pgids: Vec<i32> = Vec::new();
    // A process with no row (it died between the `lsof` listing and this
    // call) contributes nothing to signal — `probe_processes` simply omits
    // it, rather than falling back to the bare pid, which would leave its
    // children bound to the port.
    for proc in probe_processes(&listening_pids(port)) {
        if !pgids.contains(&proc.pgid) {
            pgids.push(proc.pgid);
        }
    }
    pgids
}

fn listening_pids(port: u16) -> Vec<i32> {
    // lsof exits 1 with no output when nothing matches, which is the expected
    // answer on a free port — indistinguishable from an lsof-less machine
    // here, and both mean "no pid to offer".
    tt_exec::run("lsof", &["-ti", &format!("tcp:{port}"), "-sTCP:LISTEN"])
        .map(|out| parse_lsof_pids(&out.stdout))
        .unwrap_or_default()
}

/// What one `ps` row tells us about a listener.
struct ProcInfo {
    pgid: i32,
    command: String,
}

/// Look up every pid in one `ps` call rather than one call per pid per field
/// — a `npm run dev` tree leaves three or four listeners on the port, and the
/// pgid and the display name come out of the same row.
fn probe_processes(pids: &[i32]) -> Vec<ProcInfo> {
    if pids.is_empty() {
        return Vec::new();
    }
    let list = pids.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    let Ok(out) = tt_exec::run("ps", &["-o", "pgid=,args=", "-p", &list]) else {
        return Vec::new();
    };
    if !out.ok() {
        return Vec::new();
    }
    out.stdout.lines().filter_map(parse_ps_row).collect()
}

/// One `ps -o pgid=,args=` row: whitespace-padded pgid, then the command
/// line. Rows we can't read a pgid out of are dropped — there is nothing to
/// signal without one.
///
/// **A pgid below 2 is rejected, not signaled.** `signal_groups` negates this
/// value and hands it to `kill(2)`, where the two smallest inputs are not
/// process groups at all but wildcards with no way back: `kill(0, sig)`
/// signals *the calling process's own group* — this app, killing itself
/// mid-removal — and `kill(-1, sig)` signals *every process the user has
/// permission to signal*, i.e. their entire login session. Real `ps` output
/// never yields either (a listener on a TCP port has a normal process group),
/// so this costs nothing in practice; it's here because the blast radius of
/// being wrong once is the whole machine, and the only thing standing between
/// a malformed row and `kill(-1)` is this parse.
fn parse_ps_row(row: &str) -> Option<ProcInfo> {
    let mut fields = row.trim().splitn(2, char::is_whitespace);
    let pgid: i32 = fields.next()?.parse().ok()?;
    if pgid < 2 {
        return None;
    }
    let command = fields.next().and_then(command_name).unwrap_or_else(|| "unknown".to_string());
    Some(ProcInfo { pgid, command })
}

/// `argv[0]`'s basename: `/usr/bin/node -e …` → `node`, `npm run dev` →
/// `npm`.
///
/// From `argv[0]`, not `ps -o comm=`: on Linux `comm` is the *thread* name
/// from `/proc/<pid>/comm`, and Node renames its main thread to `MainThread`
/// — so the single likeliest holder of a slot's port, a `vite`/`npm run dev`
/// server, would introduce itself as "MainThread (pid 1234)".
///
/// Deliberately not the whole command line — a `cargo`/`node` dev server's
/// argv runs to hundreds of characters of absolute paths, which buries the
/// one word the user is scanning for in a dialog row.
fn command_name(args: &str) -> Option<String> {
    let argv0 = args.split_whitespace().next()?;
    let name = argv0.rsplit('/').next().unwrap_or(argv0);
    (!name.is_empty()).then(|| name.to_string())
}

/// One pid per line, as `lsof -t` prints them.
fn parse_lsof_pids(stdout: &str) -> Vec<i32> {
    let mut pids: Vec<i32> = Vec::new();
    for pid in stdout.lines().filter_map(|line| line.trim().parse::<i32>().ok()) {
        if pid > 0 && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

const TERM: i32 = 15;
const KILL: i32 = 9;

/// Signal each process group. A group that's already gone (ESRCH) is the
/// success case, not a failure — it's what we were asking for.
#[cfg(unix)]
fn signal_groups(pgids: &[i32], signal: i32) {
    for &pgid in pgids {
        // Re-checked here, not just in `parse_ps_row`: `kill(0, …)` means
        // "my own process group" and `kill(-1, …)` means "everything I can
        // signal", so a pgid < 2 reaching this line would be unrecoverable
        // (see `parse_ps_row` for the full reasoning). The guard belongs next
        // to the syscall as well as at the parse, because this is the line
        // that cannot be taken back.
        if pgid < 2 {
            continue;
        }
        // SAFETY: `kill(2)` with a negative pid targets a process group; it
        // has no memory-safety implications and reports every failure
        // (nonexistent group, no permission) through errno, which we ignore
        // deliberately — see the doc comment.
        unsafe {
            libc::kill(-pgid, signal);
        }
    }
}

#[cfg(not(unix))]
fn signal_groups(_pgids: &[i32], _signal: i32) {}

fn wait_until_free(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !crate::ops::port_occupied(port) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_pids_are_parsed_and_deduped() {
        assert_eq!(parse_lsof_pids("123\n456\n123\n"), vec![123, 456]);
    }

    #[test]
    fn lsof_noise_is_ignored() {
        // An empty listing (free port) and lsof's occasional warning lines
        // must both come back as "no pids", never a bogus 0.
        assert!(parse_lsof_pids("").is_empty());
        assert!(parse_lsof_pids("lsof: WARNING: can't stat() fuse.gvfsd\n").is_empty());
        assert!(parse_lsof_pids("0\n-1\n").is_empty());
    }

    #[test]
    fn command_name_is_argv0s_basename() {
        assert_eq!(command_name("/usr/bin/node -e require('net')").as_deref(), Some("node"));
        assert_eq!(command_name("npm run dev").as_deref(), Some("npm"));
        // The case that motivated reading argv at all: a vite dev server's
        // `comm` is `MainThread`, so a long absolute path must still reduce
        // to the one word worth showing.
        assert_eq!(
            command_name(
                "/home/x/.nvm/versions/node/v22.0.0/bin/node /home/x/p/node_modules/.bin/vite"
            )
            .as_deref(),
            Some("node")
        );
    }

    #[test]
    fn command_name_declines_when_there_is_nothing_to_name() {
        assert_eq!(command_name(""), None);
        assert_eq!(command_name("   "), None);
        assert_eq!(command_name("/"), None);
    }

    #[test]
    fn holder_reads_as_a_name_and_a_pid() {
        let holder = PortHolder { pid: 4242, command: "node".to_string() };
        assert_eq!(holder.describe(), "node (pid 4242)");
    }

    #[test]
    fn ps_rows_yield_a_pgid_and_a_name() {
        // `ps -o pgid=,args=` right-pads the pgid column, and argv keeps its
        // own spacing — so the split must be "first field, then all the rest".
        let row = parse_ps_row(" 12345 /usr/bin/node /p/node_modules/.bin/vite --host").unwrap();
        assert_eq!(row.pgid, 12345);
        assert_eq!(row.command, "node");
    }

    #[test]
    fn ps_rows_without_a_pgid_are_dropped() {
        // Nothing to signal without a pgid, so these can't become listeners.
        assert!(parse_ps_row("").is_none());
        assert!(parse_ps_row("  PGID COMMAND").is_none());
    }

    #[test]
    fn pgids_that_kill_would_read_as_wildcards_are_refused() {
        // The one parse result that must never reach `kill(2)`: negated, 0
        // becomes "my own process group" (the app kills itself) and 1 becomes
        // -1, "every process this user can signal". Neither is a real pgid,
        // and neither is survivable.
        assert!(parse_ps_row("0 /usr/bin/node server.js").is_none());
        assert!(parse_ps_row("1 /sbin/init").is_none());
        assert!(parse_ps_row("-1 whatever").is_none());
        // The first legitimate pgid still passes.
        assert_eq!(parse_ps_row("2 /usr/bin/node").unwrap().pgid, 2);
    }

    #[test]
    fn a_pgid_with_no_argv_still_signals() {
        // A kernel thread or a process mid-exit can have an empty `args`;
        // it's still a real process group worth signaling.
        let row = parse_ps_row("7 ").unwrap();
        assert_eq!(row.pgid, 7);
        assert_eq!(row.command, "unknown");
    }
}
