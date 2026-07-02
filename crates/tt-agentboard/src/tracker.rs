//! In-memory agent-instance state machine. Ports slot-1
//! `runtime/agents/tracker.ts`.
//!
//! Pure logic: every method that reads the clock in TS (`Date.now()` in the
//! prune methods) takes an explicit `now_ms` here, so tests are deterministic.
//! Insertion order is preserved with `IndexMap`/`IndexSet` to match JS `Map`/`Set`
//! semantics that [`AgentTracker::get_state`]'s priority tie-break relies on.

use indexmap::{IndexMap, IndexSet};
use std::collections::{HashMap, HashSet};

use crate::types::AgentEvent;

const MAX_EVENT_TIMESTAMPS: usize = 30;
const TERMINAL_PRUNE_MS: i64 = 5 * 60 * 1000;

/// Priority of a status for [`AgentTracker::get_state`] (higher wins; ties resolve
/// to the earliest-inserted instance). Ports `STATUS_PRIORITY`.
fn status_priority(status: crate::types::AgentStatus) -> i32 {
    use crate::types::AgentStatus::*;
    match status {
        Running => 5,
        Question => 4,
        Error => 4,
        Interrupted => 3,
        Waiting => 2,
        Done => 1,
        Idle => 0,
    }
}

/// The per-session instance-map key: `agent` or `agent:threadId`. Ports `instanceKey`.
pub fn instance_key(agent: &str, thread_id: Option<&str>) -> String {
    match thread_id {
        Some(t) => format!("{agent}:{t}"),
        None => agent.to_string(),
    }
}

/// Tracks agent instances per session, their unseen state, pins, and prunes
/// dead/stale/terminal instances. Ports the `AgentTracker` class.
#[derive(Debug, Default)]
pub struct AgentTracker {
    /// session name → (instance key → latest event), insertion-ordered.
    instances: IndexMap<String, IndexMap<String, AgentEvent>>,
    event_timestamps: HashMap<String, Vec<i64>>,
    /// Per-instance unseen tracking, keyed by `session\0instanceKey`.
    unseen_instances: IndexSet<String>,
    active: HashSet<String>,
    /// session → pinned instance keys (agents backed by a live pane process).
    pinned_keys: HashMap<String, HashSet<String>>,
}

fn unseen_key(session: &str, key: &str) -> String {
    format!("{session}\0{key}")
}

