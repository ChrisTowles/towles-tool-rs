//! Port claims: the bind probe, the per-checkout claim lock, and the
//! persistent port registry — one home for "which ports may this checkout
//! hand out". Precedence when picking: the live sibling `.env` scan first,
//! the registry as its persistent backstop, then the OS bind probe at pick
//! time ([`port_occupied`]).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{OpsError, Result, TaskRoot};

const LOCK_FILE: &str = "tt-tasks.lock";
const LOCK_STALE: Duration = Duration::from_secs(60);
pub(crate) const PORT_REGISTRY_FILE: &str = "task-ports.json";

/// A port counts as occupied if EITHER loopback stack refuses the bind with
/// `AddrInUse`: a sibling task's server may hold it on IPv6 (`::1`) while
/// IPv4 (`127.0.0.1`) still looks free (or vice versa), so checking only one
/// stack lets a fresh claim collide with an already-running listener.
/// `PermissionDenied` counts too — a privileged port (<1024) in a pool is
/// one the dev server can't bind either, so handing it out claims a port
/// nothing can use. Any other failure (e.g. IPv6 simply unavailable on this
/// machine) must not make every port look taken.
/// Mirrors `isPortFree` in `scripts/task-port.mjs`, which checks both stacks
/// for the same reasons — keep the two in sync.
pub fn port_occupied(port: u16) -> bool {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    let unusable = |host: &str| {
        matches!(
            TcpListener::bind((host, port)),
            Err(e) if matches!(e.kind(), AddrInUse | PermissionDenied)
        )
    };
    unusable("127.0.0.1") || unusable("::1")
}

// claim lock — serializes port claims across concurrent creations (parallel
// agents create tasks together; without this, both scan siblings before
// either writes, and claim the same ports)

/// One filename per checkout path: `<basename>-<hash>-<file>`, shared by the
/// claim lock and the port registry. The hash only has to be
/// per-checkout-unique, not cryptographic (a collision would conflate two
/// unrelated repos' files — slower or over-conservative, never incorrect),
/// so the stdlib hasher is enough; the checkout's basename is kept as a
/// readable prefix so the file names the repo it belongs to.
pub(crate) fn checkout_keyed_filename(checkout: &Path, file: &str) -> String {
    let mut h = DefaultHasher::new();
    checkout.hash(&mut h);
    let repo = checkout.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    format!("{repo}-{:016x}-{file}", h.finish())
}

/// Path of the claim lock for `checkout`, in `tt_config::locks_dir()`.
/// Deliberately *not* inside the repo's `.git/` — that directory is git's
/// own, and a third-party tool dropping state next to git's index/ref locks
/// is not ours to do.
pub(crate) fn claim_lock_path(checkout: &Path) -> PathBuf {
    tt_config::locks_dir().join(checkout_keyed_filename(checkout, LOCK_FILE))
}

pub(crate) struct ClaimLock {
    path: PathBuf,
}

impl ClaimLock {
    pub(crate) fn acquire(checkout: &Path) -> Result<Self> {
        let path = claim_lock_path(checkout);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", parent.display())))?;
        }
        for _ in 0..100 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(_) => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Err(OpsError::LockTimeout(path.display().to_string()))
    }
}

impl Drop for ClaimLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// port registry — a persistent claimed-port ledger, independent of any one
// task's `.env`
//
// `render_task_env`'s reuse-vs-rotate logic already treats every *other*
// live task's current `.env` as "taken" — but that only holds while the
// sibling's `.env` file still exists and is readable. This ledger is the
// explicit record: every port a task is given lands here at render time
// (both freshly claimed and reused), and normally comes off when the task is
// removed through `tt task rm` ([`release_task_ports`]).
//
// It's self-healing rather than solely authoritative: every load prunes any
// entry whose owning task directory no longer exists ([`load_live_registry`])
// and persists the pruned result immediately. That's load-bearing, not just
// tidiness — `remove_task` requires the directory to still be there
// (`OpsError::NoSuchTask` otherwise), so a task wiped out some other way (a
// stray `rm -rf`, a worktree cleaned up by hand) would leak its ports forever
// without this: `release_task_ports` would never run, and nothing else would
// ever clear the entry. Pruning on every read means a claim can outlive its
// owner only until the next render or removal touches this repo's registry
// at all — never permanently.
//
// Keyed by the checkout like the claim lock ([`checkout_keyed_filename`]),
// but stored under `tt_config::task_ports_dir()` (the shared config area),
// not `locks_dir()` or the repo itself: the ledger must survive reboots (the
// locks dir is the OS temp dir), and it's this machine's state, not
// something a collaborator's clone should ever see. Every writer serializes
// on the checkout's [`ClaimLock`] (render holds it across the whole claim
// cycle; [`release_task_ports`] takes it itself), so read-modify-write
// cycles never interleave and lose entries.
//
// The registry *file path* is threaded into every function here rather than
// re-resolved internally — the public entry points resolve it once via
// [`port_registry_path`], and unit tests pass a temp path so they never
// touch the real config dir.

