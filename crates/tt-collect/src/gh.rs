//! Shared `gh` plumbing for the issue and PR collectors.
//!
//! Every invocation runs in the repo's own working directory (that's how `gh`
//! resolves the target repo) and is capped by [`GH_TIMEOUT`] so a stalled
//! network call can't wedge the caller — the app's scheduler awaits collector
//! batches serially, so one hung `gh` would otherwise stop every collector.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Hard cap per `gh` invocation. Generous for a slow API, far below the
/// collector cadence.
const GH_TIMEOUT: Duration = Duration::from_secs(30);

/// `gh list` page cap. `gh` defaults to 30 and silently truncates beyond it;
/// 200 is comfortably above any realistic assigned-issues / open-PRs count.
pub(crate) const LIST_LIMIT: &str = "200";

/// Run `gh` in `dir`, returning stdout on success or a human-readable error.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    log::debug!("gh {} (cwd {})", args.join(" "), dir.display());
    let output = tt_exec::run_in_dir_with_timeout("gh", args, dir, GH_TIMEOUT)
        .map_err(|e| format!("gh {} in {}: {e}", args.first().unwrap_or(&""), dir.display()))?;
    if !output.ok() {
        return Err(format!(
            "gh {} failed in {}: {}",
            args.first().unwrap_or(&""),
            dir.display(),
            output.stderr.trim()
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
