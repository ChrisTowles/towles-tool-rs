//! Treemap tree construction.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Local};

use crate::Result;
use crate::analyzer::{
    aggregate_session_tools, analyze_session, extract_project_name, get_model_name,
    get_primary_model,
};
use crate::labels::extract_session_label;
use crate::tools::extract_tool_data;
use crate::types::{SessionResult, TreemapNode};
use tt_claude_code::{TranscriptEntry, parse_transcript_file};

/// First 8 characters of a session ID.
fn short_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// Build turn-level nodes from session entries. Ports `buildTurnNodes`.
pub fn build_turn_nodes(
    session_id: &str,
    entries: &[TranscriptEntry],
    file_path: Option<&Path>,
) -> Vec<TreemapNode> {
    let mut children = Vec::new();
    let mut turn_number = 0;

    for entry in entries {
        if entry.entry_type != "user" && entry.entry_type != "assistant" {
            continue;
        }
        let Some(message) = &entry.message else {
            continue;
        };
        let role = message.role.as_deref().unwrap_or("");
        let model = message.model.as_deref();

        if role == "user" {
            turn_number += 1;
        }

        let Some(usage) = &message.usage else {
            continue;
        };
        let input_tokens = usage.input_tokens.unwrap_or(0);
        let output_tokens = usage.output_tokens.unwrap_or(0);
        let total_tokens = input_tokens + output_tokens;
        if total_tokens == 0 {
            continue;
        }

        let ratio = if output_tokens > 0 {
            input_tokens as f64 / output_tokens as f64
        } else if input_tokens > 0 {
            999.0
        } else {
            0.0
        };

        let tools = extract_tool_data(message.content.as_ref(), input_tokens, output_tokens);

        let tool_children: Vec<TreemapNode> = tools
            .iter()
            .map(|tool| TreemapNode {
                name: match &tool.detail {
                    Some(d) => format!("{}: {}", tool.name, d),
                    None => tool.name.clone(),
                },
                value: Some(tool.input_tokens + tool.output_tokens),
                input_tokens: Some(tool.input_tokens),
                output_tokens: Some(tool.output_tokens),
                ratio: Some(if tool.output_tokens > 0 {
                    tool.input_tokens as f64 / tool.output_tokens as f64
                } else {
                    0.0
                }),
                tool_name: Some(tool.name.clone()),
                ..Default::default()
            })
            .collect();

        // Format the turn name based on the tools used.
        let (turn_name, primary_tool_name): (String, Option<String>) = if role == "user" {
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
            let name = if unique.len() > 3 { format!("{joined}...") } else { joined };
            (name, Some(tools[0].name.clone()))
        } else {
            (format!("Turn {turn_number}: Response"), Some("Response".to_string()))
        };

        let has_tool_children = !tool_children.is_empty();
        children.push(TreemapNode {
            name: turn_name,
            value: if has_tool_children { None } else { Some(total_tokens) },
            children: if has_tool_children { Some(tool_children) } else { None },
            session_id: Some(short_id(session_id)),
            full_session_id: Some(session_id.to_string()),
            file_path: file_path.map(|p| p.to_string_lossy().to_string()),
            tool_name: primary_tool_name,
            model: Some(get_model_name(model)),
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            ratio: Some(ratio),
            tools: if tools.is_empty() { None } else { Some(tools) },
            ..Default::default()
        });
    }

    children
}

/// Build the treemap for a single session. Ports `buildSessionTreemap`.
pub fn build_session_treemap(session_id: &str, entries: &[TranscriptEntry]) -> TreemapNode {
    TreemapNode {
        name: format!("Session {}", short_id(session_id)),
        children: Some(build_turn_nodes(session_id, entries, None)),
        ..Default::default()
    }
}

/// Format a session start time (local `HH:MM:SS`) from an entry timestamp.
///
/// Deviation: the TS uses `new Date(ts).toLocaleTimeString()`, whose exact
/// format is locale-dependent. We emit a fixed `%H:%M:%S`.
fn start_time(entries: &[TranscriptEntry]) -> Option<String> {
    let ts = entries.first()?.timestamp.as_deref()?;
    let parsed = DateTime::parse_from_rfc3339(ts).ok()?;
    let local: DateTime<Local> = parsed.with_timezone(&Local);
    Some(local.format("%H:%M:%S").to_string())
}

