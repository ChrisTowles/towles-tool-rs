//! Tauri bridge for the MCP screen's tool documentation. The MCP server
//! (`tt mcp serve`) and this app are separate processes, so the screen can't
//! just ask a running server what it exposes — instead it reads the same
//! `tt_mcp::tool_definitions()` the server answers `tools/list` with, which
//! keeps the docs from ever drifting out of sync with the actual contract.

use serde_json::Value;

/// The full JSON Schema tool list (name, description, inputSchema) from the
/// MCP contract — identical to what `tools/list` returns over stdio.
#[tauri::command]
pub fn mcp_tool_docs() -> Value {
    tt_mcp::tool_definitions()
}
