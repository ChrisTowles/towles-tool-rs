//! Per-session recording engine. Ported from scribed `src/service/runtime.rs`
//! (`Runtime` → [`DictationEngine`]), reworked from "diff transcript into a
//! `KeyboardSink`" onto "hand full transcript snapshots to an event sink" —
//! diffing now happens in the app frontend (`dictation-retype.ts`), since the
//! three in-app targets (webview input, terminal, panel) each need different
//! apply semantics that don't belong in this Tauri-free crate.
//!
//! [`DictationEngine::load`] loads the model once (blocks for a few seconds);
//! [`DictationEngine::start_session`] / [`stop_session`] toggle recording on a
//! dedicated OS thread per session.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::asr::sherpa::{ModelBundle, SherpaStreamingTranscriber, StreamingConfig};
use crate::asr::{AsrError, StreamingDriver, StreamingTranscriber, Transcript};
use crate::audio::dsp::rms_dbfs;
use crate::audio::{self, AudioChunk, SAMPLE_RATE_HZ};
use crate::settings::EngineConfig;

/// Minimum wall-clock gap between successive sink notifications for partial
/// updates. Coalesces the rapid `Partial(_)` updates streaming RNN-T
/// produces (often several per audio chunk) into one UI update.
const PARTIAL_DEBOUNCE: Duration = Duration::from_millis(100);

/// Minimum wall-clock gap between mic-level sink notifications.
const LEVEL_THROTTLE: Duration = Duration::from_millis(100);

/// Abort the session after this many consecutive FFI failures in a row.
/// Prevents an error storm if the recognizer ends up in a permanently bad
/// state (e.g. dropped GPU context).
const MAX_CONSECUTIVE_INGEST_ERRORS: u32 = 50;

type SharedTranscriber = Arc<Mutex<dyn StreamingTranscriber + Send>>;
type EventSink = Arc<dyn Fn(DictationEvent) + Send + Sync>;

/// Why a session stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `stop_session()` was called.
    Requested,
    /// `max_recording_seconds` elapsed.
    MaxRecording,
    /// `silence_auto_stop_seconds` elapsed without a new committed segment.
    SilenceAutoStop,
    /// The cpal capture stream reported a fatal error.
    CaptureError,
    /// Too many consecutive recognizer failures.
    IngestErrors,
}

/// An event pushed to the session's sink. Delivered from the session thread —
/// the sink must be cheap and non-blocking (e.g. push onto a channel or emit
/// a Tauri event).
#[derive(Debug, Clone)]
pub enum DictationEvent {
    /// A committed/live-tail transcript snapshot changed.
    Transcript(Transcript),
    /// Throttled mic level, in dBFS.
    Level { dbfs: f32 },
    /// The session ended.
    Stopped {
        reason: StopReason,
        final_text: String,
    },
}

struct SessionHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

/// Snapshot of the user-tunable durations that govern a single session.
#[derive(Debug, Clone, Copy)]
struct SessionLimits {
    /// Hard ceiling on wall-clock recording time. `ZERO` disables.
    max_recording: Duration,
    /// Stop after this much wall-clock time without a new committed segment.
    /// `ZERO` disables.
    silence_auto_stop: Duration,
}

impl SessionLimits {
    fn from_config(c: &EngineConfig) -> Self {
        Self {
            max_recording: Duration::from_secs(c.max_recording_seconds as u64),
            silence_auto_stop: Duration::from_secs(c.silence_auto_stop_seconds as u64),
        }
    }
}

/// Everything the session thread needs to run one recording. Bundling these
/// keeps the thread-spawn site readable as the recognizer grows knobs.
struct SessionParams {
    input_device: String,
    chunk_samples: usize,
    silence_threshold_dbfs: f32,
    limits: SessionLimits,
    stop: Arc<AtomicBool>,
    transcriber: SharedTranscriber,
    sink: EventSink,
}

pub struct DictationEngine {
    transcriber: SharedTranscriber,
    config: EngineConfig,
    current_session: Option<SessionHandle>,
}

