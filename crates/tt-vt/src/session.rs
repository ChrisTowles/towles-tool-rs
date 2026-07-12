//! Per-terminal thread wrapper around [`Engine`].
//!
//! libghostty-vt state is `!Send`, so each terminal gets a dedicated thread
//! that owns its engine. Callers talk to it through a channel and receive
//! [`Event`]s on a sink callback (invoked on the session thread).
//!
//! Batching falls out of the loop shape: one blocking `recv`, then drain
//! everything already queued, then a single render pass. Under PTY floods
//! the drain naturally coalesces many chunks into one frame. On top of that,
//! renders are throttled to [`MIN_FRAME_INTERVAL`]: an input arriving while
//! the terminal is idle renders immediately, but a steady trickle of chunks
//! keeps being absorbed until the interval elapses. Without this, every
//! chunk gets its own frame event and the UI event queue backs up faster
//! than the webview can paint — input latency then grows with sustained
//! output and only recovers once the flood stops.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::engine::{Engine, EngineOptions, Select, VtError};
use crate::frame::Frame;
use crate::search::SearchMatch;

/// Minimum time between render passes (~90 fps). Caps how fast frames can be
/// produced so the UI side can never fall behind unboundedly.
const MIN_FRAME_INTERVAL: Duration = Duration::from_micros(1_000_000 / 90);

pub enum Input {
    /// Raw PTY output bytes.
    Bytes(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    /// Scroll the viewport by rows (up is negative); `None` jumps to bottom.
    Scroll(Option<isize>),
    /// Apply a selection operation.
    Select(Select),
    /// Reply with the active selection's plain text on the provided channel.
    Copy(mpsc::SyncSender<Option<String>>),
    /// Case-insensitive scrollback search; matches (up to `limit`) are sent
    /// back on the provided channel, top to bottom.
    Search {
        query: String,
        limit: usize,
        reply: mpsc::SyncSender<Vec<SearchMatch>>,
    },
    /// Scroll the viewport so the given absolute row is visible.
    ScrollTo(usize),
    /// Force the next render to be a full frame (re-shown pane needs a
    /// complete repaint; see [`Engine::request_full`]).
    RequestFull,
    /// Drop scrollback history, keeping the visible screen (see
    /// [`Engine::clear_scrollback`]).
    ClearScrollback,
}

#[derive(Debug)]
pub enum Event {
    /// A render frame for the UI.
    Frame(Frame),
    /// Bytes the terminal wants written back to the PTY (query replies).
    PtyReply(Vec<u8>),
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("failed to spawn tt-vt session thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error(transparent)]
    Vt(#[from] VtError),
    #[error("tt-vt session thread died before reporting readiness")]
    ThreadDied,
}

pub struct Session {
    tx: mpsc::Sender<Input>,
    join: Option<JoinHandle<()>>,
}

impl Session {
    /// Spawn the engine thread. Fails if the engine can't be created
    /// (creation happens on the new thread; the error is relayed back).
    pub fn spawn(
        opts: EngineOptions,
        mut sink: impl FnMut(Event) + Send + 'static,
    ) -> Result<Self, SpawnError> {
        let (tx, rx) = mpsc::channel::<Input>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), VtError>>(1);

        let join = std::thread::Builder::new().name("tt-vt-session".into()).spawn(move || {
            let mut engine = match Engine::new(opts) {
                Ok(e) => {
                    let _ = ready_tx.send(Ok(()));
                    e
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            // Start in the past so the first input renders immediately.
            let mut last_render = Instant::now() - MIN_FRAME_INTERVAL;
            while let Ok(first) = rx.recv() {
                let mut apply = |input: Input| match input {
                    Input::Bytes(b) => engine.feed(&b),
                    Input::Resize { cols, rows, cell_width_px, cell_height_px } => {
                        // A failed resize (zero cols during layout races)
                        // keeps the old grid; the next resize fixes it.
                        let _ = engine.resize(cols, rows, cell_width_px, cell_height_px);
                    }
                    Input::Scroll(delta) => engine.scroll(delta),
                    // Out-of-bounds coordinates (layout races) are ignored;
                    // the selection just doesn't change.
                    Input::Select(op) => {
                        let _ = engine.select(op);
                    }
                    Input::Copy(reply) => {
                        let _ = reply.try_send(engine.copy_selection().ok().flatten());
                    }
                    Input::Search { query, limit, reply } => {
                        let _ = reply.try_send(engine.search(&query, limit).unwrap_or_default());
                    }
                    Input::ScrollTo(row) => {
                        let _ = engine.scroll_to(row);
                    }
                    Input::RequestFull => engine.request_full(),
                    Input::ClearScrollback => engine.clear_scrollback(),
                };
                apply(first);
                // Absorb further input until the frame interval since the
                // last render has passed. An idle terminal renders its first
                // input with no delay; a flood coalesces into ~90 fps frames.
                loop {
                    while let Ok(more) = rx.try_recv() {
                        apply(more);
                    }
                    let elapsed = last_render.elapsed();
                    if elapsed >= MIN_FRAME_INTERVAL {
                        break;
                    }
                    match rx.recv_timeout(MIN_FRAME_INTERVAL - elapsed) {
                        Ok(more) => apply(more),
                        // Timeout: interval reached. Disconnected: render
                        // what we have; the outer recv ends the loop.
                        Err(_) => break,
                    }
                }

                let reply = engine.take_pty_output();
                if !reply.is_empty() {
                    sink(Event::PtyReply(reply));
                }
                match engine.render() {
                    Ok(Some(frame)) => {
                        sink(Event::Frame(frame));
                        last_render = Instant::now();
                    }
                    Ok(None) => {}
                    // Render errors are terminal-state bugs, not
                    // recoverable I/O; stop the session.
                    Err(_) => break,
                }
            }
        })?;

        ready_rx.recv().map_err(|_| SpawnError::ThreadDied)??;
        Ok(Self { tx, join: Some(join) })
    }

    /// Send input to the engine. Returns false if the session thread is gone.
    pub fn send(&self, input: Input) -> bool {
        self.tx.send(input).is_ok()
    }

    /// A cloneable sender for feeding this session from other threads (e.g. a
    /// PTY reader). The engine thread exits once the [`Session`] is dropped
    /// AND every cloned sender is gone.
    pub fn sender(&self) -> mpsc::Sender<Input> {
        self.tx.clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Closing the channel ends the thread's recv loop.
        let (tx, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
