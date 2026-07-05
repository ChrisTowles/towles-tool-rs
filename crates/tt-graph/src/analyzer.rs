//! Per-session token analysis and project-name extraction. Ports
//! `src/commands/graph/analyzer.ts`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::tools::extract_tool_data;
use crate::types::{Content, JournalEntry, ToolData};

/// Token breakdown for a session, produced by [`analyze_session`].
#[derive(Debug, Clone, PartialEq)]
pub struct SessionAnalysis {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub opus_tokens: i64,
    pub sonnet_tokens: i64,
    pub haiku_tokens: i64,
    pub cache_hit_rate: f64,
    /// Total cache-read tokens across the session.
    pub cache_read_tokens: i64,
    /// Total cache-creation (write) tokens across the session.
    pub cache_creation_tokens: i64,
    pub repeated_reads: i64,
    /// Opus tokens / total tokens.
    pub model_efficiency: f64,
}

/// Analyze session entries to get a token breakdown by model, plus waste
/// metrics (repeated reads, model efficiency). Ports `analyzeSession`.
pub fn analyze_session(entries: &[JournalEntry]) -> SessionAnalysis {
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut opus_tokens = 0;
    let mut sonnet_tokens = 0;
    let mut haiku_tokens = 0;
    let mut cache_read = 0;
    let mut cache_creation = 0;
    let mut total_input = 0;
    let mut file_read_counts: HashMap<String, i64> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();

    for entry in entries {
        let Some(message) = &entry.message else {
            continue;
        };

        // Skip streaming re-logs of an already-counted assistant message so
        // tokens (and its tool_use blocks) aren't double-counted.
        if let Some(key) = entry.dedup_key()
            && !seen.insert(key)
        {
            continue;
        }

        // Count file reads for the repeatedReads metric.
        if let Some(Content::Blocks(blocks)) = &message.content {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("name").and_then(Value::as_str) == Some("Read")
                    && block.get("input").is_some_and(|v| !v.is_null())
                    && let Some(fp) =
                        block.get("input").and_then(|v| v.get("file_path")).and_then(Value::as_str)
                {
                    *file_read_counts.entry(fp.to_string()).or_insert(0) += 1;
                }
            }
        }

        let Some(usage) = &message.usage else {
            continue;
        };
        let model = message.model.as_deref().unwrap_or("");
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        let tokens = input + output;

        input_tokens += input;
        output_tokens += output;
        cache_read += usage.cache_read_input_tokens.unwrap_or(0);
        cache_creation += usage.cache_creation_input_tokens.unwrap_or(0);
        total_input += input;

        if model.contains("opus") {
            opus_tokens += tokens;
        } else if model.contains("sonnet") {
            sonnet_tokens += tokens;
        } else if model.contains("haiku") {
            haiku_tokens += tokens;
        }
    }

    // Count files read more than once.
    let mut repeated_reads = 0;
    for count in file_read_counts.values() {
        if *count > 1 {
            repeated_reads += count - 1;
        }
    }

    let total_tokens = opus_tokens + sonnet_tokens + haiku_tokens;

    SessionAnalysis {
        input_tokens,
        output_tokens,
        opus_tokens,
        sonnet_tokens,
        haiku_tokens,
        cache_hit_rate: if total_input > 0 { cache_read as f64 / total_input as f64 } else { 0.0 },
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        repeated_reads,
        model_efficiency: if total_tokens > 0 {
            opus_tokens as f64 / total_tokens as f64
        } else {
            0.0
        },
    }
}

