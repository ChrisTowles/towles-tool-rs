//! Minimal OSC 52 *set-clipboard* scanner run alongside the terminal state
//! machine on the raw PTY byte feed.
//!
//! libghostty-vt parses OSC 52 (its `CommandType::ClipboardContents`) but
//! exposes neither the decoded payload nor a callback for it — `on_title_changed`
//! has no clipboard sibling — so there is no clean hook to read the copied text
//! from the engine. This standalone scanner fills that gap: it watches the same
//! bytes fed to `Terminal::vt_write`, recognizes the set-clipboard form
//! (`ESC ] 52 ; <targets> ; <base64> ST`), base64-decodes the payload, and
//! queues the resulting text for the session thread to surface as an event.
//!
//! Tradeoffs of scanning rather than hooking the library:
//! * We re-scan bytes the library already parses (cheap: a tiny byte-at-a-time
//!   state machine, no allocation until an actual OSC 52 sequence appears).
//! * Only the *write* form is handled. A read/query (`Pd == "?"`) is ignored;
//!   read-side OSC 52 is deliberately unimplemented.
//! * Targets (`Pc` — clipboard vs. primary selection) are not distinguished;
//!   every set writes the one system clipboard.
//!
//! The scanner is fragment-tolerant: a sequence split across feed calls is
//! reassembled across calls. Malformed base64, non-UTF-8, and oversized
//! payloads are dropped silently (no event, no panic).

use base64::Engine as _;

/// Hard cap on the collected `Pc;Pd` payload. A never-terminated sequence
/// (or a hostile one) can't grow the buffer without bound: once the collected
/// bytes reach this size the sequence is abandoned. ~1 MB comfortably covers
/// any real clipboard copy while bounding memory.
const MAX_PAYLOAD: usize = 1024 * 1024;

/// Guard on the OSC numeric identifier length so a stream of digits after
/// `ESC ]` can't grow the ident buffer; real OSC numbers are 1–3 digits.
const MAX_IDENT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Scanning for `ESC`.
    Ground,
    /// Saw `ESC`; expecting `]` to begin an OSC.
    Esc,
    /// Inside an OSC, accumulating the numeric identifier until `;`.
    Ident,
    /// Identifier was `52`; collecting the `Pc;Pd` payload until a terminator.
    Collect,
    /// Inside `Collect`, saw `ESC`; expecting `\` to complete a String Terminator.
    CollectEsc,
}

/// Streaming recognizer for OSC 52 set-clipboard sequences. Feed it the same
/// bytes as the terminal engine; drain decoded clipboard writes with [`take`].
///
/// [`take`]: Osc52Scanner::take
#[derive(Debug)]
pub struct Osc52Scanner {
    state: State,
    ident: Vec<u8>,
    payload: Vec<u8>,
    pending: Vec<String>,
}

impl Default for Osc52Scanner {
    fn default() -> Self {
        Self { state: State::Ground, ident: Vec::new(), payload: Vec::new(), pending: Vec::new() }
    }
}

impl Osc52Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a chunk of PTY output, recognizing any OSC 52 set-clipboard
    /// sequences within (including those spanning previous chunks).
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    /// Take the decoded clipboard writes recognized so far, in order.
    pub fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending)
    }

    fn step(&mut self, b: u8) {
        const ESC: u8 = 0x1b;
        const BEL: u8 = 0x07;
        const ST_C1: u8 = 0x9c;
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => match b {
                b']' => {
                    self.ident.clear();
                    self.state = State::Ident;
                }
                ESC => {} // stay armed on a run of ESCs
                _ => self.state = State::Ground,
            },
            State::Ident => match b {
                b';' => {
                    if self.ident == b"52" {
                        self.payload.clear();
                        self.state = State::Collect;
                    } else {
                        self.reset();
                    }
                }
                b'0'..=b'9' => {
                    self.ident.push(b);
                    if self.ident.len() > MAX_IDENT {
                        self.reset();
                    }
                }
                ESC => {
                    self.reset();
                    self.state = State::Esc;
                }
                _ => self.reset(), // non-numeric OSC ident: not 52
            },
            State::Collect => match b {
                BEL | ST_C1 => self.finish(),
                ESC => self.state = State::CollectEsc,
                _ => {
                    if self.payload.len() >= MAX_PAYLOAD {
                        self.reset(); // oversized: abandon the sequence
                    } else {
                        self.payload.push(b);
                    }
                }
            },
            State::CollectEsc => match b {
                b'\\' => self.finish(), // ESC \ String Terminator
                ESC => self.reset_esc(),
                _ => self.reset(),
            },
        }
    }

    /// Finalize a fully-collected OSC 52 payload: split `Pc;Pd`, decode the
    /// base64 data, and queue the text. A query (`Pd == "?"`), an empty payload,
    /// or undecodable data yields nothing.
    fn finish(&mut self) {
        let payload = std::mem::take(&mut self.payload);
        self.reset();
        let Some(sep) = payload.iter().position(|&c| c == b';') else {
            return; // no `Pc;Pd` separator — malformed
        };
        let data = &payload[sep + 1..];
        if data.is_empty() || data == b"?" {
            return; // clear-clipboard or read query: nothing to write
        }
        if let Some(text) = decode(data) {
            self.pending.push(text);
        }
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.ident.clear();
        self.payload.clear();
    }

    fn reset_esc(&mut self) {
        self.reset();
        self.state = State::Esc;
    }
}

