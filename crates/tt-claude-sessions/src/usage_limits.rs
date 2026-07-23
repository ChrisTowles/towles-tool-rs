//! Reads Claude Code's own cached rate-limit snapshot from `~/.claude.json`'s
//! `cachedUsageUtilization` key — the same percentages the CLI's `/usage`
//! command shows (5h session, 7-day all-model, and any 7-day model-scoped
//! cap), refreshed by the CLI itself from live API response headers whenever
//! a real request goes out. This module never makes a network call of its
//! own; it only reads what the CLI already persisted from its own traffic.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One rate-limit bar, mirroring a `limits[]` entry from the cached snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitBar {
    /// Human label: `"Session"`, `"Week (all models)"`, `"Week (Fable)"`, etc.
    pub label: String,
    pub percent: f64,
    pub resets_at: Option<String>,
    pub is_active: bool,
}

/// The cached snapshot: when the CLI last refreshed it, plus the bars.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimits {
    pub fetched_at_ms: i64,
    pub bars: Vec<UsageLimitBar>,
}

#[derive(Debug, Deserialize)]
struct RawRoot {
    #[serde(rename = "cachedUsageUtilization")]
    cached: Option<RawCache>,
}

#[derive(Debug, Deserialize)]
struct RawCache {
    #[serde(rename = "fetchedAtMs")]
    fetched_at_ms: i64,
    utilization: RawUtilization,
}

#[derive(Debug, Deserialize)]
struct RawUtilization {
    #[serde(default)]
    limits: Vec<RawLimit>,
}

#[derive(Debug, Deserialize)]
struct RawLimit {
    kind: String,
    percent: f64,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    scope: Option<RawScope>,
}

#[derive(Debug, Deserialize)]
struct RawScope {
    model: Option<RawModel>,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    display_name: Option<String>,
}

fn label_for(limit: &RawLimit) -> String {
    match limit.kind.as_str() {
        "session" => "Session".to_string(),
        "weekly_all" => "Week (all models)".to_string(),
        "weekly_scoped" => {
            let model = limit
                .scope
                .as_ref()
                .and_then(|s| s.model.as_ref())
                .and_then(|m| m.display_name.as_deref())
                .unwrap_or("scoped model");
            format!("Week ({model})")
        }
        other => other.to_string(),
    }
}

/// Reads and parses `claude_json_path` (normally `~/.claude.json`). Returns
/// `None` if the file, key, or `limits` array is absent — a first run or an
/// older CLI version predating this cache — never an error: absence is a
/// normal, expected state, not a defect.
pub fn read_cached_usage_limits(claude_json_path: &Path) -> Option<UsageLimits> {
    let bytes = std::fs::read(claude_json_path).ok()?;
    let root: RawRoot = serde_json::from_slice(&bytes).ok()?;
    let cache = root.cached?;
    if cache.utilization.limits.is_empty() {
        return None;
    }
    let bars = cache
        .utilization
        .limits
        .iter()
        .map(|l| UsageLimitBar {
            label: label_for(l),
            percent: l.percent,
            resets_at: l.resets_at.clone(),
            is_active: l.is_active,
        })
        .collect();
    Some(UsageLimits { fetched_at_ms: cache.fetched_at_ms, bars })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn fixture(json: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parses_session_weekly_and_scoped_bars() {
        let f = fixture(
            r#"{
                "cachedUsageUtilization": {
                    "fetchedAtMs": 1784846281062,
                    "utilization": {
                        "limits": [
                            {"kind": "session", "group": "session", "percent": 20, "severity": "normal", "resets_at": "2026-07-23T23:10:00Z", "scope": null, "is_active": false},
                            {"kind": "weekly_all", "group": "weekly", "percent": 52, "severity": "normal", "resets_at": "2026-07-26T17:00:00Z", "scope": null, "is_active": true},
                            {"kind": "weekly_scoped", "group": "weekly", "percent": 42, "severity": "normal", "resets_at": "2026-07-26T17:00:00Z", "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}, "is_active": false}
                        ]
                    }
                }
            }"#,
        );
        let limits = read_cached_usage_limits(f.path()).unwrap();
        assert_eq!(limits.fetched_at_ms, 1784846281062);
        assert_eq!(limits.bars.len(), 3);
        assert_eq!(limits.bars[0].label, "Session");
        assert_eq!(limits.bars[0].percent, 20.0);
        assert_eq!(limits.bars[1].label, "Week (all models)");
        assert!(limits.bars[1].is_active);
        assert_eq!(limits.bars[2].label, "Week (Fable)");
        assert_eq!(limits.bars[2].percent, 42.0);
    }

    #[test]
    fn missing_key_returns_none() {
        let f = fixture(r#"{"numStartups": 3}"#);
        assert!(read_cached_usage_limits(f.path()).is_none());
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(read_cached_usage_limits(Path::new("/nonexistent/does-not-exist.json")).is_none());
    }

    #[test]
    fn empty_limits_returns_none() {
        let f = fixture(
            r#"{"cachedUsageUtilization": {"fetchedAtMs": 1, "utilization": {"limits": []}}}"#,
        );
        assert!(read_cached_usage_limits(f.path()).is_none());
    }
}
