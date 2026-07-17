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
    /// Underline style past Single — 2 double, 3 curly, 4 dotted, 5 dashed
    /// (SGR 4:x). Absent for none/single; `flags::UNDERLINE` still answers
    /// "any underline" so the renderer keeps its one-bit fast path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ul: Option<u8>,
    /// SGR 58 underline color, packed 0xRRGGBB; absent = underline in `fg`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ulc: Option<u32>,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// One changed viewport row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowUpdate {
    /// Viewport row index (0 = top).
    pub y: u16,
    pub runs: Vec<Run>,
    /// Row-local selected column range, inclusive, when the row intersects
    /// the active selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sel: Option<(u16, u16)>,
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
    /// Cursor color a program set (OSC 12), packed 0xRRGGBB; absent = theme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// The program signalled password input — the renderer shows a lock hint.
    #[serde(skip_serializing_if = "is_false")]
    pub password: bool,
}

/// Terminal-level default colors, packed 0xRRGGBB. Always resolved — runs
/// with `fg: None`/`bg: None` fall back to these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Colors {
    pub fg: u32,
    pub bg: u32,
}

/// Terminal mode hints the view needs for input *routing* — all encoding
/// happens in the engine, so only the modes that change where an event goes
/// ship on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Modes {
    /// Alternate screen active (fullscreen TUI owns the scrollback chords).
    pub alt_screen: bool,
    /// Any mouse tracking mode enabled (clicks go to the program, not local
    /// selection; Shift bypasses).
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
    /// Present only on the frame where the OSC 7 working directory changed
    /// (a `file://` URI as the shell reported it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd: Option<String>,
    /// Rows available above the viewport (drives the scrollbar).
    pub scrollback_rows: usize,
    /// Absolute row index of the viewport's top (0 = oldest scrollback row);
    /// equals `scrollback_rows` at the live bottom. Maps absolute search
    /// match rows onto viewport rows.
    pub viewport_top: usize,
}

#[cfg(test)]
mod tests {
    //! These tests pin the *serialized* wire shape the renderer consumes
    //! (`apps/client/src/lib/term-protocol.ts`). A silent serde rename or a
    //! changed `skip_serializing_if` would break the frontend at runtime, not
    //! at compile time, so we assert the JSON directly rather than round-trip.

    use super::*;
    use serde_json::{json, to_value};

    fn cursor() -> Cursor {
        Cursor {
            x: 3,
            y: 4,
            visible: true,
            shape: CursorShape::Block,
            blinking: false,
            color: None,
            password: false,
        }
    }

    #[test]
    fn flag_bits_are_a_stable_bitfield() {
        // The renderer ANDs these exact values out of `Run.flags`; they must
        // stay a contiguous power-of-two set.
        assert_eq!(flags::BOLD, 1);
        assert_eq!(flags::ITALIC, 2);
        assert_eq!(flags::FAINT, 4);
        assert_eq!(flags::UNDERLINE, 8);
        assert_eq!(flags::INVERSE, 16);
        assert_eq!(flags::INVISIBLE, 32);
        assert_eq!(flags::STRIKETHROUGH, 64);
        assert_eq!(flags::OVERLINE, 128);
    }

    #[test]
    fn run_serializes_all_fields_when_present() {
        let run = Run {
            x: 5,
            width: 10,
            text: "hello".to_string(),
            fg: Some(0x00ff00),
            bg: Some(0x000000),
            flags: flags::BOLD | flags::UNDERLINE,
            ul: Some(3),
            ulc: Some(0xff0000),
        };
        assert_eq!(
            to_value(&run).unwrap(),
            json!({
                "x": 5,
                "width": 10,
                "text": "hello",
                "fg": 0x00ff00,
                "bg": 0x000000,
                "flags": 9,
                "ul": 3,
                "ulc": 0xff0000,
            }),
        );
    }

    #[test]
    fn run_omits_default_color_and_zero_flags() {
        // `fg`/`bg` None means "terminal default" and `flags == 0` means plain
        // text; all three are skipped to keep the common run compact. The TS
        // reader treats their absence as exactly those defaults.
        let run = Run {
            x: 0,
            width: 4,
            text: "text".to_string(),
            fg: None,
            bg: None,
            flags: 0,
            ul: None,
            ulc: None,
        };
        assert_eq!(to_value(&run).unwrap(), json!({ "x": 0, "width": 4, "text": "text" }),);
    }

