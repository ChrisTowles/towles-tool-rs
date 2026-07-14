//! Streaming sherpa-onnx backend. Ported from scribed `src/asr/sherpa.rs`.
//!
//! Built on the official upstream `sherpa-onnx` crate (Apache-2.0), which
//! replaced the deprecated `sherpa-rs` in March 2026 and version-tracks the
//! C++ project. From 1.13 onwards sherpa-onnx's `OnlineRecognizer` auto-
//! detects between standard streaming Zipformer and NVIDIA Nemotron
//! cache-aware FastConformer based on the decoder ONNX's output count, so
//! we configure exactly one transducer block here and let the engine pick
//! the right path.

use std::path::{Path, PathBuf};

use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};

use crate::asr::driver::{StreamingTranscriber, StreamingUpdate};
use crate::asr::{AsrError, EndpointRules};
use crate::audio::SAMPLE_RATE_HZ_I32;

/// Decoding strategy we pass to sherpa-onnx. `modified_beam_search` is the
/// other option; we use greedy since it's what the upstream Nemotron
/// example uses and modified-beam-search costs latency.
const DECODING_METHOD: &str = "greedy_search";

/// File layout for a sherpa-onnx streaming transducer bundle.
/// `from_dir` auto-detects `encoder*.onnx` / `decoder*.onnx` / `joiner*.onnx`
/// (preferring `.int8.onnx`), so it works for both canonical-named bundles
/// and k2-fsa's epoch-suffixed releases as well as Nemotron's
/// `encoder.int8.onnx` naming.
#[derive(Debug, Clone)]
pub struct ModelBundle {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelBundle {
    pub fn from_dir(dir: &Path) -> Self {
        Self {
            encoder: find_onnx(dir, "encoder"),
            decoder: find_onnx(dir, "decoder"),
            joiner: find_onnx(dir, "joiner"),
            tokens: dir.join("tokens.txt"),
        }
    }

    pub fn validate(&self) -> Result<(), AsrError> {
        for (label, p) in [
            ("encoder", &self.encoder),
            ("decoder", &self.decoder),
            ("joiner", &self.joiner),
            ("tokens.txt", &self.tokens),
        ] {
            if !p.exists() {
                return Err(AsrError::Load(format!("missing {label} at {}", p.display())));
            }
        }
        Ok(())
    }
}

/// Pick the best matching `<role>*.onnx` file in `dir`. Quantized
/// (`.int8.onnx`) wins when both are present.
fn find_onnx(dir: &Path, role: &str) -> PathBuf {
    let mut quantized: Option<PathBuf> = None;
    let mut plain: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with(role) {
                continue;
            }
            if name.ends_with(".int8.onnx") {
                quantized = Some(path);
            } else if name.ends_with(".onnx") {
                plain = Some(path);
            }
        }
    }
    quantized
        .or(plain)
        // Fall back to the canonical name so `validate()` produces a useful
        // error message rather than silently pointing at the directory.
        .unwrap_or_else(|| dir.join(format!("{role}.onnx")))
}

/// Load-time configuration for [`SherpaStreamingTranscriber`].
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    pub provider: String,
    pub num_threads: i32,
    pub endpoint_rules: EndpointRules,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            provider: "cpu".to_string(),
            num_threads: 1,
            endpoint_rules: EndpointRules::default(),
        }
    }
}

/// Streaming transducer backed by sherpa-onnx. Implements
/// [`StreamingTranscriber`].
///
/// Field declaration order matters for `Drop`: `stream` must precede
/// `recognizer` so the stream is destroyed before the recognizer that
/// minted it.
pub struct SherpaStreamingTranscriber {
    stream: Option<OnlineStream>,
    recognizer: OnlineRecognizer,
    /// Last hypothesis text. Sherpa emits the same text on consecutive
    /// polls between decodes; comparing the new text against this lets us
    /// skip emitting redundant Partials downstream.
    last_partial_text: String,
}

impl SherpaStreamingTranscriber {
    pub fn load(bundle: &ModelBundle, config: &StreamingConfig) -> Result<Self, AsrError> {
        bundle.validate()?;

        let encoder = path_string(&bundle.encoder)?;
        let decoder = path_string(&bundle.decoder)?;
        let joiner = path_string(&bundle.joiner)?;
        let tokens = path_string(&bundle.tokens)?;

        let mut rec_config = OnlineRecognizerConfig::default();
        rec_config.feat_config.sample_rate = SAMPLE_RATE_HZ_I32;
        rec_config.feat_config.feature_dim = 80;
        rec_config.model_config.transducer.encoder = Some(encoder);
        rec_config.model_config.transducer.decoder = Some(decoder);
        rec_config.model_config.transducer.joiner = Some(joiner);
        rec_config.model_config.tokens = Some(tokens);
        rec_config.model_config.provider = Some(config.provider.clone());
        rec_config.model_config.num_threads = config.num_threads;
        // model_type left None: sherpa-onnx reads it from the encoder ONNX
        // metadata and auto-routes to the Nemo or standard transducer impl
        // based on the decoder's output count.
        rec_config.decoding_method = Some(DECODING_METHOD.to_string());
        rec_config.max_active_paths = 4;
        rec_config.enable_endpoint = true;
        rec_config.rule1_min_trailing_silence = config.endpoint_rules.rule1_min_trailing_silence;
        rec_config.rule2_min_trailing_silence = config.endpoint_rules.rule2_min_trailing_silence;
        rec_config.rule3_min_utterance_length = config.endpoint_rules.rule3_max_utterance_seconds;

        let recognizer = OnlineRecognizer::create(&rec_config).ok_or_else(|| {
            AsrError::Load(
                "OnlineRecognizer::create returned None (check model paths and provider)"
                    .to_string(),
            )
        })?;

        let stream = recognizer.create_stream();
        Ok(Self { stream: Some(stream), recognizer, last_partial_text: String::new() })
    }