/// Build the treemap for all sessions, grouped by project then date. Ports
/// `buildAllSessionsTreemap`. Reads each session's JSONL from disk.
pub fn build_all_sessions_treemap(sessions: &[SessionResult]) -> Result<TreemapNode> {
    // Group by project (extracted name), preserving first-seen order.
    let mut project_order: Vec<String> = Vec::new();
    let mut by_project: BTreeMap<String, Vec<&SessionResult>> = BTreeMap::new();
    for session in sessions {
        let name = extract_project_name(&session.project);
        by_project
            .entry(name.clone())
            .or_insert_with(|| {
                project_order.push(name.clone());
                Vec::new()
            })
            .push(session);
    }

    // Sort projects by total tokens (descending), stable on first-seen order.
    let mut project_totals: Vec<(String, Vec<&SessionResult>)> = project_order
        .into_iter()
        .map(|name| {
            let sess = by_project.remove(&name).unwrap();
            (name, sess)
        })
        .collect();
    project_totals.sort_by(|a, b| {
        let ta: i64 = a.1.iter().map(|s| s.tokens).sum();
        let tb: i64 = b.1.iter().map(|s| s.tokens).sum();
        tb.cmp(&ta)
    });

    let mut project_children: Vec<TreemapNode> = Vec::new();

    for (project_name, project_sessions) in project_totals {
        // Group by date within the project.
        let mut by_date: BTreeMap<String, Vec<&SessionResult>> = BTreeMap::new();
        for session in &project_sessions {
            by_date.entry(session.date.clone()).or_default().push(session);
        }

        // Dates most-recent first.
        let mut dates: Vec<String> = by_date.keys().cloned().collect();
        dates.sort();
        dates.reverse();

        let mut date_children: Vec<TreemapNode> = Vec::new();

        for date in dates {
            let date_sessions = &by_date[&date];
            let mut session_children: Vec<TreemapNode> = Vec::new();

            for session in date_sessions {
                let entries = parse_transcript_file(&session.path);
                let analysis = analyze_session(&entries);
                // Prefer the explicit session title (custom-title > ai-title,
                // already clean) over the heuristic label derived from message
                // text. See `parse_session_title`.
                let label = session
                    .title
                    .clone()
                    .unwrap_or_else(|| extract_session_label(&entries, &session.session_id));
                let tools = aggregate_session_tools(&entries);
                let start = start_time(&entries);
                let turn_children =
                    build_turn_nodes(&session.session_id, &entries, Some(&session.path));

                let has_turns = !turn_children.is_empty();
                session_children.push(TreemapNode {
                    name: label,
                    value: if has_turns { None } else { Some(session.tokens) },
                    children: if has_turns { Some(turn_children) } else { None },
                    session_id: Some(short_id(&session.session_id)),
                    full_session_id: Some(session.session_id.clone()),
                    file_path: Some(session.path.to_string_lossy().to_string()),
                    start_time: start,
                    model: Some(
                        get_primary_model(
                            analysis.opus_tokens,
                            analysis.sonnet_tokens,
                            analysis.haiku_tokens,
                            analysis.fable_tokens,
                        )
                        .to_string(),
                    ),
                    input_tokens: Some(analysis.input_tokens),
                    output_tokens: Some(analysis.output_tokens),
                    ratio: Some(if analysis.output_tokens > 0 {
                        analysis.input_tokens as f64 / analysis.output_tokens as f64
                    } else {
                        0.0
                    }),
                    date: Some(session.date.clone()),
                    project: Some(project_name.clone()),
                    repeated_reads: Some(analysis.repeated_reads),
                    model_efficiency: Some(analysis.model_efficiency),
                    tools: if tools.is_empty() { None } else { Some(tools) },
                    ..Default::default()
                });
            }

            date_children.push(TreemapNode {
                name: date.clone(),
                children: Some(session_children),
                date: Some(date),
                ..Default::default()
            });
        }

        project_children.push(TreemapNode {
            name: project_name.clone(),
            children: Some(date_children),
            project: Some(project_name),
            ..Default::default()
        });
    }

    Ok(TreemapNode {
        name: "All Sessions".to_string(),
        children: Some(project_children),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tt_claude_code::{Content, Message, Usage};

    fn tool_use_block(name: &str, input: Value) -> Value {
        json!({ "type": "tool_use", "id": "t", "name": name, "input": input })
    }

    fn assistant_entry(
        model: &str,
        input: i64,
        output: i64,
        content: Vec<Value>,
    ) -> TranscriptEntry {
        TranscriptEntry {
            entry_type: "assistant".to_string(),
            message: Some(Message {
                role: Some("assistant".to_string()),
                model: Some(model.to_string()),
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
    fn turn_nodes_skip_zero_token_turns() {
        let entries = [assistant_entry("claude-opus-4", 0, 0, vec![])];
        assert!(build_turn_nodes("abc12345", &entries, None).is_empty());
    }

    #[test]
    fn turn_nodes_single_tool_names_the_turn() {
        let entries = [assistant_entry(
            "claude-opus-4",
            200,
            100,
            vec![tool_use_block("Read", json!({ "file_path": "/a.ts" }))],
        )];
        let nodes = build_turn_nodes("abcdef1234", &entries, None);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "Read: a.ts");
        assert_eq!(nodes[0].tool_name.as_deref(), Some("Read"));
        // Value is deferred to the tool child.
        assert_eq!(nodes[0].value, None);
        assert_eq!(nodes[0].session_id.as_deref(), Some("abcdef12"));
        let children = nodes[0].children.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].value, Some(300));
    }

    #[test]
    fn turn_nodes_user_turn_uses_total() {
        let entries = [user_entry(100, 50)];
        let nodes = build_turn_nodes("abc12345", &entries, None);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "Turn 1: User");
        assert_eq!(nodes[0].value, Some(150));
        assert!(nodes[0].children.is_none());
    }

    #[test]
    fn session_treemap_wraps_turns() {
        let entries = [user_entry(100, 50)];
        let node = build_session_treemap("abcdef1234", &entries);
        assert_eq!(node.name, "Session abcdef12");
        assert_eq!(node.children.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn all_sessions_treemap_groups_by_project_and_date() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s1.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Do the thing\"}}\n{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-opus-4\",\"usage\":{\"input_tokens\":100,\"output_tokens\":50}}}\n",
        )
        .unwrap();

        let sessions = [SessionResult {
            session_id: "s1session".to_string(),
            path,
            date: "2025-06-15".to_string(),
            tokens: 150,
            project: "-home-code-demo".to_string(),
            mtime: 1,
            title: None,
        }];

        let root = build_all_sessions_treemap(&sessions).unwrap();
        assert_eq!(root.name, "All Sessions");
        let projects = root.children.as_ref().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "demo");
        let dates = projects[0].children.as_ref().unwrap();
        assert_eq!(dates[0].name, "2025-06-15");
        let sess = dates[0].children.as_ref().unwrap();
        assert_eq!(sess[0].name, "Do the thing");
        assert_eq!(sess[0].model.as_deref(), Some("Opus"));
    }
}
