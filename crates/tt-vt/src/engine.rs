//! Single-threaded terminal engine: owns a libghostty-vt `Terminal` plus its
//! render state, turns PTY bytes into [`Frame`]s.
//!
//! libghostty-vt types are `!Send`, so an [`Engine`] must live and die on one
//! thread; [`crate::session`] provides the per-terminal thread wrapper.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;

use libghostty_vt::fmt::{Formatter, FormatterOptions};
use libghostty_vt::mouse;
use libghostty_vt::render::{
    CellIteration, CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator,
};
use libghostty_vt::screen::{CellContentTag, CellWide, Screen};
use libghostty_vt::selection::{FormatOptions, SelectLineOptions, SelectWordOptions, Selection};
use libghostty_vt::style::Underline;
use libghostty_vt::terminal::{
    ColorScheme, Mode, Options, Point, PointCoordinate, ScrollViewport, Terminal,
};

use crate::frame::{flags, Colors, Cursor, CursorShape, Frame, Modes};
use crate::osc52::Osc52Scanner;
use crate::osc_color::{ColorQuery, OscColorScanner};
use crate::search::{self, SearchMatch};

/// A selection operation, in viewport cell coordinates.
#[derive(Debug, Clone, Copy)]
pub enum Select {
    /// Anchor→head drag selection (both ends inclusive).
    Range {
        ax: u16,
        ay: u16,
        bx: u16,
        by: u16,
    },
    /// Select the word at a cell (double-click).
    Word {
        x: u16,
        y: u16,
    },
    /// Select the line at a cell (triple-click).
    Line {
        x: u16,
        y: u16,
    },
    All,
    Clear,
}

/// Errors from the underlying libghostty-vt library.
#[derive(Debug, thiserror::Error)]
#[error("libghostty-vt: {0}")]
pub struct VtError(#[from] libghostty_vt::error::Error);

pub type Result<T> = std::result::Result<T, VtError>;

#[derive(Debug, Clone, Copy)]
pub struct EngineOptions {
    pub cols: u16,
    pub rows: u16,
    pub max_scrollback: usize,
}

/// UI theme pushed into the emulator so color queries answer the app's real
/// colors instead of libghostty's stock defaults: OSC 10/11 (how programs
/// like Claude Code detect a light vs dark background), the color-scheme DSR
/// (`CSI ? 996 n`), and indexed-color resolution all read from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Default foreground, packed 0xRRGGBB.
    pub fg: u32,
    /// Default background, packed 0xRRGGBB.
    pub bg: u32,
    /// Cursor color; `None` keeps libghostty's default (invert the cell).
    pub cursor: Option<u32>,
    /// ANSI colors 0–15. Entries 16–255 keep the stock cube/grays, which are
    /// theme-neutral by construction.
    pub palette16: [u32; 16],
    /// Whether the theme is dark — the `CSI ? 996 n` color-scheme answer.
    pub dark: bool,
}

/// Cap on mouse-wheel reports emitted for one wheel gesture. Bounds the
/// bytes a single high-delta event (a wheel fling, a coalesced touchpad
/// swipe) can inject into the application's input stream.
const MAX_WHEEL_REPORTS: u32 = 5;

pub struct Engine {
    term: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    /// Bytes the terminal wants written back to the PTY (DA1 replies etc.),
    /// filled synchronously during `feed` by the pty-write effect.
    pty_out: Rc<RefCell<Vec<u8>>>,
    title_changed: Rc<StdCell<bool>>,
    /// Whether the pushed theme is dark (see [`Theme::dark`]); read by the
    /// color-scheme query callback registered in [`Engine::new`].
    dark: Rc<StdCell<bool>>,
    /// Watches the byte feed for OSC 52 set-clipboard sequences (libghostty-vt
    /// exposes no clipboard callback); decoded copies are drained by
    /// [`Engine::take_clipboard`].
    osc52: Osc52Scanner,
    /// Watches the byte feed for OSC 10/11 color queries, which libghostty-vt
    /// does not answer; [`Engine::feed`] synthesizes the replies.
    osc_color: OscColorScanner,
    /// Force the next render to be a full frame (selection changed).
    force_full: bool,
    /// Cursor state as of the last emitted frame. libghostty-vt's dirty
    /// tracking only covers cell/row content, not the cursor — a pure
    /// cursor move (arrow keys with no cell writes) leaves `dirty()` clean,
    /// so without this a frame would never go out and the cursor would
    /// appear stuck until the next keystroke actually touched a cell.
    last_cursor: Option<Cursor>,
}

