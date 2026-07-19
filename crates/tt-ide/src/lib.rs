//! Claude Code IDE-protocol core: the towles-tool app poses as an "IDE" that
//! Claude Code CLI sessions connect to, so highlights in the app's diff panes
//! reach the session as selection context (see `docs/CLAUDE-CODE-IDE.md`).
//!
//! The protocol is MCP (JSON-RPC 2.0) over a WebSocket the IDE hosts,
//! advertised by a `~/.claude/ide/<port>.lock` file. This crate is the
//! transport-free half: lockfile schema + lifecycle, the request dispatcher
//! ([`handle_message`]), and the notification frames the IDE pushes
//! ([`selection_changed_frame`], [`at_mentioned_frame`]). Sockets, tokens and
//! clocks live in the app shell, which passes state in per call.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod diagnostics;
pub mod lockfile;

pub use lockfile::Lockfile;

/// Protocol version echoed back when the client doesn't send one (matches
/// tt-mcp's default).
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// A 0-based line/character position, exactly as the wire expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// The selected range; `isEmpty` marks a cleared/cursor-only selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRange {
    pub start: Position,
    pub end: Position,
    pub is_empty: bool,
}

/// One selection snapshot: what `selection_changed` carries and what
/// `getCurrentSelection` / `getLatestSelection` answer with. Lines and
/// characters are 0-based on the wire — callers convert at the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub text: String,
    pub file_path: String,
    pub file_url: String,
    pub selection: SelectionRange,
}

impl Selection {
    /// A non-empty selection of `path` spanning 0-based lines
    /// `start_line..=end_line`, ending after `end_character` characters of the
    /// last line. `text` is the selected file content.
    pub fn range(
        path: &Path,
        start_line: u32,
        end_line: u32,
        end_character: u32,
        text: String,
    ) -> Selection {
        Selection {
            text,
            file_path: path.to_string_lossy().into_owned(),
            file_url: file_url(path),
            selection: SelectionRange {
                start: Position { line: start_line, character: 0 },
                end: Position { line: end_line, character: end_character },
                is_empty: false,
            },
        }
    }

    /// An empty selection on `path` — what a "highlight cleared" looks like.
    pub fn cleared(path: &Path) -> Selection {
        Selection {
            text: String::new(),
            file_path: path.to_string_lossy().into_owned(),
            file_url: file_url(path),
            selection: SelectionRange {
                start: Position { line: 0, character: 0 },
                end: Position { line: 0, character: 0 },
                is_empty: true,
            },
        }
    }
}

/// `file://` URL for an absolute path (no percent-encoding — Claude Code's
/// schema treats it as an opaque string and the paths we feed are our own).
fn file_url(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

/// The `selection_changed` notification frame (IDE → CLI). The CLI caches the
/// latest one and attaches it to the next user prompt as selection context.
pub fn selection_changed_frame(selection: &Selection) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "selection_changed",
        "params": selection,
    })
    .to_string()
}

/// The `at_mentioned` notification frame (IDE → CLI): the explicit
/// "send to Claude" gesture, which becomes an `@file#Lx-y` prompt reference.
/// `line_start`/`line_end` are 0-based and omitted together when absent.
pub fn at_mentioned_frame(file_path: &str, lines: Option<(u32, u32)>) -> String {
    let mut params = json!({ "filePath": file_path });
    if let Some((start, end)) = lines {
        params["lineStart"] = json!(start);
        params["lineEnd"] = json!(end);
    }
    json!({ "jsonrpc": "2.0", "method": "at_mentioned", "params": params }).to_string()
}

/// One open editor buffer: absolute path + whether it has unsaved edits
/// (feeds `getOpenEditors.isDirty` and `checkDocumentDirty`). The code
/// viewer holds at most one at a time; the diff pane's editable modified
/// sides can hold several concurrently — both funnel into
/// [`ServerContext::open_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFile {
    pub path: String,
    pub dirty: bool,
}

