//! Low-level tmux subprocess wrapper. Ports slot-1 `mux-tmux/client.ts`.
//!
//! Every query uses a tab-delimited `-F` format string (tab is universally
//! supported by tmux and cannot appear in the fields we read). The format
//! constants and their row parsers are kept adjacent so they cannot drift.
//!
//! Deviations from the TS:
//! - `TmuxClientOptions.socketName/socketPath/throwOnError` are cut — nothing
//!   ever constructed the client with options (`new TmuxClient()` everywhere).
//! - `run` never throws: a spawn failure becomes `exit_code: -1, ok: false`,
//!   matching the TS catch-all branch.

const SEP: char = '\t';

/// Captured result of one tmux invocation.
#[derive(Debug, Clone)]
pub struct TmuxRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    /// Epoch seconds (`#{session_created}`).
    pub created_at: i64,
    pub attached_clients: u32,
    pub window_count: u32,
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub session_id: String,
    pub session_name: String,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub pane_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub index: u32,
    pub active: bool,
    pub tty: String,
    pub pid: i32,
    pub cwd: String,
    pub command: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub right: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub tty: String,
    pub pid: i32,
    pub session_name: String,
    pub width: u32,
    pub height: u32,
}

// --- Format strings (field order must match the parsers below) ---

pub(crate) const SESSION_FORMAT: &str = "#{session_id}\t#{session_name}\t#{session_created}\t#{session_attached}\t#{session_windows}\t#{session_path}";
pub(crate) const WINDOW_FORMAT: &str = "#{window_id}\t#{session_id}\t#{session_name}\t#{window_index}\t#{window_name}\t#{window_active}\t#{window_panes}";
pub(crate) const PANE_FORMAT: &str = "#{pane_id}\t#{session_name}\t#{window_id}\t#{window_index}\t#{pane_index}\t#{pane_active}\t#{pane_tty}\t#{pane_pid}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_title}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_right}";
pub(crate) const CLIENT_FORMAT: &str = "#{client_name}\t#{client_tty}\t#{client_pid}\t#{session_name}\t#{client_width}\t#{client_height}";

// --- Pure row parsers (fixture-tested) ---

/// Split one output line into fields; missing trailing fields read as "".
fn fields(line: &str, n: usize) -> Vec<&str> {
    let mut parts: Vec<&str> = line.splitn(n, SEP).collect();
    while parts.len() < n {
        parts.push("");
    }
    parts
}

fn int<T: Default + std::str::FromStr>(s: &str) -> T {
    s.parse().unwrap_or_default()
}

fn rows<T>(raw: &str, parse: impl Fn(&str) -> T) -> Vec<T> {
    raw.lines().filter(|l| !l.is_empty()).map(parse).collect()
}

pub(crate) fn parse_sessions(raw: &str) -> Vec<SessionInfo> {
    rows(raw, |line| {
        let f = fields(line, 6);
        SessionInfo {
            id: f[0].to_string(),
            name: f[1].to_string(),
            created_at: int(f[2]),
            attached_clients: int(f[3]),
            window_count: int(f[4]),
            dir: f[5].to_string(),
        }
    })
}

pub(crate) fn parse_windows(raw: &str) -> Vec<WindowInfo> {
    rows(raw, |line| {
        let f = fields(line, 7);
        WindowInfo {
            id: f[0].to_string(),
            session_id: f[1].to_string(),
            session_name: f[2].to_string(),
            index: int(f[3]),
            name: f[4].to_string(),
            active: f[5] == "1",
            pane_count: int(f[6]),
        }
    })
}

pub(crate) fn parse_panes(raw: &str) -> Vec<PaneInfo> {
    rows(raw, |line| {
        let f = fields(line, 15);
        PaneInfo {
            id: f[0].to_string(),
            session_name: f[1].to_string(),
            window_id: f[2].to_string(),
            window_index: int(f[3]),
            index: int(f[4]),
            active: f[5] == "1",
            tty: f[6].to_string(),
            pid: int(f[7]),
            cwd: f[8].to_string(),
            command: f[9].to_string(),
            title: f[10].to_string(),
            width: int(f[11]),
            height: int(f[12]),
            left: int(f[13]),
            right: int(f[14]),
        }
    })
}

