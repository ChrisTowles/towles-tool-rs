//! Streaming dictation engine — sherpa-onnx ASR over mic audio, ported from
//! the standalone `scribed` daemon and scoped down to app-only use (no
//! daemon, no global hotkey, no OS-level typing). See
//! `crates-tauri/tt-app/src/dictation.rs` for the Tauri command surface that
//! wraps [`engine::DictationEngine`], and `apps/client/src/lib/dictation.ts`
//! for how the frontend diffs transcript snapshots into its three targets
//! (webview input, terminal, panel).
//!
//! Pure core (this module's [`asr::driver`], [`retype`], [`settings`],
//! [`audio::dsp`]) compiles and tests without the `asr` feature. The
//! recognizer backend ([`asr::sherpa`]) and [`engine`] (which drives it)
//! require `--features asr`, since they link the sherpa-onnx native library.

pub mod asr;
pub mod audio;
#[cfg(feature = "asr")]
pub mod engine;
pub mod retype;
pub mod settings;

pub use asr::{AsrError, EndpointRules, Segment, Transcript};
