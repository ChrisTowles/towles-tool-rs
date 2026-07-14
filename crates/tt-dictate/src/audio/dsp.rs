//! DSP primitives. All operate on `&[f32]` slices in the range `[-1.0, 1.0]`.
//! Ported from scribed `src/audio/dsp.rs`.

/// RMS energy in decibels relative to full scale.
///
/// `0 dBFS` is a full-amplitude sine wave (RMS = 1/√2 ≈ 0.707).
/// Silence approaches `-∞ dBFS`; we add `1e-12` before the `log10` to guarantee
/// a finite floor (~-240 dBFS). Mirrors Python `claude_stt.engines._audio.rms_dbfs`.
pub fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -240.0;
    }
    let mean_sq: f64 =
        samples.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / samples.len() as f64;
    let rms = mean_sq.sqrt() + 1e-12;
    (20.0 * rms.log10()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_is_near_negative_infinity() {
        let zeros = vec![0.0_f32; 1024];
        let db = rms_dbfs(&zeros);
        assert!(db < -200.0, "got {db}");
    }

    #[test]
    fn rms_of_empty_slice_returns_floor() {
        assert_eq!(rms_dbfs(&[]), -240.0);
    }

    #[test]
    fn rms_of_full_scale_sine_is_near_zero_dbfs() {
        // A unit-amplitude sine wave has RMS = 1/sqrt(2) -> 20*log10(1/sqrt(2)) ≈ -3.01 dBFS.
        let n = 1024;
        let samples: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 100.0 * i as f32 / 16_000.0).sin())
            .collect();
        let db = rms_dbfs(&samples);
        assert!((db - (-3.01)).abs() < 0.2, "got {db}");
    }

    #[test]
    fn rms_of_dc_unit_is_zero_dbfs() {
        let samples = vec![1.0_f32; 1024];
        let db = rms_dbfs(&samples);
        assert!(db.abs() < 0.01, "got {db}");
    }

    #[test]
    fn rms_of_half_amplitude_is_neg_six_dbfs() {
        let samples = vec![0.5_f32; 1024];
        let db = rms_dbfs(&samples);
        // 20*log10(0.5) ≈ -6.02 dBFS
        assert!((db - (-6.02)).abs() < 0.05, "got {db}");
    }
}
