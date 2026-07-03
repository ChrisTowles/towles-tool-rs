//! `ttr agentboard setup|uninstall|init|restart|run|keys` — tmux integration
//! lifecycle (phase T5 of docs/AGENTBOARD-TMUX-SPEC.md). Ports slot-1
//! `src/commands/agentboard.ts`.
//!
//! Every place the TS hardcoded `tt` (the tmux.conf run-shell line, key
//! bindings, the restart respawn) uses this binary's absolute path instead —
//! the `spawn("tt")` PATH coupling is what broke the first cutover attempt.

use std::path::PathBuf;
use std::time::Duration;

use tt_agentboard::engine::ingest_addr;
use tt_agentboard::tmux::provider::hook_definitions;
use tt_agentboard::tmux::{STASH_SESSION, TmuxClient};

use crate::commands::agentboard_client::{ensure_server, http_post};
use crate::commands::agentboard_server::PID_FILE;
use crate::ui;

const DEFAULT_KEY: &str = "a";
const KEY_TOGGLE: &str = "t";
const KEY_FOCUS: &str = "s";
const MARKER: &str = "# agentboard";

/// Absolute path of this binary, for tmux.conf lines / binds / respawns.
fn self_exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "ttr".to_string())
}

fn run_shell_line() -> String {
    format!("run-shell '{} agentboard init'", self_exe())
}

fn find_tmux_conf() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidate = home.join(".config/tmux/tmux.conf");
    candidate.exists().then_some(candidate)
}

fn reload_tmux() {
    let reloaded =
        tt_exec::run("tmux", &["source-file", &shellexpand_home(".config/tmux/tmux.conf")])
            .map(|o| o.ok())
            .unwrap_or(false)
            || tt_exec::run("tmux", &["source-file", &shellexpand_home(".tmux.conf")])
                .map(|o| o.ok())
                .unwrap_or(false);
    if reloaded {
        ui::success("tmux config reloaded");
    } else {
        ui::info("Reload tmux manually: tmux source-file ~/.config/tmux/tmux.conf");
    }
}

fn shellexpand_home(rel: &str) -> String {
    dirs::home_dir()
        .map(|h| h.join(rel).to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string())
}

// --- setup / uninstall ---

pub fn setup() -> i32 {
    let Some(conf_path) = find_tmux_conf() else {
        ui::warning("No tmux.conf found. Add this line manually:");
        ui::info(&format!("  {}", run_shell_line()));
        return 0;
    };
    let conf_path = std::fs::canonicalize(&conf_path).unwrap_or(conf_path);

    let Ok(content) = std::fs::read_to_string(&conf_path) else {
        ui::error(&format!("Cannot read {}", conf_path.display()));
        return 1;
    };
    if content.contains(MARKER) {
        ui::success("Already installed in tmux.conf");
        reload_tmux();
        return 0;
    }

    // Insert before the TPM bootstrap when present, else append.
    let tpm_line = "run '~/.config/tmux/plugins/tpm/tpm'";
    let alt_tpm_line = "run-shell '~/.tmux/plugins/tpm/tpm'";
    let insert = format!("\n{MARKER}\n{}\n", run_shell_line());

    let new_content = if content.contains(tpm_line) {
        content.replace(tpm_line, &format!("{insert}\n{tpm_line}"))
    } else if content.contains(alt_tpm_line) {
        content.replace(alt_tpm_line, &format!("{insert}\n{alt_tpm_line}"))
    } else {
        format!("{content}{insert}")
    };

    if let Err(e) = std::fs::write(&conf_path, new_content) {
        ui::error(&format!("Failed to write {}: {e}", conf_path.display()));
        return 1;
    }
    ui::success(&format!("Added agentboard to {}", conf_path.display()));
    reload_tmux();
    keys()
}

