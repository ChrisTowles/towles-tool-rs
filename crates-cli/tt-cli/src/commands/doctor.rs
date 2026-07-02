//! `ttr doctor`: check that the developer tools towles-tool relies on are present.
//!
//! Ports `src/commands/doctor/checks.ts`: run `<tool> --version`, extract the
//! version string, and report found/missing. `--json` emits a structured payload.

use crate::ui;
use console::style;
use serde::Serialize;

/// A tool to probe and the argument that makes it print its version.
const TOOLS: &[(&str, &str)] = &[
    ("git", "--version"),
    ("gh", "--version"),
    ("tmux", "-V"),
    ("claude", "--version"),
    ("node", "--version"),
    ("bun", "--version"),
    ("cargo", "--version"),
];

#[derive(Serialize)]
struct ToolCheck {
    name: String,
    found: bool,
    version: Option<String>,
}

#[derive(Serialize)]
struct DoctorReport {
    tools: Vec<ToolCheck>,
    all_ok: bool,
}

pub fn run(json: bool) -> i32 {
    let tools: Vec<ToolCheck> = TOOLS.iter().map(|(name, arg)| check_tool(name, arg)).collect();
    let all_ok = tools.iter().all(|t| t.found);
    let report = DoctorReport { tools, all_ok };

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(out) => {
                println!("{out}");
                0
            }
            Err(e) => {
                ui::error(&format!("Failed to serialize report: {e}"));
                1
            }
        }
    } else {
        print_text(&report);
        // Doctor is informational: always exit 0 so it can be scripted without
        // failing a pipeline just because an optional tool is missing.
        0
    }
}

fn check_tool(name: &str, version_arg: &str) -> ToolCheck {
    match tt_exec::run(name, &[version_arg]) {
        Ok(output) if output.ok() => {
            let combined = format!("{}{}", output.stdout, output.stderr);
            ToolCheck { name: name.to_string(), found: true, version: extract_version(&combined) }
        }
        _ => ToolCheck { name: name.to_string(), found: false, version: None },
    }
}

/// Pull the first version-like token (`1.2.3`) out of arbitrary `--version` output.
fn extract_version(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String =
        text[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if version.is_empty() { None } else { Some(version) }
}

fn print_text(report: &DoctorReport) {
    ui::info("Checking dependencies...");
    println!();
    for tool in &report.tools {
        let (icon, version) = if tool.found {
            (style("✓").green(), tool.version.as_deref().unwrap_or("found").to_string())
        } else {
            (style("✗").red(), "not found".to_string())
        };
        println!("{icon} {}: {version}", tool.name);
    }
    println!();
    if report.all_ok {
        ui::success("All checks passed!");
    } else {
        ui::warning("Some tools are missing. See above for details.");
    }
}
