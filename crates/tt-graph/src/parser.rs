//! JSONL parsing and day-based filtering. Ports `src/commands/graph/parser.ts`.
//!
//! The TS version injects a `readFile` function so tests can supply content
//! without touching disk. Here the parsing logic ([`parse_jsonl`]) is split
//! from the filesystem entry point ([`read_jsonl`]) so the pure logic is
//! testable with plain strings and the fs function takes an explicit path.

use std::path::Path;

use crate::types::JournalEntry;

/// Anything with a modification time, so [`filter_by_days`] can be generic like
/// the TS `<T extends { mtime: number }>`.
pub trait HasMtime {
    fn mtime(&self) -> i64;
}

impl HasMtime for crate::types::SessionResult {
    fn mtime(&self) -> i64 {
        self.mtime
    }
}

/// Calculate the cutoff timestamp (ms since epoch) for a `days` filter. Returns
/// `0` when `days <= 0` (no filtering).
///
/// Unlike the TS `calculateCutoffMs(days)`, which reads `Date.now()` internally,
/// this takes `now_ms` explicitly so callers/tests stay deterministic.
pub fn calculate_cutoff_ms(days: f64, now_ms: i64) -> i64 {
    if days > 0.0 { now_ms - (days * 24.0 * 60.0 * 60.0 * 1000.0) as i64 } else { 0 }
}

/// Filter items by mtime against a `days` cutoff. Returns all items when
/// `days <= 0`.
pub fn filter_by_days<T: HasMtime + Clone>(items: &[T], days: f64, now_ms: i64) -> Vec<T> {
    let cutoff = calculate_cutoff_ms(days, now_ms);
    if cutoff == 0 {
        return items.to_vec();
    }
    items.iter().filter(|i| i.mtime() >= cutoff).cloned().collect()
}

/// Parse JSONL content into a list of [`JournalEntry`]. Empty lines and lines
/// that fail to parse are silently skipped (matching `parser.ts`).
pub fn parse_jsonl(content: &str) -> Vec<JournalEntry> {
    let mut entries = Vec::new();
    for line in content.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<JournalEntry>(line) {
            entries.push(entry);
        }
    }
    entries
}

/// Read a JSONL file and parse it. Returns an empty vector when the file cannot
/// be read (mirroring the tolerant behavior the graph builders rely on).
pub fn read_jsonl(path: &Path) -> Vec<JournalEntry> {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_jsonl(&content),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    const NOW: i64 = 1_700_000_000_000;

    #[derive(Clone, Debug, PartialEq)]
    struct Item {
        mtime: i64,
        name: &'static str,
    }
    impl HasMtime for Item {
        fn mtime(&self) -> i64 {
            self.mtime
        }
    }

    // ── calculateCutoffMs ──

    #[test]
    fn cutoff_returns_zero_for_non_positive_days() {
        assert_eq!(calculate_cutoff_ms(0.0, NOW), 0);
        assert_eq!(calculate_cutoff_ms(-1.0, NOW), 0);
    }

    #[test]
    fn cutoff_is_days_in_the_past() {
        assert_eq!(calculate_cutoff_ms(7.0, NOW), NOW - 7 * DAY_MS);
    }

    #[test]
    fn cutoff_is_larger_for_more_days() {
        assert!(calculate_cutoff_ms(30.0, NOW) < calculate_cutoff_ms(7.0, NOW));
    }

    // ── filterByDays ──

    fn items() -> Vec<Item> {
        vec![
            Item { mtime: NOW - DAY_MS, name: "1-day-ago" },
            Item { mtime: NOW - 5 * DAY_MS, name: "5-days-ago" },
            Item { mtime: NOW - 10 * DAY_MS, name: "10-days-ago" },
            Item { mtime: NOW - 20 * DAY_MS, name: "20-days-ago" },
        ]
    }

    #[test]
    fn filter_returns_all_when_days_non_positive() {
        assert_eq!(filter_by_days(&items(), 0.0, NOW), items());
        assert_eq!(filter_by_days(&items(), -5.0, NOW), items());
    }

    #[test]
    fn filter_drops_items_older_than_cutoff() {
        let result = filter_by_days(&items(), 7.0, NOW);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result.iter().map(|i| i.name).collect::<Vec<_>>(),
            vec!["1-day-ago", "5-days-ago"]
        );
    }

    #[test]
    fn filter_returns_empty_when_all_too_old() {
        let result = filter_by_days(&items(), 0.001, NOW);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn filter_returns_all_when_cutoff_very_large() {
        assert_eq!(filter_by_days(&items(), 365.0, NOW).len(), 4);
    }

    #[test]
    fn filter_handles_empty() {
        let empty: Vec<Item> = Vec::new();
        assert_eq!(filter_by_days(&empty, 7.0, NOW), empty);
    }

    #[test]
    fn filter_default_seven_days() {
        let sessions = vec![
            Item { mtime: NOW - DAY_MS, name: "a" },
            Item { mtime: NOW - 6 * DAY_MS, name: "b" },
            Item { mtime: NOW - 8 * DAY_MS, name: "c" },
            Item { mtime: NOW - 30 * DAY_MS, name: "d" },
        ];
        let filtered = filter_by_days(&sessions, 7.0, NOW);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|i| i.mtime > NOW - 7 * DAY_MS));
    }

    #[test]
    fn filter_one_day_is_today_only() {
        let sessions = vec![
            Item { mtime: NOW - 12 * 60 * 60 * 1000, name: "12h" },
            Item { mtime: NOW - 25 * 60 * 60 * 1000, name: "25h" },
        ];
        assert_eq!(filter_by_days(&sessions, 1.0, NOW).len(), 1);
    }

    // ── parseJsonl ──

    #[test]
    fn parse_valid_lines() {
        let content = "{\"type\":\"user\",\"sessionId\":\"s1\",\"timestamp\":\"2025-01-01T00:00:00Z\"}\n{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"2025-01-01T00:01:00Z\"}\n";
        let entries = parse_jsonl(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_type, "user");
        assert_eq!(entries[1].entry_type, "assistant");
    }

    #[test]
    fn parse_skips_empty_lines() {
        let content = "{\"type\":\"user\",\"sessionId\":\"s1\",\"timestamp\":\"t\"}\n\n\n{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"t\"}\n";
        assert_eq!(parse_jsonl(content).len(), 2);
    }

    #[test]
    fn parse_skips_invalid_json() {
        let content = "{\"type\":\"user\",\"sessionId\":\"s1\",\"timestamp\":\"t\"}\nnot-json\n{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"t\"}\n";
        assert_eq!(parse_jsonl(content).len(), 2);
    }

    #[test]
    fn parse_empty_file() {
        assert_eq!(parse_jsonl("").len(), 0);
    }
}