/// Aggregate tool usage across all entries in a session, sorted by token usage
/// descending. Ports `aggregateSessionTools`.
pub fn aggregate_session_tools(entries: &[JournalEntry]) -> Vec<ToolData> {
    // (name, count, input, output), keeping first-seen insertion order.
    let mut order: Vec<String> = Vec::new();
    let mut agg: HashMap<String, (i64, i64, i64)> = HashMap::new();

    for entry in entries {
        let Some(message) = &entry.message else {
            continue;
        };
        let Some(content @ Content::Blocks(_)) = &message.content else {
            continue;
        };
        let Some(usage) = &message.usage else {
            continue;
        };

        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        let turn_tools = extract_tool_data(Some(content), input, output);

        for tool in turn_tools {
            let entry = agg.entry(tool.name.clone()).or_insert_with(|| {
                order.push(tool.name.clone());
                (0, 0, 0)
            });
            entry.0 += 1;
            entry.1 += tool.input_tokens;
            entry.2 += tool.output_tokens;
        }
    }

    let mut tools: Vec<ToolData> = order
        .into_iter()
        .map(|name| {
            let (count, input, output) = agg[&name];
            ToolData {
                detail: Some(format!("{count}x")),
                name,
                input_tokens: input,
                output_tokens: output,
            }
        })
        .collect();
    tools.sort_by(|a, b| {
        (b.input_tokens + b.output_tokens).cmp(&(a.input_tokens + a.output_tokens))
    });
    tools
}

/// Get the primary model name (Opus / Sonnet / Haiku) from analysis token
/// totals. Ports `getPrimaryModel`.
pub fn get_primary_model(opus_tokens: i64, sonnet_tokens: i64, haiku_tokens: i64) -> &'static str {
    if opus_tokens >= sonnet_tokens && opus_tokens >= haiku_tokens {
        "Opus"
    } else if sonnet_tokens >= haiku_tokens {
        "Sonnet"
    } else {
        "Haiku"
    }
}

/// Get a short model name from a full model string. Ports `getModelName`.
pub fn get_model_name(model: Option<&str>) -> String {
    let model = match model {
        Some(m) if !m.is_empty() => m,
        _ => return "unknown".to_string(),
    };
    if model.contains("opus") {
        "Opus".to_string()
    } else if model.contains("sonnet") {
        "Sonnet".to_string()
    } else if model.contains("haiku") {
        "Haiku".to_string()
    } else {
        model.split('-').next().filter(|s| !s.is_empty()).unwrap_or("unknown").to_string()
    }
}

