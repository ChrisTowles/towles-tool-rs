//! `ttr mcp` — Model Context Protocol server over stdio.
//!
//! Thin boundary over `tt_mcp::serve`: the library owns the JSON-RPC loop and
//! tools; this just picks the store path and returns the exit code. Logging
//! goes to stderr (stdout is the protocol channel), via the env_logger that
//! `main` installs.

use crate::cli::McpCommands;

pub fn run(command: McpCommands) -> i32 {
    match command {
        McpCommands::Serve { store } => tt_mcp::serve(store.as_deref()),
    }
}
