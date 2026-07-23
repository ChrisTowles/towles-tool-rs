//! `tt collect` subcommands: calendar, issues, prs, all.
//!
//! Thin CLI boundary over the `tt-collect` crate: open the store, run the
//! requested collector(s), print one line per [`CollectSummary`], and exit 0
//! only if every collector succeeded. Non-interactive by design — no prompts,
//! no TTY requirements.
//!
//! The claude-backed calendar collector is gated on `collectors.calendar.enabled`
//! in settings (it costs tokens); the `gh` issue and PR collectors always run.

use std::io::IsTerminal;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tt_collect::CollectSummary;
use tt_store::{CollectRun, Store};

use crate::cli::{CollectCommands, CollectStatusArgs, NudgeTarget};
use crate::ui;

pub fn run(command: CollectCommands, config_dir: Option<&Path>) -> i32 {
    // Deliberately bypasses the store entirely: this runs inside a Claude Code
    // hook's timeout budget, so it must stay a cheap filesystem touch, not pay
    // for opening (and migrating) tt.db like every other collect subcommand.
    if let CollectCommands::Nudge(args) = &command {
        return run_nudge(args.target, args.trigger.as_deref());
    }

    let store = match Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            ui::error(&format!("Failed to open the data store: {e}"));
            return 1;
        }
    };
    let now = now_ms();
    let collectors = load_collector_settings(config_dir);

    if let CollectCommands::Status(args) = &command {
        return run_status(&store, &collectors, now, args);
    }

    let calendar = collectors.calendar;
    let slack = slack_config(&collectors.slack);

    let summaries = match command {
        CollectCommands::Calendar => {
            if !calendar.enabled {
                note_disabled();
                return 0;
            }
            vec![tt_collect::collect_calendar(&store, &calendar.sources, now)]
        }
        CollectCommands::Issues => {
            vec![tt_collect::collect_issues(
                &store,
                &tt_collect::tracked_repo_dirs(),
                now,
            )]
        }
        CollectCommands::Prs => {
            vec![tt_collect::collect_prs(
                &store,
                &tt_collect::tracked_repo_dirs(),
                now,
            )]
        }
        CollectCommands::Slack => match &slack {
            Some(config) => vec![tt_collect::collect_slack_dm(&store, config, now)],
            None => {
                note_skipped("slack collector disabled in settings (needs enabled + token)");
                return 0;
            }
        },
        CollectCommands::All => {
            let mut summaries = Vec::new();
            if calendar.enabled {
                summaries.push(tt_collect::collect_calendar(&store, &calendar.sources, now));
            } else {
                note_disabled();
            }
            let repos = tt_collect::tracked_repo_dirs();
            summaries.push(tt_collect::collect_issues(&store, &repos, now));
            summaries.push(tt_collect::collect_prs(&store, &repos, now));
            if let Some(config) = &slack {
                summaries.push(tt_collect::collect_slack_dm(&store, config, now));
            }
            summaries
        }
        // Handled by the early return above; never reached here.
        CollectCommands::Nudge(_) | CollectCommands::Status(_) => return 0,
    };

    print_summaries(&summaries);
    if summaries.iter().all(|s| s.ok) { 0 } else { 1 }
}

