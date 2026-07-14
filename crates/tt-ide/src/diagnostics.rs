//! Compiler diagnostics for the IDE bridge, without an LSP: parse
//! `cargo check --message-format=json` and `tsc --noEmit --pretty false`
//! output into the wire shape Claude Code's `getDiagnostics` tool expects
//! (an array of `{uri, diagnostics: [{message, severity, range, source,
//! code}]}` with 0-based positions), plus the `diagnostics_changed`
//! staleness notification. Pure parsing — the app shell owns the
//! subprocesses and schedules the runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{Position, SelectionRange};

/// One diagnostic in Claude Code's wire vocabulary. `severity` is the
/// stringified VS Code name ("Error" | "Warning" | "Information" | "Hint").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub message: String,
    pub severity: String,
    pub range: SelectionRange,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Absolute-path → diagnostics map; BTreeMap for stable output order.
pub type DiagnosticsByFile = BTreeMap<PathBuf, Vec<Diagnostic>>;

fn range(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> SelectionRange {
    SelectionRange {
        start: Position { line: start_line, character: start_col },
        end: Position { line: end_line, character: end_col },
        is_empty: start_line == end_line && start_col == end_col,
    }
}

fn severity_from_level(level: &str) -> Option<&'static str> {
    match level {
        "error" | "error: internal compiler error" => Some("Error"),
        "warning" => Some("Warning"),
        // notes/helps ride as children of their parent message; standalone
        // ones are noise at the tool level.
        _ => None,
    }
}

/// Parse `cargo check --message-format=json` stdout (one JSON object per
/// line). Only primary spans of error/warning compiler messages survive;
/// paths resolve against `workspace_root` and anything outside it (registry
/// deps, rustc internals) is dropped.
pub fn parse_cargo_json(output: &str, workspace_root: &Path) -> DiagnosticsByFile {
    let mut by_file = DiagnosticsByFile::new();
    for line in output.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(severity) =
            message.get("level").and_then(Value::as_str).and_then(severity_from_level)
        else {
            continue;
        };
        let text = message.get("message").and_then(Value::as_str).unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let code = message.pointer("/code/code").and_then(Value::as_str).map(str::to_string);
        let spans = message.get("spans").and_then(Value::as_array).cloned().unwrap_or_default();
        let Some(span) =
            spans.iter().find(|s| s.get("is_primary").and_then(Value::as_bool).unwrap_or(false))
        else {
            continue;
        };
        let Some(file_name) = span.get("file_name").and_then(Value::as_str) else {
            continue;
        };
        let path = workspace_root.join(file_name);
        if !path.starts_with(workspace_root) {
            continue;
        }
        // Cargo spans are 1-based lines and columns; the wire is 0-based.
        let l1 = span.get("line_start").and_then(Value::as_u64).unwrap_or(1).max(1) as u32;
        let c1 = span.get("column_start").and_then(Value::as_u64).unwrap_or(1).max(1) as u32;
        let l2 =
            span.get("line_end").and_then(Value::as_u64).unwrap_or(u64::from(l1)).max(1) as u32;
        let c2 =
            span.get("column_end").and_then(Value::as_u64).unwrap_or(u64::from(c1)).max(1) as u32;
        by_file.entry(path).or_default().push(Diagnostic {
            message: text.to_string(),
            severity: severity.to_string(),
            range: range(l1 - 1, c1 - 1, l2 - 1, c2 - 1),
            source: "cargo".to_string(),
            code,
        });
    }
    by_file
}

/// Parse `tsc --noEmit --pretty false` output lines of the form
/// `path/to/file.ts(12,5): error TS2304: Cannot find name 'x'.`
/// Relative paths resolve against `project_root` (tsc's cwd). The end of the
/// range is unknown to tsc's line format — one character wide.
pub fn parse_tsc(output: &str, project_root: &Path) -> DiagnosticsByFile {
    let mut by_file = DiagnosticsByFile::new();
    for line in output.lines() {
        let Some((location, rest)) = line.split_once("): ") else {
            continue;
        };
        let Some((file, coords)) = location.rsplit_once('(') else {
            continue;
        };
        let Some((line_s, col_s)) = coords.split_once(',') else {
            continue;
        };
        let (Ok(line_no), Ok(col_no)) = (line_s.trim().parse::<u32>(), col_s.trim().parse::<u32>())
        else {
            continue;
        };
        let (severity, rest) = if let Some(r) = rest.strip_prefix("error ") {
            ("Error", r)
        } else if let Some(r) = rest.strip_prefix("warning ") {
            ("Warning", r)
        } else {
            continue;
        };
        let (code, message) = match rest.split_once(": ") {
            Some((code, msg)) if code.starts_with("TS") => (Some(code.to_string()), msg),
            _ => (None, rest),
        };
        let path = {
            let p = Path::new(file.trim());
            if p.is_absolute() { p.to_path_buf() } else { project_root.join(p) }
        };
        let (l, c) = (line_no.max(1) - 1, col_no.max(1) - 1);
        by_file.entry(path).or_default().push(Diagnostic {
            message: message.trim().to_string(),
            severity: severity.to_string(),
            range: range(l, c, l, c + 1),
            source: "tsc".to_string(),
            code,
        });
    }
    by_file
}

