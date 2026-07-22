//! Environment checks for towles-tool ("doctor"): the developer tools the
//! desktop app relies on, gh auth, required Claude plugins, and the
//! agentboard/data-hub state. Tauri-free (the shared-crate rule); the app's
//! Doctor screen is the consumer (the CLI `doctor` command was removed in the
//! 2026-07-19 trim).
//!
//! Run records serialize camelCase (the TS `DoctorRunResult` shape) for the
//! app's IPC/JSON consumers. The tool list follows the current product: the
//! tmux agentboard was removed (2026-07-04, hard cutover), so `tmux`/`ttyd`
//! are no longer checked.

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

/// One agentboard/data-hub state check for the app's Doctor screen. Flattens
/// to [`NameOk`] in the [`DoctorRunResult`] record.
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
///
/// `zig` isn't in this list: it needs more than a presence probe (the `tt-vt`
/// terminal engine requires a specific 0.15.x), so it has its own
/// [`check_zig`], which is appended to the same `tools` row set.
const TOOLS: &[(&str, &str, bool)] = &[
    ("git", "--version", false),
    ("gh", "--version", false),
    ("node", "--version", false),
    ("bun", "--version", false),
    ("claude", "--version", false),
    ("cargo", "--version", false),
];

/// The major.minor of zig required to build the `tt-vt` terminal engine.
/// A machine on a different minor (0.14.x, 0.16.x) can't build it, so the check
/// treats a mismatch as a hard failure, not just "zig missing".
const ZIG_REQUIRED_MAJOR: u32 = 0;
const ZIG_REQUIRED_MINOR: u32 = 15;

/// One worktree task whose work has already landed on the base branch and is
/// safe to reclaim with `tt task clean` / `tt task rm`. The "landed" judgement
/// is [`tt_tasks::ops::work_state`]'s — never a hand-rolled git-merged check —
/// so the squash/rebase/gone-upstream shapes it already untangles stay in one
/// place.
#[derive(Debug, Clone, Serialize)]
pub struct StaleTaskCheck {
    /// Task (worktree folder) name.
    pub name: String,
    /// The task's branch.
    pub branch: String,
    /// How it landed, e.g. `"squash-merged into main"`.
    pub reason: String,
}

/// Everything one doctor run produced: the camelCase [`DoctorRunResult`]
/// record plus the rich plugin/agentboard rows display surfaces render
/// (hints, values). One struct so nothing runs its subprocesses twice.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub result: DoctorRunResult,
    pub plugins: Vec<PluginCheck>,
    pub agentboard: Vec<AgentBoardCheck>,
    /// Worktree tasks whose work has landed but that were never cleaned up.
    pub stale_tasks: Vec<StaleTaskCheck>,
}

/// Run every check. Spawns a handful of `--version`/auth subprocesses, so run
/// it off any latency-sensitive thread.
pub fn run_report() -> DoctorReport {
    let mut tools: Vec<CheckResult> =
        TOOLS.iter().map(|(name, arg, optional)| check_tool(name, arg, *optional)).collect();
    tools.push(check_zig());
    let plugins = check_claude_plugins();
    let agentboard = check_agentboard();
    let stale_tasks = check_stale_tasks();

    let result = DoctorRunResult {
        timestamp: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        tools,
        gh_auth: check_gh_auth(),
        plugins: plugins.iter().map(|p| NameOk { name: p.name.clone(), ok: p.ok }).collect(),
        agentboard: agentboard.iter().map(|a| NameOk { name: a.name.clone(), ok: a.ok }).collect(),
    };
    DoctorReport { result, plugins, agentboard, stale_tasks }
}