/// Decode an OSC 52 base64 payload to text. Accepts padded standard base64
/// (what tmux/nvim emit) and tolerates a missing `=` pad; the decoded bytes are
/// interpreted as UTF-8 (lossily, so a stray non-UTF-8 byte still yields text
/// rather than dropping the whole copy). Returns `None` when the base64 itself
/// is invalid.
fn decode(data: &[u8]) -> Option<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    let bytes = STANDARD.decode(data).or_else(|_| STANDARD_NO_PAD.decode(data)).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> Vec<String> {
        let mut s = Osc52Scanner::new();
        s.feed(bytes);
        s.take()
    }

    #[test]
    fn decodes_a_bel_terminated_set_clipboard() {
        // ESC ] 52 ; c ; aGk= BEL  — "aGk=" is base64 for "hi".
        assert_eq!(scan(b"\x1b]52;c;aGk=\x07"), vec!["hi".to_string()]);
    }

    #[test]
    fn decodes_an_st_terminated_set_clipboard() {
        // Same payload terminated by ESC \ (String Terminator) instead of BEL.
        assert_eq!(scan(b"\x1b]52;c;aGk=\x1b\\"), vec!["hi".to_string()]);
    }

    #[test]
    fn empty_target_field_still_decodes() {
        // Pc may be empty: ESC ] 52 ; ; <base64> ST.
        assert_eq!(scan(b"\x1b]52;;aGk=\x07"), vec!["hi".to_string()]);
    }

    #[test]
    fn ignores_ordinary_terminal_output_around_the_sequence() {
        let mut out = scan(b"before\x1b]52;c;aGk=\x07after");
        assert_eq!(out, vec!["hi".to_string()]);
        // And a plain title OSC is not mistaken for a clipboard write.
        out = scan(b"\x1b]0;my-title\x07");
        assert!(out.is_empty());
    }

    #[test]
    fn reassembles_a_sequence_split_across_feeds() {
        let mut s = Osc52Scanner::new();
        s.feed(b"\x1b]52;c;a");
        assert!(s.take().is_empty(), "nothing until the sequence completes");
        s.feed(b"Gk=\x07");
        assert_eq!(s.take(), vec!["hi".to_string()]);
    }

    #[test]
    fn malformed_base64_yields_no_event_and_no_panic() {
        // '@' and '!' are not base64; the sequence is dropped silently.
        assert!(scan(b"\x1b]52;c;@@@!!!\x07").is_empty());
    }

    #[test]
    fn read_query_is_ignored() {
        // Pd == "?" is a paste/query request — read-side is unimplemented.
        assert!(scan(b"\x1b]52;c;?\x07").is_empty());
    }

    #[test]
    fn missing_data_separator_is_dropped() {
        // No second ';' — malformed set-clipboard.
        assert!(scan(b"\x1b]52;caGk=\x07").is_empty());
    }

    #[test]
    fn oversized_payload_is_guarded() {
        // A giant, never-realistic payload must not be buffered without bound
        // nor produce an event; the scanner abandons it past the cap.
        let mut seq = Vec::new();
        seq.extend_from_slice(b"\x1b]52;c;");
        seq.resize(seq.len() + MAX_PAYLOAD + 1024, b'A');
        seq.extend_from_slice(b"\x07");
        let mut s = Osc52Scanner::new();
        s.feed(&seq);
        assert!(s.take().is_empty(), "oversized sequence produces no event");
        // The scanner recovered to a clean state and still works afterward.
        s.feed(b"\x1b]52;c;aGk=\x07");
        assert_eq!(s.take(), vec!["hi".to_string()]);
    }

    #[test]
    fn back_to_back_sequences_both_decode() {
        // "aGVsbG8=" is base64 for "hello".
        assert_eq!(
            scan(b"\x1b]52;c;aGk=\x07\x1b]52;p;aGVsbG8=\x07"),
            vec!["hi".to_string(), "hello".to_string()],
        );
    }
}