impl AgentTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn remove_instance(&mut self, session: &str, key: &str) {
        if let Some(inner) = self.instances.get_mut(session) {
            inner.shift_remove(key);
        }
        self.unseen_instances.shift_remove(&unseen_key(session, key));
    }

    /// Drop any session whose instance map is now empty.
    fn drop_if_empty(&mut self, session: &str) {
        if self.instances.get(session).is_some_and(IndexMap::is_empty) {
            self.instances.shift_remove(session);
        }
    }

    /// Record an event. `seed` marks pre-connection state, which always counts as
    /// unseen when terminal. Ports `applyEvent`.
    pub fn apply_event(&mut self, event: AgentEvent, seed: bool) {
        let key = instance_key(&event.agent, event.thread_id.as_deref());
        let session = event.session.clone();
        let status = event.status;
        let ts = event.ts;

        self.instances.entry(session.clone()).or_default().insert(key.clone(), event);

        let timestamps = self.event_timestamps.entry(session.clone()).or_default();
        timestamps.push(ts);
        if timestamps.len() > MAX_EVENT_TIMESTAMPS {
            let excess = timestamps.len() - MAX_EVENT_TIMESTAMPS;
            timestamps.drain(0..excess);
        }

        let ukey = unseen_key(&session, &key);
        if status.is_terminal() {
            if seed || !self.active.contains(&session) {
                self.unseen_instances.insert(ukey);
            }
        } else {
            self.unseen_instances.shift_remove(&ukey);
        }
    }

    /// The most important agent state for a session. Ports `getState`.
    pub fn get_state(&self, session: &str) -> Option<AgentEvent> {
        let inner = self.instances.get(session)?;
        let mut best: Option<&AgentEvent> = None;
        let mut best_priority = -1;
        for event in inner.values() {
            let p = status_priority(event.status);
            if p > best_priority {
                best_priority = p;
                best = Some(event);
            }
        }
        best.cloned()
    }

    /// All instances for a session, unseen flag stamped, newest-first. Ports `getAgents`.
    pub fn get_agents(&self, session: &str) -> Vec<AgentEvent> {
        let Some(inner) = self.instances.get(session) else {
            return Vec::new();
        };
        let mut out: Vec<AgentEvent> = inner
            .values()
            .map(|event| {
                let key = instance_key(&event.agent, event.thread_id.as_deref());
                let mut ev = event.clone();
                if self.unseen_instances.contains(&unseen_key(session, &key)) {
                    ev.unseen = Some(true);
                }
                ev
            })
            .collect();
        // Stable sort by descending ts, so ties keep insertion order (as in JS).
        out.sort_by_key(|e| std::cmp::Reverse(e.ts));
        out
    }

    /// Recent event timestamps for sparkline rendering. Ports `getEventTimestamps`.
    pub fn get_event_timestamps(&self, session: &str) -> Vec<i64> {
        self.event_timestamps.get(session).cloned().unwrap_or_default()
    }

    fn clear_unseen(&mut self, session: &str) {
        let Some(inner) = self.instances.get(session) else {
            return;
        };
        let ukeys: Vec<String> = inner.keys().map(|k| unseen_key(session, k)).collect();
        for ukey in ukeys {
            self.unseen_instances.shift_remove(&ukey);
        }
    }

    /// Clear unseen flags for a session. Returns whether anything was unseen. Ports `markSeen`.
    pub fn mark_seen(&mut self, session: &str) -> bool {
        if !self.is_unseen(session) {
            return false;
        }
        self.clear_unseen(session);
        true
    }

    /// Remove a specific instance. Ports `dismiss`.
    pub fn dismiss(&mut self, session: &str, agent: &str, thread_id: Option<&str>) -> bool {
        let key = instance_key(agent, thread_id);
        let exists = self.instances.get(session).is_some_and(|inner| inner.contains_key(&key));
        if !exists {
            return false;
        }
        self.remove_instance(session, &key);
        self.drop_if_empty(session);
        true
    }

    /// Prune "running" instances older than `timeout_ms` (unless pinned). Ports `pruneStuck`.
    pub fn prune_stuck(&mut self, timeout_ms: i64, now_ms: i64) {
        let sessions: Vec<String> = self.instances.keys().cloned().collect();
        for session in sessions {
            let removable: Vec<String> = self.instances[&session]
                .iter()
                .filter(|(key, event)| {
                    event.status == crate::types::AgentStatus::Running
                        && now_ms - event.ts > timeout_ms
                        && !self.is_pinned(&session, key)
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in removable {
                self.remove_instance(&session, &key);
            }
            self.drop_if_empty(&session);
        }
    }

    /// When multiple instances of the same agent share a pane, keep only the most
    /// recent; remove superseded predecessors unless pinned. Ports `pruneSupersededByPane`.
    pub fn prune_superseded_by_pane(&mut self) {
        let sessions: Vec<String> = self.instances.keys().cloned().collect();
        for session in sessions {
            // group key (`paneId\0agent`) → [(instance key, activity ts)]
            let mut groups: IndexMap<String, Vec<(String, i64)>> = IndexMap::new();
            for (key, event) in &self.instances[&session] {
                let Some(pane_id) = &event.pane_id else {
                    continue;
                };
                let group_key = format!("{pane_id}\0{}", event.agent);
                let ts =
                    event.details.as_ref().and_then(|d| d.last_activity_at).unwrap_or(event.ts);
                groups.entry(group_key).or_default().push((key.clone(), ts));
            }
            let mut removable: Vec<String> = Vec::new();
            for mut list in groups.into_values() {
                if list.len() < 2 {
                    continue;
                }
                list.sort_by_key(|x| std::cmp::Reverse(x.1));
                for (key, _) in list.into_iter().skip(1) {
                    if !self.is_pinned(&session, &key) {
                        removable.push(key);
                    }
                }
            }
            for key in removable {
                self.remove_instance(&session, &key);
            }
            self.drop_if_empty(&session);
        }
    }

    /// Prune instances whose last activity is older than `timeout_ms`, optionally
    /// restricted to one status; skips pinned. Ports the private `pruneByAge`.
    fn prune_by_age(
        &mut self,
        timeout_ms: i64,
        only_status: Option<crate::types::AgentStatus>,
        now_ms: i64,
    ) {
        let sessions: Vec<String> = self.instances.keys().cloned().collect();
        for session in sessions {
            let removable: Vec<String> = self.instances[&session]
                .iter()
                .filter(|(key, event)| {
                    if let Some(s) = only_status
                        && event.status != s
                    {
                        return false;
                    }
                    if self.is_pinned(&session, key) {
                        return false;
                    }
                    let last_seen =
                        event.details.as_ref().and_then(|d| d.last_activity_at).unwrap_or(event.ts);
                    now_ms - last_seen > timeout_ms
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in removable {
                self.remove_instance(&session, &key);
            }
            self.drop_if_empty(&session);
        }
    }

    /// Prune any instance whose last activity is older than `timeout_ms`. Ports `pruneStale`.
    pub fn prune_stale(&mut self, timeout_ms: i64, now_ms: i64) {
        self.prune_by_age(timeout_ms, None, now_ms);
    }

    /// Prune "idle" instances older than `timeout_ms` unless pinned. Ports `pruneIdle`.
    pub fn prune_idle(&mut self, timeout_ms: i64, now_ms: i64) {
        self.prune_by_age(timeout_ms, Some(crate::types::AgentStatus::Idle), now_ms);
    }

    /// Prune terminal instances older than the terminal timeout, but only if seen
    /// and not pinned. Ports `pruneTerminal`.
    pub fn prune_terminal(&mut self, now_ms: i64) {
        let sessions: Vec<String> = self.instances.keys().cloned().collect();
        for session in sessions {
            let removable: Vec<String> = self.instances[&session]
                .iter()
                .filter(|(key, event)| {
                    event.status.is_terminal()
                        && !self.unseen_instances.contains(&unseen_key(&session, key))
                        && !self.is_pinned(&session, key)
                        && now_ms - event.ts > TERMINAL_PRUNE_MS
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in removable {
                self.remove_instance(&session, &key);
            }
            self.drop_if_empty(&session);
        }
    }

    /// Whether any instance in the session is unseen. Ports `isUnseen`.
    pub fn is_unseen(&self, session: &str) -> bool {
        let Some(inner) = self.instances.get(session) else {
            return false;
        };
        inner.keys().any(|key| self.unseen_instances.contains(&unseen_key(session, key)))
    }

    /// Session names that have any unseen instance, first-seen order. Ports `getUnseen`.
    pub fn get_unseen(&self) -> Vec<String> {
        let mut sessions: IndexSet<String> = IndexSet::new();
        for ukey in &self.unseen_instances {
            if let Some((session, _)) = ukey.split_once('\0') {
                sessions.insert(session.to_string());
            }
        }
        sessions.into_iter().collect()
    }

    /// Focus a session: make it the sole active one and clear its unseen flags.
    /// Returns whether it had been unseen. Ports `handleFocus`.
    pub fn handle_focus(&mut self, session: &str) -> bool {
        self.active.clear();
        self.active.insert(session.to_string());
        let had_unseen = self.is_unseen(session);
        if had_unseen {
            self.clear_unseen(session);
        }
        had_unseen
    }

    /// Replace the active-session set. Ports `setActiveSessions`.
    pub fn set_active_sessions(&mut self, sessions: &[String]) {
        self.active.clear();
        self.active.extend(sessions.iter().cloned());
    }

    /// Set the pinned instance keys for a single session (live pane-backed agents).
    /// Ports `setPinnedInstances`.
    pub fn set_pinned_instances(&mut self, session: Option<&str>, keys: &[String]) {
        self.pinned_keys.clear();
        if let Some(session) = session
            && !keys.is_empty()
        {
            self.pinned_keys.insert(session.to_string(), keys.iter().cloned().collect());
        }
    }

    /// Set pinned instance keys for multiple sessions at once. Ports `setPinnedInstancesMulti`.
    pub fn set_pinned_instances_multi(&mut self, keys_by_session: &HashMap<String, Vec<String>>) {
        self.pinned_keys.clear();
        for (session, keys) in keys_by_session {
            if !keys.is_empty() {
                self.pinned_keys.insert(session.clone(), keys.iter().cloned().collect());
            }
        }
    }

    /// Whether an instance is pinned (backed by a live pane). Ports `isPinned`.
    pub fn is_pinned(&self, session: &str, key: &str) -> bool {
        self.pinned_keys.get(session).is_some_and(|s| s.contains(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentEventDetails, AgentStatus};

    fn ev(session: &str, agent: &str, status: AgentStatus, ts: i64) -> AgentEvent {
        AgentEvent {
            agent: agent.into(),
            session: session.into(),
            status,
            ts,
            thread_id: None,
            thread_name: None,
            unseen: None,
            pane_id: None,
            details: None,
        }
    }

    #[test]
    fn instance_key_with_and_without_thread() {
        assert_eq!(instance_key("claude", None), "claude");
        assert_eq!(instance_key("claude", Some("t1")), "claude:t1");
    }

    #[test]
    fn get_state_picks_highest_priority() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s", "a", AgentStatus::Idle, 1), false);
        t.apply_event(ev("s", "b", AgentStatus::Running, 2), false);
        t.apply_event(ev("s", "c", AgentStatus::Done, 3), false);
        assert_eq!(t.get_state("s").unwrap().status, AgentStatus::Running);
    }

    #[test]
    fn get_state_ties_keep_first_inserted() {
        let mut t = AgentTracker::new();
        // Two equal-priority (error=4) instances; the first inserted wins.
        t.apply_event(ev("s", "first", AgentStatus::Error, 1), false);
        t.apply_event(ev("s", "second", AgentStatus::Error, 2), false);
        assert_eq!(t.get_state("s").unwrap().agent, "first");
    }

    #[test]
    fn get_agents_sorted_newest_first_with_unseen_stamp() {
        let mut t = AgentTracker::new();
        // seed=true → terminal marked unseen.
        t.apply_event(ev("s", "old", AgentStatus::Done, 10), true);
        t.apply_event(ev("s", "new", AgentStatus::Running, 20), false);
        let agents = t.get_agents("s");
        assert_eq!(agents[0].agent, "new");
        assert_eq!(agents[1].agent, "old");
        assert_eq!(agents[1].unseen, Some(true));
        assert_eq!(agents[0].unseen, None);
    }

    #[test]
    fn active_session_terminal_is_seen_but_inactive_is_unseen() {
        let mut t = AgentTracker::new();
        t.set_active_sessions(&["active".into()]);
        t.apply_event(ev("active", "a", AgentStatus::Done, 1), false);
        t.apply_event(ev("idlebg", "b", AgentStatus::Done, 1), false);
        assert!(!t.is_unseen("active"));
        assert!(t.is_unseen("idlebg"));
    }

    #[test]
    fn non_terminal_event_clears_unseen() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s", "a", AgentStatus::Done, 1), true);
        assert!(t.is_unseen("s"));
        // Same instance goes back to running → seen again.
        t.apply_event(ev("s", "a", AgentStatus::Running, 2), false);
        assert!(!t.is_unseen("s"));
    }

    #[test]
    fn mark_seen_and_get_unseen() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s1", "a", AgentStatus::Done, 1), true);
        t.apply_event(ev("s2", "b", AgentStatus::Error, 1), true);
        let mut unseen = t.get_unseen();
        unseen.sort();
        assert_eq!(unseen, vec!["s1".to_string(), "s2".to_string()]);
        assert!(t.mark_seen("s1"));
        assert!(!t.mark_seen("s1")); // already seen
        assert_eq!(t.get_unseen(), vec!["s2".to_string()]);
    }

    #[test]
    fn dismiss_removes_instance_and_empty_session() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s", "a", AgentStatus::Running, 1), false);
        assert!(t.dismiss("s", "a", None));
        assert!(!t.dismiss("s", "a", None));
        assert!(t.get_state("s").is_none());
    }

    #[test]
    fn handle_focus_clears_unseen_and_sets_active() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s", "a", AgentStatus::Done, 1), true);
        assert!(t.handle_focus("s"));
        assert!(!t.is_unseen("s"));
        // A new terminal event while active is seen.
        t.apply_event(ev("s", "a", AgentStatus::Error, 2), false);
        assert!(!t.is_unseen("s"));
    }

    #[test]
    fn prune_stuck_removes_old_running_unless_pinned() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s", "a", AgentStatus::Running, 0), false);
        t.apply_event(ev("s", "b", AgentStatus::Running, 0), false);
        t.set_pinned_instances(Some("s"), &["b".into()]);
        t.prune_stuck(1000, 5000); // both are 5000ms old > 1000
        assert!(t.dismiss("s", "b", None)); // b survived (pinned)
        assert!(!t.dismiss("s", "a", None)); // a was pruned
    }

    #[test]
    fn prune_terminal_keeps_unseen_and_pinned() {
        let mut t = AgentTracker::new();
        t.set_active_sessions(&["s".into()]); // so terminal isn't auto-unseen
        t.apply_event(ev("s", "seen", AgentStatus::Done, 0), false);
        t.apply_event(ev("s", "unseen", AgentStatus::Done, 0), true);
        t.prune_terminal(10 * 60 * 1000); // > TERMINAL_PRUNE_MS
        assert!(!t.dismiss("s", "seen", None)); // seen terminal pruned
        assert!(t.dismiss("s", "unseen", None)); // unseen kept
    }

    #[test]
    fn prune_idle_only_targets_idle() {
        let mut t = AgentTracker::new();
        t.apply_event(ev("s", "idle", AgentStatus::Idle, 0), false);
        t.apply_event(ev("s", "run", AgentStatus::Running, 0), false);
        t.prune_idle(1000, 5000);
        assert!(!t.dismiss("s", "idle", None)); // idle pruned
        assert!(t.dismiss("s", "run", None)); // running kept
    }

    #[test]
    fn prune_by_age_uses_last_activity_when_present() {
        let mut t = AgentTracker::new();
        let mut e = ev("s", "a", AgentStatus::Running, 0);
        e.details = Some(AgentEventDetails { last_activity_at: Some(4500), ..Default::default() });
        t.apply_event(e, false);
        // event ts is 0 (very old) but lastActivityAt is recent → not stale.
        t.prune_stale(1000, 5000);
        assert!(t.dismiss("s", "a", None));
    }

    #[test]
    fn prune_superseded_by_pane_keeps_most_recent() {
        let mut t = AgentTracker::new();
        let mut a = ev("s", "claude", AgentStatus::Idle, 1);
        a.thread_id = Some("old".into());
        a.pane_id = Some("%1".into());
        let mut b = ev("s", "claude", AgentStatus::Running, 2);
        b.thread_id = Some("new".into());
        b.pane_id = Some("%1".into());
        t.apply_event(a, false);
        t.apply_event(b, false);
        t.prune_superseded_by_pane();
        // Older instance (thread "old") superseded, newer kept.
        assert!(!t.dismiss("s", "claude", Some("old")));
        assert!(t.dismiss("s", "claude", Some("new")));
    }

    #[test]
    fn event_timestamps_capped() {
        let mut t = AgentTracker::new();
        for i in 0..40 {
            t.apply_event(ev("s", "a", AgentStatus::Running, i), false);
        }
        let ts = t.get_event_timestamps("s");
        assert_eq!(ts.len(), MAX_EVENT_TIMESTAMPS);
        assert_eq!(ts[0], 10); // oldest 10 dropped
        assert_eq!(*ts.last().unwrap(), 39);
    }
}
