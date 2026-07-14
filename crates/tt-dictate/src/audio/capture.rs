//! cpal-backed microphone capture. Ported from scribed `src/audio/capture.rs`.
//!
//! The capture stream pushes [`AudioChunk`] values onto a [`crossbeam_channel`]
//! receiver, decoupling the real-time audio callback from the inference loop.
//! Whatever input format the device offers, we convert it to mono `f32` at
//! 16 kHz before sending. Conversion is naive nearest-neighbour resampling —
//! adequate for ASR input where small fidelity loss is invisible.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::Sender;
use parking_lot::Mutex;

use super::device::ResolvedInput;
use super::{AudioError, SAMPLE_RATE_HZ};

/// A contiguous slice of mono f32 frames at [`SAMPLE_RATE_HZ`].
pub type AudioChunk = Vec<f32>;

/// Owns a live cpal stream. When dropped, the stream stops.
///
/// `error_flag` is set by the cpal error callback whenever the underlying
/// device reports a fatal condition (USB unplug, sample-rate mismatch, ALSA
/// xrun cascade). The session loop polls it so a dead capture stream stops
/// the session promptly instead of spinning on `recv_timeout` until cpal
/// eventually drops the sender.
pub struct CaptureStream {
    _stream: cpal::Stream,
    pub device_name: String,
    pub native_sample_rate: u32,
    pub native_channels: u16,
    error_flag: Arc<AtomicBool>,
}

impl CaptureStream {
    /// True if cpal's error callback has reported a fatal condition. Sticky
    /// once set — caller should drop the stream and rebuild.
    pub fn errored(&self) -> bool {
        self.error_flag.load(Ordering::SeqCst)
    }
}

/// Start a capture stream. Audio frames are accumulated until at least
/// `chunk_samples` (at 16 kHz mono) are buffered, then sent as one chunk over
/// `tx`. The accumulator is owned by a `Mutex<Vec<f32>>` shared with the cpal
/// callback.
pub fn start(
    input: ResolvedInput,
    chunk_samples: usize,
    tx: Sender<AudioChunk>,
) -> Result<CaptureStream, AudioError> {
    let device = input.device;
    let config = device.default_input_config().map_err(|e| AudioError::Cpal(e.to_string()))?;
    let native_sample_rate = config.sample_rate().0;
    let native_channels = config.channels();
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let accumulator: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(chunk_samples)));
    let error_flag = Arc::new(AtomicBool::new(false));
    let err_flag_for_cb = error_flag.clone();
    let err_fn = move |err| {
        log::error!("cpal stream error: {err}");
        err_flag_for_cb.store(true, Ordering::SeqCst);
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let acc = accumulator.clone();
            let tx = tx.clone();
            let in_rate = native_sample_rate;
            let in_chans = native_channels;
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| {
                        ingest::<f32>(data, in_chans, in_rate, &acc, chunk_samples, &tx)
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Cpal(e.to_string()))?
        }
        cpal::SampleFormat::I16 => {
            let acc = accumulator.clone();
            let tx = tx.clone();
            let in_rate = native_sample_rate;
            let in_chans = native_channels;
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        ingest::<i16>(data, in_chans, in_rate, &acc, chunk_samples, &tx)
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Cpal(e.to_string()))?
        }
        cpal::SampleFormat::U16 => {
            let acc = accumulator.clone();
            let tx = tx.clone();
            let in_rate = native_sample_rate;
            let in_chans = native_channels;
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        ingest::<u16>(data, in_chans, in_rate, &acc, chunk_samples, &tx)
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Cpal(e.to_string()))?
        }
        other => return Err(AudioError::Cpal(format!("unsupported sample format: {other:?}"))),
    };

    stream.play().map_err(|e| AudioError::Cpal(e.to_string()))?;
    Ok(CaptureStream {
        _stream: stream,
        device_name: input.name,
        native_sample_rate,
        native_channels,
        error_flag,
    })
}

/// Marker trait for sample types we know how to convert to mono f32.
trait IntoMonoF32: Copy {
    fn to_f32(self) -> f32;
}

impl IntoMonoF32 for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}

impl IntoMonoF32 for i16 {
    fn to_f32(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
}

impl IntoMonoF32 for u16 {
    fn to_f32(self) -> f32 {
        (self as f32 - u16::MAX as f32 / 2.0) / (u16::MAX as f32 / 2.0)
    }
}

fn ingest<S: IntoMonoF32>(
    data: &[S],
    in_channels: u16,
    in_rate: u32,
    accumulator: &Arc<Mutex<Vec<f32>>>,
    chunk_samples: usize,
    tx: &Sender<AudioChunk>,
) {
    let mono = to_mono_f32(data, in_channels);
    let resampled = if in_rate == SAMPLE_RATE_HZ { mono } else { resample_to_16k(&mono, in_rate) };

    let mut acc = accumulator.lock();
    acc.extend_from_slice(&resampled);
    while acc.len() >= chunk_samples {
        let chunk: Vec<f32> = acc.drain(..chunk_samples).collect();
        // try_send avoids blocking the real-time thread if the consumer is slow.
        if tx.try_send(chunk).is_err() {
            log::warn!("audio: dropping chunk — consumer is not keeping up");
        }
    }
}

/// Average channels into a mono stream.
fn to_mono_f32<S: IntoMonoF32>(data: &[S], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return data.iter().map(|s| s.to_f32()).collect();
    }
    let chans = channels as usize;
    let frames = data.len() / chans;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * chans;
        let sum: f32 = data[start..start + chans].iter().map(|s| s.to_f32()).sum();
        out.push(sum / chans as f32);
    }
    out
}

/// Nearest-neighbour resample to 16 kHz. Good enough for ASR input — for very
/// high sample rates (96 kHz) the alias is in inaudible bands the encoder
/// ignores. Replace with a proper FIR or `rubato` resampler if quality becomes
/// an issue.
fn resample_to_16k(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == SAMPLE_RATE_HZ {
        return samples.to_vec();
    }
    let ratio = SAMPLE_RATE_HZ as f64 / from_rate as f64;
    let out_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = (i as f64 / ratio).round() as usize;
        let src = src.min(samples.len() - 1);
        out.push(samples[src]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_passthrough() {
        let data: Vec<f32> = vec![0.1, 0.2, 0.3];
        assert_eq!(to_mono_f32(&data, 1), data);
    }

    #[test]
    fn stereo_averaged_to_mono() {
        let data: Vec<f32> = vec![0.0, 1.0, 0.5, 0.5];
        let mono = to_mono_f32(&data, 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }

    #[test]
    fn i16_full_scale_maps_to_one() {
        let s: i16 = i16::MAX;
        assert!((s.to_f32() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn u16_midpoint_maps_to_zero() {
        let s: u16 = u16::MAX / 2 + 1;
        assert!(s.to_f32().abs() < 1e-3);
    }

    #[test]
    fn resample_48k_to_16k_drops_two_thirds() {
        let from = 48_000u32;
        let samples: Vec<f32> = (0..300).map(|i| i as f32).collect();
        let out = resample_to_16k(&samples, from);
        // 300 * 16/48 = 100
        assert_eq!(out.len(), 100);
    }

    #[test]
    fn resample_16k_is_identity() {
        let samples: Vec<f32> = vec![0.1, 0.2, 0.3];
        let out = resample_to_16k(&samples, SAMPLE_RATE_HZ);
        assert_eq!(out, samples);
    }
}
