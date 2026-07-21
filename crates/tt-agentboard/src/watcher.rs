//! Agent-watcher contract. Ports slot-1 `runtime/contracts/agent-watcher.ts`
//! (§0 of the watcher spec), reframed for an externally-driven scan tick.
//!
//! Deviation from the TS `start(ctx)`/`stop()` (which own a `setInterval` timer):
//! the 2s tick is **driven externally** here — the bridge calls [`AgentWatcher::scan`]
//! with an explicit `now_ms` on whatever schedule it owns. This keeps the watcher
//! deterministic and unit-testable without timers or tokio. An optional
//! `notify`-based accelerant lives in [`crate::fs_notify`], isolated from this core.

use crate::types::AgentEvent;

/// Constants shared by watchers (§0). `JOURNAL_IDLE_TIMEOUT_MS` comes from
/// [`crate::types::JOURNAL_IDLE_TIMEOUT_MS`].
pub const POLL_MS: i64 = 2000;
/// Files untouched longer than this are skipped by a scan entirely.
pub const STALE_MS: i64 = 5 * 60 * 1000;
pub const JSONL_SUFFIX: &str = ".jsonl";

/// Callback context the bridge provides to a watcher. Ports `AgentWatcherContext`.
pub trait WatcherContext {
    /// Resolve a project directory to a session name, or `None` if unmatched.
    ///
    /// Per adopted fix #3 the watcher passes the **raw encoded** project-dir name;
    /// the implementation re-encodes known repo paths and prefix-matches.
    fn resolve_session(&self, project_dir: &str) -> Option<String>;

    /// Resolve an agent's OS pid to the tmux session whose pane owns it
    /// (T7): the tmux server walks the pid's ancestry to a pane pid. Hosts
    /// without pane knowledge (the desktop app) keep the default `None`,
    /// falling back to [`WatcherContext::resolve_session`] on the cwd.
    fn resolve_session_by_pid(&self, _pid: i32) -> Option<String> {
        None
    }

    /// Emit an event (the bridge applies it to the tracker and broadcasts).
    fn emit(&mut self, event: AgentEvent);
}

/// A source that detects agent status by watching external data. Ports `AgentWatcher`,
/// with the scan tick driven by the caller instead of an internal timer.
pub trait AgentWatcher {
    /// Unique watcher name (e.g. `"claude-code"`).
    fn name(&self) -> &str;

    /// Perform one full scan at logical time `now_ms`, emitting via `ctx`. The
    /// caller drives this on an interval (and may call it eagerly on fs events).
    fn scan(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64);
}
