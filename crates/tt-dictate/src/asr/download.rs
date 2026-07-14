//! Lazy model download. Ported from scribed `src/asr/download.rs`, reworked
//! onto `ureq` (workspace HTTP client — trusts the OS cert store, unlike
//! bundled-webpki `reqwest`) with a progress callback instead of an
//! `indicatif` bar (the app renders its own progress UI from
//! `dictation://model` events) and an atomic extract-then-rename (scribed's
//! extract-in-place left a half-extracted directory looking like a cache hit
//! if the process died mid-extract).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::asr::AsrError;

/// A named model bundle.
#[derive(Debug, Clone)]
pub struct ModelArchive {
    pub name: &'static str,
    pub url: &'static str,
    pub extracted_dir: &'static str,
}

/// NVIDIA Nemotron streaming 0.6B (FastConformer-CacheAware-RNNT, 1120 ms chunk,
/// int8 quantized, 16 kHz). ~442 MB download. NVIDIA Open Model License.
pub const STREAMING_MODEL: ModelArchive = ModelArchive {
    name: "nemotron-streaming-en-0.6b-1120ms-int8-2026-04-25",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-speech-streaming-en-0.6b-1120ms-int8-2026-04-25.tar.bz2",
    extracted_dir: "sherpa-onnx-nemotron-speech-streaming-en-0.6b-1120ms-int8-2026-04-25",
};

/// Ensure the named model exists locally. Returns the directory containing the
/// extracted files. Downloads + extracts on first call. `on_progress(bytes_done,
/// bytes_total)` is called as the download streams; `bytes_total` is 0 if the
/// server didn't send a `Content-Length`.
pub fn ensure(
    archive: &ModelArchive,
    cache_dir: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, AsrError> {
    let target = cache_dir.join(archive.extracted_dir);
    if target.exists() {
        log::info!("model cache hit at {}", target.display());
        return Ok(target);
    }
    fs::create_dir_all(cache_dir).map_err(|e| AsrError::Load(e.to_string()))?;

    let archive_path = cache_dir.join(format!("{}.tar.bz2", archive.name));
    download(archive.url, &archive_path, &mut on_progress)?;

    // Extract into a scratch dir first, then rename the finished result into
    // place — a crash mid-extract must never leave `target` looking cached.
    let scratch = tempfile::tempdir_in(cache_dir).map_err(|e| AsrError::Load(e.to_string()))?;
    extract(&archive_path, scratch.path())?;
    let _ = fs::remove_file(&archive_path);

    let extracted = scratch.path().join(archive.extracted_dir);
    if !extracted.exists() {
        return Err(AsrError::Load(format!(
            "archive did not contain expected dir {}",
            archive.extracted_dir
        )));
    }
    fs::rename(&extracted, &target).map_err(|e| AsrError::Load(e.to_string()))?;
    Ok(target)
}

fn download(
    url: &str,
    dest: &Path,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), AsrError> {
    log::info!("downloading model from {url}");
    let response = ureq::get(url).call().map_err(|e| AsrError::Load(e.to_string()))?;
    let total = response.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);

    let mut out = fs::File::create(dest).map_err(|e| AsrError::Load(e.to_string()))?;
    let mut reader = response.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| AsrError::Load(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| AsrError::Load(e.to_string()))?;
        done += n as u64;
        on_progress(done, total);
    }
    Ok(())
}

fn extract(archive: &Path, into: &Path) -> Result<(), AsrError> {
    // Shell out to `tar` rather than pulling in tar+bzip2 crates; tar is
    // universally available on Linux + macOS and saves a hundred KB of deps.
    let status = std::process::Command::new("tar")
        .arg("-xjf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|e| AsrError::Load(format!("failed to spawn tar: {e}")))?;
    if !status.success() {
        return Err(AsrError::Load(format!("tar extraction failed (exit {status})")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ensure_returns_cached_path_without_network() {
        let dir = tempdir().unwrap();
        let prebuilt = dir.path().join(STREAMING_MODEL.extracted_dir);
        fs::create_dir_all(&prebuilt).unwrap();
        let result = ensure(&STREAMING_MODEL, dir.path(), |_, _| {}).unwrap();
        assert_eq!(result, prebuilt);
    }
}
