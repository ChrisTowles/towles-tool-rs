//! Engine-agnostic streaming driver. Wraps a [`StreamingTranscriber`] and
//! turns its frame-level partial / endpoint events into a cumulative
//! [`Transcript`]. Endpointing happens inside the recognizer; this module
//! is just transcript bookkeeping.

use crate::asr::{AsrError, Segment, Transcript};

/// One frame-level update from the streaming recognizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingUpdate {
    /// Current best hypothesis. May shrink, grow, or be rewritten on the
    /// next tick.
    Partial(String),
    /// The recognizer hit one of its endpoint rules. The string is the
    /// final text for the segment that just ended; the next `accept_waveform`
    /// call starts a fresh segment.
    Endpoint(String),
    /// Nothing new this tick — either not enough samples to decode another
    /// frame, or the hypothesis text didn't change.
    Idle,
}

/// What a streaming ASR backend must expose.
pub trait StreamingTranscriber: Send {
    /// Hand the recognizer some audio. Samples must be 16 kHz mono f32 in
    /// the range \[-1.0, 1.0\]. Cheap — the recognizer queues them
    /// internally; the actual decode happens during `poll`.
    fn accept_waveform(&mut self, samples: &[f32]) -> Result<(), AsrError>;

    /// Drive the decoder forward and report what changed. Call this in a
    /// loop after every `accept_waveform` until it returns `Idle`.
    fn poll(&mut self) -> Result<StreamingUpdate, AsrError>;

    /// Tell the recognizer no more samples are coming. Forces any partial
    /// frame to be decoded and flushed.
    fn input_finished(&mut self) -> Result<(), AsrError>;

    /// Drop all state and start a fresh utterance.
    fn reset(&mut self) -> Result<(), AsrError>;
}

/// Cumulative transcript state for one recording session. `ingest` per
/// audio chunk, `finalize` once on hotkey release.
#[derive(Default)]
pub struct StreamingDriver {
    committed: Vec<Segment>,
    live_tail: String,
}

