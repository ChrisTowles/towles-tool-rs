//! Single-threaded terminal engine: owns a libghostty-vt `Terminal` plus its
//! render state, turns PTY bytes into [`Frame`]s.
//!
//! libghostty-vt types are `!Send`, so an [`Engine`] must live and die on one
//! thread; [`crate::session`] provides the per-terminal thread wrapper.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;

use libghostty_vt::fmt::{Formatter, FormatterOptions};
use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator};
use libghostty_vt::screen::{CellWide, Screen};
use libghostty_vt::selection::{FormatOptions, SelectLineOptions, SelectWordOptions, Selection};
use libghostty_vt::style::Underline;
use libghostty_vt::terminal::{Mode, Options, Point, PointCoordinate, ScrollViewport, Terminal};

use crate::frame::{flags, Colors, Cursor, CursorShape, Frame, Modes};
use crate::osc52::Osc52Scanner;
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

pub struct Engine {
    term: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    /// Bytes the terminal wants written back to the PTY (DA1 replies etc.),
    /// filled synchronously during `feed` by the pty-write effect.
    pty_out: Rc<RefCell<Vec<u8>>>,
    title_changed: Rc<StdCell<bool>>,
    /// Watches the byte feed for OSC 52 set-clipboard sequences (libghostty-vt
    /// exposes no clipboard callback); decoded copies are drained by
    /// [`Engine::take_clipboard`].
    osc52: Osc52Scanner,
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
        term.on_pty_write({
            let pty_out = Rc::clone(&pty_out);
            move |_term, data| pty_out.borrow_mut().extend_from_slice(data)
        })?
        .on_title_changed({
            let title_changed = Rc::clone(&title_changed);
            move |_term| title_changed.set(true)
        })?;

        Ok(Self {
            term,
            render: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            pty_out,
            title_changed,
            osc52: Osc52Scanner::new(),
            force_full: false,
            last_cursor: None,
        })
    }

    /// Feed raw PTY output into the terminal state machine.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.term.vt_write(bytes);
        self.osc52.feed(bytes);
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

                    // TODO(graphemes): CodepointGrapheme cells only carry the
                    // primary codepoint here; combining marks / ZWJ emoji need
                    // the grid-ref graphemes API.
                    let cp = raw.codepoint()?;
                    let ch = char::from_u32(cp).filter(|c| *c != '\0').unwrap_or(' ');

                    match runs.last_mut() {
                        Some(run) if run.fg == fg && run.bg == bg && run.flags == f => {
                            run.text.push(ch);
                            run.width += width;
                        }
                        _ => runs.push(crate::frame::Run {
                            x,
                            width,
                            text: ch.to_string(),
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

fn pack_rgb(c: libghostty_vt::style::RgbColor) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}
