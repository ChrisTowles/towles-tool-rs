//! Atomic file persistence for the shared agentboard config files.
//!
//! Every Agentboard instance (`tt task` runs one per checkout) reads
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

/// Read the current on-disk value, let `merge` fold this instance's locally
/// touched keys into it, then atomically write the merged value back and hand
/// it to the caller to adopt as its new in-memory state.
///
/// This is the one home of the merge-on-save clobber-safety property that every
/// shared agentboard JSON file depends on (#75, #315). Each file
/// (`sessions.json`, `windows.json`, `collapse.json`, `folder-meta.json`) is
/// written by every open Agentboard instance (`tt task` runs one per checkout).
/// A save must therefore never write this instance's *whole* view — it would
/// clobber folders/keys another window touched that this copy simply hasn't
/// heard about yet (the #75 bug: one poll saw zero repos and pruned every
/// folder's records). The safe shape is always: re-read what's on disk now,
/// merge in only the keys this instance changed, write that back. Callers own
/// the path/dirty guards and the merge itself; this owns the read → write →
/// return-merged sequence so the four `save()` sites can't drift apart.
pub fn merge_on_save<T: serde::Serialize>(
    path: &Path,
    load: impl FnOnce(&Path) -> T,
    merge: impl FnOnce(&mut T),
) -> std::io::Result<T> {
    let mut on_disk = load(path);
    merge(&mut on_disk);
    let json = serde_json::to_string_pretty(&on_disk).unwrap_or_else(|_| "{}".to_string());
    write_atomic(path, &format!("{json}\n"))?;
    Ok(on_disk)
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
    fn merge_on_save_preserves_keys_this_instance_never_touched() {
        use std::collections::BTreeMap;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shared.json");

        // Another instance already wrote two folders' records.
        let load = |p: &Path| -> BTreeMap<String, i32> {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        };
        write_atomic(&path, "{\"a\":1,\"b\":2}\n").unwrap();

        // This instance only knows about — and only touches — key "b".
        let merged = merge_on_save(&path, load, |m: &mut BTreeMap<String, i32>| {
            m.insert("b".to_string(), 20);
        })
        .unwrap();

        // "a" (never touched here) survives; "b" is updated. The merged value is
        // handed back for the caller to adopt as its own in-memory state.
        assert_eq!(merged.get("a"), Some(&1));
        assert_eq!(merged.get("b"), Some(&20));
        let on_disk = load(&path);
        assert_eq!(on_disk, merged);
    }

    #[test]
    fn merge_on_save_can_remove_a_key_and_starts_from_empty_when_missing() {
        use std::collections::BTreeMap;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shared.json");
        let load = |p: &Path| -> BTreeMap<String, i32> {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        };

        // Missing file → the merge starts from an empty value, no error.
        let merged = merge_on_save(&path, load, |m: &mut BTreeMap<String, i32>| {
            m.insert("x".to_string(), 9);
        })
        .unwrap();
        assert_eq!(merged.get("x"), Some(&9));

        // A merge that removes the key writes it back out gone.
        let merged = merge_on_save(&path, load, |m: &mut BTreeMap<String, i32>| {
            m.remove("x");
        })
        .unwrap();
        assert!(merged.is_empty());
        assert!(load(&path).is_empty());
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
