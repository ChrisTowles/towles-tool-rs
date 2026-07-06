//! shpool-backed session persistence. Shells spawned by [`crate::terminal`]
//! run *inside* a session on a dedicated [shpool] daemon, so closing the app
//! (or the app dying) merely disconnects them — the daemon owns the shell
//! process and the next `attach` with the same name resumes it, restoring a
//! screenful of history. The daemon is service-managed (`systemctl --user`)
//! where possible so sessions are never children of the app and get a clean
//! login environment rather than inheriting the app's.
//!
//! One daemon serves every slot's app instance (a per-user socket); session
//! names carry the slot prefix instead, so slots never collide. Everything
//! degrades gracefully: no `shpool` binary → the terminal falls back to a
//! plain direct PTY spawn (nothing survives a restart); no systemd → the
//! attach client auto-daemonizes a background daemon as a last resort.
//!
//! [shpool]: https://github.com/shell-pool/shpool

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Whether the `shpool` binary is usable (checked once per app run).
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("shpool")
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// The dedicated daemon socket: `$XDG_RUNTIME_DIR/towles-tool/shpool.sock`,
/// or `~/.local/run/towles-tool/shpool.sock` on platforms without a runtime
/// dir (macOS). Unix socket paths must stay under ~104 bytes, so this is kept
/// deliberately short — never under a deep per-session directory.
pub fn socket_path() -> PathBuf {
    let base = dirs::runtime_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("run")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("towles-tool").join("shpool.sock")
}

/// Daemon config: no prompt-prefix injection (the rail labels sessions, the
/// shell prompt shouldn't), screenful restore on reattach, and forward the
/// attach client's `TT_SESSION_ID`/`TERM` into newly created session shells —
/// that's how a Claude launched inside the shell is attributed back to its
/// session (see `tt_agentboard::procenv`).
const CONF: &str = r#"# Managed by Towles Tool — rewritten whenever the app spawns a terminal.
prompt_prefix = ""
session_restore_mode = "screen"
forward_env = ["TT_SESSION_ID", "TERM"]
"#;

/// Write (refresh) our daemon config, returning its path. `None` when there
/// is no config dir or the write fails — callers then run shpool with its
/// defaults rather than failing the spawn.
fn conf_path() -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("towles-tool");
    let path = dir.join("shpool.toml");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, CONF).ok()?;
    Some(path)
}

/// The daemon session name for a terminal: `tt-<slot>-<term_id>`. Slot-scoped
/// because every slot's app shares one daemon. Sanitized so ids round-trip
/// exactly between `attach` and `list` (shpool names are also used in socket
/// dir paths, so keep them to a conservative charset).
pub fn session_name(term_id: &str) -> String {
    let sanitized: String = term_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("tt-{}-{}", crate::slot_label(), sanitized)
}

/// argv for the PTY attach client (everything after the `shpool` program):
/// attach-or-create `term_id`'s session on our socket, force-stealing any
/// stale client, rooted at `dir` when the session is first created.
pub fn attach_args(term_id: &str, dir: Option<&std::path::Path>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(conf) = conf_path() {
        args.push("-c".into());
        args.push(conf.to_string_lossy().into_owned());
    }
    args.push("-s".into());
    args.push(socket_path().to_string_lossy().into_owned());
    args.push("attach".into());
    args.push("-f".into());
    if let Some(dir) = dir {
        args.push("-d".into());
        args.push(dir.to_string_lossy().into_owned());
    }
    args.push(session_name(term_id));
    args
}

