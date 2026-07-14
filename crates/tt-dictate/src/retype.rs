//! The retype state machine.
//!
//! Ported from scribed `src/output/retype.rs`, itself ported from
//! `daemon_service.py:73-109` in the original Python project. Given a stream of
//! evolving transcripts (each one a "live tail" the ASR engine has revised
//! again), [`RetypeState`] computes the minimum keystrokes to take the focused
//! window from the previously-typed text to the new transcript.
//!
//! # The algorithm
//!
//! 1. If the new text is empty or equal to the previously typed text, do nothing.
//! 2. If the new text starts with the previously typed text, append the suffix.
//! 3. Otherwise, find the longest common prefix between the two strings *in
//!    characters* (not bytes — see invariant below). Delete the diverging
//!    tail with backspaces, then type the new tail.
//!
//! # The char-vs-byte invariant
//!
//! Python's `len(str)` returns code points; Rust's `str.len()` returns bytes.
//! All keystroke arithmetic in this module operates on `char` counts because
//! one backspace deletes one user-visible grapheme cluster (approximately —
//! we don't handle combining marks specially, neither does the Python source).
//! String slicing uses byte offsets resolved through `char_indices()`.

/// What [`RetypeState::diff`] emits — a target for the caller's apply sink
/// (in this workspace, `dictation-retype.ts` in the app frontend).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetypeStep<'a> {
    /// Number of backspaces to issue.
    pub backspaces: usize,
    /// Text to type after the backspaces.
    pub insert: &'a str,
}

impl<'a> RetypeStep<'a> {
    pub fn is_noop(&self) -> bool {
        self.backspaces == 0 && self.insert.is_empty()
    }
}

/// Tracks what has been typed into the focused window so we can issue the
/// minimum keystrokes for each new transcript.
///
/// Constructed at the start of a recording session (after the previous one's
/// final transcript has settled) and dropped when the session ends.
#[derive(Debug, Default, Clone)]
pub struct RetypeState {
    /// The full text currently visible in the focused window.
    typed_text: String,
    /// Number of *characters* (not bytes) currently in `typed_text`. Cached
    /// so we don't recompute on every diff.
    typed_chars: usize,
}

impl RetypeState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn typed_text(&self) -> &str {
        &self.typed_text
    }

    pub fn typed_chars(&self) -> usize {
        self.typed_chars
    }

    /// Compute what to do to transition from `typed_text` to `new_text`.
    /// Mutates internal state to reflect the new typed text — the caller is
    /// expected to follow through by actually issuing the keystrokes returned.
    pub fn diff<'a>(&mut self, new_text: &'a str) -> RetypeStep<'a> {
        let new_text = new_text.trim_end_matches(' ');

        if new_text.is_empty() {
            return RetypeStep { backspaces: 0, insert: "" };
        }
        if new_text == self.typed_text {
            return RetypeStep { backspaces: 0, insert: "" };
        }

        // Fast path: new text extends old.
        if new_text.starts_with(&self.typed_text) {
            let insert = &new_text[self.typed_text.len()..];
            let new_chars = self.typed_chars + insert.chars().count();
            self.typed_text = new_text.to_string();
            self.typed_chars = new_chars;
            return RetypeStep { backspaces: 0, insert };
        }

        // Slow path: find longest common prefix in *characters*.
        let common_chars = longest_common_prefix_chars(&self.typed_text, new_text);
        let common_bytes = byte_offset_of_char(new_text, common_chars);
        let backspaces = self.typed_chars - common_chars;
        let insert = &new_text[common_bytes..];

        self.typed_text = new_text.to_string();
        self.typed_chars = new_text.chars().count();

        RetypeStep { backspaces, insert }
    }

    /// Reset to a freshly-empty session.
    pub fn reset(&mut self) {
        self.typed_text.clear();
        self.typed_chars = 0;
    }
}