    fn stream_ref(&self) -> Result<&OnlineStream, AsrError> {
        self.stream.as_ref().ok_or(AsrError::NotLoaded)
    }

    fn poll_once(&mut self) -> Result<StreamingUpdate, AsrError> {
        let stream = self.stream_ref()?;

        let mut decoded = false;
        while self.recognizer.is_ready(stream) {
            self.recognizer.decode(stream);
            decoded = true;
        }

        let is_endpoint = self.recognizer.is_endpoint(stream);

        if is_endpoint {
            let text = self.recognizer.get_result(stream).map(|r| r.text).unwrap_or_default();
            log::debug!("sherpa endpoint: {text}");
            self.recognizer.reset(stream);
            self.last_partial_text.clear();
            return Ok(StreamingUpdate::Endpoint(text));
        }

        if !decoded {
            return Ok(StreamingUpdate::Idle);
        }

        let text = self.recognizer.get_result(stream).map(|r| r.text).unwrap_or_default();
        if text == self.last_partial_text {
            Ok(StreamingUpdate::Idle)
        } else {
            log::debug!("sherpa partial: {text}");
            // Reuse the dedup cache's existing allocation via clone_from
            // (avoids dropping + allocating fresh each time the partial grows).
            self.last_partial_text.clone_from(&text);
            Ok(StreamingUpdate::Partial(text))
        }
    }
}

impl StreamingTranscriber for SherpaStreamingTranscriber {
    fn accept_waveform(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        if samples.is_empty() {
            return Ok(());
        }
        let stream = self.stream_ref()?;
        stream.accept_waveform(SAMPLE_RATE_HZ_I32, samples);
        Ok(())
    }

    fn poll(&mut self) -> Result<StreamingUpdate, AsrError> {
        self.poll_once()
    }

    fn input_finished(&mut self) -> Result<(), AsrError> {
        let stream = self.stream_ref()?;
        stream.input_finished();
        Ok(())
    }

    fn reset(&mut self) -> Result<(), AsrError> {
        // Destroy + recreate the stream rather than calling
        // recognizer.reset(): also flushes any feature frames the previous
        // stream had queued, matching the behavior we relied on previously.
        self.stream = None;
        self.stream = Some(self.recognizer.create_stream());
        self.last_partial_text.clear();
        Ok(())
    }
}

fn path_string(p: &Path) -> Result<String, AsrError> {
    p.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| AsrError::Load(format!("non-UTF-8 path: {}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn from_dir_falls_back_to_plain_onnx_when_no_int8() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("encoder.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("decoder.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("joiner.onnx"), b"stub").unwrap();
        let b = ModelBundle::from_dir(dir.path());
        assert_eq!(b.encoder, dir.path().join("encoder.onnx"));
        assert_eq!(b.tokens, dir.path().join("tokens.txt"));
    }

    #[test]
    fn from_dir_picks_int8_when_present() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("encoder.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("encoder.int8.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("decoder.int8.onnx"), b"stub").unwrap();
        fs::write(dir.path().join("joiner.int8.onnx"), b"stub").unwrap();
        let b = ModelBundle::from_dir(dir.path());
        assert_eq!(b.encoder, dir.path().join("encoder.int8.onnx"));
        assert_eq!(b.decoder, dir.path().join("decoder.int8.onnx"));
        assert_eq!(b.joiner, dir.path().join("joiner.int8.onnx"));
    }

    #[test]
    fn from_dir_matches_long_filenames() {
        // k2-fsa publishes models with descriptive filenames like
        // `encoder-epoch-99-avg-1-chunk-16-left-128.onnx`.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("encoder-epoch-99-avg-1-chunk-16-left-128.onnx"), b"stub")
            .unwrap();
        fs::write(dir.path().join("decoder-epoch-99-avg-1-chunk-16-left-128.onnx"), b"stub")
            .unwrap();
        fs::write(dir.path().join("joiner-epoch-99-avg-1-chunk-16-left-128.onnx"), b"stub")
            .unwrap();
        let b = ModelBundle::from_dir(dir.path());
        assert!(b.encoder.file_name().unwrap().to_str().unwrap().starts_with("encoder-"));
        assert!(b.decoder.file_name().unwrap().to_str().unwrap().starts_with("decoder-"));
        assert!(b.joiner.file_name().unwrap().to_str().unwrap().starts_with("joiner-"));
    }

    #[test]
    fn validate_complains_about_missing_files() {
        let dir = tempdir().unwrap();
        let b = ModelBundle::from_dir(dir.path());
        let err = b.validate().unwrap_err();
        assert!(matches!(err, AsrError::Load(_)));
    }

    #[test]
    fn validate_passes_with_all_files_present() {
        let dir = tempdir().unwrap();
        for name in [
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ] {
            fs::write(dir.path().join(name), b"stub").unwrap();
        }
        let b = ModelBundle::from_dir(dir.path());
        b.validate().unwrap();
    }
}
