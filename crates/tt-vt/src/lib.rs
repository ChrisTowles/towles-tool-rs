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
pub mod session;

pub use engine::{Engine, EngineOptions, VtError};
pub use frame::Frame;
pub use session::{Event, Input, Session, SpawnError};

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
                Event::PtyReply(_) => {}
            }
        };
        assert_eq!(
            frame.changed.iter().find(|r| r.y == 0).map(|r| r.runs[0].text.clone()),
            Some("hello".into())
        );
        drop(session); // joins the thread
    }
}