pub(crate) fn parse_clients(raw: &str) -> Vec<ClientInfo> {
    rows(raw, |line| {
        let f = fields(line, 6);
        ClientInfo {
            name: f[0].to_string(),
            tty: f[1].to_string(),
            pid: int(f[2]),
            session_name: f[3].to_string(),
            width: int(f[4]),
            height: int(f[5]),
        }
    })
}

/// First `session → cwd` hit wins (tmux lists the active pane first).
pub(crate) fn parse_active_session_dirs(raw: &str) -> indexmap::IndexMap<String, String> {
    let mut dirs = indexmap::IndexMap::new();
    for line in raw.lines().filter(|l| !l.is_empty()) {
        let Some((session, cwd)) = line.split_once(SEP) else {
            continue;
        };
        dirs.entry(session.to_string()).or_insert_with(|| cwd.to_string());
    }
    dirs
}

/// `show-environment -g NAME` prints `NAME=value`; extract the value.
pub(crate) fn parse_env_value(stdout: &str) -> Option<String> {
    stdout.split_once('=').map(|(_, v)| v.to_string())
}

// --- Split-window options ---

#[derive(Debug, Clone, Default)]
pub struct SplitWindowOptions<'a> {
    pub target: &'a str,
    /// `true` = `-v` (vertical); default horizontal (`-h`).
    pub vertical: bool,
    /// "before" = `-b` flag (split left/above); default right/below.
    pub before: bool,
    /// Split against the entire window (`-f`) instead of only the target pane.
    pub full_window: bool,
    pub size: Option<u32>,
    pub command: Option<&'a str>,
}

// --- TmuxClient ---

/// Thin subprocess wrapper. Methods shell out to `tmux`; failures are
/// swallowed to empty results (matching the TS client), so callers decide
/// what absence means.
#[derive(Debug, Clone, Default)]
pub struct TmuxClient;

impl TmuxClient {
    pub fn new() -> Self {
        Self
    }

    /// Run any tmux subcommand and capture the result. Never fails: a spawn
    /// error becomes `exit_code: -1, ok: false`.
    pub fn run(&self, args: &[&str]) -> TmuxRunResult {
        match tt_exec::run("tmux", args) {
            Ok(out) => TmuxRunResult {
                exit_code: out.exit_code,
                ok: out.exit_code == 0,
                stdout: out.stdout.trim().to_string(),
                stderr: out.stderr.trim().to_string(),
            },
            Err(e) => TmuxRunResult {
                exit_code: -1,
                ok: false,
                stdout: String::new(),
                stderr: e.to_string(),
            },
        }
    }

    /// Direct tmux call where only stdout matters (join-pane, resize-window
    /// on stash sessions — operations the TS ran outside format parsing).
    pub fn raw_run(&self, args: &[&str]) -> String {
        self.run(args).stdout
    }

