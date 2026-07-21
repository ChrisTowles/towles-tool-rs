//! Preview screen backend: the capture half of the annotate-and-send flow.
//!
//! The Preview screen embeds a running dev server in an `<iframe>` and lets
//! the user draw annotations over it on a DOM canvas. "Send to agent" needs a
//! pixel-accurate PNG of what the user sees — iframe plus annotations — and
//! the DOM can't produce one (a cross-origin iframe taints any canvas it's
//! drawn into). This is the same problem Claude Desktop's page-preview pane
//! solves with Electron's `webContents.capturePage()`; the WebKitGTK
//! equivalent is `webkit_web_view_get_snapshot`, which rasterizes the app
//! webview's composited viewport — embedded iframe included, since the
//! snapshot is a privileged native API with no CORS-taint concept.
//!
//! `preview_capture` snapshots the visible viewport and crops to the preview
//! surface's rect; `preview_write_feedback` stages the annotated PNG under
//! `tt_config::pasted_images_dir()` (outside any repo — same reasoning as
//! `tt_slots::pasted`) so the frontend can name its path in a prompt typed
//! into an agent session's PTY.

use serde::Deserialize;
use tt_slots::pasted::{self, PastedImage};

/// The preview surface's rectangle in CSS pixels (`getBoundingClientRect`
/// relative to the viewport), plus the `devicePixelRatio` that scales it into
/// the snapshot surface's device-pixel space.
///
/// Only the Linux capture path reads these fields; the macOS/Windows
/// `preview_capture` stub deserializes a `CaptureRect` off the IPC boundary
/// but ignores it, so mark them non-dead there rather than losing the shape.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct CaptureRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: f64,
}

/// Rasterize the main webview's visible viewport and crop to `rect`,
/// returning a base64 PNG. The crop happens here rather than in the webview
/// precisely because the webview can't read these pixels itself (see the
/// module docs).
#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn preview_capture(
    window: tauri::WebviewWindow,
    rect: CaptureRect,
) -> Result<String, String> {
    use base64::Engine as _;
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            // Runs on the GTK main thread — the only place webkit calls are
            // legal, and the snapshot's async completion lands on the main
            // loop too. So this callback does the cheapest possible work: a
            // clipped blit into a crop surface and a memcpy of its bytes out.
            // The per-pixel un-premultiply and the PNG encode both wait for
            // spawn_blocking below, off the compositor thread.
            webview.inner().snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                None::<&webkit2gtk::gio::Cancellable>,
                move |result| {
                    let cropped = result
                        .map_err(|e| format!("webview snapshot failed: {e}"))
                        .and_then(|surface| crop_surface_argb(&surface, rect));
                    let _ = tx.send(cropped);
                },
            );
        })
        .map_err(|e| format!("with_webview: {e}"))?;

    let cropped = rx.await.map_err(|_| "snapshot callback dropped".to_string())??;
    let png = tauri::async_runtime::spawn_blocking(move || {
        let (width, height, rgba) = argb_to_straight_rgba(cropped);
        pasted::rgba_to_png(width, height, &rgba)
    })
    .await
    .map_err(|e| format!("encode task failed: {e}"))?
    .map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

/// A crop of the snapshot surface, still in cairo's premultiplied
/// native-endian ARGB32 with the row stride tight to `width` — the raw form
/// that leaves the GTK thread, converted to straight RGBA off-thread.
#[cfg(target_os = "linux")]
struct CroppedArgb {
    width: u32,
    height: u32,
    /// Premultiplied BGRA (little-endian ARGB32), `width * height * 4` bytes.
    data: Vec<u8>,
}

/// The Windows/macOS shells have no capture path yet (wry exposes no
/// `capturePage` equivalent there); the frontend surfaces this message on the
/// send action rather than hiding the whole screen.
#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub async fn preview_capture(
    _window: tauri::WebviewWindow,
    _rect: CaptureRect,
) -> Result<String, String> {
    Err("preview capture is only implemented for the Linux (WebKitGTK) shell".to_string())
}

