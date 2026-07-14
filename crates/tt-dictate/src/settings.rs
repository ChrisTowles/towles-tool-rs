//! Engine configuration, built from [`tt_config::DictationSettings`].
//!
//! Ported from scribed `src/config.rs`'s `Config::sanitize` — the clamping
//! logic survives, but the TOML load/save machinery and the fields scribed
//! dropped for app-only use (hotkey, mode, output_mode, excluded_apps,
//! soft_newlines, sound_effects) are gone. `chunk_ms` is no longer
//! user-tunable; it's the [`CHUNK_MS`] constant.

use tt_config::DictationSettings;

use crate::asr::EndpointRules;

/// Audio chunk size fed to the streaming recognizer, in milliseconds.
/// 120ms is scribed's tuned middle ground between decode latency and FFI
/// overhead; no longer exposed as a setting.
pub const CHUNK_MS: u32 = 120;

/// Sanitized engine tuning, ready to hand to [`crate::engine::DictationEngine`].
/// Built only via [`EngineConfig::from_settings`], which clamps every
/// numeric field to a safe range — a user's typo in settings.json must never
/// crash the engine or lock up a recording session.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    pub input_device: String,
    pub max_recording_seconds: u32,
    pub silence_auto_stop_seconds: u32,
    pub silence_threshold_dbfs: f32,
    pub endpoint_rule1_silence_seconds: f32,
    pub endpoint_rule2_silence_seconds: f32,
    pub endpoint_rule3_max_utterance_seconds: f32,
}

impl EngineConfig {
    pub fn from_settings(settings: &DictationSettings) -> Self {
        Self {
            input_device: settings.input_device.clone(),
            max_recording_seconds: settings.max_recording_seconds.clamp(10, 3600),
            silence_auto_stop_seconds: settings.silence_auto_stop_seconds.min(3600),
            silence_threshold_dbfs: clamp_f32(settings.silence_threshold_dbfs, -90.0, 0.0),
            endpoint_rule1_silence_seconds: clamp_f32(
                settings.endpoint_rule1_silence_seconds,
                0.1,
                60.0,
            ),
            endpoint_rule2_silence_seconds: clamp_f32(
                settings.endpoint_rule2_silence_seconds,
                0.1,
                60.0,
            ),
            endpoint_rule3_max_utterance_seconds: clamp_f32(
                settings.endpoint_rule3_max_utterance_seconds,
                5.0,
                600.0,
            ),
        }
    }

    /// Convenience: the chunk size in samples at `sample_rate_hz`.
    pub fn chunk_samples(&self, sample_rate_hz: u32) -> usize {
        (CHUNK_MS as f32 / 1000.0 * sample_rate_hz as f32) as usize
    }

    pub fn endpoint_rules(&self) -> EndpointRules {
        EndpointRules {
            rule1_min_trailing_silence: self.endpoint_rule1_silence_seconds,
            rule2_min_trailing_silence: self.endpoint_rule2_silence_seconds,
            rule3_max_utterance_seconds: self.endpoint_rule3_max_utterance_seconds,
        }
    }
}

fn clamp_f32(value: f32, min: f32, max: f32) -> f32 {
    if value.is_nan() {
        return min;
    }
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_sanitize_unchanged() {
        let settings = DictationSettings::default();
        let cfg = EngineConfig::from_settings(&settings);
        assert_eq!(cfg.max_recording_seconds, 300);
        assert_eq!(cfg.silence_auto_stop_seconds, 60);
        assert_eq!(cfg.silence_threshold_dbfs, -60.0);
        assert_eq!(cfg.endpoint_rule1_silence_seconds, 2.4);
        assert_eq!(cfg.endpoint_rule2_silence_seconds, 1.0);
        assert_eq!(cfg.endpoint_rule3_max_utterance_seconds, 20.0);
    }

    #[test]
    fn sanitize_clamps_out_of_range_values() {
        let settings = DictationSettings {
            endpoint_rule1_silence_seconds: -1.0,
            endpoint_rule2_silence_seconds: 9999.0,
            endpoint_rule3_max_utterance_seconds: 0.0,
            max_recording_seconds: 0,
            ..Default::default()
        };
        let cfg = EngineConfig::from_settings(&settings);
        assert_eq!(cfg.endpoint_rule1_silence_seconds, 0.1);
        assert_eq!(cfg.endpoint_rule2_silence_seconds, 60.0);
        assert_eq!(cfg.endpoint_rule3_max_utterance_seconds, 5.0);
        assert_eq!(cfg.max_recording_seconds, 10);
    }

    #[test]
    fn sanitize_clamps_silence_threshold_dbfs() {
        let too_low = EngineConfig::from_settings(&DictationSettings {
            silence_threshold_dbfs: -500.0,
            ..Default::default()
        });
        assert_eq!(too_low.silence_threshold_dbfs, -90.0);

        let too_high = EngineConfig::from_settings(&DictationSettings {
            silence_threshold_dbfs: 12.0,
            ..Default::default()
        });
        assert_eq!(too_high.silence_threshold_dbfs, 0.0);

        let nan = EngineConfig::from_settings(&DictationSettings {
            silence_threshold_dbfs: f32::NAN,
            ..Default::default()
        });
        assert_eq!(nan.silence_threshold_dbfs, -90.0);
    }

    #[test]
    fn sanitize_handles_nan_on_every_endpoint_rule() {
        let cfg = EngineConfig::from_settings(&DictationSettings {
            endpoint_rule1_silence_seconds: f32::NAN,
            endpoint_rule2_silence_seconds: f32::NAN,
            endpoint_rule3_max_utterance_seconds: f32::NAN,
            ..Default::default()
        });
        assert_eq!(cfg.endpoint_rule1_silence_seconds, 0.1, "rule1 floor");
        assert_eq!(cfg.endpoint_rule2_silence_seconds, 0.1, "rule2 floor");
        assert_eq!(cfg.endpoint_rule3_max_utterance_seconds, 5.0, "rule3 floor");
    }

    #[test]
    fn chunk_samples_uses_chunk_ms_constant() {
        let cfg = EngineConfig::from_settings(&DictationSettings::default());
        // 120 ms * 16 kHz = 1920 samples.
        assert_eq!(cfg.chunk_samples(16_000), 1_920);
    }

    #[test]
    fn endpoint_rules_round_trip_to_engine_struct() {
        let cfg = EngineConfig::from_settings(&DictationSettings {
            endpoint_rule1_silence_seconds: 3.0,
            endpoint_rule2_silence_seconds: 0.7,
            endpoint_rule3_max_utterance_seconds: 30.0,
            ..Default::default()
        });
        let rules = cfg.endpoint_rules();
        assert_eq!(rules.rule1_min_trailing_silence, 3.0);
        assert_eq!(rules.rule2_min_trailing_silence, 0.7);
        assert_eq!(rules.rule3_max_utterance_seconds, 30.0);
    }
}
