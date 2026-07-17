//! Per-terminal thread wrapper around [`Engine`].
//!
//! libghostty-vt state is `!Send`, so each terminal gets a dedicated thread
//! that owns its engine. Callers talk to it through channels and receive
//! [`Event`]s on a sink callback (invoked on the session thread).
//!
//! Batching falls out of the loop shape: one blocking wait, then drain
//! everything already queued, then a single render pass. Under PTY floods
//! the drain naturally coalesces many chunks into one frame. On top of that,
//! renders are throttled to [`MIN_FRAME_INTERVAL`]: an input arriving while
//! the terminal is idle renders immediately, but a steady trickle of chunks
//! keeps being absorbed until the interval elapses. Without this, every
//! chunk gets its own frame event and the UI event queue backs up faster
//! than the webview can paint — input latency then grows with sustained
//! output and only recovers once the flood stops.
//!
//! # Backpressure and the control fast-path
//!
//! Two problems the throttle alone doesn't solve, both handled here by
//! splitting the byte and control inputs onto separate channels:
//!
//! * **Bounded memory.** The frame *emitter* is throttled, but a firehose
//!   (`cat huge.log`) into a slow webview would let raw PTY bytes queue
//!   without bound. Bytes ride a *bounded* channel
//!   ([`MAX_QUEUED_BYTE_CHUNKS`]); once it fills, the feeder — the PTY reader
//!   thread — blocks in [`Sender::send`], so the kernel's PTY buffer fills and
//!   applies real flow control to the shell. The engine's queue can never
//!   grow past a few MB.
//! * **Responsive UI.** Resize/scroll/copy/selection must never wait behind a
//!   backlog of bytes. Control rides its own *unbounded* channel that the
//!   engine drains *first* on every pass, so a saturated byte queue can neither
//!   block a control send nor delay it behind queued output.
//!
//! The engine thread blocks on a third, dumb `wake` channel that every send
//! pings after enqueuing its payload; on wake it drains control then bytes.
//! The wake channel carries no ordering, so control priority holds regardless
//! of the order sends happened in.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::engine::{Engine, EngineOptions, PasteOutcome, Select, Theme, VtError};
use crate::frame::Frame;
use crate::search::SearchMatch;

/// Minimum time between render passes (~90 fps). Caps how fast frames can be
/// produced so the UI side can never fall behind unboundedly.
const MIN_FRAME_INTERVAL: Duration = Duration::from_micros(1_000_000 / 90);

/// Render interval while the pane is hidden (~2 fps). Frontend panes never
/// unmount — a backgrounded tab sits behind another one at `display:none` —
/// so without this a session streaming output (an active agent, a chatty
/// build) keeps rendering at the interactive cap for a canvas nothing is
/// painting. Still fast enough to keep title/cursor state fresh for the
/// rail's live label; [`Input::RequestFull`] catches the canvas up in full
/// once the pane is shown again.
const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_millis(500);

/// Longest a synchronized-output batch (DEC mode 2026) may hold rendering.
/// While an application keeps BSU open the loop defers frames so half-drawn
/// updates never reach the canvas — but a program that crashes mid-batch
/// must not freeze the pane, so after this long the frame ships anyway.
/// 150 ms matches the hold cap other emulators use (kitty, contour).
const SYNC_OUTPUT_MAX_HOLD: Duration = Duration::from_millis(150);

/// Cap on unconsumed PTY byte chunks queued for the engine. At the PTY
/// reader's 64 KiB read size this bounds the in-flight backlog to ~4 MB —
/// far more than any interactive burst, so normal output never blocks, but a
/// firehose into a stalled engine blocks the reader instead of ballooning
/// memory. Control inputs are never counted against this bound.
const MAX_QUEUED_BYTE_CHUNKS: usize = 64;

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
    /// Report a mouse-wheel gesture at viewport cell (`x`, `y`) to the
    /// application in its negotiated mouse protocol (`lines` rows, up is
    /// negative). No-op unless the application enabled mouse tracking.
    Wheel { x: u16, y: u16, lines: i32 },
    /// Apply a selection operation.
    Select(Select),
    /// Reply with the active selection's plain text on the provided channel.
    Copy(mpsc::SyncSender<Option<String>>),
    /// Paste text into the shell through libghostty's paste encoder (strips
    /// dangerous control bytes, honors bracketed paste). The outcome is sent
    /// on `reply`; `NeedsConfirm` means nothing was written (see
    /// [`Engine::paste`]).
    Paste {
        text: String,
        force: bool,
        reply: mpsc::SyncSender<PasteOutcome>,
    },
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
    /// The pane was shown or hidden in the frontend — widens the render
    /// interval to [`HIDDEN_FRAME_INTERVAL`] while hidden.
    Visibility(bool),
    /// Push the UI theme (default colors, ANSI palette, dark/light) into the
    /// emulator so color queries answer the truth (see [`Engine::set_theme`]).
    Theme(Theme),
}

