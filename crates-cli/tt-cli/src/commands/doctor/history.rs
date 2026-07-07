//! Doctor run history and diffing.
//!
//! Ports `src/commands/doctor/history.ts`. The history file is SHARED with the
//! TypeScript CLI at `$XDG_CONFIG_HOME/tt/doctor-history.json` (default
//! `~/.config/tt/doctor-history.json` — note `tt`, not `towles-tool`). The
//! [`tt_doctor::DoctorRunResult`] serde shape must match the TS `DoctorRunResult`
//! exactly (camelCase) so both tools can read each other's records.

use std::cmp::Ordering;
use std::path::PathBuf;
use tt_doctor::{DoctorRunResult, NameOk};

const MAX_HISTORY: usize = 50;

/// A single change between two tracked runs.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffEntry {
    pub category: &'static str,
    pub name: String,
    pub change: Change,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Upgraded,
    Downgraded,
    Passed,
    Failed,
}

/// Path to the shared history file, honoring `XDG_CONFIG_HOME` (so tests can redirect it).
pub fn history_path() -> PathBuf {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config"));
    config_dir.join("tt").join("doctor-history.json")
}

/// Load tracked runs. Returns an empty vec when the file is missing or unparseable.
pub fn load_history(path: &std::path::Path) -> Vec<DoctorRunResult> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Append `result`, trim to the last [`MAX_HISTORY`] runs, and write pretty JSON.
pub fn save_history(path: &std::path::Path, result: DoctorRunResult) -> std::io::Result<()> {
    let mut history = load_history(path);
    history.push(result);
    if history.len() > MAX_HISTORY {
        history = history.split_off(history.len() - MAX_HISTORY);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&history)?;
    std::fs::write(path, json)
}

/// Compute the changes from `previous` to `current`. Pure logic, ported 1:1 from `diffRuns`.
pub fn diff_runs(previous: &DoctorRunResult, current: &DoctorRunResult) -> Vec<DiffEntry> {
    let mut entries = Vec::new();

    for curr in &current.tools {
        match previous.tools.iter().find(|t| t.name == curr.name) {
            None => entries.push(DiffEntry {
                category: "tool",
                name: curr.name.clone(),
                change: Change::Added,
                old_value: None,
                new_value: curr.version.clone(),
            }),
            Some(prev) => {
                if prev.version != curr.version
                    && let (Some(pv), Some(cv)) = (&prev.version, &curr.version)
                {
                    let change = if compare_versions(pv, cv) == Ordering::Greater {
                        Change::Downgraded
                    } else {
                        Change::Upgraded
                    };
                    entries.push(DiffEntry {
                        category: "tool",
                        name: curr.name.clone(),
                        change,
                        old_value: prev.version.clone(),
                        new_value: curr.version.clone(),
                    });
                }
                if prev.ok != curr.ok {
                    entries.push(DiffEntry {
                        category: "tool",
                        name: curr.name.clone(),
                        change: if curr.ok { Change::Passed } else { Change::Failed },
                        old_value: Some(pass_fail(prev.ok)),
                        new_value: Some(pass_fail(curr.ok)),
                    });
                }
            }
        }
    }

    for prev in &previous.tools {
        if !current.tools.iter().any(|t| t.name == prev.name) {
            entries.push(DiffEntry {
                category: "tool",
                name: prev.name.clone(),
                change: Change::Removed,
                old_value: prev.version.clone(),
                new_value: None,
            });
        }
    }

    if previous.gh_auth != current.gh_auth {
        entries.push(DiffEntry {
            category: "auth",
            name: "gh auth".to_string(),
            change: if current.gh_auth { Change::Passed } else { Change::Failed },
            old_value: Some(pass_fail(previous.gh_auth)),
            new_value: Some(pass_fail(current.gh_auth)),
        });
    }

    diff_name_ok("plugin", &previous.plugins, &current.plugins, &mut entries);
    diff_name_ok("agentboard", &previous.agentboard, &current.agentboard, &mut entries);

    entries
}

/// Shared added/passed/failed diffing for the `{name, ok}` lists (plugins, agentboard).
fn diff_name_ok(
    category: &'static str,
    previous: &[NameOk],
    current: &[NameOk],
    entries: &mut Vec<DiffEntry>,
) {
    for curr in current {
        match previous.iter().find(|p| p.name == curr.name) {
            None => entries.push(DiffEntry {
                category,
                name: curr.name.clone(),
                change: Change::Added,
                old_value: None,
                new_value: None,
            }),
            Some(prev) if prev.ok != curr.ok => entries.push(DiffEntry {
                category,
                name: curr.name.clone(),
                change: if curr.ok { Change::Passed } else { Change::Failed },
                old_value: None,
                new_value: None,
            }),
            Some(_) => {}
        }
    }
}

fn pass_fail(ok: bool) -> String {
    if ok { "pass".to_string() } else { "fail".to_string() }
}

