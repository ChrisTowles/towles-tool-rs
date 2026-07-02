//! `ttr doctor`: check the developer tools towles-tool relies on, plus gh auth,
//! required Claude plugins, and AgentBoard state.
//!
//! Ports `src/commands/doctor.ts` + `doctor/checks.ts` + `doctor/history.ts`.
//! Run records serialize to the TS `DoctorRunResult` shape (camelCase) so the
//! `--track`/`--diff` history file stays interoperable with the TypeScript CLI.
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - Output format is selected with `--json` (a bool flag), not TS's `--format json`.
//! - An extra `cargo` tool check is included; `diff` tolerates added/removed tools.

mod history;

use crate::ui;
use console::style;
use serde::{Deserialize, Serialize};

/// Result of probing one tool. Matches the TS `CheckResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    /// Version string, or `null` when the tool wasn't found (serialized as JSON null).
    pub version: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// A `{name, ok}` pair, matching the TS shape used for plugins and agentboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameOk {
    pub name: String,
    pub ok: bool,
}

/// A full doctor run. Serde shape matches the TS `DoctorRunResult` exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorRunResult {
    pub timestamp: String,
    pub tools: Vec<CheckResult>,
    pub gh_auth: bool,
    pub plugins: Vec<NameOk>,
    pub agentboard: Vec<NameOk>,
}

/// Tools to probe: (binary, version arg, optional). Mirrors the TS `runAllChecks`
/// list (git gh node bun claude tmux, ttyd optional) plus an extra `cargo` check.
const TOOLS: &[(&str, &str, bool)] = &[
    ("git", "--version", false),
    ("gh", "--version", false),
    ("node", "--version", false),
    ("bun", "--version", false),
    ("claude", "--version", false),
    ("tmux", "-V", false),
    ("ttyd", "--version", true),
    ("cargo", "--version", false),
];