/// Current wall-clock time in epoch milliseconds. Read at the CLI boundary so
/// the library collectors stay clock-injected and deterministic.
fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Touch one collector's nudge file (creating the nudge dir if needed). The
/// app's scheduler watches this directory and, on a change to `target`'s file,
/// runs that collect immediately instead of waiting for its normal poll
/// cadence — see `crates-tauri/tt-app/src/scheduler.rs`. Content is just the
/// timestamp, for debuggability; only the file's existence/mtime is ever read.
///
/// Emits a `hook.nudge` telemetry event regardless of outcome — this is the
/// only record a `gh pr`/`gh issue` mutation leaves in `tt-telemetry`, since
/// those run as plain Bash-tool subprocesses (via the `gh-pr-nudge.sh`
/// PostToolUse hook), never through `tt-exec`, so no `process.spawn` span
/// exists for the mutation itself.
fn run_nudge(target: NudgeTarget, trigger: Option<&str>) -> i32 {
    let dir = match tt_config::nudge_dir_path() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::info!(
                nudge_target = target.to_collect().key(),
                trigger,
                outcome = "resolve_dir_failed",
                error = %e,
                "hook.nudge"
            );
            ui::error(&format!("Failed to resolve nudge dir: {e}"));
            return 1;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::info!(
            nudge_target = target.to_collect().key(),
            trigger,
            outcome = "create_dir_failed",
            error = %e,
            "hook.nudge"
        );
        ui::error(&format!("Failed to create nudge dir: {e}"));
        return 1;
    }
    match std::fs::write(dir.join(target.to_collect().file_name()), now_ms().to_string()) {
        Ok(()) => {
            tracing::info!(
                nudge_target = target.to_collect().key(),
                trigger,
                outcome = "ok",
                "hook.nudge"
            );
            0
        }
        Err(e) => {
            tracing::info!(
                nudge_target = target.to_collect().key(),
                trigger,
                outcome = "write_failed",
                error = %e,
                "hook.nudge"
            );
            ui::error(&format!("Failed to write nudge file: {e}"));
            1
        }
    }
}

/// Load the collector settings block. Defaults if the settings file can't be
/// resolved or read.
fn load_collector_settings(config_dir: Option<&Path>) -> tt_config::CollectorsSettings {
    let path = match config_dir {
        Some(dir) => dir.join(format!("{}.settings.json", tt_config::TOOL_NAME)),
        None => match tt_config::config_path() {
            Ok(path) => path,
            Err(_) => return tt_config::CollectorsSettings::default(),
        },
    };
    tt_config::load_from(&path).map(|s| s.collectors).unwrap_or_default()
}

/// The Slack collector's runtime config, or `None` when disabled/unconfigured.
fn slack_config(slack: &tt_config::SlackDmCollector) -> Option<tt_collect::SlackDmConfig> {
    if !slack.enabled || slack.token.trim().is_empty() {
        return None;
    }
    Some(tt_collect::SlackDmConfig {
        token: slack.token.clone(),
        watch_user_id: slack.watch_user_id.clone(),
        watch_name: slack.watch_name.clone(),
    })
}

fn note_disabled() {
    note_skipped("calendar collector disabled in settings");
}