impl Engine {
    pub fn new(opts: EngineOptions) -> Result<Self> {
        let mut term = Terminal::new(Options {
            cols: opts.cols,
            rows: opts.rows,
            max_scrollback: opts.max_scrollback,
        })?;

        let pty_out: Rc<RefCell<Vec<u8>>> = Rc::default();
        let title_changed: Rc<StdCell<bool>> = Rc::default();
        // Dark until the first `set_theme` — the app's default look. The cell
        // is shared with the color-scheme query callback below.
        let dark: Rc<StdCell<bool>> = Rc::new(StdCell::new(true));
        term.on_pty_write({
            let pty_out = Rc::clone(&pty_out);
            move |_term, data| pty_out.borrow_mut().extend_from_slice(data)
        })?
        .on_title_changed({
            let title_changed = Rc::clone(&title_changed);
            move |_term| title_changed.set(true)
        })?
        .on_color_scheme({
            let dark = Rc::clone(&dark);
            move |_term| Some(if dark.get() { ColorScheme::Dark } else { ColorScheme::Light })
        })?;

        Ok(Self {
            term,
            render: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            pty_out,
            title_changed,
            dark,
            osc52: Osc52Scanner::new(),
            osc_color: OscColorScanner::new(),
            force_full: false,
            last_cursor: None,
        })
    }

    /// Push the UI theme into the emulator: default fg/bg/cursor colors, the
    /// ANSI 0–15 palette entries, and the dark/light answer for color-scheme
    /// queries. Forces a full repaint so rows rendered under the old theme
    /// pick up the new defaults.
    pub fn set_theme(&mut self, theme: &Theme) -> Result<()> {
        self.term.set_default_fg_color(Some(unpack_rgb(theme.fg)))?;
        self.term.set_default_bg_color(Some(unpack_rgb(theme.bg)))?;
        self.term.set_default_cursor_color(theme.cursor.map(unpack_rgb))?;
        let mut palette = self.term.default_color_palette()?;
        for (slot, packed) in palette.iter_mut().zip(theme.palette16) {
            *slot = unpack_rgb(packed);
        }
        self.term.set_default_color_palette(Some(palette))?;
        self.dark.set(theme.dark);
        self.force_full = true;
        Ok(())
    }

