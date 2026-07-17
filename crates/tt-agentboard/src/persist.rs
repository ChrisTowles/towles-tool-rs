//! Atomic file persistence for the shared agentboard config files.
//!
//! Every Agentboard instance (`tt slot` runs one per checkout) reads
//! and writes the same `~/.config/towles-tool/agentboard/*.json` files. A plain
//! `std::fs::write` truncates then streams, so a concurrent reader can observe
//! an empty or half-written file — which is how a torn `repos.json` read made
//! one instance's engine poll see zero repos and prune every folder's session
//! records (#75). Writing to a temp file in the same directory and `rename`ing
//! over the target makes the swap atomic on POSIX: readers see either the old
//! or the new content, never a fragment.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `contents` to `path` atomically: create parent dirs, write a sibling
/// temp file, then rename it over the target. The temp name embeds the pid and
/// a process-wide counter so concurrent writers (other instances, or two
/// threads in this one) never collide on the temp file itself.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}-{seq}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_content_and_creates_parents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("file.json");
        write_atomic(&path, "{}\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
    }

    #[test]
    fn overwrites_existing_and_leaves_no_temp_files() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.json");
        write_atomic(&path, "old").unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["file.json"]);
    }
}
