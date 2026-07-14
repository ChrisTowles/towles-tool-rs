//! The `~/.claude/ide/<port>.lock` discovery file. Claude Code scans this
//! directory, parses the **filename** as the WebSocket port (there is no port
//! field in the JSON), and connects when either `CLAUDE_CODE_SSE_PORT` matches
//! the port or its cwd sits under one of `workspaceFolders` (then it also
//! checks that `pid` is alive). All paths are injected so tests never touch
//! the real home directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Lockfile JSON, camelCase on disk — field names are Claude Code's contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    /// The IDE process that owns the server; the CLI checks it is alive.
    pub pid: u32,
    /// Absolute roots this server speaks for (one: the terminal's cwd).
    pub workspace_folders: Vec<String>,
    /// Shown by the CLI in `/ide` and the status line.
    pub ide_name: String,
    /// Always `"ws"` — tells the CLI to speak WebSocket, not legacy SSE.
    pub transport: String,
    pub running_in_windows: bool,
    /// Bearer secret the CLI must echo in `x-claude-code-ide-authorization`.
    pub auth_token: String,
}

impl Lockfile {
    pub fn new(pid: u32, workspace_folder: &Path, ide_name: &str, auth_token: &str) -> Lockfile {
        Lockfile {
            pid,
            workspace_folders: vec![workspace_folder.to_string_lossy().into_owned()],
            ide_name: ide_name.to_string(),
            transport: "ws".to_string(),
            running_in_windows: cfg!(windows),
            auth_token: auth_token.to_string(),
        }
    }
}

/// The default lock directory (`~/.claude/ide`), honoring `CLAUDE_CONFIG_DIR`
/// the way the CLI does. `home` is injected for testability.
pub fn lock_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("ide")
}

fn lock_path(dir: &Path, port: u16) -> PathBuf {
    dir.join(format!("{port}.lock"))
}

/// Write the lockfile for `port` (dir 0700, file 0600 like the extension's),
/// returning its path.
pub fn write(dir: &Path, port: u16, lockfile: &Lockfile) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let path = lock_path(dir, port);
    let json = serde_json::to_string(lockfile)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// Remove the lockfile for `port`; missing is fine (already cleaned up).
pub fn remove(dir: &Path, port: u16) {
    let _ = fs::remove_file(lock_path(dir, port));
}

/// Sweep lockfiles left behind by crashed towles-tool instances: any
/// `*.lock` in `dir` whose `ideName` matches ours and whose pid is no longer
/// alive. Other IDEs' lockfiles are never touched. Returns how many were
/// removed. `is_pid_alive` is injected so tests control liveness.
pub fn sweep_stale(dir: &Path, ide_name: &str, is_pid_alive: &dyn Fn(u32) -> bool) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(lockfile) = serde_json::from_str::<Lockfile>(&contents) else {
            continue;
        };
        if lockfile.ide_name == ide_name
            && !is_pid_alive(lockfile.pid)
            && fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_wire_schema() {
        let lockfile = Lockfile::new(4242, Path::new("/repo/slot-a"), "Towles Tool", "tok-1");
        let json = serde_json::to_value(&lockfile).unwrap();
        // Field names are Claude Code's contract — assert the exact casing.
        assert_eq!(json["pid"], 4242);
        assert_eq!(json["workspaceFolders"][0], "/repo/slot-a");
        assert_eq!(json["ideName"], "Towles Tool");
        assert_eq!(json["transport"], "ws");
        assert_eq!(json["authToken"], "tok-1");
        assert!(json["runningInWindows"].is_boolean());
    }

    #[test]
    fn write_and_remove_use_the_port_as_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ide");
        let lockfile = Lockfile::new(1, Path::new("/w"), "Towles Tool", "t");
        let path = write(&dir, 34567, &lockfile).unwrap();
        assert_eq!(path.file_name().unwrap(), "34567.lock");
        let parsed: Lockfile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed, lockfile);
        remove(&dir, 34567);
        assert!(!path.exists());
        remove(&dir, 34567); // second removal is a clean no-op
    }

    #[cfg(unix)]
    #[test]
    fn lockfile_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ide");
        let path =
            write(&dir, 40000, &Lockfile::new(1, Path::new("/w"), "Towles Tool", "t")).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn sweep_removes_only_our_dead_lockfiles() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        write(&dir, 1001, &Lockfile::new(11, Path::new("/w"), "Towles Tool", "t")).unwrap();
        write(&dir, 1002, &Lockfile::new(22, Path::new("/w"), "Towles Tool", "t")).unwrap();
        write(&dir, 1003, &Lockfile::new(33, Path::new("/w"), "Visual Studio Code", "t")).unwrap();
        fs::write(dir.join("junk.lock"), "not json").unwrap();

        // pid 22 is "alive"; 11 is dead; 33 belongs to another IDE.
        let removed = sweep_stale(&dir, "Towles Tool", &|pid| pid == 22);
        assert_eq!(removed, 1);
        assert!(!dir.join("1001.lock").exists(), "our dead lockfile swept");
        assert!(dir.join("1002.lock").exists(), "our live lockfile kept");
        assert!(dir.join("1003.lock").exists(), "other IDE's lockfile untouched");
        assert!(dir.join("junk.lock").exists(), "unparseable file untouched");
    }

    #[test]
    fn sweep_of_missing_dir_is_a_no_op() {
        assert_eq!(sweep_stale(Path::new("/no/such/dir"), "Towles Tool", &|_| true), 0);
    }
}
