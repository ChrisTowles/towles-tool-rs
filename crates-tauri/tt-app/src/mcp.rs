//! Tauri bridge for the MCP screen's tool documentation. The server runs in
//! this process ([`crate::mcp_http`]), so the screen reads the tool list
//! straight from [`tt_mcp::tool_definitions`] — the same source `tools/list`
//! answers from — instead of round-tripping a request through the socket. It
//! is also the only way the screen can show the contract when this instance
//! lost the bind race and is serving nothing.

use serde_json::Value;

/// The full JSON Schema tool list (name, description, inputSchema) from the
/// MCP contract — identical to what `tools/list` returns to a client.
#[tauri::command]
pub fn mcp_tool_docs() -> Value {
    tt_mcp::tool_definitions()
}
