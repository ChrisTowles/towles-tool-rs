//! Safely replacing the full content of an existing journal file.
//!
//! [`save_file`] performs an optimistic-concurrency guarded, atomic overwrite: the
//! caller passes the content it originally loaded, and the save refuses to proceed if
//! the file has changed on disk since then (an external edit, a `journal_log` append,
//! another window). This turns the classic lost-update race into a surfaced
//! [`Error::FileChangedOnDisk`] instead of silently clobbering the other write.
//!
//! The write itself is atomic: content is written to a sibling temp file in the same
//! directory, flushed, then `rename`d over the target. A crash mid-write leaves the
//! original intact rather than a truncated file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, Result};

/// Per-process counter making sibling temp-file names unique without reading the clock.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A sibling temp path in the same directory as `path`, so the final [`std::fs::rename`]
/// stays within one filesystem and is therefore atomic.
fn temp_path_for(path: &Path) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp.{pid}.{n}"))
}

/// Replace the full content of an existing file at `path` with `new_content`.
///
/// `expected_original` is the content the caller loaded earlier; the save re-reads the
/// file and refuses to overwrite (returning [`Error::FileChangedOnDisk`]) if it no
/// longer matches, so a concurrent append or external edit is reported rather than
/// silently lost. The write is atomic (temp file + rename in the same directory).
///
/// Returns an [`Error::Io`] if the file (or its parent directory) does not exist or
/// cannot be read/written.
pub fn save_file(path: &Path, expected_original: &str, new_content: &str) -> Result<()> {
    let current = std::fs::read_to_string(path)?;
    if current != expected_original {
        return Err(Error::FileChangedOnDisk);
    }

    let tmp = temp_path_for(path);
    let write_result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(new_content.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(e));
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_round_trip_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "original body\n").unwrap();

        save_file(&path, "original body\n", "brand new body\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "brand new body\n");
    }

    #[test]
    fn save_to_missing_parent_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist").join("note.md");

        let err = save_file(&path, "", "content").unwrap_err();
        assert!(matches!(err, Error::Io(_)), "expected Io error, got {err:?}");
    }

    #[test]
    fn save_preserves_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        let original = "café ☕ 日本語\n";
        std::fs::write(&path, original).unwrap();

        let updated = "naïve résumé — 🚀 λ ∑\n";
        save_file(&path, original, updated).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), updated);
    }

    #[test]
    fn concurrent_append_after_load_is_detected_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        let loaded = "line one\n";
        std::fs::write(&path, loaded).unwrap();

        // Simulate another writer (e.g. `journal_log`) appending after we loaded.
        std::fs::write(&path, "line one\nappended by someone else\n").unwrap();

        let err = save_file(&path, loaded, "my edit only\n").unwrap_err();
        assert!(matches!(err, Error::FileChangedOnDisk), "expected guard, got {err:?}");
        // The concurrent append must survive untouched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "line one\nappended by someone else\n");
    }

    #[test]
    fn save_after_matching_reload_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "v1\n").unwrap();

        // Overwrite, then re-read to get the fresh marker before saving again.
        std::fs::write(&path, "v2\n").unwrap();
        let fresh = std::fs::read_to_string(&path).unwrap();
        save_file(&path, &fresh, "v3\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v3\n");
    }
}
