//! Parsing session-transcript JSONL into [`TranscriptEntry`] values.
//!
//! The pure string entry point ([`parse_transcript`]) is split from the
//! filesystem one ([`parse_transcript_file`]) so logic stays testable with plain
//! strings and callers pass explicit paths. Blank and malformed lines are
//! silently skipped; an unreadable file yields an empty vec — the tolerant
//! behavior every consumer relies on.

use std::path::Path;

use crate::types::TranscriptEntry;

/// Parse JSONL content into a list of [`TranscriptEntry`]. Empty lines and lines
/// that fail to parse are silently skipped.
pub fn parse_transcript(content: &str) -> Vec<TranscriptEntry> {
    content
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<TranscriptEntry>(line).ok())
        .collect()
}

/// Read and parse a session-transcript JSONL file. Returns an empty vec when the
/// file cannot be read.
pub fn parse_transcript_file(path: &Path) -> Vec<TranscriptEntry> {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_transcript(&content),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_blank_and_malformed() {
        let content = "\n{\"type\":\"user\"}\nnot json\n  \n{\"type\":\"assistant\"}";
        let entries = parse_transcript(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_type, "user");
        assert_eq!(entries[1].entry_type, "assistant");
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(parse_transcript_file(Path::new("/no/such/file.jsonl")).is_empty());
    }
}
