//! Minimal OSC 9 / OSC 777 *desktop notification* scanner run alongside the
//! terminal state machine on the raw PTY byte feed.
//!
//! Programs raise attention with `ESC ] 9 ; <body> ST` (the iTerm2/ConEmu
//! convention — what Claude Code emits when it needs input) or
//! `ESC ] 777 ; notify ; <title> ; <body> ST` (the urxvt extension).
//! libghostty-vt identifies `ShowDesktopNotification` in its standalone OSC
//! parser but exposes no payload accessor and no terminal callback, so —
//! like [`crate::osc52`] and [`crate::osc_color`] — a byte-at-a-time scanner
//! fills the gap.
//!
//! ConEmu overloads OSC 9 with numeric sub-commands (`9;4;st;pr` progress,
//! `9;1;…` message box, …); a body starting `<digits> ;`—or a bare number—is
//! treated as one of those and skipped rather than surfaced as a phantom
//! notification. Fragment-tolerant and bounded like the sibling scanners.

/// Hard cap on a collected notification body; a hostile unterminated
/// sequence can't grow the buffer without bound.
const MAX_BODY: usize = 4 * 1024;

/// Guard on the OSC numeric identifier length (mirrors the sibling scanners).
const MAX_IDENT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Scanning for `ESC`.
    Ground,
    /// Saw `ESC`; expecting `]` to begin an OSC.
    Esc,
    /// Inside an OSC, accumulating the numeric identifier until `;`.
    Ident,
    /// Identifier was 9/777; collecting the body until a terminator.
    Collect,
    /// Inside `Collect`, saw `ESC`; expecting `\` to complete an ST.
    CollectEsc,
}

/// Which notification form is being collected (they differ in body layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `OSC 9 ; body`.
    Osc9,
    /// `OSC 777 ; notify ; title ; body`.
    Osc777,
}

/// Streaming recognizer for OSC 9 / OSC 777 notifications. Feed it the same
/// bytes as the terminal engine; drain notification bodies with [`take`].
///
/// [`take`]: OscNotifyScanner::take
#[derive(Debug)]
pub struct OscNotifyScanner {
    state: State,
    form: Form,
    ident: Vec<u8>,
    body: Vec<u8>,
    pending: Vec<String>,
}

impl Default for OscNotifyScanner {
    fn default() -> Self {
        Self {
            state: State::Ground,
            form: Form::Osc9,
            ident: Vec::new(),
            body: Vec::new(),
            pending: Vec::new(),
        }
    }
}

impl OscNotifyScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a chunk of PTY output, recognizing notifications within
    /// (including those spanning previous chunks).
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    /// Take the notification bodies recognized so far, in order.
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
                ESC => {}
                _ => self.state = State::Ground,
            },
            State::Ident => match b {
                b';' => match self.ident.as_slice() {
                    b"9" => {
                        self.form = Form::Osc9;
                        self.body.clear();
                        self.state = State::Collect;
                    }
                    b"777" => {
                        self.form = Form::Osc777;
                        self.body.clear();
                        self.state = State::Collect;
                    }
                    _ => self.reset(),
                },
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
                _ => self.reset(),
            },
            State::Collect => match b {
                BEL | ST_C1 => self.finish(),
                ESC => self.state = State::CollectEsc,
                _ => {
                    if self.body.len() >= MAX_BODY {
                        self.reset();
                    } else {
                        self.body.push(b);
                    }
                }
            },
            State::CollectEsc => match b {
                b'\\' => self.finish(),
                ESC => {
                    self.reset();
                    self.state = State::Esc;
                }
                _ => self.reset(),
            },
        }
    }

    fn finish(&mut self) {
        let body = std::mem::take(&mut self.body);
        let form = self.form;
        self.reset();
        let text = String::from_utf8_lossy(&body);
        let message: Option<String> = match form {
            Form::Osc9 => {
                // ConEmu numeric sub-commands ("4;1;50" progress, "1;…"
                // message box, or a bare number) are not notifications.
                let numeric_op = text
                    .split_once(';')
                    .map(|(head, _)| !head.is_empty() && head.bytes().all(|c| c.is_ascii_digit()))
                    .unwrap_or_else(|| {
                        !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit())
                    });
                (!numeric_op && !text.is_empty()).then(|| text.into_owned())
            }
            Form::Osc777 => {
                // 777;notify;title;body — anything else under 777 is ignored.
                let mut parts = text.splitn(3, ';');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some("notify"), Some(title), body) => {
                        let body = body.unwrap_or("");
                        let joined = if body.is_empty() {
                            title.to_string()
                        } else if title.is_empty() {
                            body.to_string()
                        } else {
                            format!("{title}: {body}")
                        };
                        (!joined.is_empty()).then_some(joined)
                    }
                    _ => None,
                }
            }
        };
        if let Some(message) = message {
            self.pending.push(message);
        }
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.ident.clear();
        self.body.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> Vec<String> {
        let mut s = OscNotifyScanner::new();
        s.feed(bytes);
        s.take()
    }

    #[test]
    fn osc9_body_becomes_a_notification() {
        assert_eq!(
            scan(b"\x1b]9;Claude needs your input\x07"),
            vec!["Claude needs your input".to_string()]
        );
        assert_eq!(scan(b"\x1b]9;done\x1b\\"), vec!["done".to_string()]);
    }

    #[test]
    fn conemu_numeric_ops_are_not_notifications() {
        assert!(scan(b"\x1b]9;4;1;50\x07").is_empty(), "progress");
        assert!(scan(b"\x1b]9;2;message box\x07").is_empty(), "message box op");
        assert!(scan(b"\x1b]9;12\x07").is_empty(), "bare numeric op");
    }

    #[test]
    fn osc777_notify_joins_title_and_body() {
        assert_eq!(
            scan(b"\x1b]777;notify;Build;tests passed\x1b\\"),
            vec!["Build: tests passed".to_string()]
        );
        assert_eq!(scan(b"\x1b]777;notify;just a title\x07"), vec!["just a title".to_string()]);
        assert!(scan(b"\x1b]777;other;thing\x07").is_empty());
    }

    #[test]
    fn reassembles_across_feeds_and_ignores_other_oscs() {
        let mut s = OscNotifyScanner::new();
        s.feed(b"out\x1b]9;hel");
        assert!(s.take().is_empty());
        s.feed(b"lo\x07");
        assert_eq!(s.take(), vec!["hello".to_string()]);
        assert!(scan(b"\x1b]0;title\x07").is_empty());
    }

    #[test]
    fn oversized_body_is_abandoned_and_scanner_recovers() {
        let mut seq = Vec::new();
        seq.extend_from_slice(b"\x1b]9;");
        seq.resize(seq.len() + MAX_BODY + 512, b'A');
        seq.extend_from_slice(b"\x07");
        let mut s = OscNotifyScanner::new();
        s.feed(&seq);
        assert!(s.take().is_empty());
        s.feed(b"\x1b]9;ok\x07");
        assert_eq!(s.take(), vec!["ok".to_string()]);
    }
}
