//! Claude Desktop `.claude/launch.json` dev-server configs.
//!
//! Claude Desktop's "Set up dev server" flow saves the dev servers it detects
//! in a checkout to `<dir>/.claude/launch.json`: a `version` plus
//! `configurations[]`, each carrying `name`, `runtimeExecutable` (the command
//! — `pnpm`, `npm`, `node`, `python`…), `runtimeArgs`, and the `port` the
//! server listens on. This module reads that same file so a config that works
//! there works here: the app lists a folder's configs, launches one by typing
//! `runtimeExecutable runtimeArgs…` into a PTY (the same way it launches
//! `claude`), and [`port_listening`] tells "already running" apart from
//! "stopped" so a second launch is never offered while something holds the
//! port.
//!
//! The file is owned by Claude Desktop — we read it, never write it — so
//! parsing is deliberately tolerant: every field is defaulted, unknown fields
//! are ignored, and a config we can't launch (empty executable) is kept by the
//! parser and filtered by callers via [`LaunchConfig::launchable`].

use std::path::{Path, PathBuf};

/// `<dir>/.claude/launch.json` — where Claude Desktop saves a checkout's
/// dev-server configs.
pub fn launch_file_path(dir: &Path) -> PathBuf {
    dir.join(".claude").join("launch.json")
}

/// Whether `dir` has a `launch.json` at all — the cheap existence probe
/// [`crate::git_info::compute_git_info`] stamps onto
/// [`crate::types::FolderData`] so the client can gate its dev-servers
/// affordance without reading the file every poll.
pub fn has_launch_file(dir: &Path) -> bool {
    launch_file_path(dir).is_file()
}

/// One dev server / app in `launch.json`'s `configurations[]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchConfig {
    /// Display name, e.g. `"blog"`.
    #[serde(default)]
    pub name: String,
    /// The command itself, e.g. `"pnpm"`. Empty = not launchable.
    #[serde(default)]
    pub runtime_executable: String,
    /// Arguments for the command, e.g. `["--filter", "@x/blog", "dev"]`.
    #[serde(default)]
    pub runtime_args: Vec<String>,
    /// Port the server listens on once up. Two configs may share one (the
    /// blog's `"all"` config covers the same server as `"blog"`), and a
    /// config without a port simply can't be probed.
    #[serde(default)]
    pub port: Option<u16>,
}

impl LaunchConfig {
    /// A config the app can actually start — parsing keeps every entry, this
    /// is the caller-side filter.
    pub fn launchable(&self) -> bool {
        !self.runtime_executable.trim().is_empty()
    }
}

/// The whole `launch.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchFile {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub configurations: Vec<LaunchConfig>,
}

/// Read `<dir>/.claude/launch.json`. `Ok(None)` when the file doesn't exist
/// (the common case — most checkouts have none); `Err` only for a file that
/// exists but can't be read or parsed, so the UI can say "malformed" instead
/// of silently showing nothing.
pub fn read_launch_file(dir: &Path) -> crate::Result<Option<LaunchFile>> {
    let path = launch_file_path(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    Ok(Some(serde_json::from_str(&text)?))
}

/// Whether something is accepting TCP connections on localhost:`port` — the
/// "already running" probe. A connect (not a bind test) so a listener that's
/// genuinely serving counts and nothing else does; both loopback stacks are
/// tried since dev servers bind either. On loopback a closed port refuses
/// immediately, so the timeout only bounds pathological cases.
pub fn port_listening(port: u16) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    let timeout = std::time::Duration::from_millis(250);
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
    .iter()
    .any(|addr| TcpStream::connect_timeout(addr, timeout).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real file Claude Desktop wrote for the blog repo — the reference
    /// fixture this feature exists to be compatible with.
    const BLOG_FIXTURE: &str = r#"{
      "version": "0.0.1",
      "configurations": [
        {
          "name": "blog",
          "runtimeExecutable": "pnpm",
          "runtimeArgs": ["--filter", "@chris-towles/blog", "dev"],
          "port": 3000
        },
        {
          "name": "mcp",
          "runtimeExecutable": "pnpm",
          "runtimeArgs": ["mcp:dev"],
          "port": 8081
        },
        {
          "name": "all",
          "runtimeExecutable": "pnpm",
          "runtimeArgs": ["dev"],
          "port": 3000
        }
      ]
    }"#;

    fn write_launch(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(launch_file_path(dir), text).unwrap();
    }

    #[test]
    fn parses_the_claude_desktop_fixture() {
        let file: LaunchFile = serde_json::from_str(BLOG_FIXTURE).unwrap();
        assert_eq!(file.version, "0.0.1");
        assert_eq!(file.configurations.len(), 3);
        let blog = &file.configurations[0];
        assert_eq!(blog.name, "blog");
        assert_eq!(blog.runtime_executable, "pnpm");
        assert_eq!(blog.runtime_args, vec!["--filter", "@chris-towles/blog", "dev"]);
        assert_eq!(blog.port, Some(3000));
        assert!(blog.launchable());
        // Two configs sharing one port is legal (the fixture does it).
        assert_eq!(file.configurations[2].port, Some(3000));
    }

    #[test]
    fn tolerates_unknown_fields_and_missing_optionals() {
        let file: LaunchFile = serde_json::from_str(
            r#"{
              "version": "0.0.2",
              "futureTopLevel": true,
              "configurations": [
                {"name": "bare", "runtimeExecutable": "npm", "env": {"A": "1"}}
              ]
            }"#,
        )
        .unwrap();
        let cfg = &file.configurations[0];
        assert!(cfg.runtime_args.is_empty());
        assert_eq!(cfg.port, None);
        assert!(cfg.launchable());
    }

    #[test]
    fn empty_executable_is_kept_but_not_launchable() {
        let file: LaunchFile =
            serde_json::from_str(r#"{"configurations": [{"name": "broken"}]}"#).unwrap();
        assert_eq!(file.configurations.len(), 1);
        assert!(!file.configurations[0].launchable());
    }

    #[test]
    fn read_missing_file_is_none() {
        let root = tempfile::TempDir::new().unwrap();
        assert_eq!(read_launch_file(root.path()).unwrap(), None);
    }

    #[test]
    fn read_parses_from_disk() {
        let root = tempfile::TempDir::new().unwrap();
        write_launch(root.path(), BLOG_FIXTURE);
        let file = read_launch_file(root.path()).unwrap().unwrap();
        assert_eq!(file.configurations.len(), 3);
        assert!(has_launch_file(root.path()));
    }

    #[test]
    fn read_malformed_file_is_an_error() {
        let root = tempfile::TempDir::new().unwrap();
        write_launch(root.path(), "{not json");
        assert!(read_launch_file(root.path()).is_err());
    }

    #[test]
    fn has_launch_file_false_without_one() {
        let root = tempfile::TempDir::new().unwrap();
        assert!(!has_launch_file(root.path()));
    }

    #[test]
    fn port_listening_tracks_a_real_listener() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_listening(port));
        drop(listener);
        assert!(!port_listening(port));
    }
}