/// Per-message snapshot of the server's world, passed in by the transport:
/// the dispatcher itself is stateless so tests can drive it directly.
#[derive(Debug, Clone)]
pub struct ServerContext {
    /// Shown to the CLI as the server name (e.g. "Towles Tool").
    pub ide_name: String,
    /// The single workspace folder this server roots (the terminal's cwd).
    pub workspace_folder: PathBuf,
    /// The latest selection made in the app's diff pane, if any.
    pub selection: Option<Selection>,
    /// Files currently open (or, for the diff pane, dirty) in the app —
    /// preferred by `getOpenEditors` over the selection's file. Usually 0 or
    /// 1 entry (the Files tab's single viewer); the diff pane can add
    /// several more when multiple files there have unsaved edits at once.
    pub open_files: Vec<OpenFile>,
    /// Current compiler diagnostics for this folder, already in the
    /// `getDiagnostics` wire shape (`[{uri, diagnostics: [...]}]`, see
    /// [`diagnostics::to_wire`]). Empty array when no check has run.
    pub diagnostics: Value,
}

/// Handle one incoming JSON-RPC message from the CLI. Returns the response to
/// send back, or `None` for notifications (which get no response).
pub fn handle_message(message: &str, ctx: &ServerContext) -> Option<String> {
    let value: Value = match serde_json::from_str(message) {
        Ok(value) => value,
        Err(_) => return Some(error_response(Value::Null, -32700, "Parse error")),
    };
    if value.is_array() {
        return Some(error_response(Value::Null, -32600, "Invalid Request"));
    }

    // Requests carry an `id`; notifications (`notifications/initialized`, …)
    // do not and receive no response.
    let id = match value.get("id") {
        Some(id) if !id.is_null() => id.clone(),
        _ => return None,
    };
    let method = match value.get("method").and_then(Value::as_str) {
        Some(method) => method,
        None => return Some(error_response(id, -32600, "Invalid Request")),
    };

    let response = match method {
        "initialize" => success_response(id, initialize_result(&value, ctx)),
        "ping" => success_response(id, json!({})),
        "tools/list" => success_response(id, json!({ "tools": tool_definitions() })),
        "tools/call" => tools_call(id, &value, ctx),
        _ => error_response(id, -32601, "Method not found"),
    };
    Some(response)
}

fn initialize_result(request: &Value, ctx: &ServerContext) -> Value {
    let requested = request
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": requested,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": ctx.ide_name, "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tools_call(id: Value, request: &Value, ctx: &ServerContext) -> String {
    let params = request.get("params");
    let Some(name) = params.and_then(|p| p.get("name")).and_then(Value::as_str) else {
        return tool_error_response(id, "tools/call is missing the tool name");
    };
    let args = params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
    let result = match name {
        "getCurrentSelection" => current_selection(ctx, "No active editor found"),
        "getLatestSelection" => current_selection(ctx, "No selection available"),
        "getWorkspaceFolders" => workspace_folders(ctx),
        "getOpenEditors" => open_editors(ctx),
        "getDiagnostics" => diagnostics_for(ctx, &args),
        "checkDocumentDirty" => check_document_dirty(ctx, &args),
        // openFile has app-side effects; the shell intercepts it before this
        // dispatcher (see the app's ide.rs). Reaching here is a wiring bug.
        _ => return tool_error_response(id, &format!("Unknown tool: {name}")),
    };
    tool_result_response(id, &result)
}

/// `checkDocumentDirty`: dirty state of the matching open file, VS Code's
/// answer shapes ("Document not open" for anything else).
fn check_document_dirty(ctx: &ServerContext, args: &Value) -> Value {
    let requested = args.get("filePath").and_then(Value::as_str).unwrap_or_default();
    match ctx.open_files.iter().find(|f| f.path == requested) {
        Some(open) => json!({ "success": true, "filePath": open.path, "isDirty": open.dirty }),
        None => json!({ "success": false, "message": format!("Document not open: {requested}") }),
    }
}

/// `getCurrentSelection` / `getLatestSelection`: for this server the diff
/// pane's highlight IS the editor state, so both answer from the same cache;
/// only the no-selection message differs (mirroring VS Code's wording).
fn current_selection(ctx: &ServerContext, missing: &str) -> Value {
    match &ctx.selection {
        Some(selection) => {
            let mut value = serde_json::to_value(selection).unwrap_or_else(|_| json!({}));
            value["success"] = json!(true);
            value
        }
        None => json!({ "success": false, "message": missing }),
    }
}