fn note_skipped(msg: &str) {
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

// ---------------------------------------------------------------------------
// `collect status`
// ---------------------------------------------------------------------------

/// One collector's health line for `collect status`. `ran_at`/`age_ms`/`ok`/
/// `message` are all `None` when the collector has never run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusRow {
    /// Stable collector key (the `record_run` key), e.g. `claude:calendar`.
    collector: &'static str,
    /// Whether the collector would run given current settings.
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ran_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    age_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// Print each collector's enabled state and last-run health without running any
/// collection. Reuses [`Store::runs`] (the same read MCP's `collect_status`
/// uses) rather than issuing its own SQL.
fn run_status(
    store: &Store,
    collectors: &tt_config::CollectorsSettings,
    now: i64,
    args: &CollectStatusArgs,
) -> i32 {
    let runs = match store.runs() {
        Ok(runs) => runs,
        Err(e) => {
            ui::error(&format!("Failed to read collector runs: {e}"));
            return 1;
        }
    };
    let rows = status_rows(collectors, &runs, now);

    if args.json {
        match serde_json::to_string_pretty(&rows) {
            Ok(json) => {
                println!("{json}");
                0
            }
            Err(e) => {
                ui::error(&format!("Failed to serialize status: {e}"));
                1
            }
        }
    } else {
        print_status_table(&rows);
        0
    }
}

/// Build the fixed, ordered set of collector rows, joining settings (enabled)
/// with the store's last-run records (age/ok/message) by collector key.
fn status_rows(
    collectors: &tt_config::CollectorsSettings,
    runs: &[CollectRun],
    now: i64,
) -> Vec<StatusRow> {
    let enabled = [
        // Enabled *and* something to pull: a calendar collector whose every
        // source is switched off would never run, so reporting it as enabled
        // would be a lie.
        (
            "claude:calendar",
            collectors.calendar.enabled && collectors.calendar.sources.iter().any(|s| s.enabled),
        ),
        ("issues", collectors.issues.enabled),
        ("prs", collectors.prs.enabled),
        ("slack:dm", collectors.slack.enabled && !collectors.slack.token.trim().is_empty()),
    ];

    enabled
        .into_iter()
        .map(|(collector, enabled)| {
            let run = runs.iter().find(|r| r.collector == collector);
            StatusRow {
                collector,
                enabled,
                ran_at: run.map(|r| r.ran_at),
                age_ms: run.map(|r| now - r.ran_at),
                ok: run.map(|r| r.ok),
                message: run.and_then(|r| r.message.clone()),
            }
        })
        .collect()
}

/// Render the human table to stdout, one line per collector: state marker,
/// collector key, ok/FAIL mark, last-run age, and any message. The whole table
/// goes to stdout (a failed collector is a normal status row, not a CLI error),
/// with the mark colored only when stdout is a terminal.
fn print_status_table(rows: &[StatusRow]) {
    let tty = std::io::stdout().is_terminal();
    for row in rows {
        let state = if row.enabled { "enabled " } else { "disabled" };
        let last = match row.age_ms {
            Some(age) => format_age(age),
            None => "never".to_string(),
        };
        let mark = match row.ok {
            Some(true) => "ok",
            Some(false) => "FAIL",
            None => "-",
        };
        let mark = if tty {
            match row.ok {
                Some(true) => console::style(format!("{mark:<4}")).green().to_string(),
                Some(false) => console::style(format!("{mark:<4}")).red().bold().to_string(),
                None => console::style(format!("{mark:<4}")).dim().to_string(),
            }
        } else {
            format!("{mark:<4}")
        };
        let note = row.message.as_deref().map(|m| format!("  {m}")).unwrap_or_default();
        println!("{state}  {:<16} {mark}  {last}{note}", row.collector);
    }
}

/// Format an age in milliseconds as a compact "N<unit> ago" string. Pure so it
/// can be unit-tested; the clock is read once at the CLI boundary and passed in.
fn format_age(age_ms: i64) -> String {
    if age_ms < 0 {
        return "just now".to_string();
    }
    let secs = age_ms / 1000;
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_age_buckets_by_unit() {
        assert_eq!(format_age(-5), "just now");
        assert_eq!(format_age(0), "0s ago");
        assert_eq!(format_age(45_000), "45s ago");
        assert_eq!(format_age(32 * 60_000), "32m ago");
        assert_eq!(format_age(3 * 3_600_000), "3h ago");
        assert_eq!(format_age(2 * 86_400_000), "2d ago");
    }

    #[test]
    fn status_rows_mark_never_run_and_join_by_key() {
        let collectors = tt_config::CollectorsSettings::default();
        let runs = vec![CollectRun {
            collector: "issues".to_string(),
            ran_at: 1_000,
            ok: false,
            message: Some("boom".to_string()),
        }];
        let rows = status_rows(&collectors, &runs, 61_000);

        // Fixed order: calendar, issues, prs, slack.
        assert_eq!(rows[0].collector, "claude:calendar");
        assert_eq!(rows[0].age_ms, None); // never ran
        assert_eq!(rows[0].ok, None);

        assert_eq!(rows[1].collector, "issues");
        assert_eq!(rows[1].age_ms, Some(60_000));
        assert_eq!(rows[1].ok, Some(false));
        assert_eq!(rows[1].message.as_deref(), Some("boom"));

        // Defaults: issues + prs enabled, calendar + slack off.
        assert!(!rows[0].enabled);
        assert!(rows[1].enabled);
        assert!(rows[2].enabled);
        assert!(!rows[3].enabled);
    }
}
