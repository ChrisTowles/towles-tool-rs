//! Minimal OSC 10/11 *color query* scanner run alongside the terminal state
//! machine on the raw PTY byte feed.
//!
//! libghostty-vt answers CSI-level queries (DA1, DSR, `CSI ? 996 n`) through
//! its pty-write effect, but an OSC color *query* (`ESC ] 11 ; ? ST`)
//! produces no reply at all in 0.2.0 — verified empirically; only the *set*
//! form is applied. Programs probe OSC 11 to learn the terminal background
//! (it's how Claude Code and many TUIs pick a dark or light palette), so the
//! engine recognizes the query itself and synthesizes the xterm-format reply
//! from the terminal's *effective* colors — a program's own OSC 10/11
//! set-override wins over the app theme, matching xterm.
//!
//! Same tradeoffs as [`crate::osc52`]: the bytes are re-scanned by a tiny
//! byte-at-a-time state machine (no allocation on the hot path), sequences
//! split across feed calls are reassembled, and anything malformed is
//! dropped silently. Only the query form (`Pt == "?"`) is recognized here —
//! sets are libghostty's job. Indexed OSC 4 queries are deliberately not
//! handled (rarely probed; revisit on demand).

/// Which default color an OSC query asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorQuery {
    /// OSC 10 — default foreground.
    Foreground,
    /// OSC 11 — default background.
    Background,
}

impl ColorQuery {
    /// The OSC identifier to echo back in the reply.
    pub fn ident(self) -> u8 {
        match self {
            ColorQuery::Foreground => 10,
            ColorQuery::Background => 11,
        }
    }
}

/// How the query was terminated; the reply mirrors it (xterm behavior). A C1
/// ST (0x9C) query is answered with the 7-bit `ESC \` form, which every
/// consumer accepts and can't corrupt UTF-8 output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminator {
    Bel,
    St,
}

impl Terminator {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Terminator::Bel => b"\x07",
            Terminator::St => b"\x1b\\",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Scanning for `ESC`.
    Ground,
    /// Saw `ESC`; expecting `]` to begin an OSC.
    Esc,
    /// Inside an OSC, accumulating the numeric identifier until `;`.
    Ident,
    /// Identifier was 10/11; expecting exactly `?`.
    Query(ColorQuery),
    /// Saw `?`; expecting a terminator.
    Term(ColorQuery),
    /// Inside `Term`, saw `ESC`; expecting `\` to complete a String Terminator.
    TermEsc(ColorQuery),
}

/// Guard on the OSC numeric identifier length; real OSC numbers are 1–3
/// digits (mirrors [`crate::osc52`]).
const MAX_IDENT: usize = 8;

/// Streaming recognizer for OSC 10/11 color queries. Feed it the same bytes
/// as the terminal engine; drain recognized queries with [`take`].
///
/// [`take`]: OscColorScanner::take
#[derive(Debug)]
pub struct OscColorScanner {
    state: State,
    ident: Vec<u8>,
    pending: Vec<(ColorQuery, Terminator)>,
}

impl Default for OscColorScanner {
    fn default() -> Self {
        Self { state: State::Ground, ident: Vec::new(), pending: Vec::new() }
    }
}

impl OscColorScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a chunk of PTY output, recognizing any OSC 10/11 color queries
    /// within (including those spanning previous chunks).
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    /// Take the queries recognized so far, in order.
    pub fn take(&mut self) -> Vec<(ColorQuery, Terminator)> {
        std::mem::take(&mut self.pending)
    }

    fn step(&mut self, b: u8) {
        const ESC: u8 = 0x1b;
        const BEL: u8 = 0x07;
        const ST_C1: u8 = 0x9c;
        self.state = match self.state {
            State::Ground => {
                if b == ESC {
                    State::Esc
                } else {
                    State::Ground
                }
            }
            State::Esc => match b {
                b']' => {
                    self.ident.clear();
                    State::Ident
                }
                ESC => State::Esc, // stay armed on a run of ESCs
                _ => State::Ground,
            },
            State::Ident => match b {
                b';' => match self.ident.as_slice() {
                    b"10" => State::Query(ColorQuery::Foreground),
                    b"11" => State::Query(ColorQuery::Background),
                    _ => State::Ground,
                },
                b'0'..=b'9' => {
                    self.ident.push(b);
                    if self.ident.len() > MAX_IDENT {
                        State::Ground
                    } else {
                        State::Ident
                    }
                }
                ESC => State::Esc,
                _ => State::Ground, // non-numeric OSC ident
            },
            State::Query(q) => match b {
                b'?' => State::Term(q),
                ESC => State::Esc,
                _ => State::Ground, // a set (`Pt` is a color spec), not a query
            },
            State::Term(q) => match b {
                BEL => {
                    self.pending.push((q, Terminator::Bel));
                    State::Ground
                }
                ST_C1 => {
                    self.pending.push((q, Terminator::St));
                    State::Ground
                }
                ESC => State::TermEsc(q),
                _ => State::Ground, // `?` followed by more data: not a bare query
            },
            State::TermEsc(q) => match b {
                b'\\' => {
                    self.pending.push((q, Terminator::St));
                    State::Ground
                }
                ESC => State::Esc,
                _ => State::Ground,
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> Vec<(ColorQuery, Terminator)> {
        let mut s = OscColorScanner::new();
        s.feed(bytes);
        s.take()
    }

    #[test]
    fn recognizes_bel_and_st_terminated_queries() {
        assert_eq!(scan(b"\x1b]11;?\x07"), vec![(ColorQuery::Background, Terminator::Bel)]);
        assert_eq!(scan(b"\x1b]11;?\x1b\\"), vec![(ColorQuery::Background, Terminator::St)]);
        assert_eq!(scan(b"\x1b]10;?\x07"), vec![(ColorQuery::Foreground, Terminator::Bel)]);
    }

    #[test]
    fn ignores_color_sets_and_other_oscs() {
        // A set (real color payload) is libghostty's to apply, not ours.
        assert!(scan(b"\x1b]11;#101010\x07").is_empty());
        assert!(scan(b"\x1b]0;title\x07").is_empty());
        // OSC 110/111 (reset) share the "1" prefix — must not match.
        assert!(scan(b"\x1b]111;?\x07").is_empty());
    }

    #[test]
    fn reassembles_a_query_split_across_feeds() {
        let mut s = OscColorScanner::new();
        s.feed(b"out\x1b]1");
        s.feed(b"1;");
        assert!(s.take().is_empty());
        s.feed(b"?\x07more");
        assert_eq!(s.take(), vec![(ColorQuery::Background, Terminator::Bel)]);
    }

    #[test]
    fn back_to_back_queries_both_surface() {
        assert_eq!(
            scan(b"\x1b]10;?\x07\x1b]11;?\x1b\\"),
            vec![
                (ColorQuery::Foreground, Terminator::Bel),
                (ColorQuery::Background, Terminator::St),
            ],
        );
    }
}