fn workspace_folders(ctx: &ServerContext) -> Value {
    let path = ctx.workspace_folder.to_string_lossy();
    let name = ctx
        .workspace_folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone().into_owned());
    json!({
        "success": true,
        "folders": [{ "name": name, "uri": format!("file://{path}"), "path": path }],
    })
}

/// Usually the app shows one file at a time (code viewer, else the diff
/// pane's selected file), reported as the single active "tab" so
/// `@`-mention file pickers have something to anchor on; falls back to the
/// selection's file when nothing is explicitly open. The diff pane can also
/// contribute several concurrently-dirty files at once — with more than one
/// tab, none is marked active/group-active, since nothing here tracks which
/// of them has focus.
fn open_editors(ctx: &ServerContext) -> Value {
    let open: Vec<OpenFile> = if ctx.open_files.is_empty() {
        ctx.selection
            .as_ref()
            .map(|sel| vec![OpenFile { path: sel.file_path.clone(), dirty: false }])
            .unwrap_or_default()
    } else {
        ctx.open_files.clone()
    };
    let single = open.len() == 1;
    let tabs: Vec<Value> = open
        .iter()
        .map(|OpenFile { path: file_path, dirty }| {
            let name = Path::new(file_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file_path.clone());
            json!({
                "uri": format!("file://{file_path}"),
                "isActive": single,
                "isPinned": false,
                "isPreview": false,
                "isDirty": dirty,
                "label": name,
                "groupIndex": 0,
                "viewColumn": 1,
                "isGroupActive": single,
                "fileName": file_path,
            })
        })
        .collect();
    json!({ "tabs": tabs })
}

/// `getDiagnostics`: the folder's cached compiler diagnostics, optionally
/// narrowed to one file when the CLI passes `uri`.
fn diagnostics_for(ctx: &ServerContext, args: &Value) -> Value {
    let all = ctx.diagnostics.as_array().cloned().unwrap_or_default();
    match args.get("uri").and_then(Value::as_str) {
        Some(uri) => Value::Array(
            all.into_iter()
                .filter(|entry| entry.get("uri").and_then(Value::as_str) == Some(uri))
                .collect(),
        ),
        None => Value::Array(all),
    }
}

/// Tool definitions advertised in `tools/list`. Only what the app actually
/// implements — the CLI never calls tools that aren't listed, and degrades
/// gracefully without them (e.g. no `openDiff` → terminal diffs).
fn tool_definitions() -> Value {
    let empty_object = json!({ "type": "object", "properties": {}, "additionalProperties": false });
    json!([
        {
            "name": "getCurrentSelection",
            "description": "Get the current text selection in the active editor",
            "inputSchema": empty_object,
        },
        {
            "name": "getLatestSelection",
            "description": "Get the most recent text selection, even if the editor is no longer active",
            "inputSchema": empty_object,
        },
        {
            "name": "getWorkspaceFolders",
            "description": "Get all workspace folders currently open in the IDE",
            "inputSchema": empty_object,
        },
        {
            "name": "getOpenEditors",
            "description": "Get information about currently open editors",
            "inputSchema": empty_object,
        },
        {
            "name": "getDiagnostics",
            "description": "Get language diagnostics from the IDE",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "uri": { "type": "string", "description": "Optional file URI to get diagnostics for. If not provided, gets diagnostics for all files." }
                },
                "additionalProperties": false,
            },
        },
        {
            "name": "checkDocumentDirty",
            "description": "Check if a document has unsaved changes",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filePath": { "type": "string", "description": "Absolute path of the file to check" }
                },
                "required": ["filePath"],
                "additionalProperties": false,
            },
        },
        {
            "name": "openFile",
            "description": "Open a file in the IDE and optionally select a range of text",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filePath": { "type": "string", "description": "Path to the file to open" },
                    "preview": { "type": "boolean" },
                    "startText": { "type": "string", "description": "Text pattern where the selection starts" },
                    "endText": { "type": "string", "description": "Text pattern where the selection ends" },
                    "selectToEndOfLine": { "type": "boolean" },
                    "makeFrontmost": { "type": "boolean" }
                },
                "required": ["filePath"],
                "additionalProperties": false,
            },
        },
        {
            "name": "openDiff",
            "description": "Open a diff view comparing a file with proposed new contents; blocks until the user accepts or rejects",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old_file_path": { "type": "string", "description": "Path to the current file" },
                    "new_file_path": { "type": "string", "description": "Path the new contents would be saved to" },
                    "new_file_contents": { "type": "string", "description": "Proposed contents" },
                    "tab_name": { "type": "string", "description": "Label for the diff tab" }
                },
                "required": ["old_file_path", "new_file_path", "new_file_contents", "tab_name"],
                "additionalProperties": false,
            },
        },
        {
            "name": "close_tab",
            "description": "Close a diff tab by name (rejects a pending review)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab_name": { "type": "string" }
                },
                "required": ["tab_name"],
                "additionalProperties": false,
            },
        },
        {
            "name": "closeAllDiffTabs",
            "description": "Close all open diff tabs (rejects pending reviews)",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        },
    ])
}

