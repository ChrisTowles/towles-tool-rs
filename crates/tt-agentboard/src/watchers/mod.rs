//! Agent watchers. Phase 2 ported claude-code; phase 5 adds amp, codex, opencode.

pub mod amp;
pub mod claude_code;
pub mod claude_pid;
pub mod claude_usage;
pub mod codex;
pub mod opencode;

use std::path::Path;
use std::time::UNIX_EPOCH;

/// Modification time of a file in ms since the Unix epoch, or `None` on error.
/// Shared by the amp/codex watchers.
pub(crate) fn mtime_ms(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}