/// Best-effort kill of the daemon-side session (explicit pane close — the
/// shell and anything in it dies). No-op when shpool is absent or the session
/// is already gone.
pub fn kill_session(term_id: &str) {
    if !available() {
        return;
    }
    let _ = Command::new("shpool")
        .arg("-s")
        .arg(socket_path())
        .arg("kill")
        .arg(session_name(term_id))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Names of every session alive on the daemon (attached or disconnected).
/// Empty when shpool is absent or the daemon isn't running. Compare via
/// [`session_name`].
pub fn live_session_names() -> HashSet<String> {
    if !available() {
        return HashSet::new();
    }
    let Ok(out) = Command::new("shpool")
        .arg("-s")
        .arg(socket_path())
        .args(["list", "--json"])
        .stderr(Stdio::null())
        .output()
    else {
        return HashSet::new();
    };
    if !out.status.success() {
        return HashSet::new();
    }
    parse_list_names(&String::from_utf8_lossy(&out.stdout))
}

/// Whether `term_id` has a session alive on the daemon (in `names` from
/// [`live_session_names`]).
pub fn is_persisted(names: &HashSet<String>, term_id: &str) -> bool {
    names.contains(&session_name(term_id))
}

/// Kill every daemon-side session belonging to this slot ("quit and kill all"
/// from the close dialog). Other slots' sessions on the shared daemon are
/// untouched — the slot prefix is the namespace.
pub fn kill_slot_sessions() {
    let prefix = format!("tt-{}-", crate::slot_label());
    for name in live_session_names().iter().filter(|n| n.starts_with(&prefix)) {
        let _ = Command::new("shpool")
            .arg("-s")
            .arg(socket_path())
            .arg("kill")
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn parse_list_names(json: &str) -> HashSet<String> {
    #[derive(serde::Deserialize)]
    struct Reply {
        #[serde(default)]
        sessions: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        name: String,
    }
    serde_json::from_str::<Reply>(json)
        .map(|r| r.sessions.into_iter().map(|s| s.name).collect())
        .unwrap_or_default()
}

/// Make sure a daemon is serving our socket before an attach. Preference
/// order: already running → install/start a `systemctl --user` unit (the
/// daemon is then owned by the service manager: not our child, login env) →
/// do nothing and let the attach client auto-daemonize one.
pub fn ensure_daemon() {
    if !available() || daemon_running() {
        return;
    }
    #[cfg(target_os = "linux")]
    ensure_systemd_unit();
}

/// Cheap liveness probe: `list` succeeds only when a daemon answers the socket.
fn daemon_running() -> bool {
    Command::new("shpool")
        .arg("-s")
        .arg(socket_path())
        .arg("list")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Render the systemd user unit. The shpool binary path is resolved now (PATH
/// at unit-write time) because ExecStart requires an absolute path.
#[cfg(target_os = "linux")]
fn render_unit(bin: &str) -> String {
    let conf = conf_path().map(|p| format!(" -c {}", p.display())).unwrap_or_default();
    format!(
        "[Unit]\n\
         Description=Towles Tool shpool daemon (persistent agentboard sessions)\n\
         \n\
         [Service]\n\
         ExecStart={bin}{conf} -s {sock} daemon\n\
         RuntimeDirectory=towles-tool\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        sock = socket_path().display(),
    )
}

/// Write + enable `towles-tool-shpool.service` under `systemctl --user`, then
/// wait briefly for the socket to come up. Silent best-effort: any failure
/// just leaves the auto-daemonize fallback to do its thing.
#[cfg(target_os = "linux")]
fn ensure_systemd_unit() {
    let Ok(bin) = which_shpool() else { return };
    let Some(unit_dir) = dirs::config_dir().map(|c| c.join("systemd").join("user")) else {
        return;
    };
    if std::fs::create_dir_all(&unit_dir).is_err() {
        return;
    }
    let unit_path = unit_dir.join("towles-tool-shpool.service");
    if std::fs::write(&unit_path, render_unit(&bin)).is_err() {
        return;
    }
    let systemctl = |args: &[&str]| {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !systemctl(&["daemon-reload"]) {
        return;
    }
    // `enable --now` also restarts nothing if the unit is already active.
    if !systemctl(&["enable", "--now", "towles-tool-shpool.service"]) {
        return;
    }
    for _ in 0..8 {
        if daemon_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Absolute path of the `shpool` binary (`command -v` semantics).
#[cfg(target_os = "linux")]
fn which_shpool() -> Result<String, ()> {
    let out = Command::new("sh").args(["-c", "command -v shpool"]).output().map_err(|_| ())?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !path.is_empty() { Ok(path) } else { Err(()) }
}

#[cfg(test)]
mod tests {
    use super::{parse_list_names, session_name};

    #[test]
    fn session_names_are_slot_prefixed_and_sanitized() {
        let name = session_name("shell 1");
        assert!(name.starts_with("tt-"), "slot prefix: {name}");
        assert!(name.ends_with("-shell_1"), "sanitized id: {name}");
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn session_names_round_trip_for_plain_ids() {
        // A same-charset id must map 1:1 so `list` names match `attach` names.
        assert_eq!(session_name("abc-123"), session_name("abc-123"));
        assert_ne!(session_name("abc-123"), session_name("abc-124"));
    }

    #[test]
    fn parses_list_json() {
        let json = r#"{"sessions":[
            {"name":"tt-slot-0-a","started_at_unix_ms":1,"status":"Disconnected"},
            {"name":"tt-slot-0-b","started_at_unix_ms":2,"status":"Attached"}
        ]}"#;
        let names = parse_list_names(json);
        assert!(names.contains("tt-slot-0-a"));
        assert!(names.contains("tt-slot-0-b"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn parses_empty_and_garbage_list_output() {
        assert!(parse_list_names(r#"{"sessions":[]}"#).is_empty());
        assert!(parse_list_names("not json").is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unit_points_at_our_socket_and_daemon_mode() {
        let unit = super::render_unit("/usr/bin/shpool");
        assert!(unit.contains("ExecStart=/usr/bin/shpool"));
        assert!(unit.contains(" -s "));
        assert!(unit.trim_end().ends_with("WantedBy=default.target"));
        assert!(unit.contains(" daemon\n"));
    }
}
