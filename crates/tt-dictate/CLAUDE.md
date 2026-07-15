# CLAUDE.md — crates/tt-dictate

Streaming dictation engine: sherpa-onnx ASR over `cpal` mic capture,
ported from the standalone `scribed` daemon. Owns audio capture, DSP, the
ASR driver, and per-session `DictationEngine` transcript events — it does
**not** own retyping into the focused field; the frontend diffs
transcript snapshots itself and does the typing.

## The `asr` feature is off by default

`sherpa-onnx` is a native/static-lib dependency, so it's gated behind the
`asr` cargo feature rather than a hard dependency. Core logic (driver,
retype, settings, DSP) builds and runs its unit tests everywhere without
it; only `crates-tauri/tt-app` enables `tt-dictate` with `features = ["asr"]`
(see its `Cargo.toml`). If you're adding a test or a new consumer of this
crate, default to *not* requiring `asr` — only reach for it if you actually
need real transcription, not just the driver/session plumbing.

## Threading and failure model

`DictationEngine::start_session` spawns a dedicated OS thread per
recording session (not a tokio task) — ASR inference is blocking FFI work.
Partial-transcript and audio-level updates are debounced to 100ms so the
frontend isn't flooded on every audio frame. The session aborts after 50
consecutive FFI ingest errors in a row, rather than retrying forever or
crashing — a persistently broken audio device degrades to "no transcript"
instead of pegging a thread.
