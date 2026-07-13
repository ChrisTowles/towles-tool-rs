//! Terminal state on libghostty-vt.
//!
//! The app's PTYs feed raw bytes in; renderers get [`frame::Frame`]s out —
//! dirty rows as style runs, cursor, title, scrollback depth. No rendering
//! and no PTY handling lives here (Ghostty's library deliberately excludes
//! both), and per the workspace rule this crate is Tauri-free.
//!
//! Building this crate compiles libghostty-vt from source at the Rust
//! binding's pinned ghostty commit and needs `zig` 0.15.x on PATH
//! (dotfiles `functions/18-zig.sh`).

pub mod engine;
pub mod frame;
pub mod osc52;
pub mod search;
pub mod session;

pub use engine::{Engine, EngineOptions, Select, VtError};
pub use frame::{Frame, Modes};
pub use search::SearchMatch;
pub use session::{Event, Input, Sender, Session, SpawnError};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::flags;

    fn engine(cols: u16, rows: u16) -> Engine {
        Engine::new(EngineOptions { cols, rows, max_scrollback: 1000 }).expect("engine")
    }

    fn row_text(frame: &Frame, y: u16) -> String {
        frame
            .changed
            .iter()
            .find(|r| r.y == y)
            .map(|r| r.runs.iter().map(|run| run.text.as_str()).collect::<String>())
            .unwrap_or_default()
    }

    #[test]
    fn styled_text_becomes_runs() {
        let mut e = engine(40, 5);
        e.feed(b"plain \x1b[1;32mbold-green\x1b[0m tail");
        let frame = e.render().expect("render").expect("first frame");

        assert!(frame.full, "first frame is a full redraw");
        assert_eq!(frame.cols, 40);
        assert_eq!(row_text(&frame, 0), "plain bold-green tail");

        let row = frame.changed.iter().find(|r| r.y == 0).unwrap();
        let green = row.runs.iter().find(|r| r.text == "bold-green").expect("styled run split out");
        assert_ne!(green.flags & flags::BOLD, 0);
        assert!(green.fg.is_some(), "palette green resolves to an rgb fg");
        let plain = row.runs.iter().find(|r| r.text.starts_with("plain")).unwrap();
        assert_eq!(plain.flags, 0);
        assert_eq!(plain.fg, None);
    }

    #[test]
    fn render_is_incremental() {
        let mut e = engine(20, 4);
        e.feed(b"one\r\n");
        e.render().expect("render").expect("frame");

        assert!(e.render().expect("render").is_none(), "nothing changed since last render");

        e.feed(b"two");
        let frame = e.render().expect("render").expect("frame");
        assert!(!frame.full);
        assert_eq!(frame.changed.len(), 1, "only the cursor row changed");
        assert_eq!(row_text(&frame, 1), "two");
    }

    #[test]
    fn cursor_only_move_still_renders_a_frame() {
        let mut e = engine(20, 4);
        e.feed(b"abc");
        let frame = e.render().expect("render").expect("first frame");
        assert_eq!(frame.cursor.x, 3);
        assert!(e.render().expect("render").is_none(), "engine is clean");

        // CUB: move the cursor left with no cell writes — libghostty-vt's
        // dirty tracking only covers cell content, so without tracking the
        // cursor separately this render would wrongly report nothing changed
        // and the frontend would never learn the cursor moved.
        e.feed(b"\x1b[D");
        let frame = e.render().expect("render").expect("cursor move must still produce a frame");
        assert!(!frame.full);
        assert!(frame.changed.is_empty(), "no cell content changed, only the cursor");
        assert_eq!(frame.cursor.x, 2);

        assert!(e.render().expect("render").is_none(), "clean again after the cursor settles");
    }

    #[test]
    fn request_full_forces_a_full_frame_from_a_clean_engine() {
        let mut e = engine(20, 4);
        e.feed(b"one\r\ntwo");
        e.render().expect("render").expect("first frame");
        assert!(e.render().expect("render").is_none(), "engine is clean");

        // A re-shown pane asks for a full repaint even though nothing is
        // dirty — the frame must exist, be full, and carry every row (#47).
        e.request_full();
        let frame = e.render().expect("render").expect("forced frame");
        assert!(frame.full);
        assert_eq!(frame.changed.len(), 4, "all viewport rows resent");
        assert_eq!(row_text(&frame, 0), "one");
        assert_eq!(row_text(&frame, 1), "two");

        // One-shot: the next render is incremental again.
        assert!(e.render().expect("render").is_none());
    }

    #[test]
    fn title_change_is_reported_once() {
        let mut e = engine(20, 4);
        e.feed(b"\x1b]0;my-title\x07");
        let frame = e.render().expect("render").expect("frame");
        assert_eq!(frame.title.as_deref(), Some("my-title"));

        e.feed(b"x");
        let frame = e.render().expect("render").expect("frame");
        assert_eq!(frame.title, None, "unchanged title is not re-sent");
    }

    #[test]
    fn device_query_produces_pty_reply() {
        let mut e = engine(20, 4);
        e.feed(b"\x1b[c"); // DA1: who are you?
        let reply = e.take_pty_output();
        assert!(reply.starts_with(b"\x1b["), "DA1 reply written back for the PTY, got {reply:?}");
    }

    #[test]
    fn resize_reflows_wrapped_lines() {
        let mut e = engine(10, 4);
        e.feed(b"aaaaaaaaaabbbb"); // wraps after 10 cols
        let frame = e.render().expect("render").expect("frame");
        assert_eq!(row_text(&frame, 0), "aaaaaaaaaa");
        assert_eq!(row_text(&frame, 1), "bbbb");

        e.resize(20, 4, 8, 16).expect("resize");
        let frame = e.render().expect("render").expect("frame");
        assert_eq!(frame.cols, 20);
        assert_eq!(row_text(&frame, 0), "aaaaaaaaaabbbb", "line unwraps on widen");
    }

    #[test]
    fn modes_track_terminal_state() {
        let mut e = engine(20, 4);
        e.feed(b"x");
        let frame = e.render().expect("render").expect("frame");
        assert!(!frame.modes.app_cursor_keys);
        assert!(!frame.modes.alt_screen);

        // Enter alt screen (1049), app cursor keys (DECCKM), bracketed paste.
        e.feed(b"\x1b[?1049h\x1b[?1h\x1b[?2004h");
        let frame = e.render().expect("render").expect("frame");
        assert!(frame.modes.app_cursor_keys);
        assert!(frame.modes.alt_screen);
        assert!(frame.modes.bracketed_paste);
        assert!(!frame.modes.mouse_tracking);

        // Mode flips alone don't dirty cells (no frame goes out until
        // something paints), so ride along with a visible byte.
        e.feed(b"\x1b[?1000hx");
        let frame = e.render().expect("render").expect("frame");
        assert!(frame.modes.mouse_tracking);
    }

    #[test]
    fn wheel_reports_only_when_the_app_tracks_the_mouse() {
        let mut e = engine(20, 4);
        e.feed(b"hi");
        assert!(e.take_pty_output().is_empty());

        // No mouse tracking: a wheel gesture writes nothing to the PTY — in
        // particular it is never translated into arrow keys.
        e.wheel(3, 1, -2).expect("wheel");
        assert!(e.take_pty_output().is_empty(), "untracked wheel must not reach the app");

        // Normal tracking (1000) + SGR encoding (1006), vim/htop style:
        // wheel up is a button-64 press at the 1-based cell position.
        e.feed(b"\x1b[?1000h\x1b[?1006h");
        e.wheel(3, 1, -1).expect("wheel");
        assert_eq!(e.take_pty_output(), b"\x1b[<64;4;2M");

        // Down is button 65; one report per line.
        e.wheel(3, 1, 2).expect("wheel");
        assert_eq!(e.take_pty_output(), b"\x1b[<65;4;2M\x1b[<65;4;2M");

        // A fling's report count is capped.
        e.wheel(0, 0, -100).expect("wheel");
        assert_eq!(e.take_pty_output(), b"\x1b[<64;1;1M".repeat(5));

        // Legacy tracking without SGR gets the negotiated X10 encoding
        // (button and coords as offset bytes), not SGR.
        e.feed(b"\x1b[?1006l");
        e.wheel(3, 1, -1).expect("wheel");
        assert_eq!(e.take_pty_output(), b"\x1b[M\x60\x24\x22");
    }

    #[test]
    fn scroll_moves_viewport_into_scrollback() {
        let mut e = engine(10, 3);
        for i in 0..20 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.render().expect("render").expect("frame");

        e.scroll(Some(-5));
        let frame = e.render().expect("render").expect("scrolled frame");
        assert!(frame.changed.iter().any(|r| !r.runs.is_empty()));
        let top = row_text(&frame, 0);
        assert!(
            top.starts_with("line") && top != "line18",
            "viewport moved off the live tail, got {top:?}"
        );

        e.scroll(None); // back to bottom
        let frame = e.render().expect("render").expect("frame");
        assert!(row_text(&frame, 0).starts_with("line"));
    }

    #[test]
    fn clear_scrollback_drops_history_but_keeps_the_visible_screen() {
        let mut e = engine(10, 3);
        for i in 0..20 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        let frame = e.render().expect("render").expect("frame");
        // 20 lines through a 3-row viewport leaves a real scrollback tail.
        assert!(frame.scrollback_rows > 0, "precondition: scrollback exists");
        // The live viewport shows the most recent lines.
        assert_eq!(row_text(&frame, 0), "line18");
        assert_eq!(row_text(&frame, 1), "line19");

        e.clear_scrollback();
        let frame = e.render().expect("render").expect("clear forces a frame");
        assert!(frame.full, "clearing scrollback forces a full repaint");
        assert_eq!(frame.scrollback_rows, 0, "scrollback history dropped");
        assert_eq!(frame.viewport_top, 0, "viewport top collapses with the history");
        // The visible screen is untouched — same rows as before the clear.
        assert_eq!(row_text(&frame, 0), "line18");
        assert_eq!(row_text(&frame, 1), "line19");
    }

    #[test]
    fn selection_highlights_rows_and_copies_text() {
        let mut e = engine(20, 4);
        e.feed(b"alpha beta\r\ngamma");
        e.render().expect("render").expect("frame");

        e.select(engine::Select::Range { ax: 0, ay: 0, bx: 2, by: 1 }).expect("select");
        let frame = e.render().expect("render").expect("selection forces a frame");
        assert!(frame.full, "selection change repaints everything");
        let row0 = frame.changed.iter().find(|r| r.y == 0).unwrap();
        let row1 = frame.changed.iter().find(|r| r.y == 1).unwrap();
        assert_eq!(row0.sel, Some((0, 19)), "row 0 selected to line end");
        assert_eq!(row1.sel.map(|s| s.0), Some(0), "row 1 selected from col 0");

        let text = e.copy_selection().expect("copy").expect("selection text");
        assert!(text.contains("alpha beta") && text.contains("gam"), "got {text:?}");

        e.select(engine::Select::Clear).expect("clear");
        let frame = e.render().expect("render").expect("clear forces a frame");
        assert!(frame.changed.iter().all(|r| r.sel.is_none()));
        assert_eq!(e.copy_selection().expect("copy"), None);
    }

    #[test]
    fn word_selection_snaps_to_boundaries() {
        let mut e = engine(20, 4);
        e.feed(b"alpha beta");
        e.render().expect("render").expect("frame");

        e.select(engine::Select::Word { x: 7, y: 0 }).expect("select word");
        let text = e.copy_selection().expect("copy").expect("word text");
        assert_eq!(text, "beta");
    }

    #[test]
    fn wide_chars_advance_two_columns() {
        let mut e = engine(20, 4);
        e.feed("日本 x".as_bytes());
        let frame = e.render().expect("render").expect("frame");
        let row = frame.changed.iter().find(|r| r.y == 0).unwrap();
        assert_eq!(row_text(&frame, 0), "日本 x");
        let total: u16 = row.runs.iter().map(|r| r.width).sum();
        assert_eq!(total, 6, "2+2 wide cells + space + x");
        let cursor_row_run = &row.runs[0];
        assert_eq!(cursor_row_run.x, 0);
    }

    #[test]
    fn search_spans_scrollback_and_reports_absolute_rows() {
        let mut e = engine(10, 3);
        for i in 0..20 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.render().expect("render").expect("frame");

        // "line1" matches line1 plus line10..line19 (substring), top to bottom.
        let matches = e.search("line1", 100).expect("search");
        assert_eq!(matches.len(), 11);
        assert_eq!(matches[0], SearchMatch { row: 1, col: 0, width: 5 });
        assert!(matches.iter().all(|m| m.col == 0 && m.width == 5));
        assert!(matches.windows(2).all(|w| w[0].row < w[1].row), "ordered top to bottom");

        // The oldest row lives in scrollback, well above the 3-row viewport.
        assert_eq!(
            e.search("line0", 10).expect("search"),
            vec![SearchMatch { row: 0, col: 0, width: 5 }]
        );

        // The limit caps the result count.
        assert_eq!(e.search("line", 4).expect("search").len(), 4);
    }

    #[test]
    fn search_is_case_insensitive_and_column_exact_across_wide_chars() {
        let mut e = engine(20, 4);
        e.feed("Hello \x1b[1m日本\x1b[0m World".as_bytes());
        e.render().expect("render").expect("frame");

        assert_eq!(
            e.search("hello", 10).expect("search"),
            vec![SearchMatch { row: 0, col: 0, width: 5 }]
        );
        // "World" sits after two wide chars (4 columns) — column is grid-exact.
        assert_eq!(
            e.search("WORLD", 10).expect("search"),
            vec![SearchMatch { row: 0, col: 11, width: 5 }]
        );
        assert!(e.search("absent", 10).expect("search").is_empty());
        assert!(e.search("", 10).expect("search").is_empty());
    }

    #[test]
    fn scroll_to_moves_the_viewport_and_frames_carry_viewport_top() {
        let mut e = engine(10, 3);
        for i in 0..40 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        let frame = e.render().expect("render").expect("frame");
        // At the live bottom the viewport top equals the scrollback depth.
        assert_eq!(frame.viewport_top, frame.scrollback_rows);
        assert!(frame.viewport_top > 0);

        e.scroll_to(0).expect("scroll_to top row");
        let frame = e.render().expect("render").expect("scrolled frame");
        assert_eq!(frame.viewport_top, 0);
        assert_eq!(row_text(&frame, 0), "line0");

        // Scrolling to an already-visible row keeps the viewport put.
        e.scroll_to(1).expect("scroll_to visible row");
        assert_eq!(e.viewport_top().expect("viewport_top"), 0);
    }

    #[test]
    fn frame_serializes_to_compact_json() {
        let mut e = engine(20, 4);
        e.feed(b"hi");
        let frame = e.render().expect("render").expect("frame");
        let json = serde_json::to_value(&frame).expect("serialize");
        assert_eq!(json["full"], true);
        assert_eq!(json["changed"][0]["runs"][0]["text"], "hi");
        assert!(
            json["changed"][0]["runs"][0].get("fg").is_none(),
            "default colors are omitted from the wire format"
        );
    }

    #[test]
    fn session_thread_round_trips() {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let session = Session::spawn(
            EngineOptions { cols: 20, rows: 4, max_scrollback: 100 },
            move |event| {
                let _ = event_tx.send(event);
            },
        )
        .expect("spawn session");

        assert!(session.send(Input::Bytes(b"hello".to_vec())));
        let frame = loop {
            match event_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("session produced an event")
            {
                Event::Frame(f) => break f,
                Event::PtyReply(_) | Event::Clipboard(_) => {}
            }
        };
        assert_eq!(
            frame.changed.iter().find(|r| r.y == 0).map(|r| r.runs[0].text.clone()),
            Some("hello".into())
        );
        drop(session); // joins the thread
    }

    #[test]
    fn session_surfaces_osc52_clipboard_writes() {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let session = Session::spawn(
            EngineOptions { cols: 20, rows: 4, max_scrollback: 100 },
            move |event| {
                let _ = event_tx.send(event);
            },
        )
        .expect("spawn session");

        // A program copies "hi" via OSC 52 (aGk= is base64 for "hi").
        assert!(session.send(Input::Bytes(b"\x1b]52;c;aGk=\x07".to_vec())));
        let clip = loop {
            match event_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("session produced an event")
            {
                Event::Clipboard(text) => break text,
                Event::Frame(_) | Event::PtyReply(_) => {}
            }
        };
        assert_eq!(clip, "hi");
        drop(session);
    }

    #[test]
    fn session_coalesces_bursts_into_few_frames() {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let session = Session::spawn(
            EngineOptions { cols: 40, rows: 8, max_scrollback: 100 },
            move |event| {
                let _ = event_tx.send(event);
            },
        )
        .expect("spawn session");

        // A flood of small chunks, like a TUI redrawing on every keystroke.
        for i in 0..200 {
            assert!(session.send(Input::Bytes(format!("chunk {i}\r\n").into_bytes())));
        }
        // Dropping closes the channel; the thread drains queued input, renders,
        // and exits, which also drops the sink and ends the event stream.
        drop(session);

        let frames = event_rx.iter().filter(|e| matches!(e, Event::Frame(_))).count();
        assert!(frames > 0, "the burst must produce at least one frame");
        assert!(frames < 20, "200 rapid chunks must coalesce into few frames, got {frames}");
    }
}