fn longest_common_prefix_chars(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn byte_offset_of_char(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map(|(i, _)| i).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink that records every step. Test-only: the real apply sink lives in
    /// TypeScript (`dictation-retype.ts`, native value setter + `input` event
    /// for React-controlled inputs); scribed's `KeyboardSink` trait and its
    /// enigo/ydotool/clipboard backends were app-only dead weight and were not
    /// ported.
    #[derive(Debug, Default)]
    struct RecordingSink {
        steps: Vec<(usize, String)>,
    }

    impl RecordingSink {
        fn apply(&mut self, step: RetypeStep<'_>) {
            self.steps.push((step.backspaces, step.insert.to_string()));
        }

        /// Simulate the cumulative effect of all recorded steps on an
        /// initially-empty buffer. The result is what the user would
        /// actually see.
        fn simulate(&self) -> String {
            let mut buf = String::new();
            for (back, ins) in &self.steps {
                for _ in 0..*back {
                    buf.pop();
                }
                buf.push_str(ins);
            }
            buf
        }
    }

    #[test]
    fn first_emission_inserts_full_text() {
        let mut s = RetypeState::new();
        let step = s.diff("hello");
        assert_eq!(step.backspaces, 0);
        assert_eq!(step.insert, "hello");
        assert_eq!(s.typed_text(), "hello");
    }

    #[test]
    fn append_only_avoids_backspaces() {
        let mut s = RetypeState::new();
        s.diff("hello");
        let step = s.diff("hello world");
        assert_eq!(step.backspaces, 0);
        assert_eq!(step.insert, " world");
    }

    #[test]
    fn correction_backspaces_diverging_tail() {
        let mut s = RetypeState::new();
        s.diff("the quick brown fix");
        let step = s.diff("the quick brown fox");
        // Old: "the quick brown fix" (19 chars), common prefix "the quick brown f" (17 chars).
        // Backspace 2 chars ("ix"), type 2 chars ("ox").
        assert_eq!(step.backspaces, 2);
        assert_eq!(step.insert, "ox");
    }

    #[test]
    fn equal_text_is_noop() {
        let mut s = RetypeState::new();
        s.diff("hello");
        let step = s.diff("hello");
        assert!(step.is_noop());
    }

    #[test]
    fn empty_text_is_noop_and_does_not_clear_state() {
        let mut s = RetypeState::new();
        s.diff("hello");
        let step = s.diff("");
        assert!(step.is_noop());
        assert_eq!(s.typed_text(), "hello");
    }

    #[test]
    fn trailing_spaces_ignored() {
        let mut s = RetypeState::new();
        s.diff("hello");
        let step = s.diff("hello   ");
        assert!(step.is_noop());
    }

    #[test]
    fn unicode_emoji_uses_char_counts() {
        let mut s = RetypeState::new();
        s.diff("hello 🌊");
        // "hello 🌊" has 7 chars; "hello 🌅" also 7 chars; common prefix 6.
        let step = s.diff("hello 🌅");
        assert_eq!(step.backspaces, 1);
        assert_eq!(step.insert, "🌅");
    }

    #[test]
    fn unicode_multibyte_chars_count_as_one_each() {
        let mut s = RetypeState::new();
        // "café" = 4 chars but 5 bytes in UTF-8.
        s.diff("café");
        assert_eq!(s.typed_chars(), 4);
        let step = s.diff("cafés");
        assert_eq!(step.backspaces, 0);
        assert_eq!(step.insert, "s");
    }

    #[test]
    fn full_replacement_backspaces_everything() {
        let mut s = RetypeState::new();
        s.diff("apple");
        let step = s.diff("banana");
        assert_eq!(step.backspaces, 5);
        assert_eq!(step.insert, "banana");
    }

    #[test]
    fn reset_clears_state() {
        let mut s = RetypeState::new();
        s.diff("hello");
        s.reset();
        let step = s.diff("world");
        assert_eq!(step.backspaces, 0);
        assert_eq!(step.insert, "world");
    }

    #[test]
    fn recording_sink_simulates_final_state() {
        let mut s = RetypeState::new();
        let mut sink = RecordingSink::default();
        for t in [
            "the",
            "the quick",
            "the quick brown fix",
            "the quick brown fox",
        ] {
            let step = s.diff(t);
            sink.apply(step);
        }
        assert_eq!(sink.simulate(), "the quick brown fox");
    }

    use proptest::prelude::*;

    proptest! {
        /// Core invariant: after every diff+apply round, the visible window
        /// state (as simulated by the recording sink) equals the engine's
        /// internal `typed_text`. This holds regardless of input — empties
        /// noop, full replacements work, unicode survives.
        #[test]
        fn invariant_simulated_state_matches_typed_text(
            texts in proptest::collection::vec("[a-zA-Z0-9 ]{0,40}", 1..20)
        ) {
            let mut s = RetypeState::new();
            let mut sink = RecordingSink::default();
            for t in &texts {
                let step = s.diff(t);
                sink.apply(step);
                prop_assert_eq!(sink.simulate(), s.typed_text().to_string());
            }
        }

        /// Same invariant, but over arbitrary unicode strings (including
        /// emoji, combining marks, control chars).
        #[test]
        fn invariant_simulated_state_matches_typed_text_unicode(
            texts in proptest::collection::vec(".{0,30}", 1..15)
        ) {
            let mut s = RetypeState::new();
            let mut sink = RecordingSink::default();
            for t in &texts {
                let step = s.diff(t);
                sink.apply(step);
                prop_assert_eq!(sink.simulate(), s.typed_text().to_string());
            }
        }

        /// After diffing to a non-empty trimmed text, `typed_text` equals that
        /// trimmed text.
        #[test]
        fn invariant_typed_text_matches_trimmed_input(text in "[a-zA-Z0-9 ]{1,40}") {
            let mut s = RetypeState::new();
            let trimmed = text.trim_end_matches(' ');
            s.diff(&text);
            if !trimmed.is_empty() {
                prop_assert_eq!(s.typed_text(), trimmed);
            }
        }
    }
}