pub(crate) fn port_registry_path(checkout: &Path) -> Result<PathBuf> {
    let dir = tt_config::task_ports_dir()
        .map_err(|e| OpsError::Io(format!("cannot resolve the port registry dir: {e}")))?;
    Ok(dir.join(checkout_keyed_filename(checkout, PORT_REGISTRY_FILE)))
}

/// port → owning task name (or the primary checkout's own dir name).
/// Missing/unreadable/corrupt reads as empty — a fresh or damaged registry
/// blocks nothing; the live `.env` scan is still the first line of defense.
fn load_port_registry(path: &Path) -> BTreeMap<u16, String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_port_registry(path: &Path, registry: &BTreeMap<u16, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", parent.display())))?;
    }
    let text = serde_json::to_string_pretty(registry)
        .map_err(|e| OpsError::Io(format!("cannot encode port registry: {e}")))?;
    fs::write(path, text).map_err(|e| OpsError::Io(format!("cannot write {}: {e}", path.display())))
}

/// The registry, pruned of any entry whose owner no longer has a directory
/// on disk — the primary checkout's own name always counts as alive (it's
/// where this call is running from). Persists the pruned result when
/// anything was actually dropped, so a leaked entry doesn't linger in the
/// file just because nothing else happens to touch this repo's registry.
fn load_live_registry(sr: &TaskRoot, path: &Path) -> BTreeMap<u16, String> {
    let mut registry = load_port_registry(path);
    let primary = sr.checkout.file_name().and_then(|n| n.to_str());
    let before = registry.len();
    registry.retain(|_, owner| Some(owner.as_str()) == primary || sr.task_dir(owner).is_dir());
    if registry.len() != before {
        let _ = save_port_registry(path, &registry);
    }
    registry
}

/// Record `task`'s current port claims, replacing any previous entries for
/// `task` — called on every render so the registry always matches what
/// `.env` says right now for a still-existing task.
pub(crate) fn record_task_ports(
    sr: &TaskRoot,
    path: &Path,
    task: &str,
    ports: &BTreeSet<u16>,
) -> Result<()> {
    // A task with no claims doesn't materialize a registry file — a repo
    // with no port template would otherwise grow one empty ledger per
    // checkout it ever rendered.
    if ports.is_empty() && !path.exists() {
        return Ok(());
    }
    let mut registry = load_live_registry(sr, path);
    registry.retain(|_, owner| owner != task);
    for &port in ports {
        registry.insert(port, task.to_string());
    }
    save_port_registry(path, &registry)
}

/// Ports the registry says some *other* task holds — merged into
/// `sibling_claims` before picking, so a claim survives even against a
/// sibling whose `.env` this render can't currently read.
pub(crate) fn registry_claims(sr: &TaskRoot, path: &Path, task: &str) -> BTreeSet<u16> {
    load_live_registry(sr, path)
        .into_iter()
        .filter(|(_, owner)| owner != task)
        .map(|(port, _)| port)
        .collect()
}

/// Release every port the registry recorded for `task` — call once removal
/// is certain, so a removed task's ports become claimable again immediately
/// rather than waiting on the next prune. Serializes on the claim lock so it
/// can't interleave with a concurrent render's read-modify-write and drop
/// that render's freshly recorded ports; best-effort — on a lock timeout the
/// release is skipped rather than failing the removal, and the prune in
/// [`load_live_registry`] reclaims the entry later anyway.
pub(crate) fn release_task_ports(checkout: &Path, path: &Path, task: &str) {
    let Ok(_lock) = ClaimLock::acquire(checkout) else {
        return;
    };
    let mut registry = load_port_registry(path);
    let before = registry.len();
    registry.retain(|_, owner| owner != task);
    if registry.len() != before {
        let _ = save_port_registry(path, &registry);
    }
}
