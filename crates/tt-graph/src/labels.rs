//! Session-label extraction. Ports `src/commands/graph/labels.ts`.
//!
//! The cleanup pipeline mirrors the TS regex chain exactly, using compiled
//! [`regex`] patterns held in `LazyLock`s.

use std::sync::LazyLock;

use regex::Regex;

use tt_claude_code::{Content, TranscriptEntry};

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
});
static COMMAND_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/\S+\s*").unwrap());
static XML_WITH_CONTENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>[^<]*</[^>]+>").unwrap());
static XML_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static CAVEAT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*Caveat:.*$").unwrap());
static NEWLINE_TAIL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n.*").unwrap());
static CONTROL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x00-\x1F]+").unwrap());

/// Extract the first text block from an array content, if any.
fn first_text_block(content: &Content) -> Option<String> {
    let blocks = content.blocks()?;
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
            && !text.is_empty()
        {
            return Some(text.to_string());
        }
    }
    None
}

/// Extract a meaningful label from session entries. Ports `extractSessionLabel`.
pub fn extract_session_label(entries: &[TranscriptEntry], session_id: &str) -> String {
    let mut first_user_text: Option<String> = None;
    let mut first_assistant_text: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut slug: Option<String> = None;

    for entry in entries {
        if git_branch.is_none()
            && let Some(gb) = &entry.git_branch
        {
            git_branch = Some(gb.clone());
        }
        if slug.is_none()
            && let Some(s) = &entry.slug
        {
            slug = Some(s.clone());
        }

        let Some(message) = &entry.message else {
            continue;
        };

        // First user message with actual text (not a UUID reference).
        if first_user_text.is_none()
            && entry.entry_type == "user"
            && message.role.as_deref() == Some("user")
        {
            match &message.content {
                Some(Content::Text(text)) => {
                    if !UUID_RE.is_match(text) && !text.is_empty() {
                        first_user_text = Some(text.clone());
                    }
                }
                Some(content @ Content::Blocks(_)) => {
                    if let Some(text) = first_text_block(content) {
                        first_user_text = Some(text);
                    }
                }
                None => {}
            }
        }

        // First assistant text response.
        if first_assistant_text.is_none()
            && entry.entry_type == "assistant"
            && message.role.as_deref() == Some("assistant")
            && let Some(content) = &message.content
            && let Some(text) = first_text_block(content)
        {
            first_assistant_text = Some(text);
        }

        if first_user_text.is_some() {
            break;
        }
    }

    // Priority: user text > assistant text > git branch > slug > short ID.
    let short_id: String = session_id.chars().take(8).collect();
    let mut label = first_user_text
        .or(first_assistant_text)
        .or_else(|| git_branch.clone())
        .or_else(|| slug.clone())
        .unwrap_or_else(|| short_id.clone());

    // Clean up the label (each step mirrors the TS regex chain).
    label = COMMAND_PREFIX_RE.replace(&label, "").into_owned();
    label = XML_WITH_CONTENT_RE.replace_all(&label, "").into_owned();
    label = XML_TAG_RE.replace_all(&label, "").into_owned();
    label = CAVEAT_RE.replace(&label, "").into_owned();
    label = NEWLINE_TAIL_RE.replace_all(&label, "").into_owned();
    label = CONTROL_RE.replace_all(&label, " ").into_owned();
    label = label.trim().to_string();

    // If still empty or too short, use fallback.
    if label.chars().count() < 3 {
        label = slug.unwrap_or(short_id);
    }

    // Truncate very long labels.
    if label.chars().count() > 80 {
        let head: String = label.chars().take(77).collect();
        label = format!("{head}...");
    }

    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_text_entry(content: &str) -> TranscriptEntry {
        TranscriptEntry {
            entry_type: "user".to_string(),
            message: Some(tt_claude_code::Message {
                role: Some("user".to_string()),
                content: Some(Content::Text(content.to_string())),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn user_blocks_entry(text: &str) -> TranscriptEntry {
        TranscriptEntry {
            entry_type: "user".to_string(),
            message: Some(tt_claude_code::Message {
                role: Some("user".to_string()),
                content: Some(Content::Blocks(vec![json!({ "type": "text", "text": text })])),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn assistant_blocks_entry(text: &str) -> TranscriptEntry {
        TranscriptEntry {
            entry_type: "assistant".to_string(),
            message: Some(tt_claude_code::Message {
                role: Some("assistant".to_string()),
                content: Some(Content::Blocks(vec![json!({ "type": "text", "text": text })])),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn uses_first_user_text() {
        let entries = [user_text_entry("Fix the bug in parser")];
        assert_eq!(extract_session_label(&entries, "abc12345"), "Fix the bug in parser");
    }

    #[test]
    fn skips_uuid_only_user_messages() {
        let entries = [
            user_text_entry("a1b2c3d4-e5f6-7890-abcd-ef1234567890"),
            user_text_entry("Real message"),
        ];
        assert_eq!(extract_session_label(&entries, "abc12345"), "Real message");
    }

    #[test]
    fn extracts_text_from_array_content() {
        let entries = [user_blocks_entry("Array content message")];
        assert_eq!(extract_session_label(&entries, "abc12345"), "Array content message");
    }

    #[test]
    fn falls_back_to_assistant_text() {
        let entries = [assistant_blocks_entry("I'll help you with that")];
        assert_eq!(extract_session_label(&entries, "abc12345"), "I'll help you with that");
    }

    #[test]
    fn falls_back_to_git_branch() {
        let entry = TranscriptEntry {
            entry_type: "user".to_string(),
            git_branch: Some("feat/new-feature".to_string()),
            ..Default::default()
        };
        assert_eq!(extract_session_label(&[entry], "abc12345"), "feat/new-feature");
    }

    #[test]
    fn falls_back_to_short_session_id() {
        assert_eq!(extract_session_label(&[], "abc12345-long-id"), "abc12345");
    }

    #[test]
    fn removes_command_prefix() {
        let entries = [user_text_entry("/review Fix the parser")];
        assert_eq!(extract_session_label(&entries, "abc12345"), "Fix the parser");
    }

    #[test]
    fn removes_xml_tags() {
        let entries = [user_text_entry("<tag>content</tag> Real text")];
        assert_eq!(extract_session_label(&entries, "abc12345"), "Real text");
    }

    #[test]
    fn truncates_labels_over_80_chars() {
        let entries = [user_text_entry(&"A".repeat(100))];
        let label = extract_session_label(&entries, "abc12345");
        assert_eq!(label.chars().count(), 80);
        assert!(label.ends_with("..."));
    }

    #[test]
    fn uses_slug_fallback_for_short_labels() {
        let entry = TranscriptEntry { slug: Some("my-slug".to_string()), ..Default::default() };
        assert_eq!(extract_session_label(&[entry], "abc12345"), "my-slug");
    }
}