/// Merge maps from several runners (cargo + tsc projects) into one.
pub fn merge(maps: Vec<DiagnosticsByFile>) -> DiagnosticsByFile {
    let mut merged = DiagnosticsByFile::new();
    for map in maps {
        for (path, mut diags) in map {
            merged.entry(path).or_default().append(&mut diags);
        }
    }
    merged
}

/// The `getDiagnostics` wire payload: `[{uri, diagnostics: [...]}]`.
pub fn to_wire(by_file: &DiagnosticsByFile) -> Value {
    let entries: Vec<Value> = by_file
        .iter()
        .map(|(path, diags)| {
            json!({
                "uri": format!("file://{}", path.to_string_lossy()),
                "diagnostics": diags,
            })
        })
        .collect();
    Value::Array(entries)
}

/// The `diagnostics_changed` notification frame (IDE → CLI): signals
/// staleness; the CLI re-pulls via `getDiagnostics`.
pub fn diagnostics_changed_frame(uris: &[String]) -> String {
    json!({ "jsonrpc": "2.0", "method": "diagnostics_changed", "params": { "uris": uris } })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_json_keeps_primary_spans_of_errors_and_warnings() {
        let root = Path::new("/repo");
        let lines = [
            // A warning with a primary span, 1-based coordinates.
            r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `x`","code":{"code":"unused_variables"},"spans":[{"is_primary":true,"file_name":"src/main.rs","line_start":3,"line_end":3,"column_start":9,"column_end":10}]}}"#,
            // An error whose primary span sits in a second file.
            r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"is_primary":false,"file_name":"src/lib.rs","line_start":1,"line_end":1,"column_start":1,"column_end":1},{"is_primary":true,"file_name":"src/lib.rs","line_start":7,"line_end":7,"column_start":5,"column_end":9}]}}"#,
            // Noise that must be ignored: artifacts, notes, spanless messages.
            r#"{"reason":"compiler-artifact","target":{"name":"x"}}"#,
            r#"{"reason":"compiler-message","message":{"level":"note","message":"required by","spans":[]}}"#,
            r#"{"reason":"compiler-message","message":{"level":"warning","message":"crate-level lint","spans":[]}}"#,
            "not json at all",
        ]
        .join("\n");

        let map = parse_cargo_json(&lines, root);
        assert_eq!(map.len(), 2);
        let main = &map[&PathBuf::from("/repo/src/main.rs")];
        assert_eq!(main[0].severity, "Warning");
        assert_eq!(main[0].code.as_deref(), Some("unused_variables"));
        assert_eq!(main[0].range.start, Position { line: 2, character: 8 });
        let lib = &map[&PathBuf::from("/repo/src/lib.rs")];
        assert_eq!(lib[0].severity, "Error");
        assert_eq!(lib[0].code.as_deref(), Some("E0308"));
        assert_eq!(lib[0].range.start, Position { line: 6, character: 4 });
        assert_eq!(lib[0].range.end, Position { line: 6, character: 8 });
    }

    #[test]
    fn tsc_lines_parse_paths_codes_and_positions() {
        let root = Path::new("/repo/apps/client");
        let out = "src/lib/ide.ts(12,5): error TS2304: Cannot find name 'foo'.\n\
                   src/lib/ide.ts(30,1): warning TS6133: 'bar' is declared but never used.\n\
                   Some unrelated summary line\n";
        let map = parse_tsc(out, root);
        let diags = &map[&PathBuf::from("/repo/apps/client/src/lib/ide.ts")];
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, "Error");
        assert_eq!(diags[0].code.as_deref(), Some("TS2304"));
        assert_eq!(diags[0].message, "Cannot find name 'foo'.");
        assert_eq!(diags[0].range.start, Position { line: 11, character: 4 });
        assert_eq!(diags[0].range.end, Position { line: 11, character: 5 });
        assert_eq!(diags[1].severity, "Warning");
    }

    #[test]
    fn wire_shape_matches_the_getdiagnostics_contract() {
        let mut map = DiagnosticsByFile::new();
        map.insert(
            PathBuf::from("/repo/src/a.rs"),
            vec![Diagnostic {
                message: "boom".into(),
                severity: "Error".into(),
                range: range(1, 0, 1, 4),
                source: "cargo".into(),
                code: Some("E0001".into()),
            }],
        );
        let wire = to_wire(&map);
        assert_eq!(wire[0]["uri"], "file:///repo/src/a.rs");
        assert_eq!(wire[0]["diagnostics"][0]["severity"], "Error");
        assert_eq!(wire[0]["diagnostics"][0]["range"]["start"]["line"], 1);
        assert_eq!(wire[0]["diagnostics"][0]["code"], "E0001");
    }

    #[test]
    fn merge_concatenates_per_file() {
        let mut a = DiagnosticsByFile::new();
        a.insert(PathBuf::from("/r/x.rs"), vec![]);
        let mut b = DiagnosticsByFile::new();
        b.insert(PathBuf::from("/r/x.rs"), vec![]);
        b.insert(PathBuf::from("/r/y.ts"), vec![]);
        assert_eq!(merge(vec![a, b]).len(), 2);
    }

    #[test]
    fn diagnostics_changed_frame_is_wire_exact() {
        let frame: Value =
            serde_json::from_str(&diagnostics_changed_frame(&["file:///r/a.rs".into()])).unwrap();
        assert_eq!(frame["method"], "diagnostics_changed");
        assert_eq!(frame["params"]["uris"][0], "file:///r/a.rs");
        assert!(frame.get("id").is_none());
    }
}
