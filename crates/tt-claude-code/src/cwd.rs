//! Session-cwd projection: the real filesystem directory a session ran in.
//!
//! Unlike the encoded project directory name (`/` → `-`, lossy for paths
//! containing literal hyphens), `cwd` on each transcript line is the exact
//! path — the precise identity a "resume this session elsewhere" action needs.

use std::path::Path;

use crate::parse::{parse_transcript, parse_transcript_file};
use crate::types::TranscriptEntry;

/// The session's working directory: the last non-empty `cwd` seen (mirrors
/// [`crate::title::session_title`]'s "last wins" convention, though in
/// practice this is constant across every line of a session).
pub fn session_cwd(entries: &[TranscriptEntry]) -> Option<String> {
    let mut cwd = None;
    for entry in entries {
        if let Some(c) = entry.cwd.as_deref() {
            let c = c.trim();
            if !c.is_empty() {
                cwd = Some(c.to_string());
            }
        }
    }
    cwd
}

/// [`session_cwd`] over raw JSONL content.
pub fn session_cwd_str(content: &str) -> Option<String> {
    session_cwd(&parse_transcript(content))
}

/// [`session_cwd`] over a JSONL file. `None` when unreadable/missing or when
/// the transcript carries no `cwd`.
pub fn session_cwd_file(path: &Path) -> Option<String> {
    session_cwd(&parse_transcript_file(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_last_non_empty_cwd() {
        let c = "{\"cwd\":\"/a/b\"}\n{\"cwd\":\"  \"}\n{\"cwd\":\"/a/c\"}";
        assert_eq!(session_cwd_str(c), Some("/a/c".to_string()));
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(session_cwd_str("{\"type\":\"user\"}"), None);
        assert_eq!(session_cwd_file(Path::new("/no/such.jsonl")), None);
    }
}
