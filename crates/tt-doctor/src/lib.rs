//! Environment checks for towles-tool ("doctor"): the developer tools the CLI
//! and desktop app rely on, gh auth, required Claude plugins, and the
//! agentboard/data-hub state. Tauri-free (the shared-crate rule) so both
//! `ttr doctor` and the app's Doctor screen run the same checks.
//!
//! Run records serialize to the TS `DoctorRunResult` shape (camelCase) so the
//! `--track`/`--diff` history file stays interoperable with the TypeScript CLI.
//! The tool list follows the current product: the tmux agentboard was removed
//! (2026-07-04, hard cutover), so `tmux`/`ttyd` are no longer checked.

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

/// One required Claude plugin check, with an install hint when missing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCheck {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
}

/// One agentboard/data-hub state check for display surfaces (the CLI report
/// and the app's Doctor screen). Flattens to [`NameOk`] in the history record.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBoardCheck {
    pub name: String,
    pub value: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Tools to probe: (binary, version arg, optional).
pub const TOOLS: &[(&str, &str, bool)] = &[
    ("git", "--version", false),
    ("gh", "--version", false),
    ("node", "--version", false),
    ("bun", "--version", false),
    ("claude", "--version", false),
    ("cargo", "--version", false),
];

/// Everything one doctor run produced: the interop-shaped [`DoctorRunResult`]
/// (history/JSON) plus the rich plugin/agentboard rows display surfaces
/// render (hints, values). One struct so nothing runs its subprocesses twice.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub result: DoctorRunResult,
    pub plugins: Vec<PluginCheck>,
    pub agentboard: Vec<AgentBoardCheck>,
}

impl DoctorReport {
    /// Whether every non-warning check passed.
    pub fn all_ok(&self) -> bool {
        self.result.tools.iter().all(|c| c.ok || c.warning.is_some())
            && self.result.gh_auth
            && self.plugins.iter().all(|c| c.ok)
            && self.agentboard.iter().all(|c| c.ok || c.warning.is_some())
    }
}

/// Run every check. Spawns a handful of `--version`/auth subprocesses, so run
/// it off any latency-sensitive thread.
pub fn run_report() -> DoctorReport {
    let tools: Vec<CheckResult> =
        TOOLS.iter().map(|(name, arg, optional)| check_tool(name, arg, *optional)).collect();
    let plugins = check_claude_plugins();
    let agentboard = check_agentboard();

    let result = DoctorRunResult {
        timestamp: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        tools,
        gh_auth: check_gh_auth(),
        plugins: plugins.iter().map(|p| NameOk { name: p.name.clone(), ok: p.ok }).collect(),
        agentboard: agentboard.iter().map(|a| NameOk { name: a.name.clone(), ok: a.ok }).collect(),
    };
    DoctorReport { result, plugins, agentboard }
}

/// The interop-shaped run record alone (history writers, `--json`).
pub fn run_all_checks() -> DoctorRunResult {
    run_report().result
}

/// Probe one tool's presence + version.
pub fn check_tool(name: &str, version_arg: &str, optional: bool) -> CheckResult {
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
pub fn extract_version(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String =
        text[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if version.is_empty() { None } else { Some(version) }
}

/// Whether `gh auth status` reports an authenticated account.
pub fn check_gh_auth() -> bool {
    matches!(tt_exec::run("gh", &["auth", "status"]), Ok(out) if out.ok())
}

/// Claude plugins the workflows expect (currently `code-simplifier`).
pub fn check_claude_plugins() -> Vec<PluginCheck> {
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

/// Agentboard/data-hub state, post-pivot: repos on the rail (the watch list
/// every collector and the rail read), and the data-hub db the day screens
/// read. The old tmux-agentboard db/config checks were retired with that
/// system.
pub fn check_agentboard() -> Vec<AgentBoardCheck> {
    let mut results = Vec::new();

    let repos_path = tt_agentboard::repos::default_repos_path();
    let repos = tt_agentboard::repos::load_repos(&repos_path);
    results.push(AgentBoardCheck {
        name: "repos".to_string(),
        value: if repos.is_empty() {
            "none configured".to_string()
        } else {
            format!("{} on the rail", repos.len())
        },
        ok: !repos.is_empty(),
        warning: repos.is_empty().then(|| "no repos configured".to_string()),
        hint: repos
            .is_empty()
            .then(|| "Add repos in the app: Agentboard → manage repos".to_string()),
    });

    let db_path = dirs::data_dir().unwrap_or_default().join("towles-tool").join("tt.db");
    let db_exists = db_path.exists();
    results.push(AgentBoardCheck {
        name: "data hub".to_string(),
        value: if db_exists {
            db_path.display().to_string()
        } else {
            "not created yet".to_string()
        },
        ok: true,
        warning: (!db_exists).then(|| "created on first app launch / collect run".to_string()),
        hint: None,
    });

    results
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
            agentboard: vec![NameOk { name: "repos".to_string(), ok: false }],
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

    #[test]
    fn optional_tool_missing_is_ok_with_warning() {
        let check = check_tool("definitely-not-a-real-binary-xyz", "--version", true);
        assert!(check.ok);
        assert_eq!(check.warning.as_deref(), Some("optional, not installed"));

        let required = check_tool("definitely-not-a-real-binary-xyz", "--version", false);
        assert!(!required.ok);
        assert!(required.warning.is_none());
    }

    #[test]
    fn tools_list_reflects_the_post_pivot_product() {
        let names: Vec<&str> = TOOLS.iter().map(|(n, _, _)| *n).collect();
        assert!(!names.contains(&"tmux"), "tmux agentboard was removed (hard cutover)");
        assert!(!names.contains(&"ttyd"));
    }
}
