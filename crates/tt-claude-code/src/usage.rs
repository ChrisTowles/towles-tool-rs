//! Token-usage projection: deduplicated totals across a transcript.
//!
//! Claude Code re-logs the same assistant message's `usage` more than once
//! (streaming re-logs, compaction, sidechain copies), so summing every line
//! inflates totals relative to ccusage / Claude-Code-Usage-Monitor, both of
//! which dedup by [`TranscriptEntry::dedup_key`] (`message.id` + `requestId`).
//! Entries lacking either id are each counted once.

use std::collections::HashSet;
use std::path::Path;

use crate::parse::{parse_transcript, parse_transcript_file};
use crate::types::TranscriptEntry;

/// Deduplicated token totals across a transcript.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
}

impl UsageTotals {
    /// Input + output (the "billable" prompt/response volume, excluding cache).
    pub fn billable(&self) -> i64 {
        self.input_tokens + self.output_tokens
    }
}

/// Sum usage across entries, deduplicating by [`TranscriptEntry::dedup_key`].
pub fn usage_totals(entries: &[TranscriptEntry]) -> UsageTotals {
    let mut totals = UsageTotals::default();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in entries {
        if let Some(key) = entry.dedup_key()
            && !seen.insert(key)
        {
            continue;
        }
        let Some(usage) = entry.message.as_ref().and_then(|m| m.usage.as_ref()) else {
            continue;
        };
        totals.input_tokens += usage.input_tokens.unwrap_or(0);
        totals.output_tokens += usage.output_tokens.unwrap_or(0);
        totals.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
        totals.cache_creation_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
    }
    totals
}

/// [`usage_totals`] over raw JSONL content.
pub fn usage_totals_str(content: &str) -> UsageTotals {
    usage_totals(&parse_transcript(content))
}

/// [`usage_totals`] over a JSONL file. Zeroed totals when unreadable/missing.
pub fn usage_totals_file(path: &Path) -> UsageTotals {
    usage_totals(&parse_transcript_file(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_repeated_pair() {
        let line = "{\"requestId\":\"req_1\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":100,\"output_tokens\":20}}}";
        let content = format!("{line}\n{line}\n{line}");
        let t = usage_totals_str(&content);
        assert_eq!(t.input_tokens, 100);
        assert_eq!(t.output_tokens, 20);
        assert_eq!(t.billable(), 120);
    }

    #[test]
    fn distinct_pairs_and_id_less_each_count() {
        let content = "\
{\"requestId\":\"req_1\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":100}}}
{\"requestId\":\"req_2\",\"message\":{\"id\":\"msg_2\",\"usage\":{\"input_tokens\":50}}}
{\"message\":{\"usage\":{\"input_tokens\":10}}}
{\"message\":{\"usage\":{\"input_tokens\":10}}}";
        assert_eq!(usage_totals_str(content).input_tokens, 170);
    }

    #[test]
    fn sums_cache_tokens() {
        let content = "{\"message\":{\"usage\":{\"cache_read_input_tokens\":800,\"cache_creation_input_tokens\":200}}}";
        let t = usage_totals_str(content);
        assert_eq!(t.cache_read_tokens, 800);
        assert_eq!(t.cache_creation_tokens, 200);
        assert_eq!(t.billable(), 0);
    }

    #[test]
    fn missing_file_is_zero() {
        assert_eq!(usage_totals_file(Path::new("/no/such.jsonl")), UsageTotals::default());
    }
}
