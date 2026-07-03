//! `ttr graph`: token-accounting treemap over Claude Code session JSONL files.
//!
//! Ports `src/commands/graph/index.ts`. All pure logic (parsing, analysis,
//! treemap/bar-chart building, JSON/CSV/HTML rendering) lives in the Tauri-free
//! `tt-graph` crate; this layer resolves `~/.claude/projects`, drives output, and
//! opens the generated report in a browser.
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - The local HTTP server (`--serve`/`--port`, `server.ts`) is dropped. Graph
//!   only writes an HTML file and opens it in a browser.
//! - Auto-open is skipped when stdout is not a TTY (so tests/CI never launch a
//!   browser), in addition to the explicit `--no-open` flag.
//! - The per-command `--debug` flag is replaced by the global `-v/--verbose` flag.

use crate::cli::GraphArgs;
use crate::ui;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tt_graph::{
    OutputFormat, build_all_sessions_treemap, build_bar_chart_data, build_session_rows,
    build_session_treemap, find_recent_sessions, find_session_path, format_csv, format_json,
    generate_treemap_html, read_jsonl, session_result_for_path,
};

/// Max sessions scanned for the all-sessions view (matches the TS `500`).
const SESSION_LIMIT: usize = 500;

pub fn run(args: GraphArgs) -> i32 {
    let Some(format) = OutputFormat::parse(&args.format) else {
        ui::error(&format!("Invalid format \"{}\". Use: html, json, csv", args.format));
        return 1;
    };

    let projects_dir = claude_dir().join("projects");
    if !projects_dir.exists() {
        ui::error("No Claude projects directory found at ~/.claude/projects/");
        return 1;
    }

    let now = chrono::Local::now();
    let now_ms = now.timestamp_millis();
    let days = args.days as f64;

    match format {
        OutputFormat::Json | OutputFormat::Csv => {
            run_rows(&projects_dir, args.session.as_deref(), days, now_ms, format)
        }
        OutputFormat::Html => run_html(&projects_dir, &args, days, now_ms, &now),
    }
}

/// JSON/CSV path: flat session rows to stdout.
fn run_rows(
    projects_dir: &Path,
    session_id: Option<&str>,
    days: f64,
    now_ms: i64,
    format: OutputFormat,
) -> i32 {
    let sessions = match session_id {
        Some(id) => match find_session_path(projects_dir, id) {
            Ok(Some(path)) => match session_result_for_path(id, &path) {
                Ok(session) => vec![session],
                Err(e) => {
                    ui::error(&format!("Failed to read session {id}: {e}"));
                    return 1;
                }
            },
            Ok(None) => {
                ui::error(&format!("Session {id} not found"));
                return 1;
            }
            Err(e) => {
                ui::error(&format!("Failed to search sessions: {e}"));
                return 1;
            }
        },
        None => match find_recent_sessions(projects_dir, SESSION_LIMIT, days, now_ms) {
            Ok(sessions) => sessions,
            Err(e) => {
                ui::error(&format!("Failed to scan sessions: {e}"));
                return 1;
            }
        },
    };

    if sessions.is_empty() {
        ui::error("No sessions found");
        return 1;
    }

    let rows = match build_session_rows(&sessions) {
        Ok(rows) => rows,
        Err(e) => {
            ui::error(&format!("Failed to build session rows: {e}"));
            return 1;
        }
    };

    let output = match format {
        OutputFormat::Json => format_json(&rows),
        OutputFormat::Csv => format_csv(&rows),
        OutputFormat::Html => unreachable!(),
    };
    println!("{output}");
    0
}

/// HTML path: build the treemap, write it under `~/.claude/reports`, and open it.
fn run_html(
    projects_dir: &Path,
    args: &GraphArgs,
    days: f64,
    now_ms: i64,
    now: &chrono::DateTime<chrono::Local>,
) -> i32 {
    let mut bar_chart = tt_graph::BarChartData { days: Vec::new() };

    let treemap = match args.session.as_deref() {
        None => {
            let mut sessions = match find_recent_sessions(projects_dir, SESSION_LIMIT, days, now_ms)
            {
                Ok(sessions) => sessions,
                Err(e) => {
                    ui::error(&format!("Failed to scan sessions: {e}"));
                    return 1;
                }
            };
            if sessions.is_empty() {
                ui::error("No sessions found");
                return 1;
            }
            let days_msg =
                if days > 0.0 { format!(" (last {} days)", args.days) } else { String::new() };
            ui::info(&format!(
                "📊 Generating treemap for {} sessions{days_msg}...",
                sessions.len()
            ));
            // The treemap's parse pass fills session.tokens; build the bar
            // chart afterward so it sees the real totals.
            let treemap = match build_all_sessions_treemap(&mut sessions) {
                Ok(treemap) => treemap,
                Err(e) => {
                    ui::error(&format!("Failed to build treemap: {e}"));
                    return 1;
                }
            };
            bar_chart = build_bar_chart_data(&sessions);
            treemap
        }
        Some(id) => {
            let path = match find_session_path(projects_dir, id) {
                Ok(Some(path)) => path,
                Ok(None) => {
                    ui::error(&format!("Session {id} not found"));
                    return 1;
                }
                Err(e) => {
                    ui::error(&format!("Failed to search sessions: {e}"));
                    return 1;
                }
            };
            ui::info(&format!("📊 Generating treemap for session {id}..."));
            let entries = read_jsonl(&path);
            build_session_treemap(id, &entries)
        }
    };

    let html = generate_treemap_html(&treemap, &bar_chart);

    let reports_dir = claude_dir().join("reports");
    if let Err(e) = std::fs::create_dir_all(&reports_dir) {
        ui::error(&format!("Failed to create reports directory: {e}"));
        return 1;
    }

    // Matches the TS luxon `yyyy-MM-dd'T'HH-mmZZZ` (ZZZ = techie offset like `+0000`,
    // which chrono's `%z` renders identically).
    let timestamp = now.format("%Y-%m-%dT%H-%M%z");
    let label = match args.session.as_deref() {
        Some(id) => format!("treemap-{}-{timestamp}", &id.chars().take(8).collect::<String>()),
        None => {
            let days_label = if days > 0.0 { format!("{}d", args.days) } else { "all".to_string() };
            format!("treemap-{days_label}-{timestamp}")
        }
    };
    let output_path = reports_dir.join(format!("{label}.html"));

    if let Err(e) = std::fs::write(&output_path, html) {
        ui::error(&format!("Failed to write report: {e}"));
        return 1;
    }
    ui::info(&format!("✓ Saved to {}", output_path.display()));

    // Auto-open unless disabled, and never when stdout isn't a terminal.
    if !args.no_open && std::io::stdout().is_terminal() {
        ui::info("📈 Opening treemap...");
        open_in_browser(&output_path);
    }
    0
}

/// `~/.claude`, honoring `$HOME` so tests can redirect it.
fn claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude")
}

/// Open a file in the platform browser. Failures are surfaced but not fatal.
fn open_in_browser(path: &Path) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let path_str = path.to_string_lossy();
    if let Err(e) = tt_exec::run(cmd, &[&path_str]) {
        ui::warning(&format!("Could not open browser ({cmd}): {e}"));
    }
}