    // ─── Sessions ──────────────────────────────────────

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        parse_sessions(&self.run(&["list-sessions", "-F", SESSION_FORMAT]).stdout)
    }

    /// Create a detached session; returns its name.
    pub fn new_session(&self, name: Option<&str>, cwd: Option<&str>) -> String {
        let mut args = vec!["new-session", "-d"];
        if let Some(n) = name {
            args.extend(["-s", n]);
        }
        if let Some(c) = cwd {
            args.extend(["-c", c]);
        }
        args.extend(["-P", "-F", "#{session_name}"]);
        self.run(&args).stdout
    }

    pub fn kill_session(&self, target: &str) {
        self.run(&["kill-session", "-t", target]);
    }

    pub fn has_session(&self, target: &str) -> bool {
        self.run(&["has-session", "-t", target]).ok
    }

    // ─── Windows ───────────────────────────────────────

    /// All windows (`-a`), or one session's with `session_target`.
    pub fn list_windows(&self, session_target: Option<&str>) -> Vec<WindowInfo> {
        let mut args = vec!["list-windows"];
        match session_target {
            None => args.push("-a"),
            Some(t) => args.extend(["-t", t]),
        }
        args.extend(["-F", WINDOW_FORMAT]);
        parse_windows(&self.run(&args).stdout)
    }

    pub fn kill_window(&self, target: &str) {
        self.run(&["kill-window", "-t", target]);
    }

    // ─── Panes ─────────────────────────────────────────

    /// All panes (`-a`), or scoped to a session (`-s -t`) / window (`-t`).
    pub fn list_panes(&self, scope: PaneScope<'_>) -> Vec<PaneInfo> {
        let mut args = vec!["list-panes"];
        match scope {
            PaneScope::All => args.push("-a"),
            PaneScope::Session(t) => args.extend(["-s", "-t", t]),
            PaneScope::Window(t) => args.extend(["-t", t]),
        }
        args.extend(["-F", PANE_FORMAT]);
        parse_panes(&self.run(&args).stdout)
    }

    pub fn split_window(&self, options: &SplitWindowOptions<'_>) -> Option<PaneInfo> {
        let mut args = vec!["split-window"];
        let dir_flag = match (options.vertical, options.before) {
            (false, false) => "-h",
            (false, true) => "-hb",
            (true, false) => "-v",
            (true, true) => "-vb",
        };
        args.push(dir_flag);
        if options.full_window {
            args.push("-f");
        }
        let size;
        if let Some(s) = options.size {
            size = s.to_string();
            args.extend(["-l", &size]);
        }
        args.extend(["-t", options.target, "-P", "-F", PANE_FORMAT]);
        if let Some(cmd) = options.command {
            args.push(cmd);
        }
        let res = self.run(&args);
        if !res.ok || res.stdout.is_empty() {
            return None;
        }
        parse_panes(&res.stdout).into_iter().next()
    }

    pub fn select_pane(&self, target: &str) {
        self.run(&["select-pane", "-t", target]);
    }

    pub fn set_pane_style(&self, target: &str, style: &str) {
        self.run(&["select-pane", "-t", target, "-P", style]);
    }

    pub fn set_pane_title(&self, target: &str, title: &str) {
        self.run(&["select-pane", "-t", target, "-T", title]);
    }

    pub fn kill_pane(&self, target: &str) {
        self.run(&["kill-pane", "-t", target]);
    }

    pub fn resize_pane(&self, target: &str, width: Option<u32>, height: Option<u32>) {
        let mut args = vec![
            "resize-pane".to_string(),
            "-t".to_string(),
            target.to_string(),
        ];
        if let Some(w) = width {
            args.extend(["-x".to_string(), w.to_string()]);
        }
        if let Some(h) = height {
            args.extend(["-y".to_string(), h.to_string()]);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs);
    }

    // ─── Clients ───────────────────────────────────────

    pub fn list_clients(&self) -> Vec<ClientInfo> {
        parse_clients(&self.run(&["list-clients", "-F", CLIENT_FORMAT]).stdout)
    }

    pub fn switch_client(&self, target: &str, client_tty: Option<&str>) {
        let mut args = vec!["switch-client"];
        if let Some(tty) = client_tty {
            args.extend(["-c", tty]);
        }
        args.extend(["-t", target]);
        self.run(&args);
    }

    // ─── Display / query ───────────────────────────────

    /// `display-message -p <format>`, trimmed.
    pub fn display(&self, format: &str, target: Option<&str>) -> String {
        let mut args = vec!["display-message"];
        if let Some(t) = target {
            args.extend(["-t", t]);
        }
        args.extend(["-p", format]);
        self.run(&args).stdout
    }

    pub fn current_window_id(&self, target: Option<&str>) -> String {
        self.display("#{window_id}", target)
    }

    /// Session name of the first attached client.
    pub fn current_session(&self) -> Option<String> {
        let clients = self.list_clients();
        let first = clients.into_iter().next()?;
        if first.session_name.is_empty() { None } else { Some(first.session_name) }
    }

    /// `pane_current_path` of the target's active pane.
    pub fn session_dir(&self, target: &str) -> String {
        self.display("#{pane_current_path}", Some(target))
    }

    pub fn pane_count(&self, session: &str) -> usize {
        self.list_panes(PaneScope::Session(session)).len()
    }

    /// Active pane's cwd for every session in one `list-panes -a` call,
    /// filtered to non-sidebar panes in active windows. First hit per session
    /// wins (tmux lists the active pane first).
    pub fn active_session_dirs(&self) -> indexmap::IndexMap<String, String> {
        let res = self.run(&[
            "list-panes",
            "-a",
            "-f",
            "#{&&:#{window_active},#{!=:#{pane_title},agentboard-sidebar}}",
            "-F",
            "#{session_name}\t#{pane_current_path}",
        ]);
        parse_active_session_dirs(&res.stdout)
    }

    /// Pane count per session from one `list-panes -a` call.
    pub fn all_pane_counts(&self) -> indexmap::IndexMap<String, usize> {
        let mut counts = indexmap::IndexMap::new();
        for p in self.list_panes(PaneScope::All) {
            *counts.entry(p.session_name).or_insert(0) += 1;
        }
        counts
    }

    // ─── Popups ────────────────────────────────────────

    pub fn display_popup(&self, options: &PopupOptions<'_>) {
        let mut args: Vec<String> = vec!["display-popup".to_string()];
        if let Some(t) = options.title {
            args.extend(["-T".to_string(), t.to_string()]);
        }
        if let Some(w) = options.width {
            args.extend(["-w".to_string(), w.to_string()]);
        }
        if let Some(h) = options.height {
            args.extend(["-h".to_string(), h.to_string()]);
        }
        if let Some(s) = options.style {
            args.extend(["-s".to_string(), s.to_string()]);
        }
        if let Some(b) = options.border_style {
            args.extend(["-b".to_string(), b.to_string()]);
        }
        for (k, v) in options.env {
            args.extend(["-e".to_string(), format!("{k}={v}")]);
        }
        if options.close_on_exit {
            args.push("-E".to_string());
        }
        args.push(options.command.to_string());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs);
    }

    // ─── Hooks ─────────────────────────────────────────

    pub fn set_global_hook(&self, name: &str, command: &str) {
        self.run(&["set-hook", "-g", name, command]);
    }

    pub fn unset_global_hook(&self, name: &str) {
        self.run(&["set-hook", "-gu", name]);
    }

    // ─── Environment ───────────────────────────────────

    pub fn set_global_env(&self, name: &str, value: &str) {
        self.run(&["set-environment", "-g", name, value]);
    }

    pub fn global_env(&self, name: &str) -> Option<String> {
        let res = self.run(&["show-environment", "-g", name]);
        if !res.ok || res.stdout.is_empty() {
            return None;
        }
        parse_env_value(&res.stdout)
    }
}