/// Extract a project name from an encoded directory name. Ports
/// `extractProjectName`.
pub fn extract_project_name(encoded_project: &str) -> String {
    let parts: Vec<&str> = encoded_project.split('-').filter(|p| !p.is_empty()).collect();
    let markers = ["code", "projects", "src", "p", "repos", "git", "workspace"];

    let mut last_marker_idx: isize = -1;
    for (i, part) in parts.iter().enumerate() {
        if markers.contains(&part.to_lowercase().as_str()) {
            last_marker_idx = i as isize;
        }
    }

    let project_parts: Vec<&str> = if last_marker_idx >= 0 {
        parts[(last_marker_idx as usize + 1)..].to_vec()
    } else if parts.len() >= 2 {
        parts[parts.len() - 2..].to_vec()
    } else {
        parts.clone()
    };

    if project_parts.is_empty() {
        return parts
            .last()
            .map(|s| s.to_string())
            .unwrap_or_else(|| encoded_project.chars().take(20).collect());
    }
    project_parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Usage};
    use serde_json::json;

    fn text_block(text: &str) -> Value {
        json!({ "type": "text", "text": text })
    }
    fn tool_use_block(name: &str, input: Value) -> Value {
        json!({ "type": "tool_use", "id": "tool-stub", "name": name, "input": input })
    }

    fn assistant_entry(
        model: &str,
        input: i64,
        output: i64,
        content: Option<Vec<Value>>,
    ) -> JournalEntry {
        JournalEntry {
            entry_type: "assistant".to_string(),
            message: Some(Message {
                role: Some("assistant".to_string()),
                model: Some(model.to_string()),
                usage: Some(Usage {
                    input_tokens: Some(input),
                    output_tokens: Some(output),
                    ..Default::default()
                }),
                content: Some(Content::Blocks(
                    content.unwrap_or_else(|| vec![text_block("response")]),
                )),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // ── analyzeSession ──

    #[test]
    fn analyze_zeros_for_empty() {
        let r = analyze_session(&[]);
        assert_eq!(r.input_tokens, 0);
        assert_eq!(r.output_tokens, 0);
        assert_eq!(r.opus_tokens, 0);
        assert_eq!(r.sonnet_tokens, 0);
        assert_eq!(r.haiku_tokens, 0);
        assert_eq!(r.cache_hit_rate, 0.0);
        assert_eq!(r.repeated_reads, 0);
        assert_eq!(r.model_efficiency, 0.0);
    }

    #[test]
    fn analyze_counts_tokens_by_model() {
        let entries = [
            assistant_entry("claude-opus-4", 100, 50, None),
            assistant_entry("claude-sonnet-4", 200, 100, None),
            assistant_entry("claude-haiku-3", 50, 25, None),
        ];
        let r = analyze_session(&entries);
        assert_eq!(r.input_tokens, 350);
        assert_eq!(r.output_tokens, 175);
        assert_eq!(r.opus_tokens, 150);
        assert_eq!(r.sonnet_tokens, 300);
        assert_eq!(r.haiku_tokens, 75);
    }

    #[test]
    fn analyze_cache_hit_rate() {
        let entries = [JournalEntry {
            entry_type: "assistant".to_string(),
            message: Some(Message {
                role: Some("assistant".to_string()),
                model: Some("claude-opus-4".to_string()),
                usage: Some(Usage {
                    input_tokens: Some(1000),
                    output_tokens: Some(0),
                    cache_read_input_tokens: Some(800),
                    cache_creation_input_tokens: Some(200),
                }),
                content: Some(Content::Blocks(vec![text_block("hi")])),
                ..Default::default()
            }),
            ..Default::default()
        }];
        let r = analyze_session(&entries);
        assert_eq!(r.cache_hit_rate, 0.8);
        assert_eq!(r.cache_read_tokens, 800);
        assert_eq!(r.cache_creation_tokens, 200);
    }

    #[test]
    fn analyze_repeated_reads() {
        let content = vec![
            tool_use_block("Read", json!({ "file_path": "/a.ts" })),
            tool_use_block("Read", json!({ "file_path": "/a.ts" })),
            tool_use_block("Read", json!({ "file_path": "/b.ts" })),
        ];
        let entries = [assistant_entry("claude-opus-4", 100, 50, Some(content))];
        assert_eq!(analyze_session(&entries).repeated_reads, 1);
    }

    #[test]
    fn analyze_model_efficiency() {
        let entries = [
            assistant_entry("claude-opus-4", 400, 100, None),
            assistant_entry("claude-sonnet-4", 400, 100, None),
        ];
        assert_eq!(analyze_session(&entries).model_efficiency, 0.5);
    }

    fn deduped_entry(id: &str, request_id: &str, input: i64, output: i64) -> JournalEntry {
        JournalEntry {
            entry_type: "assistant".to_string(),
            request_id: Some(request_id.to_string()),
            message: Some(Message {
                id: Some(id.to_string()),
                role: Some("assistant".to_string()),
                model: Some("claude-opus-4".to_string()),
                usage: Some(Usage {
                    input_tokens: Some(input),
                    output_tokens: Some(output),
                    ..Default::default()
                }),
                content: Some(Content::Blocks(vec![text_block("hi")])),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn analyze_dedups_repeated_message_request_pair() {
        // Same (message.id, requestId) re-logged 3x → counted once.
        let e = deduped_entry("msg_1", "req_1", 100, 50);
        let entries = [e.clone(), e.clone(), e];
        let r = analyze_session(&entries);
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.output_tokens, 50);
    }

    #[test]
    fn analyze_counts_distinct_and_id_less_entries() {
        let entries = [
            deduped_entry("msg_1", "req_1", 100, 0),
            deduped_entry("msg_2", "req_2", 50, 0),
            // No ids → each counted (assistant_entry sets neither id nor requestId).
            assistant_entry("claude-opus-4", 10, 0, None),
            assistant_entry("claude-opus-4", 10, 0, None),
        ];
        assert_eq!(analyze_session(&entries).input_tokens, 170);
    }

    #[test]
    fn analyze_skips_entries_without_usage() {
        let entries = [JournalEntry {
            entry_type: "assistant".to_string(),
            message: Some(Message {
                role: Some("assistant".to_string()),
                content: Some(Content::Text("text".to_string())),
                ..Default::default()
            }),
            ..Default::default()
        }];
        assert_eq!(analyze_session(&entries).input_tokens, 0);
    }

    #[test]
    fn analyze_model_efficiency_third() {
        let entries = [
            assistant_entry("claude-opus-4-20250514", 100, 100, None),
            assistant_entry("claude-sonnet-4-20250514", 200, 200, None),
        ];
        // Opus 200 / total 600 = 0.333…
        assert!((analyze_session(&entries).model_efficiency - 0.333).abs() < 0.01);
    }

    // ── aggregateSessionTools ──

    #[test]
    fn aggregate_empty() {
        assert_eq!(aggregate_session_tools(&[]), Vec::new());
    }

    #[test]
    fn aggregate_across_entries() {
        let entries = [
            assistant_entry(
                "claude-opus-4",
                100,
                50,
                Some(vec![tool_use_block("Read", json!({ "file_path": "/a.ts" }))]),
            ),
            assistant_entry(
                "claude-opus-4",
                200,
                100,
                Some(vec![
                    tool_use_block("Read", json!({ "file_path": "/b.ts" })),
                    tool_use_block("Bash", json!({ "command": "ls" })),
                ]),
            ),
        ];
        let result = aggregate_session_tools(&entries);
        let read = result.iter().find(|t| t.name == "Read").unwrap();
        assert_eq!(read.detail.as_deref(), Some("2x"));
    }

    #[test]
    fn aggregate_sorts_by_tokens_desc() {
        let entries = [
            assistant_entry(
                "claude-opus-4",
                100,
                50,
                Some(vec![tool_use_block("Bash", json!({ "command": "ls" }))]),
            ),
            assistant_entry(
                "claude-opus-4",
                1000,
                500,
                Some(vec![tool_use_block("Read", json!({ "file_path": "/a.ts" }))]),
            ),
        ];
        assert_eq!(aggregate_session_tools(&entries)[0].name, "Read");
    }

    // ── getPrimaryModel ──

    #[test]
    fn primary_model_variants() {
        assert_eq!(get_primary_model(100, 50, 10), "Opus");
        assert_eq!(get_primary_model(10, 500, 10), "Sonnet");
        assert_eq!(get_primary_model(0, 0, 100), "Haiku");
        assert_eq!(get_primary_model(100, 100, 0), "Opus");
        assert_eq!(get_primary_model(0, 0, 0), "Opus");
    }

    // ── getModelName ──

    #[test]
    fn model_name_variants() {
        assert_eq!(get_model_name(None), "unknown");
        assert_eq!(get_model_name(Some("claude-opus-4-20250514")), "Opus");
        assert_eq!(get_model_name(Some("claude-sonnet-4-20250514")), "Sonnet");
        assert_eq!(get_model_name(Some("claude-3-haiku")), "Haiku");
        assert_eq!(get_model_name(Some("gpt-4-turbo")), "gpt");
        assert_eq!(get_model_name(Some("")), "unknown");
    }

    // ── extractProjectName ──

    #[test]
    fn project_name_variants() {
        assert_eq!(extract_project_name("-home-ctowles-code-p-towles-tool"), "towles-tool");
        assert_eq!(extract_project_name("-home-user-projects-my-app"), "my-app");
        assert_eq!(extract_project_name("-home-user-src-cool-lib"), "cool-lib");
        assert_eq!(extract_project_name("-foo-bar-baz"), "bar-baz");
        assert_eq!(extract_project_name("-home-user-code-myproject"), "myproject");
        assert_eq!(extract_project_name("-home-code-old-src-new-project"), "new-project");
    }
}