/// Numeric dotted-version comparison, matching the TS `compareVersions`. Missing
/// components count as 0, and non-numeric components parse as 0 (`Number("")` in JS).
fn compare_versions(a: &str, b: &str) -> Ordering {
    let pa: Vec<u64> = a.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let len = pa.len().max(pb.len());
    for i in 0..len {
        let na = pa.get(i).copied().unwrap_or(0);
        let nb = pb.get(i).copied().unwrap_or(0);
        match na.cmp(&nb) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tt_doctor::CheckResult;

    fn tool(name: &str, version: Option<&str>, ok: bool) -> CheckResult {
        CheckResult {
            name: name.to_string(),
            version: version.map(str::to_string),
            ok,
            warning: None,
        }
    }

    fn run(tools: Vec<CheckResult>, gh_auth: bool) -> DoctorRunResult {
        DoctorRunResult {
            timestamp: "2026-07-01T00:00:00.000Z".to_string(),
            tools,
            gh_auth,
            plugins: vec![],
            agentboard: vec![],
        }
    }

    #[test]
    fn compare_versions_orders_numerically() {
        assert_eq!(compare_versions("1.2.0", "1.10.0"), Ordering::Less);
        assert_eq!(compare_versions("2.0", "2.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("3.3", "3.2.9"), Ordering::Greater);
    }

    #[test]
    fn diff_detects_upgrade_and_downgrade() {
        let prev = run(vec![tool("git", Some("2.39.0"), true)], true);
        let up = run(vec![tool("git", Some("2.40.0"), true)], true);
        let diff = diff_runs(&prev, &up);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].change, Change::Upgraded);
        assert_eq!(diff[0].old_value.as_deref(), Some("2.39.0"));
        assert_eq!(diff[0].new_value.as_deref(), Some("2.40.0"));

        let down = run(vec![tool("git", Some("2.38.0"), true)], true);
        let diff = diff_runs(&prev, &down);
        assert_eq!(diff[0].change, Change::Downgraded);
    }

    #[test]
    fn diff_detects_added_and_removed_tools() {
        let prev = run(vec![tool("git", Some("2.39.0"), true)], true);
        let curr = run(vec![tool("gh", Some("2.40.0"), true)], true);
        let diff = diff_runs(&prev, &curr);
        // gh added, git removed.
        assert!(diff.iter().any(|d| d.change == Change::Added && d.name == "gh"));
        assert!(diff.iter().any(|d| d.change == Change::Removed && d.name == "git"));
    }

    #[test]
    fn diff_detects_pass_fail_transitions() {
        let prev = run(vec![tool("claude", None, false)], true);
        let curr = run(vec![tool("claude", Some("1.0.0"), true)], true);
        let diff = diff_runs(&prev, &curr);
        // version None -> Some doesn't fire (needs both present), but ok flips.
        assert!(diff.iter().any(|d| d.change == Change::Passed && d.name == "claude"));
    }

    #[test]
    fn diff_detects_gh_auth_flip() {
        let prev = run(vec![], true);
        let curr = run(vec![], false);
        let diff = diff_runs(&prev, &curr);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].category, "auth");
        assert_eq!(diff[0].change, Change::Failed);
    }

    #[test]
    fn diff_handles_plugins_and_agentboard() {
        let mut prev = run(vec![], true);
        prev.plugins = vec![NameOk { name: "code-simplifier".to_string(), ok: false }];
        let mut curr = run(vec![], true);
        curr.plugins = vec![NameOk { name: "code-simplifier".to_string(), ok: true }];
        curr.agentboard = vec![NameOk { name: "database".to_string(), ok: true }];
        let diff = diff_runs(&prev, &curr);
        assert!(diff.iter().any(|d| d.category == "plugin" && d.change == Change::Passed));
        assert!(diff.iter().any(|d| d.category == "agentboard" && d.change == Change::Added));
    }

    #[test]
    fn identical_runs_produce_no_diff() {
        let prev = run(vec![tool("git", Some("2.39.0"), true)], true);
        assert!(diff_runs(&prev, &prev).is_empty());
    }

    #[test]
    fn save_trims_to_max_history() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tt").join("doctor-history.json");
        for i in 0..(MAX_HISTORY + 5) {
            let mut r = run(vec![], true);
            r.timestamp = format!("run-{i}");
            save_history(&path, r).unwrap();
        }
        let history = load_history(&path);
        assert_eq!(history.len(), MAX_HISTORY);
        // Oldest 5 dropped; the last entry is the most recent.
        assert_eq!(history.last().unwrap().timestamp, format!("run-{}", MAX_HISTORY + 4));
    }

    #[test]
    fn load_missing_or_invalid_returns_empty() {
        let dir = TempDir::new().unwrap();
        assert!(load_history(&dir.path().join("nope.json")).is_empty());
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_history(&path).is_empty());
    }

    #[test]
    fn history_round_trips_ts_camelcase_shape() {
        // A record written in the TS `DoctorRunResult` shape must load cleanly.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("h.json");
        std::fs::write(
            &path,
            r#"[{"timestamp":"2026-01-01T00:00:00.000Z","tools":[{"name":"git","version":"2.39.0","ok":true}],"ghAuth":true,"plugins":[{"name":"code-simplifier","ok":true}],"agentboard":[{"name":"database","ok":false}]}]"#,
        )
        .unwrap();
        let history = load_history(&path);
        assert_eq!(history.len(), 1);
        assert!(history[0].gh_auth);
        assert_eq!(history[0].tools[0].version.as_deref(), Some("2.39.0"));
        assert_eq!(history[0].plugins[0].name, "code-simplifier");
    }
}