/// Scope for [`TmuxClient::list_panes`].
#[derive(Debug, Clone, Copy)]
pub enum PaneScope<'a> {
    All,
    Session(&'a str),
    Window(&'a str),
}

/// Options for [`TmuxClient::display_popup`]. `close_on_exit` defaults to
/// true in [`PopupOptions::command`].
#[derive(Debug, Clone)]
pub struct PopupOptions<'a> {
    pub command: &'a str,
    pub title: Option<&'a str>,
    pub width: Option<&'a str>,
    pub height: Option<&'a str>,
    pub style: Option<&'a str>,
    /// rounded | sharp | double | heavy | simple | padded | none
    pub border_style: Option<&'a str>,
    pub env: &'a [(&'a str, &'a str)],
    pub close_on_exit: bool,
}

impl<'a> PopupOptions<'a> {
    pub fn command(command: &'a str) -> Self {
        Self {
            command,
            title: None,
            width: None,
            height: None,
            style: None,
            border_style: None,
            env: &[],
            close_on_exit: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_rows() {
        let raw = "$1\tmain\t1719900000\t2\t3\t/home/u/proj\n$2\tside\t1719900100\t0\t1\t/tmp";
        let sessions = parse_sessions(raw);
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0],
            SessionInfo {
                id: "$1".into(),
                name: "main".into(),
                created_at: 1_719_900_000,
                attached_clients: 2,
                window_count: 3,
                dir: "/home/u/proj".into(),
            }
        );
        assert_eq!(sessions[1].name, "side");
    }