    #[test]
    fn wide_run_width_exceeds_char_count() {
        // A CJK/emoji run occupies more columns than it has chars: two glyphs,
        // four columns. `width` is authoritative for layout, independent of
        // `text` length, and the serialized `width` must reflect the columns.
        let run = Run {
            x: 0,
            width: 4,
            text: "漢字".to_string(),
            fg: None,
            bg: None,
            flags: 0,
            ul: None,
            ulc: None,
        };
        assert_eq!(run.text.chars().count(), 2);
        assert!(run.width as usize > run.text.chars().count());
        assert_eq!(to_value(&run).unwrap()["width"], json!(4));
    }

    #[test]
    fn row_update_serializes_selection_as_inclusive_pair() {
        let row = RowUpdate {
            y: 2,
            runs: vec![Run {
                x: 0,
                width: 1,
                text: "a".to_string(),
                fg: None,
                bg: None,
                flags: 0,
                ul: None,
                ulc: None,
            }],
            sel: Some((1, 6)),
        };
        assert_eq!(
            to_value(&row).unwrap(),
            json!({
                "y": 2,
                "runs": [{ "x": 0, "width": 1, "text": "a" }],
                "sel": [1, 6],
            }),
        );
    }

    #[test]
    fn row_update_omits_absent_selection() {
        let row = RowUpdate { y: 0, runs: vec![], sel: None };
        assert_eq!(to_value(&row).unwrap(), json!({ "y": 0, "runs": [] }));
    }

    #[test]
    fn cursor_shape_serializes_lowercase() {
        assert_eq!(to_value(CursorShape::Block).unwrap(), json!("block"));
        assert_eq!(to_value(CursorShape::Bar).unwrap(), json!("bar"));
        assert_eq!(to_value(CursorShape::Underline).unwrap(), json!("underline"));
        assert_eq!(to_value(CursorShape::Hollow).unwrap(), json!("hollow"));
    }

    #[test]
    fn modes_use_camel_case_keys() {
        // These multi-word fields are where a serde rename would silently
        // diverge from the TS `Modes` type.
        let modes = Modes { alt_screen: true, mouse_tracking: false };
        assert_eq!(
            to_value(modes).unwrap(),
            json!({
                "altScreen": true,
                "mouseTracking": false,
            }),
        );
    }

    #[test]
    fn colors_serialize_packed_rgb() {
        let colors = Colors { fg: 0xd0d0d0, bg: 0x101010 };
        assert_eq!(to_value(colors).unwrap(), json!({ "fg": 0xd0d0d0, "bg": 0x101010 }));
    }

    #[test]
    fn full_frame_shape_is_pinned() {
        let frame = Frame {
            full: true,
            cols: 80,
            rows: 24,
            changed: vec![RowUpdate {
                y: 0,
                runs: vec![Run {
                    x: 0,
                    width: 2,
                    text: "hi".to_string(),
                    fg: Some(0xffffff),
                    bg: None,
                    flags: flags::BOLD,
                    ul: None,
                    ulc: None,
                }],
                sel: None,
            }],
            cursor: cursor(),
            colors: Colors { fg: 0xffffff, bg: 0x000000 },
            modes: Modes { alt_screen: false, mouse_tracking: false },
            title: Some("bash".to_string()),
            pwd: None,
            scrollback_rows: 100,
            viewport_top: 42,
        };
        assert_eq!(
            to_value(&frame).unwrap(),
            json!({
                "full": true,
                "cols": 80,
                "rows": 24,
                "changed": [{
                    "y": 0,
                    "runs": [{ "x": 0, "width": 2, "text": "hi", "fg": 0xffffff, "flags": 1 }],
                }],
                "cursor": {
                    "x": 3, "y": 4, "visible": true, "shape": "block", "blinking": false,
                },
                "colors": { "fg": 0xffffff, "bg": 0x000000 },
                "modes": {
                    "altScreen": false,
                    "mouseTracking": false,
                },
                "title": "bash",
                "scrollbackRows": 100,
                "viewportTop": 42,
            }),
        );
    }

    #[test]
    fn frame_omits_absent_title() {
        let frame = Frame {
            full: false,
            cols: 1,
            rows: 1,
            changed: vec![],
            cursor: cursor(),
            colors: Colors { fg: 0, bg: 0 },
            modes: Modes { alt_screen: false, mouse_tracking: false },
            title: None,
            pwd: None,
            scrollback_rows: 0,
            viewport_top: 0,
        };
        let value = to_value(&frame).unwrap();
        assert!(value.get("title").is_none(), "title omitted when None");
        // The multi-word Frame keys survive even on a minimal frame.
        assert!(value.get("scrollbackRows").is_some());
        assert!(value.get("viewportTop").is_some());
    }
}
