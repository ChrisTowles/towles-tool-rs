//! Per-session agent-pushed metadata store. Ports §task§-1
//! `runtime/server/metadata-store.ts`.
//!
//! Timestamps are injected (`now_ms`) rather than read from the clock, matching
//! the `tt-claude-sessions` pattern.

use std::collections::{HashMap, HashSet};

use crate::text::truncate;
use crate::types::{
    MetadataLogEntry, MetadataProgress, MetadataStatus, MetadataTone, SessionMetadata,
};

const MAX_LOGS: usize = 50;
const MAX_MESSAGE_LENGTH: usize = 500;

/// A status update pushed by an agent/script (before a timestamp is stamped).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInput {
    pub text: String,
    pub tone: Option<MetadataTone>,
}

/// A progress update pushed by an agent/script.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressInput {
    pub current: Option<i64>,
    pub total: Option<i64>,
    pub percent: Option<f64>,
    pub label: Option<String>,
}

/// A log line pushed by an agent/script.
#[derive(Debug, Clone, PartialEq)]
pub struct LogInput {
    pub message: String,
    pub tone: Option<MetadataTone>,
    pub source: Option<String>,
}

/// In-memory per-session metadata (status / progress / capped log ring).
/// Ports `SessionMetadataStore`.
#[derive(Debug, Default)]
pub struct SessionMetadataStore {
    store: HashMap<String, SessionMetadata>,
}

impl SessionMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_or_create(&mut self, session: &str) -> &mut SessionMetadata {
        self.store.entry(session.to_string()).or_default()
    }

    /// Return the metadata for a session, or `None` when everything is empty.
    /// Ports `get`.
    pub fn get(&self, session: &str) -> Option<&SessionMetadata> {
        let meta = self.store.get(session)?;
        if meta.status.is_none() && meta.progress.is_none() && meta.logs.is_empty() {
            return None;
        }
        Some(meta)
    }

    /// Set (or clear, with `None`) the status line. Ports `setStatus`.
    pub fn set_status(&mut self, session: &str, status: Option<StatusInput>, now_ms: i64) {
        let Some(status) = status else {
            if let Some(meta) = self.store.get_mut(session) {
                meta.status = None;
            }
            return;
        };
        let meta = self.get_or_create(session);
        meta.status = Some(MetadataStatus {
            text: truncate(&status.text, 100),
            tone: status.tone,
            ts: now_ms,
        });
    }

    /// Set (or clear, with `None`) the progress indicator. Ports `setProgress`.
    pub fn set_progress(&mut self, session: &str, progress: Option<ProgressInput>, now_ms: i64) {
        let Some(progress) = progress else {
            if let Some(meta) = self.store.get_mut(session) {
                meta.progress = None;
            }
            return;
        };
        let meta = self.get_or_create(session);
        meta.progress = Some(MetadataProgress {
            current: progress.current,
            total: progress.total,
            percent: progress.percent,
            label: progress.label.map(|l| truncate(&l, 100)),
            ts: now_ms,
        });
    }

    /// Append a log line, trimming to the last [`MAX_LOGS`]. Ports `appendLog`.
    pub fn append_log(&mut self, session: &str, entry: LogInput, now_ms: i64) {
        let meta = self.get_or_create(session);
        meta.logs.push(MetadataLogEntry {
            message: truncate(&entry.message, MAX_MESSAGE_LENGTH),
            tone: entry.tone,
            source: entry.source.map(|s| truncate(&s, 50)),
            ts: now_ms,
        });
        if meta.logs.len() > MAX_LOGS {
            let excess = meta.logs.len() - MAX_LOGS;
            meta.logs.drain(0..excess);
        }
    }

    /// Clear a session's logs (status/progress untouched). Ports `clearLogs`.
    pub fn clear_logs(&mut self, session: &str) {
        if let Some(meta) = self.store.get_mut(session) {
            meta.logs.clear();
        }
    }

    /// Drop metadata for sessions no longer present. Ports `pruneSessions`.
    pub fn prune_sessions(&mut self, valid_names: &HashSet<String>) {
        self.store.retain(|name, _| valid_names.contains(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_session_returns_none() {
        let store = SessionMetadataStore::new();
        assert!(store.get("s").is_none());
    }

    #[test]
    fn set_status_truncates_and_stamps() {
        let mut store = SessionMetadataStore::new();
        let long = "x".repeat(200);
        store.set_status("s", Some(StatusInput { text: long, tone: Some(MetadataTone::Info) }), 42);
        let meta = store.get("s").unwrap();
        let status = meta.status.as_ref().unwrap();
        assert_eq!(status.text.chars().count(), 100);
        assert!(status.text.ends_with('…'));
        assert_eq!(status.ts, 42);
        assert_eq!(status.tone, Some(MetadataTone::Info));
    }

    #[test]
    fn clear_status_with_none() {
        let mut store = SessionMetadataStore::new();
        store.set_status("s", Some(StatusInput { text: "hi".into(), tone: None }), 1);
        store.set_status("s", None, 2);
        assert!(store.get("s").is_none());
    }

    #[test]
    fn progress_stored_and_cleared() {
        let mut store = SessionMetadataStore::new();
        store.set_progress(
            "s",
            Some(ProgressInput {
                current: Some(3),
                total: Some(10),
                percent: Some(30.0),
                label: Some("building".into()),
            }),
            5,
        );
        assert_eq!(store.get("s").unwrap().progress.as_ref().unwrap().current, Some(3));
        store.set_progress("s", None, 6);
        assert!(store.get("s").is_none());
    }

    #[test]
    fn logs_capped_at_max() {
        let mut store = SessionMetadataStore::new();
        for i in 0..60 {
            store.append_log(
                "s",
                LogInput { message: format!("line {i}"), tone: None, source: None },
                i,
            );
        }
        let meta = store.get("s").unwrap();
        assert_eq!(meta.logs.len(), MAX_LOGS);
        assert_eq!(meta.logs.first().unwrap().message, "line 10");
        assert_eq!(meta.logs.last().unwrap().message, "line 59");
    }

    #[test]
    fn clear_logs_keeps_status() {
        let mut store = SessionMetadataStore::new();
        store.set_status("s", Some(StatusInput { text: "hi".into(), tone: None }), 1);
        store.append_log("s", LogInput { message: "m".into(), tone: None, source: None }, 1);
        store.clear_logs("s");
        let meta = store.get("s").unwrap();
        assert!(meta.logs.is_empty());
        assert!(meta.status.is_some());
    }

    #[test]
    fn prune_sessions_drops_missing() {
        let mut store = SessionMetadataStore::new();
        store.set_status("keep", Some(StatusInput { text: "a".into(), tone: None }), 1);
        store.set_status("drop", Some(StatusInput { text: "b".into(), tone: None }), 1);
        let valid: HashSet<String> = ["keep".to_string()].into_iter().collect();
        store.prune_sessions(&valid);
        assert!(store.get("keep").is_some());
        assert!(store.get("drop").is_none());
    }
}