    #[test]
    fn empty_and_blank_lines_yield_nothing() {
        assert!(parse_sessions("").is_empty());
        assert!(parse_panes("\n\n").is_empty());
    }

    #[test]
    fn missing_fields_default_and_bad_ints_are_zero() {
        // Row with only 2 of 6 session fields; numeric garbage.
        let sessions = parse_sessions("$9\tname-only");
        assert_eq!(sessions[0].name, "name-only");
        assert_eq!(sessions[0].created_at, 0);
        assert_eq!(sessions[0].dir, "");
        let sessions = parse_sessions("$9\tx\tnot-a-number\t\t\t/d");
        assert_eq!(sessions[0].created_at, 0);
    }

    #[test]
    fn parses_window_rows_with_active_flag() {
        let raw = "@1\t$1\tmain\t0\tzsh\t1\t2\n@2\t$1\tmain\t1\tvim\t0\t1";
        let windows = parse_windows(raw);
        assert!(windows[0].active);
        assert!(!windows[1].active);
        assert_eq!(windows[1].pane_count, 1);
    }

    #[test]
    fn parses_pane_rows() {
        let raw = "%5\tmain\t@1\t0\t1\t1\t/dev/pts/3\t4242\t/home/u/proj\tnvim\tagentboard-sidebar\t40\t50\t120\t159";
        let panes = parse_panes(raw);
        assert_eq!(
            panes[0],
            PaneInfo {
                id: "%5".into(),
                session_name: "main".into(),
                window_id: "@1".into(),
                window_index: 0,
                index: 1,
                active: true,
                tty: "/dev/pts/3".into(),
                pid: 4242,
                cwd: "/home/u/proj".into(),
                command: "nvim".into(),
                title: "agentboard-sidebar".into(),
                width: 40,
                height: 50,
                left: 120,
                right: 159,
            }
        );
    }

    #[test]
    fn parses_client_rows() {
        let raw = "client0\t/dev/pts/1\t999\tmain\t160\t50";
        let clients = parse_clients(raw);
        assert_eq!(clients[0].tty, "/dev/pts/1");
        assert_eq!(clients[0].session_name, "main");
        assert_eq!(clients[0].width, 160);
    }

    #[test]
    fn active_session_dirs_first_hit_wins() {
        let raw = "main\t/home/u/a\nmain\t/home/u/b\nside\t/tmp";
        let dirs = parse_active_session_dirs(raw);
        assert_eq!(dirs.get("main").unwrap(), "/home/u/a");
        assert_eq!(dirs.get("side").unwrap(), "/tmp");
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn active_session_dirs_skips_malformed_lines() {
        let dirs = parse_active_session_dirs("no-separator-here\nok\t/d");
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs.get("ok").unwrap(), "/d");
    }

    #[test]
    fn env_value_parses_after_first_equals() {
        assert_eq!(parse_env_value("TT_AGENTBOARD_PORT=4201").as_deref(), Some("4201"));
        assert_eq!(parse_env_value("K=a=b").as_deref(), Some("a=b"));
        assert_eq!(parse_env_value("no-equals"), None);
    }

    #[test]
    fn format_field_counts_match_parsers() {
        // Guard against a format string and its parser drifting apart.
        assert_eq!(SESSION_FORMAT.matches("#{").count(), 6);
        assert_eq!(WINDOW_FORMAT.matches("#{").count(), 7);
        assert_eq!(PANE_FORMAT.matches("#{").count(), 15);
        assert_eq!(CLIENT_FORMAT.matches("#{").count(), 6);
    }
}
