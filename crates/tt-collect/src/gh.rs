//! Shared `gh` plumbing for the issue and PR collectors.
//!
//! Every invocation runs in the repo's own working directory (that's how `gh`
//! resolves the target repo) and is capped by [`GH_TIMEOUT`] so a stalled
//! network call can't wedge the caller — the app's scheduler awaits collector
//! batches serially, so one hung `gh` would otherwise stop every collector.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Hard cap per `gh` invocation. Generous for a slow API, far below the
/// collector cadence.
const GH_TIMEOUT: Duration = Duration::from_secs(30);

/// `gh list` page cap. `gh` defaults to 30 and silently truncates beyond it;
/// 200 is comfortably above any realistic assigned-issues / open-PRs count.
pub(crate) const LIST_LIMIT: &str = "200";

/// How long to pause every `gh` call once one reports a GitHub rate limit
/// (primary or secondary/abuse-detection). The limit is per-token, not per
/// repo or per collector (#322), so one hit means every other in-flight and
/// upcoming call is likely to be limited too. Short-circuiting locally for a
/// few minutes stops the collectors from continuing to hammer a token that's
/// already over budget; a resolved limit just means the first call after the
/// window succeeds normally, so erring generous here costs nothing but a
/// slightly stale dashboard.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(5 * 60);

static RATE_LIMITED_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

fn rate_limited_until() -> &'static Mutex<Option<Instant>> {
    RATE_LIMITED_UNTIL.get_or_init(|| Mutex::new(None))
}

/// Time remaining on an armed backoff, or `None` if it's unset/expired. Pure
/// so the decision is unit-testable without waiting on a real clock.
fn backoff_remaining(until: Option<Instant>, now: Instant) -> Option<Duration> {
    until.and_then(|u| u.checked_duration_since(now))
}

/// Whether `gh` calls are currently paused due to a recent rate-limit hit.
fn in_rate_limit_backoff() -> Option<Duration> {
    let guard = rate_limited_until().lock().ok()?;
    backoff_remaining(*guard, Instant::now())
}

/// Record that `gh` just reported a rate limit, arming the backoff window.
fn note_rate_limited() {
    log::warn!("gh reported a rate limit; pausing gh calls for {}s", RATE_LIMIT_BACKOFF.as_secs());
    if let Ok(mut guard) = rate_limited_until().lock() {
        *guard = Some(Instant::now() + RATE_LIMIT_BACKOFF);
    }
}

/// Whether a `gh` stderr message indicates GitHub's primary or secondary
/// (abuse-detection) rate limit, as opposed to some other failure (auth,
/// network, unknown repo) a backoff wouldn't help with.
fn is_rate_limit_message(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("rate limit") || s.contains("abuse detection") || s.contains("submitted too quickly")
}

/// Run `gh` in `dir`, returning stdout on success or a human-readable error.
///
/// Short-circuits without spawning a subprocess while a rate-limit backoff is
/// armed (see [`RATE_LIMIT_BACKOFF`]), and arms that backoff itself the moment
/// a call's stderr looks like a rate-limit response.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    if let Some(remaining) = in_rate_limit_backoff() {
        return Err(format!(
            "gh calls paused for {}s after a recent GitHub rate limit",
            remaining.as_secs()
        ));
    }
    log::debug!("gh {} (cwd {})", args.join(" "), dir.display());
    let output = tt_exec::run_in_dir_with_timeout("gh", args, dir, GH_TIMEOUT)
        .map_err(|e| format!("gh {} in {}: {e}", args.first().unwrap_or(&""), dir.display()))?;
    if !output.ok() {
        let stderr = output.stderr.trim();
        if is_rate_limit_message(stderr) {
            note_rate_limited();
        }
        return Err(format!(
            "gh {} failed in {}: {}",
            args.first().unwrap_or(&""),
            dir.display(),
            stderr
        ));
    }
    Ok(output.stdout)
}

/// Run a `gh ... --json` command in `dir` and parse its stdout.
pub(crate) fn run_json(dir: &Path, args: &[&str]) -> Result<serde_json::Value, String> {
    let stdout = run(dir, args)?;
    serde_json::from_str(&stdout).map_err(|e| format!("invalid gh JSON: {e}"))
}

/// `owner/repo` for the repo rooted at `dir`, via `gh repo view`.
///
/// Cached per directory for the process lifetime: a checkout's identity doesn't
/// change, and the app's scheduler would otherwise pay one extra network call
/// per repo on every tick. Failures are not cached, so a repo that comes online
/// (auth fixed, network back) resolves on the next attempt.
pub(crate) fn repo_name_with_owner(dir: &Path) -> Result<String, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock()
        && let Some(name) = map.get(dir)
    {
        return Ok(name.clone());
    }

    let value = run_json(dir, &["repo", "view", "--json", "nameWithOwner"])?;
    let name =
        value.get("nameWithOwner").and_then(|v| v.as_str()).map(|s| s.to_string()).ok_or_else(
            || format!("gh repo view returned no nameWithOwner for {}", dir.display()),
        )?;

    if let Ok(mut map) = cache.lock() {
        map.insert(dir.to_path_buf(), name.clone());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_remaining_reports_time_left_until_expiry() {
        let now = Instant::now();
        let armed = now + Duration::from_secs(60);
        assert_eq!(backoff_remaining(Some(armed), now), Some(Duration::from_secs(60)));
        assert_eq!(backoff_remaining(Some(armed), armed), Some(Duration::ZERO));
    }

    #[test]
    fn backoff_remaining_is_none_once_expired_or_unset() {
        let now = Instant::now();
        let expired = now - Duration::from_secs(1);
        assert_eq!(backoff_remaining(Some(expired), now), None);
        assert_eq!(backoff_remaining(None, now), None);
    }

    #[test]
    fn is_rate_limit_message_matches_known_github_responses() {
        assert!(is_rate_limit_message("API rate limit exceeded for user ID 123."));
        assert!(is_rate_limit_message(
            "You have exceeded a secondary rate limit. Please wait a few minutes."
        ));
        assert!(is_rate_limit_message(
            "You have exceeded a secondary rate limit and have been temporarily blocked \
             from content creation. Please retry your request again later."
        ));
        assert!(is_rate_limit_message("api.github.com says: was submitted too quickly"));
    }

    #[test]
    fn is_rate_limit_message_ignores_unrelated_failures() {
        assert!(!is_rate_limit_message("could not resolve to a Repository with the name 'o/r'"));
        assert!(!is_rate_limit_message("HTTP 401: Bad credentials"));
        assert!(!is_rate_limit_message("dial tcp: lookup api.github.com: no such host"));
    }
}
