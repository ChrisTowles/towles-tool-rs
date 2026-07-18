//! Images pasted into the new-slot form, staged as files a Claude prompt can
//! point at.
//!
//! The goal typed in that form reaches Claude as a shell-quoted argv entry
//! (`claude '<goal>'`, typed into the slot's PTY) — a string, with nowhere to
//! put bytes. So a pasted image becomes a *file*, and the goal grows a
//! reference to its path; Claude's Read tool handles images, so a path in the
//! opening prompt is the attachment.
//!
//! **These files live outside the repo**, under a `tt_config`-resolved
//! staging dir, mirroring Claude Code's own
//! `~/.claude/image-cache/<session>/` convention. Putting them *inside* the
//! new slot was tried first, on the assumption that Claude Code would prompt
//! for an out-of-workspace read — it doesn't (verified: a `claude -p` run in
//! one repo read an image under `/tmp` in default permission mode without
//! prompting). So the in-repo version bought nothing and cost a `.gitignore`
//! to keep the slot's `git status` clean.
//!
//! Staging is keyed by repo+branch rather than accumulating unique names, so
//! retrying a failed create overwrites its own directory instead of leaving a
//! copy behind, and [`prune`] ages out anything older than [`MAX_AGE_MS`].

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::layout::slot_name_from_branch;

/// Cap per image. Claude Code itself rejects images well below this, so a
/// paste over the cap is a mis-paste (a copied *file* rather than a bitmap,
/// say) and is worth an error rather than several seconds of base64 crossing
/// the IPC boundary before Claude refuses it anyway.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// How long a staged paste survives. Long enough that a slot created today
/// can still resolve its image after a restart; short enough that this
/// directory can't grow without bound. Pruning is opportunistic (on write),
/// so nothing has to run on a timer.
pub const MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// One image on its way to a slot's prompt: the MIME type plus standard
/// base64 (Tauri's JSON IPC has no bytes type — a `Vec<u8>` here would cross
/// as a megabyte-long array of JSON numbers).
///
/// Crosses in both directions: inbound from the webview for a paste the
/// browser *did* decode, and outbound from `read_clipboard_image` for the
/// Linux case where it didn't.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PastedImage {
    pub mime: String,
    pub data_base64: String,
}

#[derive(Debug, Error)]
pub enum PastedError {
    #[error("{0} isn't an image type Claude can read")]
    UnsupportedMime(String),
    #[error("pasted image is not valid base64: {0}")]
    BadBase64(String),
    #[error("pasted image is {got} bytes, over the {MAX_IMAGE_BYTES}-byte limit")]
    TooLarge { got: usize },
    #[error("couldn't encode the clipboard image: {0}")]
    BadImage(String),
    #[error("writing {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, PastedError>;

/// File extension for a clipboard MIME type. Deliberately a closed set: the
/// extension is what tells Claude's Read tool how to decode the file, so
/// guessing (or defaulting to `.png` for an unknown type) would write a file
/// that silently fails to load later, far from this call.
fn extension_for(mime: &str) -> Result<&'static str> {
    // Browsers sometimes append parameters, e.g. `image/png;charset=utf-8`.
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match base.as_str() {
        "image/png" => Ok("png"),
        "image/jpeg" | "image/jpg" => Ok("jpg"),
        "image/gif" => Ok("gif"),
        "image/webp" => Ok("webp"),
        _ => Err(PastedError::UnsupportedMime(mime.to_string())),
    }
}

/// Encode raw RGBA pixels as a PNG.
///
/// The system clipboard hands back undecoded RGBA (that's what
/// `arboard`/`tauri-plugin-clipboard-manager` return), but a file only counts
/// as an attachment if Claude's Read tool can decode it, and the webview
/// needs something it can show in an `<img>` for the thumbnail. Both want a
/// real image format, so the bytes get encoded once, here, on the way out of
/// the clipboard.
///
/// This exists because a Ctrl+V image paste on Linux never reaches the
/// webview's `paste` event, so the clipboard has to be read natively — see
/// `read_clipboard_image` in the app's `slots.rs`.
pub fn rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| PastedError::BadImage(format!("{width}x{height} overflows")))?;
    if expected == 0 {
        return Err(PastedError::BadImage("clipboard image is empty".to_string()));
    }
    if rgba.len() != expected {
        return Err(PastedError::BadImage(format!(
            "{width}x{height} needs {expected} RGBA bytes, got {}",
            rgba.len()
        )));
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| PastedError::BadImage(e.to_string()))?;
    writer.write_image_data(rgba).map_err(|e| PastedError::BadImage(e.to_string()))?;
    drop(writer);
    Ok(out)
}

/// One staging directory per pending slot: `<repo>-<branch-slug>`. Reuses
/// the branch→slot-name slug so the directory reads like the slot it belongs
/// to, and includes the repo so the same branch name in two repos doesn't
/// collide.
pub fn scope_name(repo: &str, branch: &str) -> String {
    // Both slug to `None` only for input that is entirely punctuation. The
    // branch is already validated before a slot is created, so this is a
    // belt-and-braces fallback rather than a reachable path — but it must
    // still be a legal single directory name, never an empty one that would
    // write straight into the staging root.
    let parts: Vec<String> =
        [repo, branch].iter().filter_map(|s| slot_name_from_branch(s)).collect();
    if parts.is_empty() { "unnamed".to_string() } else { parts.join("-") }
}