/// Worktree tasks in the running checkout whose work has already landed on the
/// base branch and can be reclaimed with `tt task clean`. Read-only: it reuses
/// [`tt_tasks::ops::work_state`] exactly the way the Agentboard rail does (never
/// its own git-merged check) but, unlike `tt task clean`, removes nothing and
/// does no `fetch` — a diagnostic must not mutate. A checkout that isn't
/// task-capable (no `.git` above the cwd) reports nothing.
fn check_stale_tasks() -> Vec<StaleTaskCheck> {
    use tt_tasks::ops;

    let Ok(root) = ops::discover_root(None) else {
        return Vec::new();
    };
    let refs = ops::base_refs(&root.checkout);

    root.tasks()
        .into_iter()
        .filter_map(|(name, dir)| {
            let branch = ops::git_task(&dir, &["branch", "--show-current"])
                .ok()
                .filter(|o| o.ok())
                .map(|o| o.stdout.trim().to_string())?;
            // A detached HEAD has no branch to judge; the base branch itself is
            // never a stale task.
            if branch.is_empty() || branch == refs.base {
                return None;
            }
            let branch_ref = format!("refs/heads/{branch}");
            let work = ops::work_state(
                &refs,
                &dir,
                &branch_ref,
                ops::uncommitted_count(&dir),
                ops::orphaned_count(&dir),
            );
            classify_stale_task(&name, &branch, &refs.base, &work)
        })
        .collect()
}

/// Whether a task's [`WorkState`] means it is safe to `tt task clean`, and the
/// finding to surface if so. Mirrors `clean`'s own keep/remove gate: only
/// *content* proof counts (a bare gone-upstream is indistinguishable from a
/// branch deleted unmerged — see [`tt_tasks::LandedVia::is_content_proof`]),
/// and any work removal would lose — uncommitted, unlanded, or orphaned commits
/// ([`WorkState::holds_work`]) — keeps the task off the list.
fn classify_stale_task(
    name: &str,
    branch: &str,
    base: &str,
    work: &tt_tasks::WorkState,
) -> Option<StaleTaskCheck> {
    let via = work.landed?;
    if !via.is_content_proof() || work.holds_work() {
        return None;
    }
    Some(StaleTaskCheck {
        name: name.to_string(),
        branch: branch.to_string(),
        reason: format!("{} into {base}", via.label()),
    })
}

/// Probe one tool's presence + version.
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

/// Classify the terminal parser's compile-time optimize mode (the caller —
/// the app, which is what links `tt-vt` — passes
/// `tt_vt::parser_optimize_mode()`; the `tt` CLI has no VT engine and never
/// runs this). A Debug-mode Zig parser is ~3 orders of magnitude slower and
/// saturates a core at ~130 KB/s of PTY output, so a busy terminal pegs its
/// engine thread and the whole app reads as laggy. The
/// `[profile.dev.package.libghostty-vt-sys]` override in the workspace
/// Cargo.toml makes dev builds use ReleaseFast; this check exists so losing
/// that override (or a libghostty crate bump changing its build script)
/// surfaces here instead of as months of unexplained dev-build lag.
pub fn check_vt_parser(optimize_mode: &str) -> CheckResult {
    let ok = optimize_mode != "Debug";
    CheckResult {
        name: "vt-parser".to_string(),
        version: Some(optimize_mode.to_string()),
        ok,
        warning: (!ok).then(|| {
            "terminal parser compiled in Zig Debug mode (~1000x slower; busy panes peg a core) \
             — restore the [profile.dev.package.libghostty-vt-sys] override in Cargo.toml"
                .to_string()
        }),
    }
}

