//! Per-session turn/tool breakdown — the drill-down behind a Sessions-table
//! row ("why was *this* session huge?").
//!
//! Flat by design: a ranked tool list plus a turn list, not a nested tree.
//! The deep project→date→session→turn→tool treemap this replaces was an
//! explorer nobody explored; the one place per-session depth answers a live
//! question is when the user has already picked a session.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Result;
use crate::analyzer::{aggregate_session_tools, get_model_name};
use crate::tools::extract_tool_data;
use crate::types::ToolData;
use tt_claude_code::TranscriptEntry;

/// Find the transcript path for a session ID under the projects dir.
pub fn find_session_path(projects_dir: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    for project_entry in std::fs::read_dir(projects_dir)? {
        let project_path = project_entry?.path();
        if !project_path.is_dir() {
            continue;
        }
        let jsonl_path = project_path.join(format!("{session_id}.jsonl"));
        if jsonl_path.exists() {
            return Ok(Some(jsonl_path));
        }
    }
    Ok(None)
}

/// One prompt-or-response step of the session, named after what it did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnBreakdown {
    /// Human name: `"Turn 3: User"`, `"Read: main.rs"`, `"Bash, Edit, Read"`.
    pub name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Dominant tool for color-coding; `None` for user prompts.
    pub tool_name: Option<String>,
    pub model: String,
}

/// Everything the breakdown dialog renders for one session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBreakdown {
    /// Tools ranked by attributed tokens, with `"Nx"` call counts in `detail`.
    pub tools: Vec<ToolData>,
    /// Session steps in transcript order.
    pub turns: Vec<TurnBreakdown>,
}