/// Crop the snapshot surface to `rect` (scaled by DPR) into a tight-stride
/// ARGB32 buffer. Runs on the GTK thread, so it stays memcpy-cheap: a cairo
/// clip-and-blit of just the crop region, then a row-by-row copy that drops
/// the surface's alignment padding. No per-pixel arithmetic here — that's
/// `argb_to_straight_rgba`'s job, off-thread.
#[cfg(target_os = "linux")]
fn crop_surface_argb(surface: &cairo::Surface, rect: CaptureRect) -> Result<CroppedArgb, String> {
    let dpr = if rect.device_pixel_ratio.is_finite() && rect.device_pixel_ratio > 0.0 {
        rect.device_pixel_ratio
    } else {
        1.0
    };
    let width = (rect.width * dpr).round() as i32;
    let height = (rect.height * dpr).round() as i32;
    if width <= 0 || height <= 0 {
        return Err("capture rect is empty".to_string());
    }

    let mut cropped = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|e| format!("create crop surface: {e}"))?;
    {
        let cr = cairo::Context::new(&cropped).map_err(|e| format!("cairo context: {e}"))?;
        cr.set_source_surface(surface, -(rect.x * dpr).round(), -(rect.y * dpr).round())
            .map_err(|e| format!("set snapshot source: {e}"))?;
        cr.paint().map_err(|e| format!("paint crop: {e}"))?;
    }
    cropped.flush();

    let stride = cropped.stride() as usize;
    let (w, h) = (width as usize, height as usize);
    let src = cropped.data().map_err(|e| format!("read surface data: {e}"))?;
    let mut data = vec![0u8; w * h * 4];
    for row in 0..h {
        data[row * w * 4..(row + 1) * w * 4]
            .copy_from_slice(&src[row * stride..row * stride + w * 4]);
    }
    Ok(CroppedArgb { width: width as u32, height: height as u32, data })
}

/// Convert cairo's premultiplied little-endian ARGB32 (byte order B,G,R,A)
/// into the straight-alpha RGBA `rgba_to_png` expects. Un-premultiplying is a
/// per-pixel divide, so this runs off the GTK thread (in the same
/// `spawn_blocking` as the PNG encode).
#[cfg(target_os = "linux")]
fn argb_to_straight_rgba(cropped: CroppedArgb) -> (u32, u32, Vec<u8>) {
    let mut buf = cropped.data;
    for px in buf.chunks_exact_mut(4) {
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        // Un-premultiply so translucent page regions don't come out dark.
        let un = |c: u8| -> u8 {
            if a == 0 || a == 255 { c } else { ((c as u32 * 255) / a as u32).min(255) as u8 }
        };
        px[0] = un(r);
        px[1] = un(g);
        px[2] = un(b);
        px[3] = a;
    }
    (cropped.width, cropped.height, buf)
}

/// Stage an annotated preview capture as a PNG file and return its absolute
/// path for the caller to name in an agent prompt (`promptWithImages` on the
/// client). One scope per send — unlike the new-slot flow, a second send must
/// not clear the previous one's directory out from under an agent that
/// hasn't read the first file yet — with `pasted::prune` sweeping scopes
/// older than its retention window on every write.
#[tauri::command]
pub async fn preview_write_feedback(
    repo: String,
    images: Vec<PastedImage>,
) -> Result<Vec<String>, String> {
    let base = tt_config::pasted_images_dir();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    tauri::async_runtime::spawn_blocking(move || {
        let scope = pasted::scope_name(&repo, &format!("preview-{now_ms}"));
        let paths = pasted::write_images(&base, &scope, &images, now_ms)?;
        tracing::info!(repo = %repo, count = paths.len(), "preview.feedback_staged");
        Ok(paths)
    })
    .await
    .map_err(|e| format!("feedback task failed: {e}"))?
    .map(|paths: Vec<std::path::PathBuf>| {
        paths.iter().map(|p| p.to_string_lossy().to_string()).collect()
    })
    .map_err(|e: pasted::PastedError| e.to_string())
}
