//! Single-threaded terminal engine: owns a libghostty-vt `Terminal` plus its
//! render state, turns PTY bytes into [`Frame`]s.
//!
//! libghostty-vt types are `!Send`, so an [`Engine`] must live and die on one
//! thread; [`crate::session`] provides the per-terminal thread wrapper.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;

use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator};
use libghostty_vt::screen::{CellWide, Screen};
use libghostty_vt::style::Underline;
use libghostty_vt::terminal::{Mode, Options, ScrollViewport, Terminal};

use crate::frame::{flags, Colors, Cursor, CursorShape, Frame, Modes};

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
        })
    }

    /// Feed raw PTY output into the terminal state machine.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.term.vt_write(bytes);
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
        if dirty == Dirty::Clean && title.is_none() {
            return Ok(None);
        }
        let full = dirty == Dirty::Full;

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
                changed.push(crate::frame::RowUpdate { y, runs });
                row.set_dirty(false)?;
            }
            y += 1;
        }
        snap.set_dirty(Dirty::Clean)?;

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
        }))
    }
}

fn pack_rgb(c: libghostty_vt::style::RgbColor) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}
