//! Day-cutoff computation for session filtering.
//!
//! Transcript **parsing** (`parse_transcript`, token counting, title extraction)
//! lives in the shared [`tt_claude_code`] crate — the single home for the
//! internal Claude Code JSONL schema. This module keeps only the mtime-based
//! day cutoff, whose clock is injected (`now_ms`) so tests stay deterministic.

/// Calculate the cutoff timestamp (ms since epoch) for a `days` filter. Returns
/// `0` when `days <= 0` (no filtering).
///
/// Takes `now_ms` explicitly so callers/tests stay deterministic.
pub fn calculate_cutoff_ms(days: f64, now_ms: i64) -> i64 {
    if days > 0.0 { now_ms - (days * 24.0 * 60.0 * 60.0 * 1000.0) as i64 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    const NOW: i64 = 1_700_000_000_000;

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
}