// ---------------------------------------------------------------------------
// JSON-RPC response builders (same shapes as tt-mcp's).

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// MCP tool result: the payload rides as a JSON string inside a text content
/// block (how the VS Code extension answers every tool). Public so the app
/// shell can answer the tools it intercepts (openFile) in the same shape.
pub fn tool_result_response(id: Value, result: &Value) -> String {
    let text = result.to_string();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": text }] },
    })
    .to_string()
}

fn tool_error_response(id: Value, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": message }], "isError": true },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(selection: Option<Selection>) -> ServerContext {
        ServerContext {
            ide_name: "Towles Tool".to_string(),
            workspace_folder: PathBuf::from("/repo/slot-a"),
            selection,
            open_files: Vec::new(),
            diagnostics: json!([]),
        }
    }

    fn call(ctx: &ServerContext, tool: &str) -> Value {
        let request = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": tool, "arguments": {} },
        })
        .to_string();
        let response: Value =
            serde_json::from_str(&handle_message(&request, ctx).expect("response")).unwrap();
        let text = response["result"]["content"][0]["text"].as_str().expect("text content");
        serde_json::from_str(text).expect("tool payload is JSON")
    }

    #[test]
    fn initialize_echoes_protocol_version_and_names_the_ide() {
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" },
        })
        .to_string();
        let response: Value =
            serde_json::from_str(&handle_message(&request, &ctx_with(None)).unwrap()).unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "Towles Tool");
    }

    #[test]
    fn notifications_get_no_response() {
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
        assert_eq!(handle_message(&note, &ctx_with(None)), None);
    }

    #[test]
    fn unknown_method_is_a_json_rpc_error() {
        let request = json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/list" }).to_string();
        let response: Value =
            serde_json::from_str(&handle_message(&request, &ctx_with(None)).unwrap()).unwrap();
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn tools_list_advertises_only_the_implemented_set() {
        let request = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }).to_string();
        let response: Value =
            serde_json::from_str(&handle_message(&request, &ctx_with(None)).unwrap()).unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "getCurrentSelection",
                "getLatestSelection",
                "getWorkspaceFolders",
                "getOpenEditors",
                "getDiagnostics",
                "checkDocumentDirty",
                "openFile",
                "openDiff",
                "close_tab",
                "closeAllDiffTabs"
            ]
        );
    }

    #[test]
    fn selection_tools_answer_from_the_cached_selection() {
        let selection = Selection::range(
            Path::new("/repo/slot-a/src/main.rs"),
            4,
            6,
            12,
            "fn main() {}".to_string(),
        );
        let ctx = ctx_with(Some(selection));

        let current = call(&ctx, "getCurrentSelection");
        assert_eq!(current["success"], true);
        assert_eq!(current["filePath"], "/repo/slot-a/src/main.rs");
        assert_eq!(current["selection"]["start"]["line"], 4);
        assert_eq!(current["selection"]["end"]["line"], 6);
        assert_eq!(current["selection"]["end"]["character"], 12);
        assert_eq!(current["selection"]["isEmpty"], false);

        let latest = call(&ctx, "getLatestSelection");
        assert_eq!(latest["success"], true);

        let editors = call(&ctx, "getOpenEditors");
        assert_eq!(editors["tabs"][0]["label"], "main.rs");
    }

    #[test]
    fn selection_tools_report_missing_selection() {
        let ctx = ctx_with(None);
        let current = call(&ctx, "getCurrentSelection");
        assert_eq!(current["success"], false);
        assert_eq!(current["message"], "No active editor found");
        let latest = call(&ctx, "getLatestSelection");
        assert_eq!(latest["message"], "No selection available");
        let editors = call(&ctx, "getOpenEditors");
        assert_eq!(editors["tabs"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn workspace_folders_reports_the_single_root() {
        let folders = call(&ctx_with(None), "getWorkspaceFolders");
        assert_eq!(folders["folders"][0]["name"], "slot-a");
        assert_eq!(folders["folders"][0]["path"], "/repo/slot-a");
        assert_eq!(folders["folders"][0]["uri"], "file:///repo/slot-a");
    }

    #[test]
    fn diagnostics_answer_the_empty_set() {
        let diags = call(&ctx_with(None), "getDiagnostics");
        assert_eq!(diags, json!([]));
    }

    #[test]
    fn open_editors_prefers_the_viewer_file_over_the_selection() {
        let selection =
            Selection::range(Path::new("/repo/slot-a/src/sel.rs"), 0, 0, 1, "x".to_string());
        let mut ctx = ctx_with(Some(selection));
        ctx.open_files =
            vec![OpenFile { path: "/repo/slot-a/src/open.rs".to_string(), dirty: true }];
        let editors = call(&ctx, "getOpenEditors");
        assert_eq!(editors["tabs"][0]["fileName"], "/repo/slot-a/src/open.rs");
        assert_eq!(editors["tabs"][0]["label"], "open.rs");
        assert_eq!(editors["tabs"][0]["uri"], "file:///repo/slot-a/src/open.rs");
        assert_eq!(editors["tabs"][0]["isDirty"], true);
    }

    #[test]
    fn check_document_dirty_answers_for_the_open_file_only() {
        let mut ctx = ctx_with(None);
        ctx.open_files = vec![OpenFile { path: "/repo/slot-a/a.rs".to_string(), dirty: true }];

        let request = |path: &str| {
            json!({
                "jsonrpc": "2.0", "id": 11, "method": "tools/call",
                "params": { "name": "checkDocumentDirty", "arguments": { "filePath": path } },
            })
            .to_string()
        };
        let parse = |raw: String| -> Value {
            let response: Value = serde_json::from_str(&raw).unwrap();
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap()
        };

        let hit = parse(handle_message(&request("/repo/slot-a/a.rs"), &ctx).unwrap());
        assert_eq!(hit["success"], true);
        assert_eq!(hit["isDirty"], true);

        let miss = parse(handle_message(&request("/repo/slot-a/b.rs"), &ctx).unwrap());
        assert_eq!(miss["success"], false);
    }

    #[test]
    fn open_editors_reports_every_diff_pane_file_with_none_marked_active() {
        // Unlike the single-viewer-file case, several files can be dirty in
        // the diff pane at once — none of them has a real "active" signal.
        let mut ctx = ctx_with(None);
        ctx.open_files = vec![
            OpenFile { path: "/repo/slot-a/src/a.rs".to_string(), dirty: true },
            OpenFile { path: "/repo/slot-a/src/b.rs".to_string(), dirty: false },
        ];
        let editors = call(&ctx, "getOpenEditors");
        let tabs = editors["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0]["fileName"], "/repo/slot-a/src/a.rs");
        assert_eq!(tabs[0]["isDirty"], true);
        assert_eq!(tabs[0]["isActive"], false);
        assert_eq!(tabs[1]["fileName"], "/repo/slot-a/src/b.rs");
        assert_eq!(tabs[1]["isDirty"], false);
        assert_eq!(tabs[1]["isActive"], false);
    }

    #[test]
    fn check_document_dirty_finds_any_of_several_open_files() {
        let mut ctx = ctx_with(None);
        ctx.open_files = vec![
            OpenFile { path: "/repo/slot-a/src/a.rs".to_string(), dirty: true },
            OpenFile { path: "/repo/slot-a/src/b.rs".to_string(), dirty: false },
        ];
        assert_eq!(
            check_document_dirty(&ctx, &json!({ "filePath": "/repo/slot-a/src/a.rs" }))["isDirty"],
            true
        );
        assert_eq!(
            check_document_dirty(&ctx, &json!({ "filePath": "/repo/slot-a/src/b.rs" }))["isDirty"],
            false
        );
        assert_eq!(
            check_document_dirty(&ctx, &json!({ "filePath": "/repo/slot-a/src/c.rs" }))["success"],
            false
        );
    }

    #[test]
    fn diagnostics_filter_by_uri_when_requested() {
        let mut ctx = ctx_with(None);
        ctx.diagnostics = json!([
            { "uri": "file:///repo/slot-a/src/a.rs", "diagnostics": [{ "message": "boom" }] },
            { "uri": "file:///repo/slot-a/src/b.rs", "diagnostics": [] },
        ]);

        let all = call(&ctx, "getDiagnostics");
        assert_eq!(all.as_array().unwrap().len(), 2);

        let request = json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": { "name": "getDiagnostics",
                        "arguments": { "uri": "file:///repo/slot-a/src/a.rs" } },
        })
        .to_string();
        let response: Value =
            serde_json::from_str(&handle_message(&request, &ctx).unwrap()).unwrap();
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let filtered: Value = serde_json::from_str(text).unwrap();
        assert_eq!(filtered.as_array().unwrap().len(), 1);
        assert_eq!(filtered[0]["uri"], "file:///repo/slot-a/src/a.rs");
    }

    #[test]
    fn selection_changed_frame_is_wire_exact() {
        let selection =
            Selection::range(Path::new("/w/a.ts"), 10, 12, 0, "const x = 1".to_string());
        let frame: Value = serde_json::from_str(&selection_changed_frame(&selection)).unwrap();
        assert_eq!(frame["method"], "selection_changed");
        assert_eq!(frame["params"]["filePath"], "/w/a.ts");
        assert_eq!(frame["params"]["fileUrl"], "file:///w/a.ts");
        assert_eq!(frame["params"]["selection"]["start"]["line"], 10);
        assert_eq!(frame["params"]["selection"]["isEmpty"], false);
        assert!(frame.get("id").is_none(), "notifications carry no id");
    }

    #[test]
    fn cleared_selection_is_empty_at_origin() {
        let cleared = Selection::cleared(Path::new("/w/a.ts"));
        assert!(cleared.selection.is_empty);
        assert_eq!(cleared.selection.start, Position { line: 0, character: 0 });
        assert_eq!(cleared.text, "");
    }

    #[test]
    fn at_mentioned_frame_omits_lines_together() {
        let with: Value =
            serde_json::from_str(&at_mentioned_frame("/w/a.ts", Some((3, 9)))).unwrap();
        assert_eq!(with["params"]["lineStart"], 3);
        assert_eq!(with["params"]["lineEnd"], 9);
        let without: Value = serde_json::from_str(&at_mentioned_frame("/w/a.ts", None)).unwrap();
        assert_eq!(without["params"], json!({ "filePath": "/w/a.ts" }));
    }

    #[test]
    fn malformed_json_and_batches_are_rejected() {
        let parse: Value =
            serde_json::from_str(&handle_message("{nope", &ctx_with(None)).unwrap()).unwrap();
        assert_eq!(parse["error"]["code"], -32700);
        let batch: Value =
            serde_json::from_str(&handle_message("[]", &ctx_with(None)).unwrap()).unwrap();
        assert_eq!(batch["error"]["code"], -32600);
    }
}
