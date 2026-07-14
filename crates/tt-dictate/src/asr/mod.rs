//! ASR bounded context — engines that turn audio chunks into transcripts.
//!
//! [`driver`] holds the engine-agnostic [`StreamingDriver`] that turns
//! frame-level recognizer updates into a cumulative transcript. [`sherpa`]
//! (gated behind `--features asr`) is the production backend.
//!
//! Ported from scribed `src/asr/mod.rs`.

use thiserror::Error;

pub mod download;
pub mod driver;

#[cfg(feature = "asr")]
pub mod sherpa;

pub use driver::{StreamingDriver, StreamingTranscriber, StreamingUpdate};

/// Tuning knobs for the streaming recognizer's rule-based endpoint detector.
///
/// Lives at module root (not under `sherpa`) so [`crate::settings::EngineConfig`]
/// can build it without depending on the `asr` feature.
#[derive(Debug, Clone, Copy)]
pub struct EndpointRules {
    /// Seconds of trailing silence required to fire an endpoint when nothing
    /// has been decoded yet. Catches "user opened the mic then walked away".
    pub rule1_min_trailing_silence: f32,
    /// Seconds of trailing silence required after non-blank tokens have been
    /// decoded. The main "user paused at the end of a sentence" trigger.
    pub rule2_min_trailing_silence: f32,
    /// Hard ceiling on a single utterance in seconds. Forces an endpoint when
    /// speech exceeds this length. (Sherpa names the C-API field
    /// `rule3_min_utterance_length`, with "min" meaning "min length to *fire*";
    /// the user-visible meaning is a max.)
    pub rule3_max_utterance_seconds: f32,
}

impl Default for EndpointRules {
    fn default() -> Self {
        Self {
            rule1_min_trailing_silence: 2.4,
            rule2_min_trailing_silence: 1.0,
            rule3_max_utterance_seconds: 20.0,
        }
    }
}

/// A committed transcript fragment. Produced when the streaming recognizer
/// signals an endpoint; immutable thereafter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
}

/// The full transcript visible to the user for the current recording session.
/// Equal to `committed.join(" ")` + `" " + live_tail` (when non-empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    pub committed: Vec<Segment>,
    pub live_tail: String,
}

impl Transcript {
    /// Render the transcript as a single string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (i, seg) in self.committed.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&seg.text);
        }
        if !self.live_tail.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&self.live_tail);
        }
        out
    }
}

#[derive(Debug, Error)]
pub enum AsrError {
    #[error("model load failed: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("model not yet loaded")]
    NotLoaded,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_transcript_renders_empty() {
        let t = Transcript::default();
        assert_eq!(t.render(), "");
    }

    #[test]
    fn segments_only() {
        let t = Transcript {
            committed: vec![
                Segment { text: "hello".into() },
                Segment { text: "world".into() },
            ],
            live_tail: String::new(),
        };
        assert_eq!(t.render(), "hello world");
    }

    #[test]
    fn segments_plus_tail() {
        let t = Transcript {
            committed: vec![Segment { text: "hello".into() }],
            live_tail: "there friend".into(),
        };
        assert_eq!(t.render(), "hello there friend");
    }

    #[test]
    fn tail_only() {
        let t = Transcript { committed: vec![], live_tail: "fresh".into() };
        assert_eq!(t.render(), "fresh");
    }
}
