//! The render frame protocol: what the engine emits after processing PTY
//! bytes, and what a renderer (the app's canvas terminal view) consumes.
//!
//! Frames carry only dirty rows as style *runs* (consecutive cells sharing
//! fg/bg/flags), which keeps a 200-column row to a handful of entries
//! instead of 200 per-cell objects.

use serde::Serialize;

/// Style flag bits on a [`Run`]. Matches what the renderer needs to draw;
/// underline variants (double/curly/...) are collapsed to one flag.
pub mod flags {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const FAINT: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const INVISIBLE: u16 = 1 << 5;
    pub const STRIKETHROUGH: u16 = 1 << 6;
    pub const OVERLINE: u16 = 1 << 7;
}

/// A run of consecutive cells on one row sharing the same style.
///
/// `width` is in terminal columns and can exceed `text` char count when the
/// run contains wide (CJK/emoji) characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// Starting column of the run.
    pub x: u16,
    /// Total column width of the run.
    pub width: u16,
    pub text: String,
    /// Packed 0xRRGGBB; `None` means "use the terminal default".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<u32>,
    #[serde(skip_serializing_if = "is_zero")]
    pub flags: u16,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

/// One changed viewport row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowUpdate {
    /// Viewport row index (0 = top).
    pub y: u16,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// Terminal-level default colors, packed 0xRRGGBB. Always resolved — runs
/// with `fg: None`/`bg: None` fall back to these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Colors {
    pub fg: u32,
    pub bg: u32,
}

/// Terminal mode hints the renderer needs for input encoding and wheel
/// behavior. Mirrors what xterm.js tracks internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Modes {
    /// DECCKM: arrows send SS3 (`ESC O A`) instead of CSI (`ESC [ A`).
    pub app_cursor_keys: bool,
    /// Mode 2004: wrap pasted text in `ESC [200~` / `ESC [201~`.
    pub bracketed_paste: bool,
    /// Alternate screen active (fullscreen TUI; wheel becomes arrow keys).
    pub alt_screen: bool,
    /// Any mouse tracking mode enabled.
    pub mouse_tracking: bool,
}

/// One render frame: everything that changed since the previous frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// When true the renderer must repaint everything; `rows` contains all
    /// viewport rows, not just changed ones.
    pub full: bool,
    pub cols: u16,
    pub rows: u16,
    pub changed: Vec<RowUpdate>,
    pub cursor: Cursor,
    pub colors: Colors,
    pub modes: Modes,
    /// Present only on the frame where the OSC 0/2 title changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Rows available above the viewport (drives the scrollbar).
    pub scrollback_rows: usize,
}