#[derive(Debug)]
pub enum Event {
    /// A render frame for the UI.
    Frame(Frame),
    /// Bytes the terminal wants written back to the PTY (query replies).
    PtyReply(Vec<u8>),
    /// Text a program copied via an OSC 52 set-clipboard sequence. The host
    /// writes it to the system clipboard, gated on this terminal being focused.
    Clipboard(String),
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

/// Cloneable handle for feeding a session. [`Input::Bytes`] rides a bounded
/// channel — sending blocks when the engine is behind, which is the
/// backpressure that reaches the PTY reader. Every other (control) input rides
/// an unbounded channel the engine drains first, so control is never blocked
/// behind queued bytes.
#[derive(Clone)]
pub struct Sender {
    bytes: mpsc::SyncSender<Vec<u8>>,
    control: mpsc::Sender<Input>,
    wake: mpsc::Sender<()>,
}

impl Sender {
    /// Send an input to the engine. Bytes may block under backpressure;
    /// control never does. Returns false once the session thread is gone.
    pub fn send(&self, input: Input) -> bool {
        match input {
            Input::Bytes(bytes) => {
                if self.bytes.send(bytes).is_err() {
                    return false;
                }
            }
            control => {
                if self.control.send(control).is_err() {
                    return false;
                }
            }
        }
        // Payload is enqueued; wake the engine. A failed wake means the engine
        // is gone — the payload send above would then have failed too, so on
        // success here there is nothing to report.
        let _ = self.wake.send(());
        true
    }