/// Build the breakdown from parsed transcript entries.
pub fn build_session_breakdown(entries: &[TranscriptEntry]) -> SessionBreakdown {
    let mut turns = Vec::new();
    let mut turn_number = 0;
    let mut seen = std::collections::HashSet::new();

    for entry in entries {
        if entry.entry_type != "user" && entry.entry_type != "assistant" {
            continue;
        }
        let Some(message) = &entry.message else {
            continue;
        };
        // Skip streaming re-logs of an already-counted assistant message, the
        // same dedup rule `analyze_session` applies — without it a heavy turn
        // shows up two or three times.
        if let Some(key) = entry.dedup_key()
            && !seen.insert(key)
        {
            continue;
        }
        let role = message.role.as_deref().unwrap_or("");
        if role == "user" {
            turn_number += 1;
        }

        let Some(usage) = &message.usage else {
            continue;
        };
        let input_tokens = usage.input_tokens.unwrap_or(0);
        let output_tokens = usage.output_tokens.unwrap_or(0);
        if input_tokens + output_tokens == 0 {
            continue;
        }

        let tools = extract_tool_data(message.content.as_ref(), input_tokens, output_tokens);
        let (name, tool_name): (String, Option<String>) = if role == "user" {
            (format!("Turn {turn_number}: User"), None)
        } else if tools.len() == 1 {
            let t = &tools[0];
            let name = match &t.detail {
                Some(d) => format!("{}: {}", t.name, d),
                None => t.name.clone(),
            };
            (name, Some(t.name.clone()))
        } else if tools.len() > 1 {
            let mut unique: Vec<String> = Vec::new();
            for t in &tools {
                if !unique.contains(&t.name) {
                    unique.push(t.name.clone());
                }
            }
            let joined = unique.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            let name = if unique.len() > 3 { format!("{joined}…") } else { joined };
            (name, Some(tools[0].name.clone()))
        } else {
            (format!("Turn {turn_number}: Response"), Some("Response".to_string()))
        };

        turns.push(TurnBreakdown {
            name,
            input_tokens,
            output_tokens,
            tool_name,
            model: get_model_name(message.model.as_deref()),
        });
    }

    SessionBreakdown { tools: aggregate_session_tools(entries), turns }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tt_claude_code::{Content, Message, Usage};

    fn tool_use_block(name: &str, input: Value) -> Value {
        json!({ "type": "tool_use", "id": "t", "name": name, "input": input })
    }

    fn assistant_entry(input: i64, output: i64, content: Vec<Value>) -> TranscriptEntry {
        TranscriptEntry {
            entry_type: "assistant".to_string(),
            message: Some(Message {
                role: Some("assistant".to_string()),
                model: Some("claude-fable-5".to_string()),
                usage: Some(Usage {
                    input_tokens: Some(input),
                    output_tokens: Some(output),
                    ..Default::default()
                }),
                content: Some(Content::Blocks(content)),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn user_entry(input: i64, output: i64) -> TranscriptEntry {
        TranscriptEntry {
            entry_type: "user".to_string(),
            message: Some(Message {
                role: Some("user".to_string()),
                usage: Some(Usage {
                    input_tokens: Some(input),
                    output_tokens: Some(output),
                    ..Default::default()
                }),
                content: Some(Content::Text("hi".to_string())),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn empty_session_is_empty() {
        let b = build_session_breakdown(&[]);
        assert!(b.turns.is_empty());
        assert!(b.tools.is_empty());
    }

    #[test]
    fn zero_token_turns_are_skipped() {
        let b = build_session_breakdown(&[assistant_entry(0, 0, vec![])]);
        assert!(b.turns.is_empty());
    }

    #[test]
    fn single_tool_turn_named_after_tool() {
        let entries = [
            user_entry(100, 50),
            assistant_entry(200, 100, vec![tool_use_block("Read", json!({"file_path": "/a.rs"}))]),
        ];
        let b = build_session_breakdown(&entries);
        assert_eq!(b.turns.len(), 2);
        assert_eq!(b.turns[0].name, "Turn 1: User");
        assert_eq!(b.turns[0].tool_name, None);
        assert_eq!(b.turns[1].name, "Read: a.rs");
        assert_eq!(b.turns[1].tool_name.as_deref(), Some("Read"));
        assert_eq!(b.turns[1].model, "Fable");
        // Tool aggregation rides the same entries.
        assert_eq!(b.tools.len(), 1);
        assert_eq!(b.tools[0].name, "Read");
        assert_eq!(b.tools[0].detail.as_deref(), Some("1x"));
    }

    #[test]
    fn multi_tool_turn_joins_unique_names() {
        let entries = [assistant_entry(
            300,
            100,
            vec![
                tool_use_block("Read", json!({"file_path": "/a.rs"})),
                tool_use_block("Edit", json!({"file_path": "/a.rs"})),
                tool_use_block("Read", json!({"file_path": "/b.rs"})),
            ],
        )];
        let b = build_session_breakdown(&entries);
        assert_eq!(b.turns[0].name, "Read, Edit");
        assert_eq!(b.turns[0].tool_name.as_deref(), Some("Read"));
    }

    #[test]
    fn streaming_relogs_are_deduped() {
        let mut e = assistant_entry(100, 40, vec![json!({"type": "text", "text": "hi"})]);
        e.request_id = Some("req_1".to_string());
        e.message.as_mut().unwrap().id = Some("msg_1".to_string());
        let entries = [e.clone(), e.clone(), e];
        assert_eq!(build_session_breakdown(&entries).turns.len(), 1);
    }

    #[test]
    fn find_session_path_locates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-code-demo");
        std::fs::create_dir(&proj).unwrap();
        let file = proj.join("wanted.jsonl");
        std::fs::write(&file, "{}").unwrap();

        assert_eq!(find_session_path(tmp.path(), "wanted").unwrap(), Some(file));
        assert_eq!(find_session_path(tmp.path(), "missing").unwrap(), None);
    }

    #[test]
    fn plain_response_turn_named_response() {
        let entries = [assistant_entry(
            100,
            40,
            vec![json!({"type": "text", "text": "done"})],
        )];
        let b = build_session_breakdown(&entries);
        assert_eq!(b.turns[0].name, "Turn 0: Response");
        assert_eq!(b.turns[0].tool_name.as_deref(), Some("Response"));
    }
}
