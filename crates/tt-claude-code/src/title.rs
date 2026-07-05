//! Session-title projection: the human name of a session.
//!
//! Claude Code writes two title-carrying line types, both flat and re-emitted as
//! the title changes (so the **last** occurrence wins):
//! `{"type":"custom-title","customTitle":"…"}` (user-set, authoritative) and
//! `{"type":"ai-title","aiTitle":"…"}` (auto-generated fallback).

use std::path::Path;

use crate::parse::{parse_transcript, parse_transcript_file};
use crate::types::TranscriptEntry;

/// The session title from parsed entries: the **last** non-empty `customTitle`,
/// else the **last** non-empty `aiTitle`, else `None`. Titles are trimmed.
pub fn session_title(entries: &[TranscriptEntry]) -> Option<String> {
    let mut custom: Option<String> = None;
    let mut ai: Option<String> = None;
    for entry in entries {
        if let Some(t) = entry.custom_title.as_deref() {
            let t = t.trim();
            if !t.is_empty() {
                custom = Some(t.to_string());
            }
        }
        if let Some(t) = entry.ai_title.as_deref() {
            let t = t.trim();
            if !t.is_empty() {
                ai = Some(t.to_string());
            }
        }
    }
    custom.or(ai)
}

/// [`session_title`] over raw JSONL content.
pub fn session_title_str(content: &str) -> Option<String> {
    session_title(&parse_transcript(content))
}

/// [`session_title`] over a JSONL file. `None` when unreadable/missing or when
/// the transcript carries no title line.
pub fn session_title_file(path: &Path) -> Option<String> {
    session_title(&parse_transcript_file(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_custom_over_ai() {
        let c = "{\"type\":\"ai-title\",\"aiTitle\":\"Auto\"}\n{\"type\":\"custom-title\",\"customTitle\":\"Mine\"}";
        assert_eq!(session_title_str(c), Some("Mine".to_string()));
    }

    #[test]
    fn falls_back_to_ai() {
        assert_eq!(
            session_title_str("{\"type\":\"ai-title\",\"aiTitle\":\"Auto\"}"),
            Some("Auto".to_string())
        );
    }

    #[test]
    fn last_occurrence_wins() {
        let c = "{\"type\":\"ai-title\",\"aiTitle\":\"First\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"Second\"}";
        assert_eq!(session_title_str(c), Some("Second".to_string()));
    }

    #[test]
    fn empty_treated_as_absent() {
        let c = "{\"type\":\"custom-title\",\"customTitle\":\"  \"}\n{\"type\":\"ai-title\",\"aiTitle\":\"Real\"}";
        assert_eq!(session_title_str(c), Some("Real".to_string()));
    }

    #[test]
    fn none_when_missing() {
        assert_eq!(session_title_str("{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}"), None);
        assert_eq!(session_title_file(Path::new("/no/such.jsonl")), None);
    }
}
