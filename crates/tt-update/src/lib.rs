//! Update check against the GitHub Releases API: fetch the latest release for
//! a `owner/repo` and compare its tag against the running app's version.
//! Tauri-free (the shared-crate rule) so the app's update banner and any
//! future `tt` CLI surface can share it.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to initialize native TLS: {0}")]
    Tls(String),
    #[error("update check request failed: {0}")]
    Request(String),
    #[error("GitHub returned an unexpected release payload: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Per-call HTTP cap. A hung update check must not block app startup.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// GitHub API requires a `User-Agent` header on every request.
const USER_AGENT: &str = "towles-tool-update-check";

/// The shared HTTP agent, built once with native-tls rather than ureq's default
/// rustls+webpki-roots stack. Corporate TLS-inspecting proxies (Zscaler and
/// similar) inject their own root CA into the OS trust store, not into
/// rustls's bundled Mozilla roots — native-tls verifies against the OS store,
/// so intercepted networks still work. Mirrors `tt_collect::slack::agent`.
fn agent() -> Result<&'static ureq::Agent> {
    static AGENT: OnceLock<Result<ureq::Agent>> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            let connector =
                native_tls::TlsConnector::new().map_err(|e| Error::Tls(e.to_string()))?;
            Ok(ureq::AgentBuilder::new().tls_connector(Arc::new(connector)).build())
        })
        .as_ref()
        .map_err(|e| Error::Tls(e.to_string()))
}

/// The latest release GitHub reports for a repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// The release's git tag, e.g. `v0.2.0`.
    pub tag_name: String,
    /// The release page a user can open for notes + downloads.
    pub html_url: String,
}

/// Fetch the latest published release of `owner/repo` from the GitHub API
/// (`GET /repos/{owner}/{repo}/releases/latest`). Drafts and prereleases are
/// excluded by that endpoint already.
pub fn fetch_latest_release(owner_repo: &str) -> Result<ReleaseInfo> {
    let url = format!("https://api.github.com/repos/{owner_repo}/releases/latest");
    let response = agent()?
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| Error::Request(e.to_string()))?;
    let body: serde_json::Value = response.into_json().map_err(|e| Error::Parse(e.to_string()))?;
    extract_release(&body).ok_or_else(|| Error::Parse(body.to_string()))
}

/// Pull `tag_name`/`html_url` out of a `releases/latest` JSON payload. Split
/// from [`fetch_latest_release`] so it unit-tests with inline fixtures instead
/// of a live network call.
fn extract_release(body: &serde_json::Value) -> Option<ReleaseInfo> {
    let tag_name = body.get("tag_name")?.as_str()?.to_string();
    let html_url = body.get("html_url")?.as_str()?.to_string();
    Some(ReleaseInfo { tag_name, html_url })
}

/// A dotted `major.minor.patch` version, parsed from a tag like `v1.2.3` or
/// `1.2.3` (a `v` prefix and any trailing `-pre`/`+build` suffix are ignored).
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// Whether `latest` is a strictly newer version than `current`. Unparseable
/// input (either side) fails safe to `false` rather than nagging the user over
/// a tag that doesn't look like a version.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// Result of a full check: the latest release info plus whether it's newer
/// than `current_version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub update_available: bool,
}

/// Fetch the latest release of `owner_repo` and compare it against
/// `current_version` (the running app's version, without a `v` prefix).
pub fn check_for_update(owner_repo: &str, current_version: &str) -> Result<UpdateCheck> {
    let release = fetch_latest_release(owner_repo)?;
    let update_available = is_newer(current_version, &release.tag_name);
    Ok(UpdateCheck {
        current_version: current_version.to_string(),
        latest_version: release.tag_name,
        release_url: release.html_url,
        update_available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v_prefixed_semver() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
    }

    #[test]
    fn defaults_missing_components_to_zero() {
        assert_eq!(parse_version("v2"), Some((2, 0, 0)));
        assert_eq!(parse_version("v2.5"), Some((2, 5, 0)));
    }

    #[test]
    fn strips_prerelease_and_build_metadata() {
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1.2.3+build.5"), Some((1, 2, 3)));
    }

    #[test]
    fn rejects_non_numeric_leading_component() {
        assert_eq!(parse_version("release-branch"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn is_newer_compares_major_minor_patch() {
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(is_newer("0.1.0", "v0.1.1"));
        assert!(is_newer("0.1.0", "v1.0.0"));
        assert!(!is_newer("0.2.0", "v0.1.0"));
        assert!(!is_newer("0.1.0", "v0.1.0"));
    }

    #[test]
    fn is_newer_fails_safe_on_unparseable_input() {
        assert!(!is_newer("0.1.0", "not-a-version"));
        assert!(!is_newer("not-a-version", "v0.1.0"));
    }

    #[test]
    fn extract_release_reads_tag_and_url() {
        let body = serde_json::json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/ChrisTowles/towles-tool-rs/releases/tag/v0.2.0",
            "draft": false,
        });
        let release = extract_release(&body).unwrap();
        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(
            release.html_url,
            "https://github.com/ChrisTowles/towles-tool-rs/releases/tag/v0.2.0"
        );
    }

    #[test]
    fn extract_release_none_on_missing_fields() {
        assert!(extract_release(&serde_json::json!({})).is_none());
        assert!(extract_release(&serde_json::json!({"tag_name": "v1.0.0"})).is_none());
    }

    #[test]
    fn check_for_update_serializes_camel_case() {
        let check = UpdateCheck {
            current_version: "0.1.0".to_string(),
            latest_version: "v0.2.0".to_string(),
            release_url: "https://example.com/releases/v0.2.0".to_string(),
            update_available: true,
        };
        let json = serde_json::to_value(&check).unwrap();
        assert_eq!(json["currentVersion"], serde_json::json!("0.1.0"));
        assert_eq!(json["updateAvailable"], serde_json::json!(true));
    }
}
