//! Telemetry for the `tt` CLI and the desktop app: `tracing` instrumentation
//! plus an event-log sink that streams every span and event to disk as JSONL,
//! and the reader the Telemetry viewer screen uses to read it back. One
//! crate for both halves so the writer and reader can never disagree about
//! the on-disk schema.
//!
//! The point is answering questions *later*. "Which task spawned that `gh`
//! call, how long did it take, and what did it exit with?" should be a `jq`
//! away, without reproducing the problem under a debugger. So the sink is
//! always on, always flushed, and always local.
//!
//! # Shape
//!
//! - [`init`] installs the global subscriber. Call it once, early, from a
//!   binary — never from a library.
//! - A `fmt` layer keeps the human-readable stderr output the `-v` flag and
//!   `RUST_LOG` used to drive under `env_logger`.
//! - An [`layer::EventLogLayer`] writes the structured record to
//!   `<data_dir>/telemetry/events-<date>.jsonl`, instance-scoped so each
//!   worktree gets its own log. **The naive unscoped path is a trap**: for a
//!   process run from a `towles-tool-rs` checkout, `tt_config::state_scope()`
//!   always resolves `<data_dir>` to a *scoped* directory —
//!   `~/.local/share/towles-tool/tasks/<scope>/telemetry/…` — never the bare
//!   `~/.local/share/towles-tool/telemetry/…`. That bare path exists on disk
//!   but stays permanently empty, because nothing ever writes there; checking
//!   it by eye (or with a stray `find`/`grep`) reads as "telemetry is broken"
//!   when it's simply elsewhere. Resolve the real path with
//!   `tt_config::telemetry_dir()` (or read it back via [`list_days`]/
//!   [`read_day`]) rather than assuming the unscoped location.
//! - [`list_days`]/[`read_day`] read those files back, for the Telemetry
//!   viewer screen's Tauri bridge (`crates-tauri/tt-app/src/telemetry.rs`).
//!   There is no cache here, unlike `tt-claude-sessions` — a day's log was
//!   assumed to be small (bounded by subprocess spawns and discrete user
//!   actions, never per-keystroke input), but an actively-used dev checkout
//!   has been observed producing 75,000+ records in a single day, so that
//!   assumption does not hold for a long-running session. The frontend caps
//!   *rendered* rows to cope with that; `read_day` itself still returns the
//!   whole file unconditionally, so a large day still costs a full read,
//!   parse, and IPC payload on every screen focus/refresh. Bounded
//!   reads/pagination here would be the deeper fix if that cost becomes a
//!   problem in practice — not yet implemented.
//!
//! `tracing-subscriber`'s `tracing-log` feature captures the `log::` macros
//! still in the tree, so existing call sites keep reporting while individual
//! seams are converted to spans.

mod event_log;
mod layer;
mod reader;
mod schema;
mod types;

pub use event_log::EventLog;
pub use layer::EventLogLayer;
pub use reader::{list_days, read_day};
pub use types::TelemetryRecord;

use serde_json::{Map, Value};
use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// Days of event-log history kept before rotation prunes a file.
const RETAIN_DAYS: usize = 14;

/// Set to `0`/`false` to skip the disk sink entirely (stderr logging still
/// works). For contexts that must not write state at all.
const DISABLE_ENV: &str = "TT_TELEMETRY";