pub fn run(json: bool, track: bool, diff: bool) -> i32 {
    if !json {
        ui::info("Checking dependencies...");
        println!();
    }

    let result = run_all_checks();

    if json {
        match serde_json::to_string_pretty(&result) {
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

    print_report(&result);

    if track {
        let path = history::history_path();
        if let Err(e) = history::save_history(&path, result.clone()) {
            ui::error(&format!("Failed to save history: {e}"));
        } else {
            println!("\n{}", style("Results saved to history.").dim());
        }
    }

    if diff {
        print_diff(&result);
    }

    // Doctor is informational: always exit 0 so it can be scripted.
    0
}

fn print_report(result: &DoctorRunResult) {
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
    for check in check_claude_plugins() {
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
    println!("{}", style("AgentBoard:").bold());
    let agentboard = check_agentboard();
    for check in &agentboard {
        let icon = status_icon(check.ok, check.warning.is_some());
        println!("{icon} {}: {}", check.name, check.value);
        if let Some(hint) = &check.hint {
            println!("  {}", style(hint).dim());
        }
    }

    let plugins = check_claude_plugins();
    let all_ok = result.tools.iter().all(|c| c.ok || c.warning.is_some())
        && result.gh_auth
        && plugins.iter().all(|c| c.ok)
        && agentboard.iter().all(|c| c.ok || c.warning.is_some());
    println!();
    if all_ok {
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

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

fn run_all_checks() -> DoctorRunResult {
    let tools: Vec<CheckResult> =
        TOOLS.iter().map(|(name, arg, optional)| check_tool(name, arg, *optional)).collect();
    let plugins = check_claude_plugins();
    let agentboard = check_agentboard();

    DoctorRunResult {
        timestamp: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        tools,
        gh_auth: check_gh_auth(),
        plugins: plugins.into_iter().map(|p| NameOk { name: p.name, ok: p.ok }).collect(),
        agentboard: agentboard.into_iter().map(|a| NameOk { name: a.name, ok: a.ok }).collect(),
    }
}

fn check_tool(name: &str, version_arg: &str, optional: bool) -> CheckResult {
    match tt_exec::run(name, &[version_arg]) {
        Ok(output) if output.ok() => {
            let combined = format!("{}{}", output.stdout, output.stderr);
            CheckResult {
                name: name.to_string(),
                version: extract_version(&combined),
                ok: true,
                warning: None,
            }
        }
        _ => CheckResult {
            name: name.to_string(),
            version: None,
            ok: optional,
            warning: optional.then(|| "optional, not installed".to_string()),
        },
    }
}

/// Pull the first version-like token (`1.2.3`) out of arbitrary `--version` output.
fn extract_version(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String =
        text[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if version.is_empty() { None } else { Some(version) }
}

fn check_gh_auth() -> bool {
    matches!(tt_exec::run("gh", &["auth", "status"]), Ok(out) if out.ok())
}

struct PluginCheck {
    name: String,
    ok: bool,
    install_hint: Option<String>,
}

fn check_claude_plugins() -> Vec<PluginCheck> {
    const REQUIRED_ID: &str = "code-simplifier@claude-plugins-official";
    const NAME: &str = "code-simplifier";
    let install_hint = format!("Run: claude plugin install {REQUIRED_ID} --scope user");

    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }

    let installed = match tt_exec::run("claude", &["plugin", "list", "--json"]) {
        Ok(out) if out.ok() => serde_json::from_str::<Vec<Entry>>(&out.stdout)
            .map(|plugins| plugins.iter().any(|p| p.id == REQUIRED_ID))
            .unwrap_or(false),
        _ => false,
    };

    vec![PluginCheck {
        name: NAME.to_string(),
        ok: installed,
        install_hint: if installed { None } else { Some(install_hint) },
    }]
}

struct AgentBoardCheck {
    name: String,
    value: String,
    ok: bool,
    warning: Option<String>,
    hint: Option<String>,
}

fn check_agentboard() -> Vec<AgentBoardCheck> {
    use std::path::PathBuf;

    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config"));
    let default_data_dir = config_home.join("towles-tool").join("agentboard");
    let data_dir =
        std::env::var_os("AGENTBOARD_DATA_DIR").map(PathBuf::from).unwrap_or(default_data_dir);
    let db_path = data_dir.join("agentboard.db");
    let config_path = data_dir.join("config.json");

    let mut results = Vec::new();

    let db_exists = db_path.exists();
    results.push(AgentBoardCheck {
        name: "database".to_string(),
        value: if db_exists { db_path.display().to_string() } else { "not found".to_string() },
        ok: db_exists,
        warning: None,
        hint: if db_exists {
            None
        } else {
            Some("Run: tt ag (starts server and creates DB automatically)".to_string())
        },
    });

    let repo_paths = read_repo_paths(&config_path);
    let has_paths = !repo_paths.is_empty();
    results.push(AgentBoardCheck {
        name: "scan paths".to_string(),
        value: if has_paths { repo_paths.join(", ") } else { "none configured".to_string() },
        ok: has_paths,
        warning: if has_paths { None } else { Some("no scan paths".to_string()) },
        hint: if has_paths {
            None
        } else {
            Some("Run: tt ag → open Workspaces → run the onboarding wizard".to_string())
        },
    });

    results.push(AgentBoardCheck {
        name: "data dir".to_string(),
        value: data_dir.display().to_string(),
        ok: true,
        warning: None,
        hint: None,
    });

    results
}

/// Read `repoPaths` from an AgentBoard `config.json`, tolerating a missing or corrupt file.
fn read_repo_paths(config_path: &std::path::Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    value
        .get("repoPaths")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_version_pulls_first_dotted_token() {
        assert_eq!(extract_version("git version 2.39.0").as_deref(), Some("2.39.0"));
        assert_eq!(extract_version("v20.1.0").as_deref(), Some("20.1.0"));
        assert_eq!(extract_version("tmux 3.3a").as_deref(), Some("3.3"));
        assert_eq!(extract_version("no digits here"), None);
    }

    #[test]
    fn run_result_serializes_to_ts_camelcase_shape() {
        let result = DoctorRunResult {
            timestamp: "2026-07-01T00:00:00.000Z".to_string(),
            tools: vec![CheckResult {
                name: "git".to_string(),
                version: Some("2.39.0".to_string()),
                ok: true,
                warning: None,
            }],
            gh_auth: true,
            plugins: vec![NameOk { name: "code-simplifier".to_string(), ok: true }],
            agentboard: vec![NameOk { name: "database".to_string(), ok: false }],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["ghAuth"], serde_json::json!(true));
        assert_eq!(json["tools"][0]["version"], serde_json::json!("2.39.0"));
        // A not-found tool serializes version as null (field always present).
        assert!(json["tools"][0].get("version").is_some());
    }

    #[test]
    fn not_found_tool_serializes_null_version_and_omits_warning() {
        let check = CheckResult { name: "gh".to_string(), version: None, ok: false, warning: None };
        let json = serde_json::to_value(&check).unwrap();
        assert!(json["version"].is_null());
        assert!(json.get("warning").is_none());
    }
}
