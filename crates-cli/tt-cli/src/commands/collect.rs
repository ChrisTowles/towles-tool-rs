//! `ttr collect` subcommands: calendar, email, prs, all.
//!
//! Thin CLI boundary over the `tt-collect` crate: open the store, run the
//! requested collector(s), print one line per [`CollectSummary`], and exit 0
//! only if every collector succeeded. Non-interactive by design — no prompts,
//! no TTY requirements.
//!
//! The claude-backed collectors (calendar, email) are gated on
//! `assistant.enabled` in settings; the `gh` PR collector always runs.

use std::io::IsTerminal;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use tt_collect::CollectSummary;
use tt_store::Store;

use crate::cli::CollectCommands;
use crate::ui;

pub fn run(command: CollectCommands, config_dir: Option<&Path>) -> i32 {
    let store = match Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            ui::error(&format!("Failed to open the data store: {e}"));
            return 1;
        }
    };
    let now = now_ms();

    let summaries = match command {
        CollectCommands::Calendar => {
            if !assistant_enabled(config_dir) {
                note_disabled();
                return 0;
            }
            vec![tt_collect::collect_calendar(&store, now)]
        }
        CollectCommands::Email => {
            if !assistant_enabled(config_dir) {
                note_disabled();
                return 0;
            }
            tt_collect::collect_email_and_tasks(&store, now)
        }
        CollectCommands::Prs => {
            vec![tt_collect::collect_prs(
                &store,
                &tt_collect::tracked_repo_dirs(),
                now,
            )]
        }
        CollectCommands::All => {
            let mut summaries = Vec::new();
            if assistant_enabled(config_dir) {
                summaries.push(tt_collect::collect_calendar(&store, now));
                summaries.extend(tt_collect::collect_email_and_tasks(&store, now));
            } else {
                note_disabled();
            }
            summaries.push(tt_collect::collect_prs(&store, &tt_collect::tracked_repo_dirs(), now));
            summaries
        }
    };

    print_summaries(&summaries);
    if summaries.iter().all(|s| s.ok) { 0 } else { 1 }
}

/// Current wall-clock time in epoch milliseconds. Read at the CLI boundary so
/// the library collectors stay clock-injected and deterministic.
fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Whether the claude-backed collectors are enabled. Defaults to enabled if the
/// settings file can't be resolved or read.
fn assistant_enabled(config_dir: Option<&Path>) -> bool {
    let path = match config_dir {
        Some(dir) => dir.join(format!("{}.settings.json", tt_config::TOOL_NAME)),
        None => match tt_config::config_path() {
            Ok(path) => path,
            Err(_) => return true,
        },
    };
    tt_config::load_from(&path).map(|s| s.assistant.enabled).unwrap_or(true)
}

fn note_disabled() {
    let msg = "assistant collectors disabled in settings";
    if std::io::stdout().is_terminal() {
        println!("{}", console::style(msg).dim());
    } else {
        println!("{msg}");
    }
}

fn print_summaries(summaries: &[CollectSummary]) {
    for s in summaries {
        if s.ok {
            let note = s.message.as_deref().map(|m| format!(" ({m})")).unwrap_or_default();
            ui::success(&format!("{}: {} item(s){note}", s.collector, s.count));
        } else {
            ui::error(&format!("{}: {}", s.collector, s.message.as_deref().unwrap_or("failed")));
        }
    }
}