impl StreamingDriver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_transcript(&self) -> Transcript {
        Transcript { committed: self.committed.clone(), live_tail: self.live_tail.clone() }
    }

    /// Push `chunk` to the recognizer, then drain partials/endpoints.
    /// Returns the post-drain transcript if anything changed.
    pub fn ingest(
        &mut self,
        chunk: &[f32],
        transcriber: &mut dyn StreamingTranscriber,
    ) -> Result<Option<Transcript>, AsrError> {
        transcriber.accept_waveform(chunk)?;
        self.drain(transcriber)
    }

    /// Drive the recognizer to its final state for the current session and
    /// return the resulting transcript.
    pub fn finalize(
        &mut self,
        transcriber: &mut dyn StreamingTranscriber,
    ) -> Result<Transcript, AsrError> {
        transcriber.input_finished()?;
        self.drain(transcriber)?;
        // Commit whatever's still sitting in the live tail — input is done,
        // so by definition the partial we have is the best we'll get.
        if !self.live_tail.trim().is_empty() {
            self.committed.push(Segment { text: std::mem::take(&mut self.live_tail) });
        } else {
            self.live_tail.clear();
        }
        Ok(self.current_transcript())
    }

    fn drain(
        &mut self,
        transcriber: &mut dyn StreamingTranscriber,
    ) -> Result<Option<Transcript>, AsrError> {
        let mut changed = false;
        loop {
            match transcriber.poll()? {
                StreamingUpdate::Partial(text) => {
                    // Skip empty/whitespace partials. Sherpa can emit "" mid-
                    // utterance during silence frames; letting that overwrite
                    // live_tail would briefly clear what's been typed and force
                    // a backspace burst when the next non-empty partial arrives.
                    if text.trim().is_empty() {
                        continue;
                    }
                    if text != self.live_tail {
                        self.live_tail = text;
                        changed = true;
                    }
                }
                StreamingUpdate::Endpoint(text) => {
                    if !text.trim().is_empty() {
                        self.committed.push(Segment { text });
                        changed = true;
                    }
                    if !self.live_tail.is_empty() {
                        self.live_tail.clear();
                        changed = true;
                    }
                }
                StreamingUpdate::Idle => break,
            }
        }
        Ok(if changed { Some(self.current_transcript()) } else { None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Replays a scripted sequence of updates as the driver polls. One
    /// scripted update is consumed per `poll()` call; `accept_waveform`
    /// just records how many samples were pushed.
    #[derive(Default)]
    struct FakeStreaming {
        queue: VecDeque<StreamingUpdate>,
        samples_pushed: usize,
        input_finished_calls: usize,
        resets: usize,
    }

    impl FakeStreaming {
        fn enqueue<I: IntoIterator<Item = StreamingUpdate>>(&mut self, updates: I) {
            self.queue.extend(updates);
        }
    }

    impl StreamingTranscriber for FakeStreaming {
        fn accept_waveform(&mut self, samples: &[f32]) -> Result<(), AsrError> {
            self.samples_pushed += samples.len();
            Ok(())
        }
        fn poll(&mut self) -> Result<StreamingUpdate, AsrError> {
            Ok(self.queue.pop_front().unwrap_or(StreamingUpdate::Idle))
        }
        fn input_finished(&mut self) -> Result<(), AsrError> {
            self.input_finished_calls += 1;
            Ok(())
        }
        fn reset(&mut self) -> Result<(), AsrError> {
            self.resets += 1;
            self.queue.clear();
            Ok(())
        }
    }

    fn chunk(n: usize) -> Vec<f32> {
        vec![0.1; n]
    }

    #[test]
    fn partial_becomes_live_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Partial("hello".into())]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap().unwrap();
        assert_eq!(result.live_tail, "hello");
        assert!(result.committed.is_empty());
    }

    #[test]
    fn duplicate_partial_yields_no_change() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Partial("hello".into())]);
        let _ = d.ingest(&chunk(1600), &mut t).unwrap();
        // Second ingest with the same partial — should not signal a change.
        t.enqueue([StreamingUpdate::Partial("hello".into())]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap();
        assert!(result.is_none(), "no-change partial should not emit a transcript");
    }

    #[test]
    fn endpoint_commits_segment_and_clears_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([
            StreamingUpdate::Partial("hello".into()),
            StreamingUpdate::Endpoint("hello world".into()),
        ]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "hello world");
        assert_eq!(result.live_tail, "");
    }

    #[test]
    fn post_endpoint_partial_starts_a_new_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([
            StreamingUpdate::Endpoint("first segment".into()),
            StreamingUpdate::Partial("second".into()),
        ]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "first segment");
        assert_eq!(result.live_tail, "second");
    }

    #[test]
    fn finalize_commits_pending_live_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Partial("trailing".into())]);
        d.ingest(&chunk(1600), &mut t).unwrap();
        // No more updates queued — finalize should commit the tail as-is.
        let result = d.finalize(&mut t).unwrap();
        assert_eq!(t.input_finished_calls, 1);
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "trailing");
        assert!(result.live_tail.is_empty());
    }

    #[test]
    fn finalize_on_empty_state_returns_empty_transcript() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        let result = d.finalize(&mut t).unwrap();
        assert!(result.committed.is_empty());
        assert!(result.live_tail.is_empty());
    }

    #[test]
    fn finalize_drops_whitespace_only_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Partial("   ".into())]);
        d.ingest(&chunk(1600), &mut t).unwrap();
        let result = d.finalize(&mut t).unwrap();
        assert!(
            result.committed.is_empty(),
            "whitespace-only tail must not become a committed segment"
        );
    }

    #[test]
    fn ingest_pushes_samples_to_transcriber() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        d.ingest(&chunk(1600), &mut t).unwrap();
        d.ingest(&chunk(800), &mut t).unwrap();
        assert_eq!(t.samples_pushed, 2400);
    }

    #[test]
    fn empty_endpoint_with_empty_tail_yields_no_change() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Endpoint(String::new())]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap();
        assert!(result.is_none(), "empty endpoint on empty tail must not signal change");
    }

    #[test]
    fn whitespace_only_endpoint_does_not_commit() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Endpoint("   ".into())]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap();
        assert!(
            result.is_none() || result.unwrap().committed.is_empty(),
            "whitespace-only endpoint must not commit a segment"
        );
    }

    #[test]
    fn finalize_after_endpoint_does_not_double_commit() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Endpoint("hello".into())]);
        d.ingest(&chunk(1600), &mut t).unwrap();
        let result = d.finalize(&mut t).unwrap();
        assert_eq!(result.committed.len(), 1, "must not re-commit after a clean endpoint");
        assert_eq!(result.committed[0].text, "hello");
        assert!(result.live_tail.is_empty());
    }

    #[test]
    fn chained_endpoints_in_one_drain_all_commit_in_order() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([
            StreamingUpdate::Endpoint("first".into()),
            StreamingUpdate::Endpoint("second".into()),
            StreamingUpdate::Endpoint("third".into()),
        ]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 3);
        assert_eq!(result.committed[0].text, "first");
        assert_eq!(result.committed[1].text, "second");
        assert_eq!(result.committed[2].text, "third");
        assert!(result.live_tail.is_empty());
    }

    #[test]
    fn empty_partial_does_not_clear_live_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Partial("hello".into())]);
        d.ingest(&chunk(1600), &mut t).unwrap();
        // Now an empty partial arrives (silence frame, dropout)
        t.enqueue([StreamingUpdate::Partial(String::new())]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap();
        // Either None (no change) is acceptable, or Some with live_tail still "hello".
        match result {
            None => {}
            Some(t) => assert_eq!(t.live_tail, "hello", "empty partial must not erase live_tail"),
        }
    }

    #[test]
    fn whitespace_only_partial_does_not_clear_live_tail() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        t.enqueue([StreamingUpdate::Partial("hello".into())]);
        d.ingest(&chunk(1600), &mut t).unwrap();
        t.enqueue([StreamingUpdate::Partial("   ".into())]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap();
        match result {
            None => {}
            Some(t) => assert_eq!(t.live_tail, "hello"),
        }
    }

    #[test]
    fn mixed_partial_endpoint_idle_interleaving() {
        let mut d = StreamingDriver::new();
        let mut t = FakeStreaming::default();
        // First chunk: partial grows, then endpoint commits, then new partial starts.
        t.enqueue([
            StreamingUpdate::Partial("foo".into()),
            StreamingUpdate::Partial("foo bar".into()),
            StreamingUpdate::Endpoint("foo bar baz".into()),
            StreamingUpdate::Partial("qux".into()),
        ]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].text, "foo bar baz");
        assert_eq!(result.live_tail, "qux");
        // Second chunk: a refinement of the in-progress tail, then idle.
        t.enqueue([
            StreamingUpdate::Partial("qux quux".into()),
            StreamingUpdate::Idle,
        ]);
        let result = d.ingest(&chunk(1600), &mut t).unwrap().unwrap();
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.live_tail, "qux quux");
    }
}