/// Decode `images` and write them into `<base>/<scope>/`, returning their
/// absolute paths in the order given (which is the order the prompt will
/// reference them in, so it's the order the user pasted).
///
/// All-or-nothing by intent: the first failure returns `Err` and the caller
/// surfaces it instead of launching Claude on a prompt that references an
/// image that isn't there.
pub fn write_images(
    base: &Path,
    scope: &str,
    images: &[PastedImage],
    now_ms: u64,
) -> Result<Vec<PathBuf>> {
    if images.is_empty() {
        return Ok(Vec::new());
    }
    // Best-effort: a prune failure must never block an actual paste.
    let _ = prune(base, now_ms);

    let dir = base.join(scope);
    // A retry of the same slot reuses this directory — clear it so a paste
    // that dropped an image can't leave the previous attempt's file behind to
    // be picked up by the next prompt.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|source| PastedError::Write { path: dir.display().to_string(), source })?;

    let mut written = Vec::with_capacity(images.len());
    for (i, image) in images.iter().enumerate() {
        let ext = extension_for(&image.mime)?;
        let bytes = BASE64
            .decode(image.data_base64.as_bytes())
            .map_err(|e| PastedError::BadBase64(e.to_string()))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(PastedError::TooLarge { got: bytes.len() });
        }
        // 1-based: the prompt references these by path, and the directory was
        // just cleared, so there's no collision to scan for.
        let path = dir.join(format!("paste-{}.{ext}", i + 1));
        std::fs::write(&path, &bytes)
            .map_err(|source| PastedError::Write { path: path.display().to_string(), source })?;
        restrict_permissions(&path);
        written.push(path);
    }
    Ok(written)
}