/// Pull the first version-like token (`1.2.3`) out of arbitrary `--version` output.
fn extract_version(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String =
        text[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if version.is_empty() { None } else { Some(version) }
}

/// Probe zig's presence *and* that its version is the required 0.15.x.
///
/// Reported as a normal tool row (so it shows alongside the others on the
/// app's Doctor screen), but with real version gating: unlike [`check_tool`], a
/// wrong minor is a failure, not a pass. `zig version` prints just the version
/// (e.g. `0.15.2`), no `--` and no `zig ` prefix, unlike the other tools.
fn check_zig() -> CheckResult {
    match tt_exec::run("zig", &["version"]) {
        Ok(output) if output.ok() => {
            let combined = format!("{}{}", output.stdout, output.stderr);
            zig_result(extract_version(&combined))
        }
        _ => zig_result(None),
    }
}

/// Build the zig row from an already-extracted version. A missing version or one
/// off the required minor is a hard failure (`ok: false`, no warning) so it
/// renders red in both the CLI and the app — a warning would show amber and, in
/// the CLI, still count as passing.
fn zig_result(version: Option<String>) -> CheckResult {
    let ok = version.as_deref().map(zig_version_satisfies).unwrap_or(false);
    CheckResult { name: "zig".to_string(), version, ok, warning: None }
}

/// Whether a dotted version string is on the required zig major.minor. Extra
/// patch/pre-release components (`0.15.0-dev.123`, already trimmed by
/// [`extract_version`] to `0.15.0`) don't matter — only major and minor gate.
fn zig_version_satisfies(version: &str) -> bool {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    matches!(
        (parts.next().flatten(), parts.next().flatten()),
        (Some(ZIG_REQUIRED_MAJOR), Some(ZIG_REQUIRED_MINOR))
    )
}

/// Whether `claude mcp list` output lists the `towles-tool` MCP server.
///
/// The list is plain text, one server per line: `<name>: <url-or-command> -
/// <status>`. Matching the *name* (never the command or URL) is the point — a
/// server whose command merely mentions "towles-tool" must not count.
///
/// The name is everything left of the last colon that precedes the ` - `
/// status, because both halves of the line carry colons of their own:
///
/// ```text
/// towles-tool: http://127.0.0.1:8787/mcp - ✔ Connected
/// plugin:towles-tool-app:towles-tool: tt mcp serve - ✔ Connected
/// ```
///
/// The second form is what this repo's own plugin actually registers, and
/// splitting on the *first* colon yields `plugin` — so the check could never
/// pass for the very registration its fix hint tells the user to install.
/// Plugin-registered names are `plugin:<plugin>:<server>`, so the server name
/// is the last colon-delimited segment of the name field.
fn tt_mcp_registered(list_output: &str) -> bool {
    list_output.lines().any(|line| {
        // `": "` (with the space), not a bare colon: the name prefix and the
        // URL both contain bare colons, and only this separates name from value.
        let Some((name, _value)) = line.split_once(": ") else {
            return false;
        };
        name.rsplit(':').next().map(str::trim) == Some("towles-tool")
    })
}

/// Whether `gh auth status` reports an authenticated account.
fn check_gh_auth() -> bool {
    matches!(tt_exec::run("gh", &["auth", "status"]), Ok(out) if out.ok())
}

/// One Claude plugin the doctor checks for, with a fix hint tailored to how
/// it's actually installed.
struct RequiredPlugin {
    /// Fully-qualified plugin id, e.g. `towles-tool-app@towles-tool`.
    id: &'static str,
    /// Short display name shown in the report.
    name: &'static str,
    /// Shown when missing.
    install_hint: &'static str,
}

/// The one way to install this repo's `towles-tool-app` plugin (which also
/// registers the `tt` MCP server), shared by every hint that suggests it so
/// the marketplace slug and plugin id can't drift between hints. Raw `claude
/// plugin` commands: the `tt install` command that used to wrap them was
/// removed in the 2026-07-19 CLI trim.
const APP_PLUGIN_INSTALL_CMD: &str = "claude plugin marketplace add ChrisTowles/towles-tool-rs \
                                      && claude plugin enable towles-tool-app@towles-tool";

/// Claude plugins the workflows expect: `code-simplifier` (an official
/// plugin some skills shell out to) and this repo's own `towles-tool-app`
/// (registers the `tt` MCP server plus the `gh pr`/`gh issue` mutation nudge
/// hook — see `packages/app`).
const REQUIRED_PLUGINS: &[RequiredPlugin] = &[
    RequiredPlugin {
        id: "code-simplifier@claude-plugins-official",
        name: "code-simplifier",
        install_hint: "Run: claude plugin install code-simplifier@claude-plugins-official --scope user",
    },
    RequiredPlugin {
        id: "towles-tool-app@towles-tool",
        name: "towles-tool-app",
        install_hint: APP_PLUGIN_INSTALL_CMD,
    },
];

/// Claude plugins the workflows expect — see [`REQUIRED_PLUGINS`]. One shared
/// `claude plugin list --json` call, checked against every required id.
fn check_claude_plugins() -> Vec<PluginCheck> {
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }

    let installed_ids: Vec<String> = match tt_exec::run("claude", &["plugin", "list", "--json"]) {
        Ok(out) if out.ok() => serde_json::from_str::<Vec<Entry>>(&out.stdout)
            .map(|plugins| plugins.into_iter().map(|p| p.id).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    REQUIRED_PLUGINS
        .iter()
        .map(|plugin| {
            let ok = installed_ids.iter().any(|id| id == plugin.id);
            PluginCheck {
                name: plugin.name.to_string(),
                ok,
                install_hint: (!ok).then(|| plugin.install_hint.to_string()),
            }
        })
        .collect()
}

/// Agentboard/data-hub state, post-pivot: repos on the rail (the watch list
/// every collector and the rail read), and the data-hub db the day screens
/// read. The old tmux-agentboard db/config checks were retired with that
/// system.
fn check_agentboard() -> Vec<AgentBoardCheck> {
    let mut results = Vec::new();

    // Which state scope this instance resolved to — makes it obvious when a task
    // checkout is reading its own scoped config/db instead of the shared default.
    results.push(AgentBoardCheck {
        name: "state scope".to_string(),
        value: match tt_config::state_scope() {
            Some(scope) => scope,
            None => "default (shared)".to_string(),
        },
        ok: true,
        warning: None,
        hint: None,
    });

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

    let db_path = tt_config::store_db_path().unwrap_or_default();
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

    results.push(check_settings_parse());
    results.push(check_tt_mcp_registered());

    results
}

/// Whether the shared settings file parses. A corrupt settings JSON otherwise
/// only surfaces when a command that loads it dies mid-run; this makes it a
/// visible doctor row. A missing file is fine — it's created with defaults on
/// first use — so only an existing-but-unparseable file fails.
fn check_settings_parse() -> AgentBoardCheck {
    let path = match tt_config::config_path() {
        Ok(path) => path,
        Err(e) => {
            return AgentBoardCheck {
                name: "settings".to_string(),
                value: "path unresolved".to_string(),
                ok: false,
                warning: Some(e.to_string()),
                hint: None,
            };
        }
    };

    if !path.exists() {
        return AgentBoardCheck {
            name: "settings".to_string(),
            value: "not created yet".to_string(),
            ok: true,
            warning: Some("created with defaults on first use".to_string()),
            hint: None,
        };
    }

    match tt_config::load_from(&path) {
        Ok(_) => AgentBoardCheck {
            name: "settings".to_string(),
            value: path.display().to_string(),
            ok: true,
            warning: None,
            hint: None,
        },
        Err(e) => AgentBoardCheck {
            name: "settings".to_string(),
            value: "failed to parse".to_string(),
            ok: false,
            warning: Some(e.to_string()),
            hint: Some(format!("Fix the JSON or reset it: {}", path.display())),
        },
    }
}

/// Whether the `towles-tool` MCP server is registered with Claude Code (`claude mcp
/// list`). The `towles-tool-app` plugin registers it; a missing registration
/// is a warning with the fix, not a hard failure.
fn check_tt_mcp_registered() -> AgentBoardCheck {
    let registered = match tt_exec::run("claude", &["mcp", "list"]) {
        Ok(out) if out.ok() => tt_mcp_registered(&out.stdout),
        _ => false,
    };

    AgentBoardCheck {
        name: "towles-tool MCP".to_string(),
        value: if registered { "registered" } else { "not registered" }.to_string(),
        ok: registered,
        warning: (!registered).then(|| "not registered with Claude Code".to_string()),
        hint: (!registered)
            .then(|| format!("Enable the towles-tool-app plugin: {APP_PLUGIN_INSTALL_CMD}")),
    }
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

    #[test]
    fn zig_has_its_own_versioned_check_not_a_tools_entry() {
        // zig needs version gating, so it's not a plain presence probe in TOOLS.
        let names: Vec<&str> = TOOLS.iter().map(|(n, _, _)| *n).collect();
        assert!(!names.contains(&"zig"), "zig is checked by check_zig, not TOOLS");
    }

    #[test]
    fn zig_version_satisfies_only_the_required_minor() {
        assert!(zig_version_satisfies("0.15.0"));
        assert!(zig_version_satisfies("0.15.2"));
        assert!(zig_version_satisfies("0.15"));
        // extract_version trims a dev suffix to the dotted head before we parse.
        assert!(zig_version_satisfies(extract_version("0.15.0-dev.123+abc").as_deref().unwrap()));

        assert!(!zig_version_satisfies("0.14.0"), "older minor can't build tt-vt");
        assert!(!zig_version_satisfies("0.16.0"), "newer minor can't build tt-vt");
        assert!(!zig_version_satisfies("1.15.0"), "wrong major");
        assert!(!zig_version_satisfies("garbage"));
        assert!(!zig_version_satisfies(""));
    }

    #[test]
    fn zig_result_is_a_hard_failure_when_missing_or_wrong_version() {
        // Missing binary → a clear failure (red), not a soft warning.
        let missing = zig_result(None);
        assert!(!missing.ok);
        assert!(missing.warning.is_none(), "failure renders red, not amber");
        assert!(missing.version.is_none());

        let wrong = zig_result(Some("0.14.0".to_string()));
        assert!(!wrong.ok);
        assert!(wrong.warning.is_none());
        assert_eq!(wrong.version.as_deref(), Some("0.14.0"), "keeps the found version");

        let good = zig_result(Some("0.15.2".to_string()));
        assert!(good.ok);
        assert!(good.warning.is_none());
    }

    #[test]
    fn vt_parser_debug_mode_fails_with_a_restore_hint() {
        let check = check_vt_parser("Debug");
        assert!(!check.ok);
        assert_eq!(check.version.as_deref(), Some("Debug"));
        assert!(
            check.warning.as_deref().unwrap_or_default().contains("libghostty-vt-sys"),
            "the warning must name the Cargo.toml override to restore"
        );
    }

    #[test]
    fn vt_parser_optimized_and_unknown_modes_pass() {
        // "unknown" (failed build-info query) must not scream — only a
        // positively-identified Debug parser is the regression this guards.
        for mode in ["ReleaseFast", "ReleaseSafe", "ReleaseSmall", "unknown"] {
            let check = check_vt_parser(mode);
            assert!(check.ok, "{mode} must pass");
            assert!(check.warning.is_none());
            assert_eq!(check.version.as_deref(), Some(mode));
        }
    }

    #[test]
    fn tt_mcp_registered_matches_the_name_field_only() {
        // The HTTP registration's URL is full of colons, so the name match has
        // to split once rather than assume one colon per line.
        let listed = "\
chrome-devtools: npx chrome-devtools-mcp@latest - ✔ Connected
towles-tool: http://127.0.0.1:8787/mcp - ✔ Connected
";
        assert!(tt_mcp_registered(listed));
    }

    /// The form `claude mcp list` actually prints for a plugin-registered
    /// server — and the only form this repo's own plugin produces, since the
    /// `towles-tool-app` plugin is how the server gets registered at all.
    /// Matching the first colon-delimited field would see `plugin` here and
    /// report "not registered" while the server sits there connected.
    #[test]
    fn tt_mcp_registered_matches_a_plugin_registered_server() {
        let listed = "\
plugin:data:bigquery: https://bigquery.googleapis.com/mcp (HTTP) - ✘ Failed to connect
plugin:towles-tool-app:towles-tool: http://127.0.0.1:8787/mcp - ✔ Connected
";
        assert!(tt_mcp_registered(listed));
    }

    #[test]
    fn tt_mcp_registered_ignores_a_command_that_merely_mentions_the_name() {
        let listed = "somethingelse: npx towles-tool-shim - ✔ Connected\n";
        assert!(!tt_mcp_registered(listed));
    }

    #[test]
    fn required_plugins_cover_code_simplifier_and_the_app_plugin() {
        let ids: Vec<&str> = REQUIRED_PLUGINS.iter().map(|p| p.id).collect();
        assert!(ids.contains(&"code-simplifier@claude-plugins-official"));
        assert!(ids.contains(&"towles-tool-app@towles-tool"));
    }

    #[test]
    fn stale_task_flags_a_landed_clean_task() {
        use tt_tasks::{LandedVia, WorkState};
        let work =
            WorkState { total_commits: 3, landed: Some(LandedVia::Squash), ..Default::default() };
        let check = classify_stale_task("feat-thing", "feat/thing", "main", &work).unwrap();
        assert_eq!(check.name, "feat-thing");
        assert_eq!(check.branch, "feat/thing");
        assert_eq!(check.reason, "squash-merged into main");
    }

    #[test]
    fn stale_task_ignores_an_active_task() {
        use tt_tasks::WorkState;
        // Not landed at all — plain active work.
        let work = WorkState { total_commits: 2, unlanded: 2, landed: None, ..Default::default() };
        assert!(classify_stale_task("t", "feat/x", "main", &work).is_none());
    }

    #[test]
    fn stale_task_ignores_gone_upstream_without_content_proof() {
        use tt_tasks::{LandedVia, WorkState};
        // A gone upstream is indistinguishable from a branch deleted unmerged,
        // so `tt task clean` keeps it — the doctor must not claim it's safe.
        let work = WorkState {
            total_commits: 2,
            unlanded: 2,
            landed: Some(LandedVia::UpstreamGone),
            ..Default::default()
        };
        assert!(classify_stale_task("t", "feat/x", "main", &work).is_none());
    }

    #[test]
    fn stale_task_ignores_landed_task_that_still_holds_work() {
        use tt_tasks::{LandedVia, WorkState};
        // Content landed, but uncommitted changes removal would destroy — kept,
        // matching `clean`'s guard.
        let dirty = WorkState {
            total_commits: 3,
            uncommitted: 2,
            landed: Some(LandedVia::Ancestor),
            ..Default::default()
        };
        assert!(classify_stale_task("t", "feat/x", "main", &dirty).is_none());

        // Orphaned commits (a detached-HEAD's work) are likewise unrecoverable.
        let orphaned = WorkState {
            total_commits: 3,
            orphaned: 1,
            landed: Some(LandedVia::Ancestor),
            ..Default::default()
        };
        assert!(classify_stale_task("t", "feat/x", "main", &orphaned).is_none());
    }

    #[test]
    fn stale_task_reason_names_each_landing_shape() {
        use tt_tasks::{LandedVia, WorkState};
        for (via, want) in [
            (LandedVia::Ancestor, "merged into main"),
            (LandedVia::Patches, "rebase-merged into main"),
            (LandedVia::Squash, "squash-merged into main"),
        ] {
            let work = WorkState { total_commits: 1, landed: Some(via), ..Default::default() };
            let check = classify_stale_task("t", "feat/x", "main", &work).unwrap();
            assert_eq!(check.reason, want);
        }
    }

    #[test]
    fn tt_mcp_registered_is_false_when_absent_or_only_in_command() {
        assert!(!tt_mcp_registered("chrome-devtools: npx chrome-devtools-mcp - ✔ Connected"));
        // A different server whose command merely mentions the name must not match.
        assert!(!tt_mcp_registered("other: towles-tool proxy - ✔ Connected"));
        // The retired stdio registration is no longer what we look for.
        assert!(!tt_mcp_registered("tt: tt mcp serve - ✔ Connected"));
        assert!(!tt_mcp_registered(""));
    }
}
