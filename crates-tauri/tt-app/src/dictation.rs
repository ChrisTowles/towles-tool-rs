//! Dictation: mic → sherpa-onnx streaming ASR → transcript events. Ported
//! from the standalone `scribed` daemon (see `crates/tt-dictate`), scoped to
//! app-only use — no daemon, no global hotkey, no OS-level typing. The
//! frontend (`src/lib/dictation.ts`) diffs transcript snapshots into one of
//! three targets (focused webview input, focused Agentboard terminal, or the
//! dictation panel); this module only loads the model, runs sessions, and
//! streams events.
//!
//! Concurrency contract mirrors `terminal.rs`: the [`DictationState`] lock is
//! only ever held for `Option` surgery — model load and session start/stop
//! happen with no lock held, then the result is installed.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tt_dictate::asr::download::{self, STREAMING_MODEL};
use tt_dictate::engine::{DictationEngine, DictationEvent, StopReason};
use tt_dictate::settings::EngineConfig;

pub const STATE_EVENT: &str = "dictation://state";
pub const TRANSCRIPT_EVENT: &str = "dictation://transcript";
pub const LEVEL_EVENT: &str = "dictation://level";
pub const MODEL_EVENT: &str = "dictation://model";
const MAIN_WINDOW_LABEL: &str = "main";

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub struct DictationState {
    engine: Mutex<Option<DictationEngine>>,
    /// True while a `dictation_start` model-load is in flight, so a second
    /// call while loading is a clean no-op rather than a racing double-load.
    loading: AtomicBool,
    /// True while `dictation_model_fetch` is downloading. Guards against a
    /// second fetch starting mid-download.
    model_fetch_running: AtomicBool,
    current_session_id: Mutex<Option<String>>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Idle,
    LoadingModel,
    Recording,
    Stopping,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatePayload {
    phase: Phase,
    session_id: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptPayload {
    session_id: String,
    seq: u64,
    committed: Vec<String>,
    live_tail: String,
    text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LevelPayload {
    dbfs: f32,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum ModelFetchPhase {
    Downloading,
    Done,
    Error,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelPayload {
    state: ModelFetchPhase,
    bytes_done: u64,
    bytes_total: u64,
    error: Option<String>,
}

fn emit_state(app: &AppHandle, phase: Phase, session_id: Option<String>, error: Option<String>) {
    let _ = app.emit_to(MAIN_WINDOW_LABEL, STATE_EVENT, StatePayload { phase, session_id, error });
}

/// `Some(dir)` if the streaming model bundle is already cached locally.
fn cached_model_dir() -> Result<Option<std::path::PathBuf>, String> {
    let dir = tt_config::models_cache_dir()
        .map_err(|e| e.to_string())?
        .join(STREAMING_MODEL.extracted_dir);
    Ok(dir.exists().then_some(dir))
}

/// Current recording status, for the panel/mic button's initial render.
#[tauri::command]
pub fn dictation_status(state: State<DictationState>) -> StatePayloadOut {
    let session_id = state.current_session_id.lock().unwrap().clone();
    let recording = {
        let mut engine = state.engine.lock().unwrap();
        engine.as_mut().is_some_and(DictationEngine::is_recording)
    };
    StatePayloadOut {
        phase: if recording {
            "recording"
        } else if state.loading.load(Ordering::SeqCst) {
            "loadingModel"
        } else {
            "idle"
        },
        session_id,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePayloadOut {
    phase: &'static str,
    session_id: Option<String>,
}

/// Whether the ASR model bundle is cached locally.
#[tauri::command]
pub fn dictation_model_status() -> Result<bool, String> {
    Ok(cached_model_dir()?.is_some())
}

/// Download the ASR model bundle (~442MB). Overlap-guarded: a second call
/// while a fetch is running returns immediately. Emits `dictation://model`
/// progress events.
#[tauri::command]
pub async fn dictation_model_fetch(app: AppHandle) -> Result<(), String> {
    if app.state::<DictationState>().model_fetch_running.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let blocking_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let app = blocking_app;
        let cache_dir = tt_config::models_cache_dir().map_err(|e| e.to_string())?;
        let progress_app = app.clone();
        let outcome = download::ensure(&STREAMING_MODEL, &cache_dir, |done, total| {
            let _ = progress_app.emit_to(
                MAIN_WINDOW_LABEL,
                MODEL_EVENT,
                ModelPayload {
                    state: ModelFetchPhase::Downloading,
                    bytes_done: done,
                    bytes_total: total,
                    error: None,
                },
            );
        });
        match outcome {
            Ok(_) => {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    MODEL_EVENT,
                    ModelPayload {
                        state: ModelFetchPhase::Done,
                        bytes_done: 0,
                        bytes_total: 0,
                        error: None,
                    },
                );
                Ok(())
            }
            Err(e) => {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    MODEL_EVENT,
                    ModelPayload {
                        state: ModelFetchPhase::Error,
                        bytes_done: 0,
                        bytes_total: 0,
                        error: Some(e.to_string()),
                    },
                );
                Err(e.to_string())
            }
        }
    })
    .await
    .map_err(|e| format!("model fetch task failed: {e}"))?;
    app.state::<DictationState>().model_fetch_running.store(false, Ordering::SeqCst);
    result
}

/// Enumerate cpal input device names, for the settings device picker.
#[tauri::command]
pub fn dictation_devices() -> Result<Vec<String>, String> {
    tt_dictate::audio::list_device_names().map_err(|e| e.to_string())
}

/// Start (or, if a session is already active, no-op) a recording session.
/// Loads the model on first call, or if settings changed since the last
/// load — both happen on a blocking task, never holding the state lock.
#[tauri::command]
pub async fn dictation_start(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || dictation_start_blocking(app))
        .await
        .map_err(|e| format!("dictation start task failed: {e}"))?
}

fn dictation_start_blocking(app: AppHandle) -> Result<(), String> {
    let state = app.state::<DictationState>();
    let settings = tt_config::load().map_err(|e| e.to_string())?;
    let cfg = EngineConfig::from_settings(&settings.dictation);

    // Reload the engine if it's missing or its config went stale (a settings
    // edit landed since the last load).
    let needs_load = {
        let engine = state.engine.lock().unwrap();
        engine.as_ref().is_none_or(|e| *e.config() != cfg)
    };
    if needs_load {
        // Resolve the model dir (a plain settings/filesystem check) before
        // flipping `loading` — a missing model must not leave the state
        // stuck in `loadingModel` forever.
        let model_dir = match cached_model_dir() {
            Ok(Some(dir)) => dir,
            Ok(None) => {
                let e = "dictation model not downloaded yet — call dictation_model_fetch first"
                    .to_string();
                emit_state(&app, Phase::Idle, None, Some(e.clone()));
                return Err(e);
            }
            Err(e) => {
                emit_state(&app, Phase::Idle, None, Some(e.clone()));
                return Err(e);
            }
        };
        state.loading.store(true, Ordering::SeqCst);
        emit_state(&app, Phase::LoadingModel, None, None);
        let loaded = DictationEngine::load(cfg, &model_dir).map_err(|e| e.to_string());
        state.loading.store(false, Ordering::SeqCst);
        match loaded {
            Ok(engine) => *state.engine.lock().unwrap() = Some(engine),
            Err(e) => {
                emit_state(&app, Phase::Idle, None, Some(e.clone()));
                return Err(e);
            }
        }
    }

    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed).to_string();
    *state.current_session_id.lock().unwrap() = Some(session_id.clone());

    let mut engine = state.engine.lock().unwrap();
    let seq = std::sync::Arc::new(AtomicU64::new(0));
    let sink_app = app.clone();
    let sink_session_id = session_id.clone();
    let start_result =
        engine.as_mut().expect("engine loaded above").start_session(move |event| match event {
            DictationEvent::Transcript(t) => {
                let n = seq.fetch_add(1, Ordering::Relaxed);
                let _ = sink_app.emit_to(
                    MAIN_WINDOW_LABEL,
                    TRANSCRIPT_EVENT,
                    TranscriptPayload {
                        session_id: sink_session_id.clone(),
                        seq: n,
                        committed: t.committed.iter().map(|s| s.text.clone()).collect(),
                        live_tail: t.live_tail.clone(),
                        text: t.render(),
                    },
                );
            }
            DictationEvent::Level { dbfs } => {
                let _ = sink_app.emit_to(MAIN_WINDOW_LABEL, LEVEL_EVENT, LevelPayload { dbfs });
            }
            DictationEvent::Stopped { reason, .. } => {
                // Engine-initiated stops (auto-stop, capture error) bypass
                // `dictation_stop`, so clear the session id here too or
                // `dictation_status` keeps reporting the dead session.
                let state = sink_app.state::<DictationState>();
                *state.current_session_id.lock().unwrap() = None;
                let error = matches!(reason, StopReason::CaptureError | StopReason::IngestErrors)
                    .then(|| format!("{reason:?}"));
                emit_state(&sink_app, Phase::Idle, None, error);
            }
        });
    drop(engine);

    if let Err(e) = start_result {
        *state.current_session_id.lock().unwrap() = None;
        let msg = e.to_string();
        emit_state(&app, Phase::Idle, None, Some(msg.clone()));
        return Err(msg);
    }
    emit_state(&app, Phase::Recording, Some(session_id), None);
    Ok(())
}

/// Stop the active session, if any. Idempotent.
#[tauri::command]
pub async fn dictation_stop(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DictationState>();
        emit_state(&app, Phase::Stopping, state.current_session_id.lock().unwrap().clone(), None);
        let mut engine = state.engine.lock().unwrap();
        if let Some(engine) = engine.as_mut() {
            engine.stop_session();
        }
        drop(engine);
        *state.current_session_id.lock().unwrap() = None;
        // The session's own DictationEvent::Stopped -> Phase::Idle only fires
        // when a session was actually active; a defensive stop() call (e.g.
        // the new-slot dialog's on-mount cleanup) with nothing recording
        // must still settle the frontend's phase back to idle, or it's stuck
        // at "stopping" forever — and anything gated on that phase (like a
        // mic button's disabled state) stays disabled forever too.
        emit_state(&app, Phase::Idle, None, None);
        Ok(())
    })
    .await
    .map_err(|e| format!("dictation stop task failed: {e}"))?
}

/// Start if idle, stop if recording — the mic button's single action.
#[tauri::command]
pub async fn dictation_toggle(app: AppHandle) -> Result<(), String> {
    let recording = {
        let state = app.state::<DictationState>();
        let mut engine = state.engine.lock().unwrap();
        engine.as_mut().is_some_and(DictationEngine::is_recording)
    };
    if recording { dictation_stop(app).await } else { dictation_start(app).await }
}

/// Stop any in-flight session when the window closes — mirrors
/// `terminal::on_window_destroyed`.
pub fn on_window_destroyed(app: &AppHandle, label: &str) {
    if label == MAIN_WINDOW_LABEL
        && let Some(state) = app.try_state::<DictationState>()
    {
        let mut engine = state.engine.lock().unwrap();
        if let Some(engine) = engine.as_mut() {
            engine.stop_session();
        }
    }
}
