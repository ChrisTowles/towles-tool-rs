//! Agent watchers. Phase 2 ported claude-code; phase 5 adds amp, codex, opencode.

pub mod amp;
pub mod claude_code;
pub mod claude_usage;
pub mod codex;
pub mod opencode;

use std::path::Path;
use std::time::UNIX_EPOCH;

/// Read a file's bytes from `offset` to EOF via seek. Incremental tail scans
/// must NOT `std::fs::read` the whole file — an active session journal grows
/// to many MB, and re-reading it every 2s tick scales with session length.
/// A seek past EOF just yields an empty read.
pub(crate) fn read_from_offset(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Modification time of a file in ms since the Unix epoch, or `None` on error.
/// Shared by the amp/codex watchers.
pub(crate) fn mtime_ms(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}