    /// Feed raw PTY output into the terminal state machine.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.term.vt_write(bytes);
        self.osc52.feed(bytes);
        self.osc_color.feed(bytes);
        // Answer OSC 10/11 color queries libghostty leaves unanswered, from
        // the *effective* colors — a program's own OSC set-override wins over
        // the pushed theme, matching xterm. An unreadable color (FFI error or
        // genuinely unset) skips the reply; the program times out like it
        // would on a dumb terminal.
        for (query, term) in self.osc_color.take() {
            let color = match query {
                ColorQuery::Foreground => self.term.fg_color(),
                ColorQuery::Background => self.term.bg_color(),
            };
            if let Ok(Some(c)) = color {
                let mut reply = format!(
                    "\x1b]{};rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}",
                    query.ident(),
                    c.r,
                    c.r,
                    c.g,
                    c.g,
                    c.b,
                    c.b
                )
                .into_bytes();
                reply.extend_from_slice(term.bytes());
                self.pty_out.borrow_mut().extend_from_slice(&reply);
            }
        }
    }

    /// Drain any OSC 52 set-clipboard writes recognized in the byte feed since
    /// the last call, in order. The caller writes these to the system clipboard
    /// (focus-gated); see [`crate::osc52`].
    pub fn take_clipboard(&mut self) -> Vec<String> {
        self.osc52.take()
    }

    /// Drain bytes the terminal produced in response to control sequences
    /// (device attribute queries, size reports, ...). The caller must write
    /// these back to the PTY.
    pub fn take_pty_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.pty_out.borrow_mut())
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        self.term.resize(cols, rows, cell_width_px, cell_height_px)?;
        Ok(())
    }

    /// Scroll the viewport into scrollback (`delta` rows; up is negative).
    /// `None` jumps back to the bottom (live) position.
    pub fn scroll(&mut self, delta: Option<isize>) {
        self.term.scroll_viewport(match delta {
            Some(d) => ScrollViewport::Delta(d),
            None => ScrollViewport::Bottom,
        });
    }

    /// Report a mouse-wheel gesture at viewport cell (`x`, `y`) to the
    /// application, encoded in whatever mouse protocol it negotiated (X10,
    /// SGR, ...) — one report per line (`lines` rows, up is negative), capped
    /// at [`MAX_WHEEL_REPORTS`]. When the application never enabled mouse
    /// tracking this writes nothing, so a stale mode hint on the caller's
    /// side can't inject bytes — and a wheel is never translated into arrow
    /// keys. Reports ride the same pty-out path as query replies; the caller
    /// drains them via [`Engine::take_pty_output`].
    pub fn wheel(&mut self, x: u16, y: u16, lines: i32) -> Result<()> {
        if lines == 0 {
            return Ok(());
        }
        let mut encoder = mouse::Encoder::new()?;
        encoder.set_options_from_terminal(&self.term).set_size(mouse::EncoderSize {
            // 1px cells make surface-space positions equal cell coordinates.
            screen_width: u32::from(self.term.cols()?),
            screen_height: u32::from(self.term.rows()?),
            cell_width: 1,
            cell_height: 1,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        });
        let mut event = mouse::Event::new()?;
        event
            .set_action(mouse::Action::Press)
            // xterm wheel buttons: 4 scrolls up, 5 scrolls down (press-only).
            .set_button(Some(if lines < 0 { mouse::Button::Four } else { mouse::Button::Five }))
            .set_position(mouse::Position { x: f32::from(x), y: f32::from(y) });
        let mut buf = Vec::new();
        for _ in 0..lines.unsigned_abs().min(MAX_WHEEL_REPORTS) {
            encoder.encode_to_vec(&event, &mut buf)?;
        }
        self.pty_out.borrow_mut().extend_from_slice(&buf);
        Ok(())
    }

    /// Apply a selection operation (viewport cell coordinates). Selection
    /// changes don't reliably mark rows dirty, so the next render is forced
    /// full to repaint highlights everywhere (including deselection).
    pub fn select(&mut self, op: Select) -> Result<()> {
        match op {
            Select::Range { ax, ay, bx, by } => {
                let a = self.grid_ref(ax, ay)?;
                let b = self.grid_ref(bx, by)?;
                let sel = Selection::new(a, b, false);
                self.term.set_selection(Some(&sel))?;
            }
            Select::Word { x, y } => {
                let g = self.grid_ref(x, y)?;
                if let Some(sel) = self.term.select_word(SelectWordOptions::new(g))? {
                    self.term.set_selection(Some(&sel))?;
                }
            }
            Select::Line { x, y } => {
                let g = self.grid_ref(x, y)?;
                if let Some(sel) = self.term.select_line(SelectLineOptions::new(g))? {
                    self.term.set_selection(Some(&sel))?;
                }
            }
            Select::All => {
                if let Some(sel) = self.term.select_all()? {
                    self.term.set_selection(Some(&sel))?;
                }
            }
            Select::Clear => {
                self.term.set_selection(None)?;
            }
        }
        self.force_full = true;
        Ok(())
    }

    /// Force the next render to emit a full frame even when libghostty
    /// reports nothing dirty. The UI calls this when a hidden pane becomes
    /// visible again: its canvas may hold stale content, and dirty-only
    /// frames would never resend rows the engine considers clean (#47).
    pub fn request_full(&mut self) {
        self.force_full = true;
    }

    /// Whether the application is holding a synchronized-output batch open
    /// (DEC private mode 2026): frames rendered between BSU (`CSI ? 2026 h`)
    /// and ESU (`CSI ? 2026 l`) would show a half-drawn update, so the
    /// session loop holds rendering while this is set (bounded by its
    /// max-hold — a program that dies mid-batch must not freeze the pane).
    /// A failed mode query reads as "not synchronized" so rendering can
    /// never get stuck on an FFI error.
    pub fn sync_output(&self) -> bool {
        self.term.mode(Mode::SYNC_OUTPUT).unwrap_or(false)
    }

    /// Drop the scrollback history while leaving the visible screen intact
    /// (right-click "Clear scrollback"). Feeds xterm's "erase saved lines"
    /// sequence (CSI 3 J), which discards rows scrolled off the top but does
    /// not touch the active viewport. Clearing scrollback doesn't dirty any
    /// visible row, so the next render is forced full — that way the frame
    /// carries the collapsed `scrollback_rows`/`viewport_top` to the UI (it
    /// derives "scrolled back" and search highlighting from those).
    pub fn clear_scrollback(&mut self) {
        self.feed(b"\x1b[3J");
        self.force_full = true;
    }

    /// Case-insensitively search the full screen (scrollback + active area)
    /// for `query`, top to bottom, up to `limit` matches. Rows come from a
    /// one-shot plain-text format pass (fast pre-filter); matching rows are
    /// then re-read cell by cell so match columns are exact across wide
    /// characters. Trailing whitespace is trimmed per row, so queries ending
    /// in spaces may miss end-of-line hits.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchMatch>> {
        let mut out = Vec::new();
        let probe = query.to_lowercase();
        if probe.is_empty() || limit == 0 {
            return Ok(out);
        }
        let mut formatter =
            Formatter::new(&self.term, FormatterOptions::new().with_unwrap(false).with_trim(true))?;
        let bytes = formatter.format_alloc(None)?;
        let text = String::from_utf8_lossy(&bytes);
        let cols = self.term.cols()?;
        for (row, line) in text.lines().enumerate() {
            if !line.to_lowercase().contains(&probe) {
                continue;
            }
            let cells = self.row_cells(row, cols)?;
            for (col, width) in search::match_row(&cells, query) {
                out.push(SearchMatch { row, col, width });
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    /// One absolute screen row's cells (char + column + width), skipping
    /// wide-char spacers. Empty cells read as spaces, like the render path.
    fn row_cells(&self, row: usize, cols: u16) -> Result<Vec<search::RowCell>> {
        let mut cells = Vec::with_capacity(cols as usize);
        for x in 0..cols {
            let point = Point::Screen(PointCoordinate { x, y: row as u32 });
            let cell = self.term.grid_ref(point)?.cell()?;
            let wide = cell.wide()?;
            if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                continue;
            }
            let width: u16 = if wide == CellWide::Wide { 2 } else { 1 };
            let ch = char::from_u32(cell.codepoint()?).filter(|c| *c != '\0').unwrap_or(' ');
            cells.push(search::RowCell { ch, col: x, width });
        }
        Ok(cells)
    }

    /// Absolute row index of the viewport's top (0 = oldest scrollback row).
    /// At the live bottom this equals the scrollback depth.
    pub fn viewport_top(&self) -> Result<usize> {
        Ok(self.term.scrollbar()?.offset as usize)
    }

    /// Scroll the viewport so absolute `row` is visible, about a third down
    /// from the top. No-op when the viewport is already there.
    pub fn scroll_to(&mut self, row: usize) -> Result<()> {
        let sb = self.term.scrollbar()?;
        let max_top = sb.total.saturating_sub(sb.len);
        let target = (row as u64).saturating_sub(sb.len / 3).min(max_top);
        let delta =
            i64::try_from(target).unwrap_or(i64::MAX) - i64::try_from(sb.offset).unwrap_or(0);
        if delta != 0 {
            self.term.scroll_viewport(ScrollViewport::Delta(delta as isize));
        }
        Ok(())
    }

    /// Plain text of the active selection, if any.
    pub fn copy_selection(&mut self) -> Result<Option<String>> {
        let bytes = self.term.format_selection_alloc(None, FormatOptions::new())?;
        Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    fn grid_ref(&self, x: u16, y: u16) -> Result<libghostty_vt::screen::GridRef<'_>> {
        Ok(self.term.grid_ref(Point::Viewport(PointCoordinate { x, y: u32::from(y) }))?)
    }

    /// Produce a frame of everything that changed since the last call, or
    /// `None` when nothing did.
    pub fn render(&mut self) -> Result<Option<Frame>> {
        let title = self
            .title_changed
            .replace(false)
            .then(|| self.term.title().map(str::to_owned))
            .transpose()?;

        let snap = self.render.update(&self.term)?;
        let dirty = snap.dirty()?;
        let force_full = std::mem::take(&mut self.force_full);

        let cursor_pos = snap.cursor_viewport()?;
        let cursor = Cursor {
            x: cursor_pos.map_or(0, |c| c.x),
            y: cursor_pos.map_or(0, |c| c.y),
            visible: snap.cursor_visible()? && cursor_pos.is_some(),
            shape: match snap.cursor_visual_style()? {
                CursorVisualStyle::Bar => CursorShape::Bar,
                CursorVisualStyle::Underline => CursorShape::Underline,
                CursorVisualStyle::BlockHollow => CursorShape::Hollow,
                _ => CursorShape::Block,
            },
            blinking: snap.cursor_blinking()?,
        };
        let cursor_moved = self.last_cursor != Some(cursor);

        if dirty == Dirty::Clean && title.is_none() && !force_full && !cursor_moved {
            return Ok(None);
        }
        self.last_cursor = Some(cursor);
        let full = dirty == Dirty::Full || force_full;

        let mut changed = Vec::new();
        let mut row_iter = self.rows.update(&snap)?;
        let mut y: u16 = 0;
        while let Some(row) = row_iter.next() {
            if full || row.dirty()? {
                let mut runs: Vec<crate::frame::Run> = Vec::new();
                let mut cell_iter = self.cells.update(row)?;
                let mut x: u16 = 0;
                while let Some(cell) = cell_iter.next() {
                    let raw = cell.raw_cell()?;
                    let wide = raw.wide()?;
                    if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                        continue;
                    }
                    let width: u16 = if wide == CellWide::Wide { 2 } else { 1 };

                    let style = cell.style()?;
                    let mut f: u16 = 0;
                    if style.bold {
                        f |= flags::BOLD;
                    }
                    if style.italic {
                        f |= flags::ITALIC;
                    }
                    if style.faint {
                        f |= flags::FAINT;
                    }
                    if style.underline != Underline::None {
                        f |= flags::UNDERLINE;
                    }
                    if style.inverse {
                        f |= flags::INVERSE;
                    }
                    if style.invisible {
                        f |= flags::INVISIBLE;
                    }
                    if style.strikethrough {
                        f |= flags::STRIKETHROUGH;
                    }
                    if style.overline {
                        f |= flags::OVERLINE;
                    }
                    let fg = cell.fg_color()?.map(pack_rgb);
                    let bg = cell.bg_color()?.map(pack_rgb);

                    // A cell tagged CodepointGrapheme carries a multi-codepoint
                    // cluster (combining marks, ZWJ emoji); pull its full base +
                    // trailing codepoints. Plain cells stay on the
                    // single-codepoint fast path (no per-cell Vec alloc).
                    let cell_text = match raw.content_tag()? {
                        CellContentTag::CodepointGrapheme => grapheme_text(cell)?,
                        _ => {
                            let cp = raw.codepoint()?;
                            char::from_u32(cp).filter(|c| *c != '\0').unwrap_or(' ').to_string()
                        }
                    };

                    match runs.last_mut() {
                        Some(run) if run.fg == fg && run.bg == bg && run.flags == f => {
                            run.text.push_str(&cell_text);
                            run.width += width;
                        }
                        _ => runs.push(crate::frame::Run {
                            x,
                            width,
                            text: cell_text,
                            fg,
                            bg,
                            flags: f,
                        }),
                    }
                    x += width;
                }
                // Trailing unstyled blanks carry no pixels; drop them. They
                // merge into a preceding default-style text run, so trim
                // inside the last run too (spaces are always 1 column).
                if let Some(run) = runs.last_mut() {
                    if run.fg.is_none() && run.bg.is_none() && run.flags == 0 {
                        let trimmed = run.text.trim_end_matches(' ');
                        let cut = (run.text.chars().count() - trimmed.chars().count()) as u16;
                        run.text.truncate(trimmed.len());
                        run.width -= cut;
                        if run.text.is_empty() {
                            runs.pop();
                        }
                    }
                }
                let sel = row.selection()?.map(|s| (s.start_x, s.end_x));
                changed.push(crate::frame::RowUpdate { y, runs, sel });
                row.set_dirty(false)?;
            }
            y += 1;
        }
        snap.set_dirty(Dirty::Clean)?;

        let palette = snap.colors()?;

        Ok(Some(Frame {
            full,
            cols: snap.cols()?,
            rows: snap.rows()?,
            changed,
            cursor,
            colors: Colors { fg: pack_rgb(palette.foreground), bg: pack_rgb(palette.background) },
            modes: Modes {
                app_cursor_keys: self.term.mode(Mode::DECCKM)?,
                bracketed_paste: self.term.mode(Mode::BRACKETED_PASTE)?,
                alt_screen: self.term.active_screen()? == Screen::Alternate,
                mouse_tracking: self.term.is_mouse_tracking()?,
            },
            title,
            scrollback_rows: self.term.scrollback_rows()?,
            viewport_top: self.viewport_top()?,
        }))
    }
}

