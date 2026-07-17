//! Tool-call extraction from message content blocks.

use serde_json::Value;

use crate::types::ToolData;
use tt_claude_code::Content;

/// Replace runs of control characters (ASCII 0–31) with a single space and
/// trim.
pub fn sanitize_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for ch in s.chars() {
        if (ch as u32) <= 0x1F {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out.trim().to_string()
}

/// Truncate a string, extracting just the filename for paths. Returns `None`
/// for `None`/empty input.
pub fn truncate_detail(s: Option<&str>, max_len: usize) -> Option<String> {
    let s = s.filter(|v| !v.is_empty())?;
    let sanitized = sanitize_string(s);
    // For file paths, show just the last segment.
    let text: String = if sanitized.contains('/') {
        sanitized.rsplit('/').next().unwrap_or(&sanitized).to_string()
    } else {
        sanitized
    };
    if text.chars().count() > max_len {
        let head: String = text.chars().take(max_len - 3).collect();
        Some(format!("{head}..."))
    } else {
        Some(text)
    }
}

fn input_str<'a>(input: Option<&'a serde_json::Map<String, Value>>, key: &str) -> Option<&'a str> {
    input?.get(key)?.as_str()
}

/// Extract a meaningful detail string from tool input.
pub fn extract_tool_detail(
    tool_name: &str,
    input: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    input?;
    match tool_name {
        "Read" | "Write" | "Edit" => truncate_detail(input_str(input, "file_path"), 30),
        "Bash" => truncate_detail(input_str(input, "command"), 50),
        "Glob" | "Grep" => truncate_detail(input_str(input, "pattern"), 50),
        "Task" => truncate_detail(input_str(input, "description"), 50),
        "WebFetch" => truncate_detail(input_str(input, "url"), 40),
        _ => None,
    }
}

/// Extract individual tool calls from message content blocks, distributing the
/// turn's tokens proportionally across each call.
pub fn extract_tool_data(
    content: Option<&Content>,
    turn_input_tokens: i64,
    turn_output_tokens: i64,
) -> Vec<ToolData> {
    let blocks = match content.and_then(Content::blocks) {
        Some(b) => b,
        None => return Vec::new(),
    };

    let mut tool_blocks: Vec<(String, Option<String>)> = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("tool_use")
            && let Some(name) = block.get("name").and_then(Value::as_str)
            && !name.is_empty()
        {
            let detail = extract_tool_detail(name, block.get("input").and_then(Value::as_object));
            tool_blocks.push((name.to_string(), detail));
        }
    }

    if tool_blocks.is_empty() {
        return Vec::new();
    }

    let n = tool_blocks.len() as f64;
    let input_per = (turn_input_tokens as f64 / n).round() as i64;
    let output_per = (turn_output_tokens as f64 / n).round() as i64;

    tool_blocks
        .into_iter()
        .map(|(name, detail)| ToolData {
            name,
            detail,
            input_tokens: input_per,
            output_tokens: output_per,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_block(text: &str) -> Value {
        json!({ "type": "text", "text": text })
    }
    fn tool_use_block(name: &str, input: Value) -> Value {
        json!({ "type": "tool_use", "id": "tool-stub", "name": name, "input": input })
    }

    // ── sanitizeString ──

    #[test]
    fn sanitize_replaces_control_chars() {
        assert_eq!(sanitize_string("hello\nworld\ttab"), "hello world tab");
    }
    #[test]
    fn sanitize_trims() {
        assert_eq!(sanitize_string("  hello  "), "hello");
    }
    #[test]
    fn sanitize_collapses_control_runs() {
        assert_eq!(sanitize_string("a\n\n\nb"), "a b");
    }
    #[test]
    fn sanitize_empty() {
        assert_eq!(sanitize_string(""), "");
    }

    // ── truncateDetail ──

    #[test]
    fn truncate_none_input() {
        assert_eq!(truncate_detail(None, 30), None);
    }
    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate_detail(Some("hello"), 30).as_deref(), Some("hello"));
    }
    #[test]
    fn truncate_long_with_ellipsis() {
        let long = "A".repeat(40);
        let result = truncate_detail(Some(&long), 30).unwrap();
        assert_eq!(result.chars().count(), 30);
        assert!(result.ends_with("..."));
    }
    #[test]
    fn truncate_extracts_filename() {
        assert_eq!(
            truncate_detail(Some("/home/user/project/file.ts"), 30).as_deref(),
            Some("file.ts")
        );
    }
    #[test]
    fn truncate_long_filename() {
        let long_file = format!("/path/{}.ts", "A".repeat(40));
        let result = truncate_detail(Some(&long_file), 30).unwrap();
        assert_eq!(result.chars().count(), 30);
        assert!(result.ends_with("..."));
    }

    // ── extractToolDetail ──

    #[test]
    fn detail_none_when_no_input() {
        assert_eq!(extract_tool_detail("Read", None), None);
    }
    #[test]
    fn detail_read_file_path() {
        let input = json!({ "file_path": "/src/index.ts" });
        assert_eq!(extract_tool_detail("Read", input.as_object()).as_deref(), Some("index.ts"));
    }
    #[test]
    fn detail_write_file_path() {
        let input = json!({ "file_path": "/src/utils.ts" });
        assert_eq!(extract_tool_detail("Write", input.as_object()).as_deref(), Some("utils.ts"));
    }
    #[test]
    fn detail_edit_file_path() {
        let input = json!({ "file_path": "/src/edit.ts" });
        assert_eq!(extract_tool_detail("Edit", input.as_object()).as_deref(), Some("edit.ts"));
    }
    #[test]
    fn detail_bash_command() {
        let input = json!({ "command": "pnpm test" });
        assert_eq!(extract_tool_detail("Bash", input.as_object()).as_deref(), Some("pnpm test"));
    }
    #[test]
    fn detail_glob_pattern() {
        let input = json!({ "pattern": "**/*.ts" });
        assert_eq!(extract_tool_detail("Glob", input.as_object()).as_deref(), Some("*.ts"));
    }
    #[test]
    fn detail_grep_pattern() {
        let input = json!({ "pattern": "TODO" });
        assert_eq!(extract_tool_detail("Grep", input.as_object()).as_deref(), Some("TODO"));
    }
    #[test]
    fn detail_unknown_tool() {
        let input = json!({ "foo": "bar" });
        assert_eq!(extract_tool_detail("CustomTool", input.as_object()), None);
    }

    // ── extractToolData ──

    #[test]
    fn data_empty_for_none() {
        assert_eq!(extract_tool_data(None, 100, 50), Vec::new());
    }
    #[test]
    fn data_empty_for_string_content() {
        let content = Content::Text("text".to_string());
        assert_eq!(extract_tool_data(Some(&content), 100, 50), Vec::new());
    }
    #[test]
    fn data_empty_when_no_tool_use() {
        let content = Content::Blocks(vec![text_block("hello")]);
        assert_eq!(extract_tool_data(Some(&content), 100, 50), Vec::new());
    }
    #[test]
    fn data_extracts_and_distributes_tokens() {
        let content = Content::Blocks(vec![
            tool_use_block("Read", json!({ "file_path": "/a.ts" })),
            tool_use_block("Bash", json!({ "command": "ls" })),
        ]);
        let result = extract_tool_data(Some(&content), 200, 100);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "Read");
        assert_eq!(result[0].input_tokens, 100);
        assert_eq!(result[0].output_tokens, 50);
        assert_eq!(result[1].name, "Bash");
        assert_eq!(result[1].input_tokens, 100);
    }
}
