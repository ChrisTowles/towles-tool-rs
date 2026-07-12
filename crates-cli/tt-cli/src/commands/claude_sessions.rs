//! `ttr claude-sessions`: Claude Code session summary across every repo, built
//! from token-accounting over session JSONL files.
//!
//! Ports `src/commands/graph/index.ts`. All pure logic (parsing, analysis,
//! treemap/bar-chart building, JSON/CSV/HTML rendering) lives in the Tauri-free
//! `tt-graph` crate; this layer resolves `~/.claude/projects`, drives output, and
//! opens the generated report in a browser.
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - The local HTTP server (`--serve`/`--port`, `server.ts`) is dropped. This
//!   command only writes an HTML file and opens it in a browser.
//! - Auto-open is skipped when stdout is not a TTY (so tests/CI never launch a
//!   browser), in addition to the explicit `--no-open` flag.
//! - The per-command `--debug` flag is replaced by the global `-v/--verbose` flag.

use crate::cli::ClaudeSessionsArgs;
use crate::ui;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tt_graph::{
    OutputFormat, build_all_sessions_treemap, build_bar_chart_data, build_session_rows,
    build_session_treemap, find_recent_sessions, find_session_path, format_csv, format_json,
    format_markdown, generate_treemap_html, parse_transcript_file, session_result_for_path,
};

/// Max sessions scanned for the all-sessions view (matches the TS `500`).
const SESSION_LIMIT: usize = 500;

pub fn run(args: ClaudeSessionsArgs) -> i32 {
    let Some(format) = OutputFormat::parse(&args.format) else {
        ui::error(&format!("Invalid format \"{}\". Use: html, json, csv, md", args.format));
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
        OutputFormat::Json | OutputFormat::Csv | OutputFormat::Markdown => {
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

    // Markdown is a glanceable summary: with no sessions it still prints the
    // table header and a zeroed totals row (exit 0), rather than erroring like
    // the machine-readable JSON/CSV formats do.
    if sessions.is_empty() && format != OutputFormat::Markdown {
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
        OutputFormat::Markdown => format_markdown(&rows),
        OutputFormat::Html => unreachable!(),
    };
    println!("{output}");
    0
}

/// HTML path: build the treemap, write it under `~/.claude/reports`, and open it.
fn run_html(
    projects_dir: &Path,
    args: &ClaudeSessionsArgs,
    days: f64,
    now_ms: i64,
    now: &chrono::DateTime<chrono::Local>,
) -> i32 {
    let mut bar_chart = tt_graph::BarChartData { days: Vec::new() };

    let treemap = match args.session.as_deref() {
        None => {
            let sessions = match find_recent_sessions(projects_dir, SESSION_LIMIT, days, now_ms) {
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
            bar_chart = build_bar_chart_data(&sessions);
            match build_all_sessions_treemap(&sessions) {
                Ok(treemap) => treemap,
                Err(e) => {
                    ui::error(&format!("Failed to build treemap: {e}"));
                    return 1;
                }
            }
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
            let entries = parse_transcript_file(&path);
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

    if should_open(args.open, args.no_open, std::io::stdout().is_terminal()) {
        ui::info("📈 Opening treemap...");
        open_in_browser(&output_path);
    }
    0
}

/// Decide whether to open the report in a browser: an explicit `--open` always
/// opens (even off a TTY); otherwise open by default unless `--no-open`, and only
/// when stdout is a terminal (so tests/CI never launch a browser). `clap` makes
/// `--open` and `--no-open` mutually exclusive.
fn should_open(open_flag: bool, no_open: bool, is_tty: bool) -> bool {
    open_flag || (!no_open && is_tty)
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

#[cfg(test)]
mod tests {
    use super::should_open;

    #[test]
    fn open_flag_forces_open_even_off_tty() {
        assert!(should_open(true, false, false));
    }

    #[test]
    fn no_open_wins_the_tty_default() {
        assert!(!should_open(false, true, true));
    }

    #[test]
    fn tty_default_opens_without_flags() {
        assert!(should_open(false, false, true));
        assert!(!should_open(false, false, false));
    }
}
