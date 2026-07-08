//! `ttr doctor`: environment checks (tools, gh auth, Claude plugins, agentboard
//! state) plus `--track`/`--diff` history. The checks themselves live in the
//! shared `tt-doctor` crate so the app's Doctor screen runs the exact same
//! probes; this module is the CLI presentation — printing, history, diffing.
//!
//! Ports `src/commands/doctor.ts` + `doctor/checks.ts` + `doctor/history.ts`.
//! Run records serialize to the TS `DoctorRunResult` shape (camelCase) so the
//! `--track`/`--diff` history file stays interoperable with the TypeScript CLI.
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - Output format is selected with `--json` (a bool flag), not TS's `--format json`.
//! - The tool list follows the current product (`cargo` added; `tmux`/`ttyd`
//!   retired with the tmux agentboard); `diff` tolerates added/removed tools.

mod history;

use crate::ui;
use console::style;
use tt_doctor::{DoctorReport, DoctorRunResult};

pub fn run(json: bool, track: bool, diff: bool) -> i32 {
    if !json {
        ui::info("Checking dependencies...");
        println!();
    }

    let report = tt_doctor::run_report();

    if json {
        // --track is still honored in JSON mode, but without a stdout note so the
        // output stays valid JSON. `--json` + `--diff` is rejected by clap (diff
        // output is human-format).
        if track {
            save_history_note(&report.result, true);
        }
        match serde_json::to_string_pretty(&report.result) {
            Ok(out) => {
                println!("{out}");
                return 0;
            }
            Err(e) => {
                ui::error(&format!("Failed to serialize report: {e}"));
                return 1;
            }
        }
    }

    print_report(&report);

    if track {
        save_history_note(&report.result, false);
    }

    if diff {
        print_diff(&report.result);
    }

    // Doctor is informational: always exit 0 so it can be scripted.
    0
}

/// Persist a tracked run. A save error always surfaces on stderr (via `ui`). The
/// success note prints only in text mode — in JSON mode stdout must stay valid
/// JSON, so the run is saved without any stdout note.
fn save_history_note(result: &DoctorRunResult, json: bool) {
    let path = history::history_path();
    match history::save_history(&path, result.clone()) {
        Err(e) => ui::error(&format!("Failed to save history: {e}")),
        Ok(()) if json => {}
        Ok(()) => println!("\n{}", style("Results saved to history.").dim()),
    }
}

fn print_report(report: &DoctorReport) {
    let result = &report.result;
    for check in &result.tools {
        let icon = status_icon(check.ok, check.warning.is_some());
        let version = check.version.as_deref().unwrap_or("not found");
        println!("{icon} {}: {version}", check.name);
        if let Some(warning) = &check.warning {
            println!("  {} {warning}", style("⚠").yellow());
        }
    }

    println!();
    let auth_icon = if result.gh_auth { style("✓").green() } else { style("⚠").yellow() };
    let auth_status = if result.gh_auth { "authenticated" } else { "not authenticated" };
    println!("{auth_icon} gh auth: {auth_status}");
    if !result.gh_auth {
        println!("  {}", style("Run: gh auth login").dim());
    }

    // Node.js version advisory.
    if let Some(node) = result.tools.iter().find(|c| c.name == "node")
        && let Some(version) = &node.version
        && let Some(major) = version.split('.').next().and_then(|s| s.parse::<u32>().ok())
        && major < 18
    {
        println!();
        println!("{} Node.js 18+ recommended (found {version})", style("⚠").yellow());
    }

    println!();
    for check in &report.plugins {
        let icon = if check.ok { style("✓").green() } else { style("✗").red() };
        let status = if check.ok { "installed" } else { "not installed" };
        println!("{icon} claude plugin {}: {status}", check.name);
        if !check.ok
            && let Some(hint) = &check.install_hint
        {
            println!("  {}", style(hint).dim());
        }
    }

    println!();
    println!("{}", style("Agentboard:").bold());
    for check in &report.agentboard {
        let icon = status_icon(check.ok, check.warning.is_some());
        println!("{icon} {}: {}", check.name, check.value);
        if let Some(hint) = &check.hint {
            println!("  {}", style(hint).dim());
        }
    }

    println!();
    if report.all_ok() {
        println!("{}", style("All checks passed!").green());
    } else {
        println!("{}", style("Some checks failed. See above for details.").yellow());
    }
}

fn print_diff(current: &DoctorRunResult) {
    let path = history::history_path();
    let runs = history::load_history(&path);
    let Some(previous) = runs.last() else {
        ui::warning("No previous runs tracked. Use --track to save a run first.");
        return;
    };
    let diffs = history::diff_runs(previous, current);
    println!(
        "\n{}",
        style(format!("Changes since last tracked run ({}):", previous.timestamp)).bold()
    );
    if diffs.is_empty() {
        println!("  {}", style("No changes detected.").dim());
    } else {
        for entry in &diffs {
            println!("  {}", format_diff_entry(entry));
        }
    }
}

/// Green check when ok, yellow warning when a warning is present, otherwise red cross.
fn status_icon(ok: bool, has_warning: bool) -> console::StyledObject<&'static str> {
    if ok {
        style("✓").green()
    } else if has_warning {
        style("⚠").yellow()
    } else {
        style("✗").red()
    }
}

fn format_diff_entry(entry: &history::DiffEntry) -> String {
    use history::Change::*;
    let cat = format!("{}/{}", entry.category, entry.name);
    let old = entry.old_value.as_deref().unwrap_or("");
    let new = entry.new_value.as_deref().unwrap_or("");
    match entry.change {
        Added => {
            let suffix = if new.is_empty() { String::new() } else { format!(" ({new})") };
            format!("{} {cat}: added{suffix}", style("+").green())
        }
        Removed => {
            let suffix = if old.is_empty() { String::new() } else { format!(" (was {old})") };
            format!("{} {cat}: removed{suffix}", style("-").red())
        }
        Upgraded => format!("{} {cat}: {old} → {new}", style("↑").green()),
        Downgraded => format!("{} {cat}: {old} → {new}", style("↓").yellow()),
        Passed => format!("{} {cat}: now passing", style("✓").green()),
        Failed => format!("{} {cat}: now failing", style("✗").red()),
    }
}