pub fn uninstall() -> i32 {
    let Some(conf_path) = find_tmux_conf() else {
        ui::info("No tmux.conf found.");
        return 0;
    };
    let conf_path = std::fs::canonicalize(&conf_path).unwrap_or(conf_path);
    let Ok(content) = std::fs::read_to_string(&conf_path) else {
        ui::error(&format!("Cannot read {}", conf_path.display()));
        return 1;
    };
    if !content.contains(MARKER) && !content.contains("agentboard init'") {
        ui::info("agentboard not found in tmux.conf");
        return 0;
    }

    let mut new_content = content
        .lines()
        .filter(|line| !line.contains(MARKER) && !line.contains("agentboard init'"))
        .collect::<Vec<_>>()
        .join("\n");
    while new_content.contains("\n\n\n") {
        new_content = new_content.replace("\n\n\n", "\n\n");
    }
    if let Err(e) = std::fs::write(&conf_path, new_content) {
        ui::error(&format!("Failed to write {}: {e}", conf_path.display()));
        return 1;
    }
    ui::success("Removed agentboard from tmux.conf");
    reload_tmux();
    0
}

// --- init (runs at tmux startup) ---

pub fn init() -> i32 {
    let (host, port) = ingest_addr();
    let tmux = TmuxClient::new();
    let exe = self_exe();

    // Read the prefix-table key with default.
    let key_out = tmux.run(&["show-option", "-gqv", "@agentboard-key"]).stdout;
    let key = if key_out.is_empty() { DEFAULT_KEY.to_string() } else { key_out };

    // Export to the tmux environment.
    tmux.set_global_env("TT_AGENTBOARD_PORT", &port.to_string());
    tmux.set_global_env("TT_AGENTBOARD_HOST", &host);

    // Keybindings via the "agentboard" command table.
    tmux.run(&[
        "bind-key",
        "-T",
        "prefix",
        &key,
        "switch-client",
        "-T",
        "agentboard",
    ]);
    tmux.run(&[
        "bind-key",
        "-T",
        "agentboard",
        KEY_TOGGLE,
        "run-shell",
        &format!("{exe} agentboard run --toggle"),
    ]);
    tmux.run(&[
        "bind-key",
        "-T",
        "agentboard",
        KEY_FOCUS,
        "run-shell",
        &format!("{exe} agentboard run --focus"),
    ]);

    // Number keys 1-9 switch to session by index.
    for i in 1..=9 {
        let cmd = format!(
            "curl -s -X POST 'http://{host}:{port}/switch-index?index={i}' -d \"$(tmux display-message -p '#{{q:client_tty}}|#{{q:session_name}}|#{{q:window_id}}')\" >/dev/null 2>&1 || true"
        );
        tmux.run(&[
            "bind-key",
            "-T",
            "agentboard",
            &i.to_string(),
            "run-shell",
            &cmd,
        ]);
    }

    // Hooks — the same definitions the server registers.
    for (name, cmd) in hook_definitions(&host, port) {
        tmux.set_global_hook(name, &cmd);
    }
    0
}

// --- restart ---

fn server_alive() -> bool {
    http_post("/refresh", "").is_ok_and(|status| status < 500)
}

fn stop_server() -> bool {
    // Preferred path: terminate the process named in the PID file.
    if let Ok(content) = std::fs::read_to_string(PID_FILE) {
        if let Ok(pid) = content.trim().parse::<i32>() {
            let _ = tt_exec::run("kill", &[&pid.to_string()]);
        }
        let _ = std::fs::remove_file(PID_FILE);
        return true;
    }
    // Fallback: a server is squatting the port without a PID file — ask it to
    // shut itself down so restart replaces it instead of no-oping.
    if server_alive() {
        let _ = http_post("/shutdown", "");
        return true;
    }
    false
}