/// Pasted screenshots are user content and can hold anything that was on
/// screen, so they're owner-only — the same choice Claude Code makes for its
/// own image cache. Best-effort: a filesystem without unix modes just keeps
/// the default.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Drop staging directories last modified more than [`MAX_AGE_MS`] ago.
/// Returns how many were removed. Errors are swallowed per-entry: pruning is
/// housekeeping, and a single undeletable directory shouldn't fail a paste.
pub fn prune(base: &Path, now_ms: u64) -> std::io::Result<usize> {
    let mut removed = 0;
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        // Nothing staged yet is the common case, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) else {
            continue;
        };
        // Age against the caller's clock, not `elapsed()`, so the cutoff is
        // injectable from tests rather than depending on wall time.
        let age_ms = now_ms.saturating_sub(since_epoch.as_millis() as u64);
        if age_ms > MAX_AGE_MS && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(data: &[u8]) -> PastedImage {
        PastedImage { mime: "image/png".into(), data_base64: BASE64.encode(data) }
    }

    #[test]
    fn writes_images_in_order_under_the_scope_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let images = vec![png(b"first"), png(b"second")];
        let paths = write_images(tmp.path(), "blog-feat-thing", &images, 0).unwrap();

        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], tmp.path().join("blog-feat-thing/paste-1.png"));
        assert_eq!(paths[1], tmp.path().join("blog-feat-thing/paste-2.png"));
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"first");
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"second");
    }

    #[test]
    fn nothing_is_written_inside_any_repo() {
        // The whole point of the staging dir: a paste must not touch the
        // checkout it's for. Guarded by construction — `write_images` only
        // ever joins under `base` — but worth pinning so a future "just put
        // it next to the code" change has to delete this test on purpose.
        let tmp = tempfile::tempdir().unwrap();
        let paths = write_images(tmp.path(), "scope", &[png(b"x")], 0).unwrap();
        assert!(paths.iter().all(|p| p.starts_with(tmp.path())));
    }

    #[test]
    fn retrying_the_same_slot_replaces_the_previous_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        write_images(tmp.path(), "scope", &[png(b"a"), png(b"b")], 0).unwrap();
        // Second attempt pasted only one image — the stale paste-2 from the
        // first attempt must not survive to be referenced by the new prompt.
        let paths = write_images(tmp.path(), "scope", &[png(b"only")], 0).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(!tmp.path().join("scope/paste-2.png").exists());
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"only");
    }

    #[test]
    fn owner_only_permissions_on_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let paths = write_images(tmp.path(), "scope", &[png(b"secret")], 0).unwrap();
            let mode = std::fs::metadata(&paths[0]).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn no_images_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(write_images(tmp.path(), "scope", &[], 0).unwrap().is_empty());
        assert!(!tmp.path().join("scope").exists());
    }

    #[test]
    fn scope_name_slugs_repo_and_branch() {
        assert_eq!(scope_name("blog", "feat/paste-images"), "blog-feat-paste-images");
        // Case is preserved, matching how slot directories are named.
        assert_eq!(scope_name("towles-tool-rs", "fix/Thing"), "towles-tool-rs-fix-Thing");
    }

    #[test]
    fn scope_name_is_always_a_single_usable_directory_name() {
        // Never empty (that would write into the staging root itself) and
        // never nested (a `/` would escape the scope dir).
        for (repo, branch) in [("", ""), ("///", "..."), ("a/b", "c/d")] {
            let scope = scope_name(repo, branch);
            assert!(!scope.is_empty(), "empty scope for {repo:?}/{branch:?}");
            assert!(!scope.contains('/'), "nested scope {scope:?}");
        }
    }

    /// Wall-clock ms, matching what the app passes as `now_ms`.
    fn now_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
            as u64
    }

    #[test]
    fn fresh_dirs_survive_a_prune() {
        let tmp = tempfile::tempdir().unwrap();
        write_images(tmp.path(), "scope", &[png(b"x")], now_ms()).unwrap();
        assert_eq!(prune(tmp.path(), now_ms()).unwrap(), 0);
        assert!(tmp.path().join("scope/paste-1.png").exists());
    }

    #[test]
    fn stale_dirs_are_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        write_images(tmp.path(), "scope", &[png(b"x")], now_ms()).unwrap();
        // Jump the clock past the cutoff rather than backdating the file —
        // this is exactly why `now_ms` is a parameter.
        let later = now_ms() + MAX_AGE_MS + 1;
        assert_eq!(prune(tmp.path(), later).unwrap(), 1);
        assert!(!tmp.path().join("scope").exists());
    }

    #[test]
    fn writing_prunes_stale_neighbours_but_keeps_the_new_paste() {
        let tmp = tempfile::tempdir().unwrap();
        write_images(tmp.path(), "old-slot", &[png(b"old")], now_ms()).unwrap();
        let later = now_ms() + MAX_AGE_MS + 1;
        let paths = write_images(tmp.path(), "new-slot", &[png(b"new")], later).unwrap();
        assert!(!tmp.path().join("old-slot").exists());
        assert!(paths[0].exists());
    }

    #[test]
    fn pruning_an_empty_base_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(prune(&tmp.path().join("never-created"), 0).unwrap(), 0);
    }

    #[test]
    fn rgba_encodes_to_a_png_that_decodes_back_to_the_same_pixels() {
        // 2x1: opaque red, opaque blue.
        let rgba = vec![255, 0, 0, 255, 0, 0, 255, 255];
        let encoded = rgba_to_png(2, 1, &rgba).unwrap();

        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n", "not a PNG signature");
        let decoder = png::Decoder::new(encoded.as_slice());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buf[..info.buffer_size()], &rgba[..]);
    }

    #[test]
    fn rgba_length_must_match_the_dimensions() {
        // A truncated clipboard buffer would otherwise encode as a corrupt
        // PNG that only fails later, when Claude tries to read it.
        let err = rgba_to_png(2, 2, &[0; 8]).unwrap_err();
        assert!(matches!(err, PastedError::BadImage(_)), "got {err:?}");
    }

    #[test]
    fn empty_clipboard_image_is_rejected() {
        assert!(matches!(rgba_to_png(0, 0, &[]), Err(PastedError::BadImage(_))));
    }

    #[test]
    fn absurd_dimensions_dont_overflow() {
        assert!(matches!(rgba_to_png(u32::MAX, u32::MAX, &[0; 4]), Err(PastedError::BadImage(_))));
    }

    #[test]
    fn extension_follows_the_mime_type() {
        assert_eq!(extension_for("image/png").unwrap(), "png");
        assert_eq!(extension_for("image/jpeg").unwrap(), "jpg");
        assert_eq!(extension_for("image/gif").unwrap(), "gif");
        assert_eq!(extension_for("image/webp").unwrap(), "webp");
    }

    #[test]
    fn mime_parameters_and_casing_dont_defeat_the_match() {
        assert_eq!(extension_for("image/PNG;charset=utf-8").unwrap(), "png");
    }

    #[test]
    fn unsupported_mime_is_an_error_not_a_guessed_extension() {
        // A wrong extension would write a file that fails to decode later,
        // far from here — better to refuse the paste at the boundary.
        assert!(matches!(extension_for("image/svg+xml"), Err(PastedError::UnsupportedMime(_))));
        assert!(matches!(extension_for("text/plain"), Err(PastedError::UnsupportedMime(_))));
    }

    #[test]
    fn bad_base64_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = PastedImage { mime: "image/png".into(), data_base64: "not!base64".into() };
        assert!(matches!(
            write_images(tmp.path(), "scope", &[bad], 0),
            Err(PastedError::BadBase64(_))
        ));
    }

    #[test]
    fn oversized_image_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let huge = png(&vec![0u8; MAX_IMAGE_BYTES + 1]);
        assert!(matches!(
            write_images(tmp.path(), "scope", &[huge], 0),
            Err(PastedError::TooLarge { .. })
        ));
    }
}
