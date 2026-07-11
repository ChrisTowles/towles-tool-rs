//! Custom session ordering, persisted to disk. Ports slot-1
//! `runtime/server/session-order.ts`.
//!
//! The persist path is parameterized (tests pass a tempdir); the default
//! location is `~/.config/towles-tool/agentboard/session-order.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A reorder direction. Ports `ReorderDelta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReorderDelta {
    Up,
    Down,
    Top,
    Bottom,
}

/// Default persisted-order path: `~/.config/towles-tool/agentboard/session-order.json`.
pub fn default_session_order_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("towles-tool")
        .join("agentboard")
        .join("session-order.json")
}

/// Maintains a custom session ordering, loaded on construction and saved after
/// every reorder when a persist path is set. Ports the `SessionOrder` class.
#[derive(Debug, Default)]
pub struct SessionOrder {
    order: Vec<String>,
    persist_path: Option<PathBuf>,
}

/// Tolerant shape for the persisted file: either a bare array of names, or an
/// object with an `order` array. Corrupt files are ignored (start fresh).
fn parse_persisted(raw: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let arr = match &value {
        serde_json::Value::Array(a) => Some(a),
        serde_json::Value::Object(o) => o.get("order").and_then(|v| v.as_array()),
        _ => None,
    };
    arr.map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

impl SessionOrder {
    /// Create with an optional persist path, loading any existing order from it.
    pub fn new(persist_path: Option<PathBuf>) -> Self {
        let order = persist_path
            .as_deref()
            .filter(|p| p.exists())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|raw| parse_persisted(&raw))
            .unwrap_or_default();
        Self { order, persist_path }
    }

    /// The current order (mainly for tests/inspection).
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// Sync with the current session names: drop stale entries, insert new ones
    /// alphabetically. Ports `sync`.
    ///
    /// Deviation: uses Rust's byte/lexicographic ordering, not JS `localeCompare`
    /// (locale collation). Matches for plain ASCII session names.
    pub fn sync(&mut self, names: &[String]) {
        let name_set: std::collections::HashSet<&String> = names.iter().collect();
        self.order.retain(|n| name_set.contains(n));

        let mut new_names: Vec<&String> =
            names.iter().filter(|n| !self.order.contains(n)).collect();
        new_names.sort();
        for n in new_names {
            let idx = self.order.iter().position(|existing| existing.as_str() > n.as_str());
            match idx {
                None => self.order.push(n.clone()),
                Some(i) => self.order.insert(i, n.clone()),
            }
        }
    }

    /// Move a session up/down by one or jump it to top/bottom, then persist. Ports `reorder`.
    pub fn reorder(&mut self, name: &str, delta: ReorderDelta) {
        let Some(idx) = self.order.iter().position(|n| n == name) else {
            return;
        };
        match delta {
            ReorderDelta::Top => {
                self.order.remove(idx);
                self.order.insert(0, name.to_string());
            }
            ReorderDelta::Bottom => {
                self.order.remove(idx);
                self.order.push(name.to_string());
            }
            ReorderDelta::Up | ReorderDelta::Down => {
                let new_idx = if matches!(delta, ReorderDelta::Up) {
                    if idx == 0 {
                        return;
                    }
                    idx - 1
                } else {
                    if idx + 1 >= self.order.len() {
                        return;
                    }
                    idx + 1
                };
                self.order.swap(idx, new_idx);
            }
        }
        self.save();
    }

    /// Apply the custom order to a list of names (stable; unknown names sort last).
    /// Ports `apply`.
    pub fn apply(&self, names: &[String]) -> Vec<String> {
        let pos = |name: &str| self.order.iter().position(|n| n == name).unwrap_or(usize::MAX);
        let mut out = names.to_vec();
        out.sort_by_key(|name| pos(name));
        out
    }

    /// Persist the current order as a JSON array plus a trailing newline. Best-effort.
    fn save(&self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        if let Ok(json) = serde_json::to_string(&self.order) {
            let _ = crate::persist::write_atomic(path, &format!("{json}\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sync_adds_alphabetically_and_removes_stale() {
        let mut so = SessionOrder::new(None);
        so.sync(&names(&["charlie", "alpha", "bravo"]));
        assert_eq!(so.order(), &names(&["alpha", "bravo", "charlie"]));
        // Remove bravo, add delta.
        so.sync(&names(&["alpha", "charlie", "delta"]));
        assert_eq!(so.order(), &names(&["alpha", "charlie", "delta"]));
    }

    #[test]
    fn reorder_up_down_top_bottom() {
        let mut so = SessionOrder::new(None);
        so.sync(&names(&["a", "b", "c", "d"]));
        so.reorder("c", ReorderDelta::Up);
        assert_eq!(so.order(), &names(&["a", "c", "b", "d"]));
        so.reorder("c", ReorderDelta::Top);
        assert_eq!(so.order(), &names(&["c", "a", "b", "d"]));
        so.reorder("a", ReorderDelta::Bottom);
        assert_eq!(so.order(), &names(&["c", "b", "d", "a"]));
        so.reorder("c", ReorderDelta::Up); // already top → no-op
        assert_eq!(so.order(), &names(&["c", "b", "d", "a"]));
    }

    #[test]
    fn reorder_unknown_name_is_noop() {
        let mut so = SessionOrder::new(None);
        so.sync(&names(&["a", "b"]));
        so.reorder("zzz", ReorderDelta::Top);
        assert_eq!(so.order(), &names(&["a", "b"]));
    }

    #[test]
    fn apply_sorts_known_first_unknown_last_stable() {
        let mut so = SessionOrder::new(None);
        so.sync(&names(&["b", "a"])); // order becomes [a, b]
        so.reorder("b", ReorderDelta::Top); // [b, a]
        let applied = so.apply(&names(&["a", "b", "z", "y"]));
        // b, a known (in order); z, y unknown keep input order.
        assert_eq!(applied, names(&["b", "a", "z", "y"]));
    }

    #[test]
    fn persists_and_reloads_from_disk() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("session-order.json");
        {
            let mut so = SessionOrder::new(Some(path.clone()));
            so.sync(&names(&["a", "b", "c"]));
            so.reorder("c", ReorderDelta::Top); // triggers save
        }
        // File written as a JSON array + newline.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with('\n'));
        assert_eq!(raw.trim(), r#"["c","a","b"]"#);
        // A fresh instance reloads it.
        let so2 = SessionOrder::new(Some(path));
        assert_eq!(so2.order(), &names(&["c", "a", "b"]));
    }

    #[test]
    fn loads_object_with_order_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session-order.json");
        std::fs::write(&path, r#"{"order":["x","y"]}"#).unwrap();
        let so = SessionOrder::new(Some(path));
        assert_eq!(so.order(), &names(&["x", "y"]));
    }

    #[test]
    fn corrupt_file_starts_fresh() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session-order.json");
        std::fs::write(&path, "not json {[").unwrap();
        let so = SessionOrder::new(Some(path));
        assert!(so.order().is_empty());
    }

    #[test]
    fn delta_serializes_lowercase() {
        assert_eq!(serde_json::to_value(ReorderDelta::Top).unwrap(), serde_json::json!("top"));
    }
}
