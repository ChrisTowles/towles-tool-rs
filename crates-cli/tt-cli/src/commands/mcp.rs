//! `tt mcp` — Model Context Protocol server over stdio.
//!
//! Thin boundary over `tt_mcp::serve`: the library owns the JSON-RPC loop and
//! tools; this just picks the store path and returns the exit code. Logging
//! goes to stderr (stdout is the protocol channel), via the `tt_otel`
//! subscriber that `main` installs.

use std::path::Path;

use crate::cli::McpCommands;

pub fn run(command: McpCommands, config_dir: Option<&Path>) -> i32 {
    match command {
        McpCommands::Serve { store } => {
            // Honor the global `--config-dir` for the server's per-call settings
            // reads (the capability gate, journal_append, collect_refresh) the
            // same way the config/journal/collect commands do.
            let settings_path =
                config_dir.map(|dir| dir.join(format!("{}.settings.json", tt_config::TOOL_NAME)));
            tt_mcp::serve(store.as_deref(), settings_path.as_deref())
        }
    }
}
