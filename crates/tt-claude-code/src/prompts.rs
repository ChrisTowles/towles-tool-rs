//! User-prompt projection: the text the human actually typed in a session.
//!
//! Used as the search corpus for "find that session where I said …". The
//! transcript's `user` lines also carry machine noise — tool results, slash
//! command envelopes (`<command-name>…`), injected `<system-reminder>` blocks,
//! sidechain (subagent) turns — which must not match a search, so this filters
//! down to human-authored text.

use crate::types::TranscriptEntry;

/// Machine-generated user-line texts, excluded from the prompt corpus.
fn is_noise(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty()
        || t.starts_with('<') // <command-name>, <system-reminder>, <local-command-stdout>, …
        || t.starts_with("Caveat:")
        || t.starts_with("[Request interrupted")
}

/// The human-typed prompt texts of a session, in order. Skips sidechain
/// (subagent) turns, tool-result blocks, and injected envelopes.
pub fn user_prompts(entries: &[TranscriptEntry]) -> Vec<String> {
    let mut prompts = Vec::new();
    for entry in entries {
        if entry.entry_type != "user" || entry.is_sidechain == Some(true) {
            continue;
        }
        let Some(message) = &entry.message else {
            continue;
        };
        if message.role.as_deref() != Some("user") {
            continue;
        }
        let Some(content) = &message.content else {
            continue;
        };
        if let Some(text) = content.as_text() {
            if !is_noise(text) {
                prompts.push(text.trim().to_string());
            }
            continue;
        }
        for text in content.text_blocks() {
            if !is_noise(text) {
                prompts.push(text.trim().to_string());
            }
        }
    }
    prompts
}

/// [`user_prompts`] joined into one search blob, capped at `max_bytes` (on a
/// char boundary). A cap keeps a 500-session scan's memory bounded.
pub fn user_prompt_blob(entries: &[TranscriptEntry], max_bytes: usize) -> String {
    let mut blob = String::new();
    for prompt in user_prompts(entries) {
        if !blob.is_empty() {
            blob.push('\n');
        }
        blob.push_str(&prompt);
        if blob.len() >= max_bytes {
            let mut cut = max_bytes;
            while !blob.is_char_boundary(cut) {
                cut -= 1;
            }
            blob.truncate(cut);
            break;
        }
    }
    blob
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_transcript;

    fn user_line(text: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":{}}}}}"#,
            serde_json::to_string(text).unwrap()
        )
    }

    #[test]
    fn collects_bare_string_and_text_blocks() {
        let content = format!(
            "{}\n{}",
            user_line("fix the bug"),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"and add a test"}]}}"#
        );
        let prompts = user_prompts(&parse_transcript(&content));
        assert_eq!(prompts, vec!["fix the bug", "and add a test"]);
    }

    #[test]
    fn skips_sidechain_noise_and_tool_results() {
        let content = [
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent task"}}"#.to_string(),
            user_line("<command-name>/foo</command-name>"),
            user_line("<system-reminder>stuff</system-reminder>"),
            user_line("Caveat: the messages below were generated"),
            user_line("[Request interrupted by user]"),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"out"}]}}"#.to_string(),
            r#"{"type":"assistant","message":{"role":"assistant","content":"reply"}}"#.to_string(),
            user_line("real question"),
        ]
        .join("\n");
        assert_eq!(user_prompts(&parse_transcript(&content)), vec!["real question"]);
    }

    #[test]
    fn blob_joins_and_caps_on_char_boundary() {
        let content = format!("{}\n{}", user_line("abcé"), user_line("defgh"));
        let entries = parse_transcript(&content);
        assert_eq!(user_prompt_blob(&entries, 1024), "abcé\ndefgh");
        // Cap of 4 falls inside 'é' (bytes 3..5) — must cut back to 3.
        assert_eq!(user_prompt_blob(&entries, 4), "abc");
    }

    #[test]
    fn empty_transcript_is_empty() {
        assert!(user_prompts(&[]).is_empty());
        assert_eq!(user_prompt_blob(&[], 100), "");
    }
}