impl DictationEngine {
    /// Load the ASR model. Blocks for a few seconds on first call.
    pub fn load(cfg: EngineConfig, model_dir: &std::path::Path) -> Result<Self, AsrError> {
        let bundle = ModelBundle::from_dir(model_dir);
        let streaming_cfg =
            StreamingConfig { endpoint_rules: cfg.endpoint_rules(), ..StreamingConfig::default() };
        let t = Instant::now();
        let transcriber = SherpaStreamingTranscriber::load(&bundle, &streaming_cfg)?;
        log::info!("asr model loaded in {:?} from {}", t.elapsed(), model_dir.display());

        Ok(Self {
            transcriber: Arc::new(Mutex::new(transcriber)),
            config: cfg,
            current_session: None,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// True if a session thread is currently active. Reaps finished handles
    /// as a side effect so a session that died on its own (capture error,
    /// FFI panic ride-out, etc.) doesn't block future starts.
    pub fn is_recording(&mut self) -> bool {
        if let Some(handle) = self.current_session.as_mut() {
            if handle.join.as_ref().is_none_or(|j| j.is_finished()) {
                if let Some(j) = handle.join.take() {
                    let _ = j.join();
                }
                self.current_session = None;
                return false;
            }
            return true;
        }
        false
    }

    /// Idempotent: a second call while a session is active is a no-op.
    /// `sink` is called from the session thread for every event; it must not
    /// block.
    pub fn start_session(
        &mut self,
        sink: impl Fn(DictationEvent) + Send + Sync + 'static,
    ) -> Result<(), AsrError> {
        if self.is_recording() {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let params = SessionParams {
            input_device: self.config.input_device.clone(),
            chunk_samples: self.config.chunk_samples(SAMPLE_RATE_HZ),
            silence_threshold_dbfs: self.config.silence_threshold_dbfs,
            limits: SessionLimits::from_config(&self.config),
            stop: stop.clone(),
            transcriber: self.transcriber.clone(),
            sink: Arc::new(sink),
        };
        let join = thread::Builder::new()
            .name("tt-dictate-session".into())
            .spawn(move || record_and_stream(params))
            .map_err(|e| AsrError::Load(format!("failed to spawn session thread: {e}")))?;
        self.current_session = Some(SessionHandle { stop, join: Some(join) });
        Ok(())
    }

    /// Signal the in-flight session to wind down and wait briefly for it to
    /// finish. Returns whether the thread actually joined within the grace
    /// period; on timeout the handle is dropped (best-effort cleanup) and a
    /// future `is_recording()` call will reap it once it does finish.
    pub fn stop_session(&mut self) -> bool {
        let Some(mut handle) = self.current_session.take() else {
            return true;
        };
        handle.stop.store(true, Ordering::SeqCst);
        let Some(join) = handle.join.take() else {
            return true;
        };
        // Brief poll loop — finalize() + sink notification usually completes
        // in <50ms. Anything longer is unusual; detach rather than block the
        // caller (typically a Tauri command handler) forever.
        let deadline = Instant::now() + Duration::from_millis(500);
        while !join.is_finished() {
            if Instant::now() >= deadline {
                log::warn!("session thread did not finish within 500ms; detaching");
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = join.join();
        true
    }
}

fn record_and_stream(params: SessionParams) {
    let SessionParams {
        input_device,
        chunk_samples,
        silence_threshold_dbfs,
        limits,
        stop,
        transcriber,
        sink,
    } = params;
    let input = match audio::resolve_device(&input_device) {
        Ok(i) => i,
        Err(e) => {
            log::error!("input device resolve failed: {e}");
            return;
        }
    };
    let device_name = input.name.clone();
    let (tx, rx) = crossbeam_channel::bounded::<AudioChunk>(64);
    let stream = match audio::capture::start(input, chunk_samples, tx) {
        Ok(s) => s,
        Err(e) => {
            log::error!("audio capture start failed: {e}");
            return;
        }
    };
    log::info!(
        "recording started: device={device_name} native_rate={} native_channels={}",
        stream.native_sample_rate,
        stream.native_channels
    );

    // Each session gets a fresh decoder state so a previous session's
    // dangling tokens never bleed into the new one.
    if let Err(e) = transcriber.lock().reset() {
        log::error!("transcriber reset failed: {e}");
        return;
    }

    let mut driver = StreamingDriver::new();
    let mut pending: Option<Transcript> = None;
    let mut last_transcript_sent: Option<Instant> = None;
    let mut last_level_sent: Option<Instant> = None;
    let mut consecutive_errors: u32 = 0;
    let session_started = Instant::now();
    let mut last_committed_change = session_started;
    let mut last_committed_count: usize = 0;
    let mut reason = StopReason::Requested;

    loop {
        if stop.load(Ordering::SeqCst) {
            reason = StopReason::Requested;
            break;
        }
        if stream.errored() {
            log::error!("cpal capture stream reported a fatal error; ending session");
            reason = StopReason::CaptureError;
            break;
        }
        if limits.max_recording > Duration::ZERO
            && session_started.elapsed() >= limits.max_recording
        {
            log::info!("max_recording_seconds ({:?}) reached, stopping", limits.max_recording);
            reason = StopReason::MaxRecording;
            break;
        }
        if limits.silence_auto_stop > Duration::ZERO
            && last_committed_change.elapsed() >= limits.silence_auto_stop
        {
            log::info!(
                "silence_auto_stop_seconds ({:?}) reached, stopping",
                limits.silence_auto_stop
            );
            reason = StopReason::SilenceAutoStop;
            break;
        }

        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                if last_level_sent.is_none_or(|i| i.elapsed() >= LEVEL_THROTTLE) {
                    sink(DictationEvent::Level { dbfs: rms_dbfs(&chunk) });
                    last_level_sent = Some(Instant::now());
                }
                match gate_and_ingest(chunk, silence_threshold_dbfs, &mut driver, &transcriber) {
                    Ok(Some(t)) => {
                        consecutive_errors = 0;
                        if t.committed.len() != last_committed_count {
                            last_committed_count = t.committed.len();
                            last_committed_change = Instant::now();
                        }
                        pending = Some(t);
                    }
                    Ok(None) => {
                        consecutive_errors = 0;
                    }
                    Err(()) => {
                        consecutive_errors += 1;
                        if consecutive_errors >= MAX_CONSECUTIVE_INGEST_ERRORS {
                            log::error!(
                                "consecutive ingest errors ({consecutive_errors}) exceeded threshold; aborting session"
                            );
                            reason = StopReason::IngestErrors;
                            break;
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
        if pending.is_some() && last_transcript_sent.is_none_or(|i| i.elapsed() >= PARTIAL_DEBOUNCE)
        {
            sink(DictationEvent::Transcript(pending.take().unwrap()));
            last_transcript_sent = Some(Instant::now());
        }
    }

    drop(stream);

    // Drain any chunks that landed between the stop signal and stream
    // teardown. We discard `pending` because finalize() will produce the
    // authoritative end-of-session transcript anyway.
    while let Ok(chunk) = rx.try_recv() {
        let _ = gate_and_ingest(chunk, silence_threshold_dbfs, &mut driver, &transcriber);
    }

    let mut tr = transcriber.lock();
    let outcome = driver.finalize(&mut *tr);
    drop(tr);
    let final_text = match outcome {
        Ok(final_transcript) => {
            let text = final_transcript.render();
            sink(DictationEvent::Transcript(final_transcript));
            log::info!("session finalized: {text:?}");
            text
        }
        Err(e) => {
            log::error!("finalize failed: {e}");
            String::new()
        }
    };
    sink(DictationEvent::Stopped { reason, final_text });
}

/// Pre-recognizer noise gate. Zero-fills the chunk if its RMS sits below
/// `threshold_dbfs`. Feeding zeros (rather than skipping the chunk) keeps
/// sherpa-onnx's feature stream advancing in lockstep with wall-clock time,
/// so the internal endpoint detector still counts trailing silence correctly.
fn gate_chunk(chunk: &mut [f32], threshold_dbfs: f32) {
    if rms_dbfs(chunk) < threshold_dbfs {
        chunk.fill(0.0);
    }
}

/// Apply the noise gate to `chunk` and feed it to the recognizer. Used in
/// both the main session loop and the post-stop drain — keeps the
/// gate-before-ingest contract in one place.
fn gate_and_ingest(
    mut chunk: AudioChunk,
    threshold_dbfs: f32,
    driver: &mut StreamingDriver,
    transcriber: &SharedTranscriber,
) -> Result<Option<Transcript>, ()> {
    gate_chunk(&mut chunk, threshold_dbfs);
    ingest_chunk(&chunk, driver, transcriber)
}

/// Returns `Ok(Some(t))` if the transcript changed, `Ok(None)` for a clean
/// no-change ingest, `Err(())` if the recognizer failed. The Err signals the
/// caller to increment its consecutive-error counter.
fn ingest_chunk(
    chunk: &[f32],
    driver: &mut StreamingDriver,
    transcriber: &SharedTranscriber,
) -> Result<Option<Transcript>, ()> {
    let mut tr = transcriber.lock();
    match driver.ingest(chunk, &mut *tr) {
        Ok(maybe) => Ok(maybe),
        Err(e) => {
            log::error!("ingest failed: {e}");
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_zeros_sub_threshold_chunk() {
        // Half-amplitude DC sits at 0 dBFS-ish (actually -6), well above -28.
        // A very small constant signal is far below -28 and should be zeroed.
        let mut quiet = vec![0.001_f32; 1024];
        gate_chunk(&mut quiet, -28.0);
        assert!(quiet.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn gate_passes_above_threshold_chunk() {
        let mut loud = vec![0.5_f32; 1024];
        gate_chunk(&mut loud, -28.0);
        assert!(loud.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn gate_at_threshold_floor_passes_a_quiet_signal() {
        // 0.0001 amplitude → RMS ≈ -80 dBFS, which is above the -90 dBFS
        // floor, so even the floor-most threshold lets it through.
        let mut whisper = vec![0.0001_f32; 1024];
        gate_chunk(&mut whisper, -90.0);
        assert!(whisper.iter().all(|&s| s == 0.0001));
    }
}