/// Filter for the disk sink: our own crates at `debug`, everything else at
/// `warn`.
///
/// The scoping is load-bearing, not tidiness. `tracing-subscriber` is built
/// with the `tracing-log` feature, so an unscoped `debug` sink would bridge in
/// every `log::debug!` from the dependency tree (hyper, tao, wry, rusqlite,
/// tokio-tungstenite) and write *and flush* each one. That is unbounded volume
/// uncorrelated with anything this log exists to answer, and it would falsify
/// the assumption [`EventLog`] relies on to justify flushing every record.
///
/// Third-party `warn`/`error` still lands, because a dependency complaining is
/// exactly the kind of thing worth having already captured.
const DISK_FILTER: &str = "warn,tt=debug,tt_agentboard=debug,tt_app=debug,tt_cli=debug,\
                           tt_collect=debug,tt_config=debug,tt_exec=debug,tt_git=debug,\
                           tt_ide=debug,tt_journal=debug,tt_mcp=debug,tt_telemetry=debug,\
                           tt_tasks=debug,tt_store=debug,tt_update=debug,tt_vt=debug";

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to resolve the telemetry directory: {0}")]
    Dir(#[from] tt_config::Error),

    #[error("A global tracing subscriber is already installed")]
    AlreadyInitialized,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Whether the disk sink is switched off by [`DISABLE_ENV`].
fn disk_sink_disabled() -> bool {
    std::env::var(DISABLE_ENV).is_ok_and(|value| matches!(value.trim(), "0" | "false"))
}

/// Resource attributes stamped on every record, in OpenTelemetry naming.
///
/// `tt.task` is the load-bearing one: several checkouts of this repo run
/// concurrently, so a record is only interpretable if it says which one
/// produced it. `tt.build_sha` (from `build.rs`) is what makes a record
/// attributable to a commit — the same fix can be absent from one running
/// binary and present in another.
fn resource(service: &str) -> Map<String, Value> {
    let mut attrs = Map::new();
    let [service_name, service_version, process_pid] = schema::RESOURCE_KEYS else {
        unreachable!("RESOURCE_KEYS is a fixed 3-element array")
    };
    attrs.insert(service_name.to_string(), Value::from(service));
    attrs.insert(service_version.to_string(), Value::from(env!("CARGO_PKG_VERSION")));
    attrs.insert(process_pid.to_string(), Value::from(std::process::id()));
    attrs.insert(
        schema::FIELD_TT_TASK.into(),
        match tt_config::state_scope() {
            Some(scope) => Value::from(scope),
            None => Value::Null,
        },
    );
    attrs.insert(schema::FIELD_TT_BUILD_SHA.into(), Value::from(env!("TT_BUILD_SHA")));
    attrs
}

/// Install the global subscriber for `service` (`"tt"`, `"tt-app"`, …).
///
/// `default_level` is the stderr filter used when `RUST_LOG` is unset — the
/// `-v` count maps onto it. The disk sink is deliberately *not* filtered by
/// `RUST_LOG`: it always records our own crates at `DEBUG` (see
/// [`DISK_FILTER`]), because the whole value of an event log is having the
/// detail already captured when a question comes up. A quiet terminal should
/// not mean a useless log.
///
/// Returns [`Error::AlreadyInitialized`] rather than panicking if called twice.
pub fn init(service: &str, default_level: &str) -> Result<()> {
    let stderr_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let disk = if disk_sink_disabled() {
        None
    } else {
        let dir = tt_config::telemetry_dir()?;
        Some(
            EventLogLayer::new(EventLog::new(dir, RETAIN_DAYS), resource(service))
                .with_filter(EnvFilter::new(DISK_FILTER)),
        )
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(stderr_filter),
        )
        .with(disk)
        .try_init()
        .map_err(|_| Error::AlreadyInitialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_names_the_service_and_process() {
        let attrs = resource("tt");
        assert_eq!(attrs["service.name"], "tt");
        assert_eq!(attrs["process.pid"], Value::from(std::process::id()));
        assert!(attrs.contains_key("tt.task"), "every record must be attributable to a task");
        assert!(
            attrs.contains_key("tt.build_sha"),
            "every record must be attributable to the commit it was built from"
        );
    }
}

#[cfg(test)]
mod disk_filter_tests {
    use super::*;
    use tracing_subscriber::Layer;

    /// Run `body` under a `DISK_FILTER`-scoped EventLogLayer; return the number
    /// of records that reached disk.
    fn records_written(body: impl FnOnce()) -> usize {
        let dir = tempfile::tempdir().unwrap();
        let layer = EventLogLayer::new(EventLog::new(dir.path(), 7), Map::new())
            .with_filter(EnvFilter::new(DISK_FILTER));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), body);
        std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default().lines().count())
            .sum()
    }

    #[test]
    fn first_party_debug_reaches_disk() {
        let n = records_written(|| tracing::debug!(target: "tt_exec", "a subprocess span"));
        assert_eq!(n, 1, "our own crates must be recorded at debug");
    }

    #[test]
    fn third_party_debug_is_dropped() {
        // The whole reason the filter is scoped: an unscoped debug sink bridges
        // in every dependency's log::debug! and writes+flushes each one.
        let n = records_written(|| {
            tracing::debug!(target: "hyper::client", "connection reused");
            tracing::debug!(target: "tao::platform_impl", "event loop tick");
        });
        assert_eq!(n, 0, "dependency debug chatter must never reach the event log");
    }

    #[test]
    fn ui_action_events_reach_disk() {
        // The frontend action seam (`tt-app`'s `ui_action` command) emits at
        // info under tt-app's own target, which `tt_app=debug` covers by
        // prefix. Pinned because the sink's default is `warn`: an action seam
        // that moved to a non-`tt_*` target would be silently swallowed, and a
        // silent event log is worse than no event log.
        let n = records_written(|| {
            tracing::info!(target: "tt_app_lib", action = "repo.icon_set", screen = "settings", "ui.action");
        });
        assert_eq!(n, 1, "user actions must reach the event log");
    }

    #[test]
    fn third_party_warnings_still_reach_disk() {
        let n = records_written(|| tracing::warn!(target: "hyper::client", "pool exhausted"));
        assert_eq!(n, 1, "a dependency complaining is worth having captured");
    }
}