pub fn restart() -> i32 {
    // 1. Kill stash sessions left over from hidden sidebars.
    let tmux = TmuxClient::new();
    for session in tmux.list_sessions() {
        if session.name.starts_with(STASH_SESSION) {
            tmux.kill_session(&session.name);
            ui::info(&format!("Killed stash session: {}", session.name));
        }
    }

    // 2. Stop the existing server, then start fresh.
    if stop_server() {
        for _ in 0..20 {
            if !server_alive() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Err(e) = ensure_server() {
        ui::error(&e);
        return 1;
    }
    ui::success("Server is running");

    // 3. Bootstrap sidebars: refresh, then ensure-sidebar for each attached
    //    client's window so sidebars appear without interaction.
    let _ = http_post("/refresh", "");
    let clients = tmux
        .run(&[
            "list-clients",
            "-F",
            "#{client_tty}|#{session_name}|#{window_id}",
        ])
        .stdout;
    let lines: Vec<&str> = clients.lines().filter(|l| !l.is_empty()).collect();
    for ctx in &lines {
        let _ = http_post("/ensure-sidebar", ctx);
    }
    ui::success(&format!("Sidebars ensured for {} client(s)", lines.len()));
    0
}

// --- run --toggle / --focus ---

fn tmux_context() -> String {
    TmuxClient::new().display("#{client_tty}|#{session_name}|#{window_id}", None)
}

fn reset_tmux_keys() {
    TmuxClient::new().run(&["switch-client", "-T", "root"]);
}

fn find_sidebar_pane(window_id: &str) -> Option<String> {
    TmuxClient::new()
        .list_panes(tt_agentboard::tmux::client::PaneScope::Window(window_id))
        .into_iter()
        .find(|p| p.title == tt_agentboard::tmux::SIDEBAR_PANE_TITLE)
        .map(|p| p.id)
}

pub fn run_toggle() -> i32 {
    if let Err(e) = ensure_server() {
        ui::error(&e);
        return 0;
    }
    let _ = http_post("/toggle", &tmux_context());
    reset_tmux_keys();
    0
}

pub fn run_focus() -> i32 {
    let tmux = TmuxClient::new();
    let window_id = tmux.current_window_id(None);
    if window_id.is_empty() {
        return 0;
    }

    // If the sidebar already exists, just focus it.
    if let Some(pane_id) = find_sidebar_pane(&window_id) {
        tmux.select_pane(&pane_id);
        reset_tmux_keys();
        return 0;
    }

    // Otherwise ensure server + sidebar, then wait for the pane to appear.
    if let Err(e) = ensure_server() {
        ui::error(&e);
        return 0;
    }
    let _ = http_post("/ensure-sidebar", &tmux_context());
    for _ in 0..20 {
        if let Some(pane_id) = find_sidebar_pane(&window_id) {
            tmux.select_pane(&pane_id);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    reset_tmux_keys();
    0
}

// --- keys ---

pub fn keys() -> i32 {
    let tmux = TmuxClient::new();
    let prefix = {
        let out = tmux.run(&["show-option", "-gv", "prefix"]).stdout;
        if out.is_empty() { "C-a".to_string() } else { out }
    };
    let key = {
        let out = tmux.run(&["show-option", "-gqv", "@agentboard-key"]).stdout;
        if out.is_empty() { DEFAULT_KEY.to_string() } else { out }
    };

    println!("AgentBoard Keybindings\n");
    println!("tmux (prefix = {prefix}, C = Ctrl):");
    println!("  {prefix} {key} {KEY_TOGGLE}     toggle sidebar");
    println!("  {prefix} {key} {KEY_FOCUS}     focus sidebar");
    println!("  {prefix} {key} 1-9   jump to session\n");
    println!("In sidebar:");
    println!("  Tab         cycle sessions");
    println!("  j / ↓       move down");
    println!("  k / ↑       move up");
    println!("  Enter / l   switch to selected session");
    println!("  1-9         jump to session");
    println!("  d           hide session");
    println!("  x           kill session");
    println!("  r           refresh");
    println!("  ?           help");
    println!("  q           quit");
    0
}