/// Collect a grapheme-cluster cell's full codepoint sequence — the base
/// codepoint followed by any combining marks / ZWJ joiners — into a string.
/// libghostty's buffer can hold NUL placeholders, which are skipped; a cluster
/// that yields no printable codepoints falls back to a space so the column
/// still advances.
fn grapheme_text(cell: &CellIteration) -> Result<String> {
    let mut text = String::new();
    for c in cell.graphemes()? {
        if c != '\0' {
            text.push(c);
        }
    }
    if text.is_empty() {
        text.push(' ');
    }
    Ok(text)
}

fn pack_rgb(c: libghostty_vt::style::RgbColor) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

fn unpack_rgb(packed: u32) -> libghostty_vt::style::RgbColor {
    libghostty_vt::style::RgbColor {
        r: ((packed >> 16) & 0xff) as u8,
        g: ((packed >> 8) & 0xff) as u8,
        b: (packed & 0xff) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(EngineOptions { cols: 20, rows: 4, max_scrollback: 10 }).expect("engine")
    }

    /// The runs on viewport row 0 of the next frame. Renders once — a second
    /// render on an unchanged engine returns `None`, so callers read every
    /// field they need from this single result.
    fn row0_runs(engine: &mut Engine) -> Vec<crate::frame::Run> {
        let frame = engine.render().expect("render").expect("a frame");
        let row = frame.changed.iter().find(|r| r.y == 0).expect("row 0 present");
        row.runs.clone()
    }

    fn row0_text(runs: &[crate::frame::Run]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn row0_width(runs: &[crate::frame::Run]) -> u16 {
        runs.iter().map(|r| r.width).sum()
    }

    #[test]
    fn plain_ascii_survives_the_fast_path() {
        let mut e = engine();
        e.feed(b"hi");
        assert_eq!(row0_text(&row0_runs(&mut e)), "hi");
    }

    #[test]
    fn combining_mark_keeps_full_cluster_in_one_cell() {
        // "e" + U+0301 (combining acute) is one grapheme cell. The old code
        // carried only the base 'e' and dropped the accent; the fix keeps both
        // codepoints while the cell still occupies a single column.
        let mut e = engine();
        e.feed("e\u{301}".as_bytes());
        let runs = row0_runs(&mut e);
        assert_eq!(row0_text(&runs), "e\u{301}");
        assert_eq!(row0_width(&runs), 1, "a combining cluster is one column wide");
    }

    #[test]
    fn emoji_variation_selector_carries_every_codepoint() {
        // Heart + U+FE0F (emoji variation selector) is one grapheme cell. The
        // base codepoint alone renders as a monochrome dingbat; the fix keeps
        // the selector so the renderer sees the emoji presentation request.
        let mut e = engine();
        e.feed("\u{2764}\u{FE0F}".as_bytes());
        let runs = row0_runs(&mut e);
        assert_eq!(row0_text(&runs), "\u{2764}\u{FE0F}");
    }

    #[test]
    fn cluster_char_count_may_exceed_column_width() {
        // The frontend relies on `width` (columns), not char count, for layout;
        // a combining cluster deliberately has more chars than columns.
        let mut e = engine();
        e.feed("e\u{301}".as_bytes());
        let runs = row0_runs(&mut e);
        let run = runs.first().expect("a run on row 0");
        assert!(run.text.chars().count() > run.width as usize);
    }
}