    /// Replace the channels with dead stand-ins so dropping this handle lets
    /// the engine thread's wake loop end (once every clone is gone too).
    fn disconnect(&mut self) {
        let (bytes, _) = mpsc::sync_channel(0);
        self.bytes = bytes;
        let (control, _) = mpsc::channel();
        self.control = control;
        let (wake, _) = mpsc::channel();
        self.wake = wake;
    }
}

pub struct Session {
    sender: Sender,
    join: Option<JoinHandle<()>>,
}

impl Session {
    /// Spawn the engine thread. Fails if the engine can't be created
    /// (creation happens on the new thread; the error is relayed back).
    pub fn spawn(
        opts: EngineOptions,
        mut sink: impl FnMut(Event) + Send + 'static,
    ) -> Result<Self, SpawnError> {
        let (bytes_tx, bytes_rx) = mpsc::sync_channel::<Vec<u8>>(MAX_QUEUED_BYTE_CHUNKS);
        let (control_tx, control_rx) = mpsc::channel::<Input>();
        let (wake_tx, wake_rx) = mpsc::channel::<()>();
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

            // `hidden` is set by `Input::Visibility` and read outside the
            // closure to pick the render interval, so it takes `&mut bool`
            // instead of living inside `apply_control`'s captures.
            let apply_control = |engine: &mut Engine, hidden: &mut bool, input: Input| match input {
                // Bytes never route through the control channel (see
                // `Sender::send`); feed defensively rather than panic.
                Input::Bytes(b) => engine.feed(&b),
                Input::Resize { cols, rows, cell_width_px, cell_height_px } => {
                    // A failed resize (zero cols during layout races)
                    // keeps the old grid; the next resize fixes it.
                    let _ = engine.resize(cols, rows, cell_width_px, cell_height_px);
                }
                Input::Scroll(delta) => engine.scroll(delta),
                // Encoding can only fail on allocation; the report is
                // best-effort like any other input.
                Input::Wheel { x, y, lines } => {
                    let _ = engine.wheel(x, y, lines);
                }
                // Out-of-bounds coordinates (layout races) are ignored;
                // the selection just doesn't change.
                Input::Select(op) => {
                    let _ = engine.select(op);
                }
                Input::Copy(reply) => {
                    let _ = reply.try_send(engine.copy_selection().ok().flatten());
                }
                // An FFI failure (allocation-only) reads as pasted-and-lost,
                // like input dropped by a full queue — never as NeedsConfirm,
                // which would raise a spurious dialog.
                Input::Paste { text, force, reply } => {
                    let _ =
                        reply.try_send(engine.paste(&text, force).unwrap_or(PasteOutcome::Pasted));
                }
                Input::Search { query, limit, reply } => {
                    let _ = reply.try_send(engine.search(&query, limit).unwrap_or_default());
                }
                Input::ScrollTo(row) => {
                    let _ = engine.scroll_to(row);
                }
                Input::RequestFull => engine.request_full(),
                Input::ClearScrollback => engine.clear_scrollback(),
                Input::Visibility(visible) => *hidden = !visible,
                // A failed theme push keeps the old colors; the next theme
                // change (or a restart) retries.
                Input::Theme(theme) => {
                    let _ = engine.set_theme(&theme);
                }
            };

            // Start in the past so the first input renders immediately.
            let mut last_render = Instant::now() - MIN_FRAME_INTERVAL;
            let mut hidden = false;
            // When the application opened a synchronized-output batch
            // (`Engine::sync_output`); bounds the render hold to
            // [`SYNC_OUTPUT_MAX_HOLD`] from this instant.
            let mut sync_since: Option<Instant> = None;
            // Block for a wake, then drain and render. Buffered wakes are
            // delivered before disconnect, so a dropped session still drains
            // its queued input before the loop ends.
            while wake_rx.recv().is_ok() {
                let mut applied = false;
                // Absorb input until the frame interval since the last render
                // has passed. An idle terminal renders its first input with no
                // delay; a flood coalesces into ~90 fps frames (or ~2 fps
                // while hidden — see `HIDDEN_FRAME_INTERVAL`). Control is
                // drained before bytes on every pass so UI ops never wait
                // behind queued output.
                loop {
                    while let Ok(input) = control_rx.try_recv() {
                        apply_control(&mut engine, &mut hidden, input);
                        applied = true;
                    }
                    while let Ok(bytes) = bytes_rx.try_recv() {
                        engine.feed(&bytes);
                        applied = true;
                    }
                    // A synchronized-output batch (DEC 2026) holds the frame
                    // until the app closes it (ESU) or the hold cap expires —
                    // we own both the emulator and the canvas, so honoring it
                    // means half-drawn TUI updates never reach the screen.
                    // Past the cap the batch renders anyway and this pass
                    // falls through to the normal interval pacing below.
                    if engine.sync_output() {
                        let since = *sync_since.get_or_insert_with(Instant::now);
                        if let Some(hold) = SYNC_OUTPUT_MAX_HOLD.checked_sub(since.elapsed()) {
                            match wake_rx.recv_timeout(hold) {
                                // More input — maybe the ESU. Re-drain.
                                Ok(()) => continue,
                                // Hold cap reached (or disconnected): render.
                                Err(_) => break,
                            }
                        }
                    } else {
                        sync_since = None;
                    }
                    let interval = if hidden { HIDDEN_FRAME_INTERVAL } else { MIN_FRAME_INTERVAL };
                    let elapsed = last_render.elapsed();
                    if elapsed >= interval {
                        break;
                    }
                    match wake_rx.recv_timeout(interval - elapsed) {
                        Ok(()) => continue,
                        // Timeout: interval reached. Disconnected: render what
                        // we have; the outer recv ends the loop.
                        Err(_) => break,
                    }
                }
                // A lone wake token whose payload an earlier pass already
                // drained: nothing changed, so skip the render.
                if !applied {
                    continue;
                }

                let reply = engine.take_pty_output();
                if !reply.is_empty() {
                    sink(Event::PtyReply(reply));
                }
                for text in engine.take_clipboard() {
                    sink(Event::Clipboard(text));
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
        Ok(Self {
            sender: Sender { bytes: bytes_tx, control: control_tx, wake: wake_tx },
            join: Some(join),
        })
    }

    /// Send input to the engine. Bytes block under backpressure (see
    /// [`Sender::send`]); control never does. Returns false if the session
    /// thread is gone.
    pub fn send(&self, input: Input) -> bool {
        self.sender.send(input)
    }

    /// A cloneable sender for feeding this session from other threads (e.g. a
    /// PTY reader). The engine thread exits once the [`Session`] is dropped
    /// AND every cloned sender is gone.
    pub fn sender(&self) -> Sender {
        self.sender.clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Drop our senders so the thread's wake loop can end once every cloned
        // sender is gone, then join it.
        self.sender.disconnect();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generous upper bound on a signal that *should* arrive — a failure here
    /// means a real hang, not a slow machine.
    const TIMEOUT: Duration = Duration::from_secs(5);
    /// Shorter bound for confirming progress has *stopped* (the feeder blocked).
    /// Successive accepted chunks arrive microseconds apart, so a gap this long
    /// means the bounded queue is genuinely full.
    const STALL_TIMEOUT: Duration = Duration::from_millis(500);

    /// A sink that parks the engine thread inside the very first frame it
    /// emits (and never again), so the test can observe the engine stalled.
    /// Returns the session plus a "parked" signal and a release trigger.
    fn spawn_parked() -> (Session, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::sync_channel::<()>(1);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let mut parked = false;
        let session = Session::spawn(
            EngineOptions { cols: 40, rows: 8, max_scrollback: 100 },
            move |event| {
                if let Event::Frame(_) = event {
                    if !parked {
                        parked = true;
                        let _ = entered_tx.send(());
                        // Block the engine thread here until the test releases.
                        let _ = release_rx.recv();
                    }
                }
            },
        )
        .expect("spawn session");
        (session, entered_rx, release_tx)
    }

    #[test]
    fn stalled_sink_bounds_the_byte_feed() {
        let (session, entered_rx, release_tx) = spawn_parked();

        // First chunk triggers a frame; the sink parks the engine thread so it
        // stops draining the byte queue.
        assert!(session.send(Input::Bytes(b"hi".to_vec())));
        entered_rx.recv_timeout(TIMEOUT).expect("engine parked in the sink");

        // Feed from another thread, reporting how many chunks the bounded byte
        // path accepts before it blocks.
        let feeder = session.sender();
        let (count_tx, count_rx) = mpsc::channel::<usize>();
        let handle = std::thread::spawn(move || {
            let mut n = 0;
            // Cap well above the bound so a broken (unbounded) feed still
            // terminates the thread instead of hanging the test.
            while n < MAX_QUEUED_BYTE_CHUNKS + 100 {
                if !feeder.send(Input::Bytes(vec![b'x'])) {
                    break;
                }
                n += 1;
                let _ = count_tx.send(n);
            }
        });

        // Drain progress until it stalls: the feeder blocks once the bounded
        // queue is full.
        let mut accepted = 0;
        while let Ok(n) = count_rx.recv_timeout(STALL_TIMEOUT) {
            accepted = n;
        }
        assert_eq!(
            accepted, MAX_QUEUED_BYTE_CHUNKS,
            "the stalled engine bounds the byte feed to the channel capacity"
        );
        assert!(!handle.is_finished(), "the feeder is blocked on backpressure, not finished");

        // Release the engine: it drains, frees the queue, and the feeder runs
        // to its safety cap and exits.
        let _ = release_tx.send(());
        handle.join().expect("feeder joins once backpressure lifts");
        drop(session);
    }

    #[test]
    fn control_is_not_blocked_by_a_saturated_byte_queue() {
        let (session, entered_rx, release_tx) = spawn_parked();

        assert!(session.send(Input::Bytes(b"hi".to_vec())));
        entered_rx.recv_timeout(TIMEOUT).expect("engine parked in the sink");

        // Saturate the byte queue (bounded), then stop; the last send blocks.
        let feeder = session.sender();
        let (satc_tx, satc_rx) = mpsc::channel::<usize>();
        let sat = std::thread::spawn(move || {
            let mut n = 0;
            while n < MAX_QUEUED_BYTE_CHUNKS + 3 {
                if !feeder.send(Input::Bytes(vec![b'x'])) {
                    break;
                }
                n += 1;
                let _ = satc_tx.send(n);
            }
        });
        // Wait until the queue is full (the feeder is now blocked on its next
        // send).
        let mut accepted = 0;
        while accepted < MAX_QUEUED_BYTE_CHUNKS {
            accepted = satc_rx.recv_timeout(TIMEOUT).expect("byte queue fills");
        }

        // With bytes saturated, a control send must still complete promptly —
        // it rides its own unbounded channel, never blocked behind bytes.
        let control = session.sender();
        let (done_tx, done_rx) = mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let ok = control.send(Input::Scroll(Some(-1)));
            let _ = done_tx.send(ok);
        });
        assert!(
            done_rx.recv_timeout(TIMEOUT).expect("control send blocked behind saturated bytes"),
            "control send accepted while the byte queue is full"
        );

        // Release the engine and confirm control is actually processed while a
        // byte backlog is pending: a Copy reply comes back.
        let _ = release_tx.send(());
        let (reply_tx, reply_rx) = mpsc::sync_channel::<Option<String>>(1);
        assert!(session.send(Input::Copy(reply_tx)));
        reply_rx.recv_timeout(TIMEOUT).expect("engine processed the control input");

        sat.join().expect("saturating feeder drains once backpressure lifts");
        drop(session);
    }
}
