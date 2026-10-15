//! Draws the terminal grid.

use std::time::Duration;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::Point;
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{RenderableCursor, Term};
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};

use super::link::{self, Run};
use super::{CellAt, PALETTE, Terminal};
use crate::render::font::{BOLD, ITALIC};
use crate::render::{Canvas, Color, Rect, boxdraw, let_go, mix};

/// How far text that a program dims goes towards what it stands on, of
/// 255: a good third of the way, as the eye takes it.
pub const DIMMED: u8 = 88;

/// How the grid looks: font metrics in physical pixels and colors.
pub struct Look<'a> {
    /// Font size.
    pub px: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Baseline below the top of a cell.
    pub base: f32,
    /// Width of a hairline.
    pub line: f32,
    pub palette: &'a [Color; PALETTE],
    pub selection: Color,
    pub cursor: Color,
    /// Cursor outline in a pane without focus.
    pub idle_cursor: Color,
}

/// The visible cells, copied out of the emulator. Kept between frames so
/// that a frame allocates nothing once the buffers have grown.
#[derive(Default)]
pub(super) struct Snapshot {
    cells: Vec<Shown>,
    /// The combining marks of all cells, each cell knows its part.
    marks: Vec<char>,
    /// What belongs to the cells: the colors the program set, the
    /// selection, the cursor, and how far the view is scrolled back.
    /// Nothing before the first copy. Not kept in place: the colors alone
    /// are a kilobyte, and a terminal is passed around when it starts.
    view: Option<Box<View>>,
    /// The text around the pointer, read anew for every frame that asks
    /// for the address in it.
    run: Run,
    /// The address the underlined cells spell, as the frame that
    /// underlined them read it: what a click on them opens.
    address: Option<String>,
}

#[derive(Clone, Copy)]
struct View {
    colors: Colors,
    selection: Option<SelectionRange>,
    cursor: RenderableCursor,
    offset: i32,
    /// The cells of the address under the pointer, while it is asked for,
    /// and the cell that was asked about.
    link: Option<SelectionRange>,
    asked: Option<CellAt>,
}

/// How long a frame waits for the emulator. The reader has it for a chunk
/// of output at a time, which is well under a millisecond of plain text,
/// but a program can make a chunk expensive: a synchronized update is
/// taken in as a whole. The frame then shows the cells it has.
const LOCK_WAIT: Duration = Duration::from_millis(4);

struct Shown {
    point: Point,
    c: char,
    fg: TermColor,
    bg: TermColor,
    underline: Option<TermColor>,
    flags: Flags,
    marks: (u32, u32),
}

impl Terminal {
    /// Draws the grid with its top left corner at `origin`. `active` says
    /// whether this terminal has the keyboard, `link` the cell whose
    /// address is drawn as one that a click opens, if it has one.
    pub fn draw(&self, cv: &mut Canvas, look: &Look, origin: (f32, f32), active: bool, link: Option<CellAt>) {
        let mut snapshot = self.snapshot.borrow_mut();
        // The emulator is locked only while the cells are copied. Looking up
        // glyphs can rasterize and upload them, and all that time the PTY
        // reader and the keyboard would wait. Nor does the frame wait long
        // for the lock: it draws the last copy again, and says so.
        if let Some(term) = self.term.try_lock_for(LOCK_WAIT) {
            self.mode.set(*term.mode());
            snapshot.copy(&term, link);
        } else {
            snapshot.keep(link);
            self.stale.set(true);
        }
        snapshot.paint(cv, look, origin, active);
    }

    /// Whether the last frame had to draw this terminal as it was before,
    /// and it is still to be drawn as it is. Asked once per frame.
    pub fn take_stale(&self) -> bool {
        self.stale.take()
    }
}

impl Snapshot {
    /// Whether the cells copied last have an address under the pointer.
    pub(super) fn linked(&self) -> bool {
        self.view.as_ref().is_some_and(|view| view.link.is_some())
    }

    /// The address that is underlined under this cell, if the last frame
    /// was asked about it and underlined one.
    pub(super) fn address(&self, at: CellAt) -> Option<String> {
        let shown = self.view.as_ref().is_some_and(|view| view.link.is_some() && view.asked == Some(at));
        self.address.clone().filter(|_| shown)
    }

    /// Copies what is to be seen out of the emulator, which the caller has
    /// locked for it, and looks for the address under `link`. That is
    /// done here, for every frame, and not when the pointer moves:
    /// output moves the text under a pointer that rests.
    fn copy<T: EventListener>(&mut self, term: &Term<T>, link: Option<CellAt>) {
        let Snapshot { cells, marks, view, run, address } = self;
        cells.clear();
        marks.clear();
        let content = term.renderable_content();
        for cell in content.display_iter {
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let from = marks.len() as u32;
            marks.extend_from_slice(cell.zerowidth().unwrap_or_default());
            cells.push(Shown {
                point: cell.point,
                c: cell.c,
                fg: cell.fg,
                bg: cell.bg,
                underline: cell.underline_color(),
                flags: cell.flags,
                marks: (from, marks.len() as u32),
            });
        }
        // What a window that was very large for a moment made them
        // hold is given back.
        let_go(cells);
        let_go(marks);
        let span = link.and_then(|at| link::find(term, Terminal::point(term, at).0, run));
        let now = View {
            colors: *content.colors,
            selection: content.selection,
            cursor: content.cursor,
            offset: content.display_offset as i32,
            link: span.as_ref().map(|span| run.cells(span)),
            asked: link,
        };
        *address = span.map(|span| run.text(span));
        match view {
            Some(view) => **view = now,
            None => *view = Some(Box::new(now)),
        }
    }

    /// Keeps the cells as they were copied last, for a frame that did not
    /// get the emulator. An address stays underlined in them only for
    /// the cell it was found under: with another cell asked about, or
    /// none, what was found says nothing, and the pointer is no hand.
    fn keep(&mut self, link: Option<CellAt>) {
        if let Some(view) = self.view.as_deref_mut().filter(|view| view.asked != link) {
            view.link = None;
            self.address = None;
        }
    }

    /// Draws the cells as they were copied last. Nothing in here can reach
    /// the emulator.
    fn paint(&self, cv: &mut Canvas, look: &Look, origin: (f32, f32), active: bool) {
        let Snapshot { cells, marks, view, .. } = self;
        let Some(View { colors, selection, cursor, offset, link, .. }) = view.as_deref().copied() else {
            return;
        };
        let (cw, ch, px) = (look.cell_w, look.cell_h, look.px);
        let line = look.line;
        let resolve = |c: TermColor| resolve(c, &colors, look.palette);
        let default_bg = TermColor::Named(NamedColor::Background);

        // Backgrounds go down first and merge into runs, glyphs and lines are
        // held back so no background can cover a neighbouring glyph.
        let mut run: Option<(Rect, Color)> = None;
        let mut dim: Option<([Color; 2], Color)> = None;
        for cell in cells.iter() {
            let flags = cell.flags;
            let x = origin.0 + cell.point.column.0 as f32 * cw;
            let y = origin.1 + (cell.point.line.0 + offset) as f32 * ch;
            let width = if flags.contains(Flags::WIDE_CHAR) { 2.0 * cw } else { cw };

            let (mut fg, mut bg) = (resolve(cell.fg), resolve(cell.bg));
            let mut filled = cell.bg != default_bg;
            if flags.contains(Flags::INVERSE) {
                (fg, bg) = (bg, fg);
                filled = true;
            }
            if flags.contains(Flags::DIM) {
                // Reckoned once for a run of cells of the same colors.
                fg = match dim {
                    Some((of, dimmed)) if of == [fg, bg] => dimmed,
                    _ => {
                        let dimmed = mix(fg, bg, DIMMED);
                        dim = Some(([fg, bg], dimmed));
                        dimmed
                    }
                };
            }
            // An address a click opens is drawn in codn's own colors
            // while it is underlined, letters, ground and line: what is
            // opened is then what can be read, also where a program
            // printed a part of it in colors that are near each other,
            // which no rule can tell from colors that are meant.
            let linked = link.is_some_and(|l| l.contains(cell.point));
            if linked {
                fg = look.palette[NamedColor::Foreground as usize];
                bg = look.palette[NamedColor::Background as usize];
                filled = true;
            }
            if selection.is_some_and(|s| s.contains(cell.point)) {
                bg = look.selection;
                filled = true;
            }
            let shape = cursor.shape;
            let cursor = cursor.point == cell.point && shape != CursorShape::Hidden;
            if cursor && active && shape == CursorShape::Block {
                (fg, bg) = (bg, look.cursor);
                filled = true;
            }

            match &mut run {
                Some((rect, color)) if filled && *color == bg && rect.y == y && rect.right() == x => {
                    rect.w += width
                }
                _ => {
                    if let Some((rect, color)) = run.take() {
                        cv.rect(rect, color);
                    }
                    if filled {
                        run = Some((Rect::new(x, y, width, ch), bg));
                    }
                }
            }

            cv.hold = true;
            if cursor {
                let cell_rect = Rect::new(x, y, width, ch);
                match shape {
                    _ if !active => cv.outline(cell_rect, 0.0, line, look.idle_cursor),
                    CursorShape::Beam => cv.rect(Rect::new(x, y, 2.0 * line, ch), look.cursor),
                    CursorShape::Underline => {
                        cv.rect(Rect::new(x, y + ch - 2.0 * line, width, 2.0 * line), look.cursor)
                    }
                    CursorShape::HollowBlock => cv.outline(cell_rect, 0.0, line, look.cursor),
                    _ => {}
                }
            }
            if !flags.contains(Flags::HIDDEN) {
                let style = (flags.contains(Flags::BOLD) as u8 * BOLD)
                    | (flags.contains(Flags::ITALIC) as u8 * ITALIC);
                let boxed = ('\u{2500}'..='\u{259f}').contains(&cell.c)
                    && boxdraw::draw(cv, cell.c, Rect::new(x, y, cw, ch), (px / 14.0).round().max(1.0), fg);
                // Most cells of a screen are blank. A blank has nothing to
                // draw, and its advance is only asked for by a mark on it.
                let marked = cell.marks.0 != cell.marks.1;
                let advance = if !boxed && cell.c != '\t' && (cell.c != ' ' || marked) {
                    cv.glyph(cell.c, style, px, x, y + look.base, fg)
                } else {
                    width
                };
                // A character without a glyph still has its cell.
                let advance = if advance > 0.0 { advance } else { width };
                for &c in &marks[cell.marks.0 as usize..cell.marks.1 as usize] {
                    // Measured from the letter as it was drawn: its advance
                    // can be smaller than the terminal's rounded cell width.
                    let pen = cv.mark_pen(c, style, px, x, advance);
                    cv.glyph(c, style, px, pen, y + look.base, fg);
                }
                // An address a click opens is underlined like text that
                // a program underlines, in the color its letters have
                // while it is: codn's own, see above.
                if flags.intersects(Flags::ALL_UNDERLINES) || linked {
                    let color = if linked { fg } else { cell.underline.map_or(fg, resolve) };
                    let under = y + look.base + 2.0 * line;
                    cv.rect(Rect::new(x, under, width, line), color);
                    if flags.contains(Flags::DOUBLE_UNDERLINE) {
                        cv.rect(Rect::new(x, under + 2.0 * line, width, line), color);
                    }
                }
                if flags.contains(Flags::STRIKEOUT) {
                    cv.rect(Rect::new(x, y + (ch * 0.55).round(), width, line), fg);
                }
            }
            cv.hold = false;
        }
        if let Some((rect, color)) = run {
            cv.rect(rect, color);
        }
        cv.flush();
    }
}

fn resolve(color: TermColor, colors: &Colors, palette: &[Color; PALETTE]) -> Color {
    let set = match color {
        TermColor::Spec(rgb) => Some(rgb),
        TermColor::Named(name) => colors[name],
        TermColor::Indexed(index) => colors[index as usize],
    };
    match (set, color) {
        (Some(rgb), _) => [rgb.r, rgb.g, rgb.b, 255],
        (None, TermColor::Named(name)) => palette[name as usize],
        (None, TermColor::Indexed(index)) => palette[index as usize],
        (None, TermColor::Spec(rgb)) => [rgb.r, rgb.g, rgb.b, 255],
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::term::Grid;

    #[test]
    fn a_click_opens_the_address_the_frame_underlined_not_what_was_printed_since() {
        use crate::term::Options;
        // The click asked the emulator anew for the address under it. A
        // program that printed another one over the first, in an update
        // that kept the emulator from the frames, had the first one
        // underlined on the screen and its own one opened.
        let grid = Grid { cols: 80, rows: 4, cell_w: 8, cell_h: 16 };
        let options = Options::default();
        let terminal = Terminal::spawn(
            "/bin/sleep",
            vec!["30".into()],
            std::path::Path::new("/"),
            grid,
            &options,
            |_| {},
        )
        .unwrap();
        let printed = |bytes: &[u8]| {
            let mut parser: Processor = Processor::new();
            parser.advance(&mut *terminal.term.lock(), bytes);
        };
        let at = CellAt { col: 3, row: 0, right: false };
        printed(b"https://good.example/");
        terminal.snapshot.borrow_mut().copy(&terminal.term.lock(), Some(at));
        assert!(terminal.linked());
        printed(b"\rhttps://evil.example/");
        // A frame that does not get the emulator draws what it has.
        terminal.snapshot.borrow_mut().keep(Some(at));
        assert_eq!(terminal.link(at).as_deref(), Some("https://good.example/"));
        // Where nothing was underlined for a cell, nothing is opened.
        assert_eq!(terminal.link(CellAt { col: 40, row: 0, right: false }), None);
        terminal.snapshot.borrow_mut().keep(None);
        assert_eq!(terminal.link(at), None, "Control let go");
        // The next frame that gets the emulator underlines what is there.
        terminal.snapshot.borrow_mut().copy(&terminal.term.lock(), Some(at));
        assert_eq!(terminal.link(at).as_deref(), Some("https://evil.example/"));
    }

    #[test]
    fn a_frame_without_the_emulator_keeps_an_address_only_for_the_cell_it_was_asked_about() {
        // Control let go while the emulator was busy left the address
        // underlined, and the pointer on another cell stayed a hand.
        let grid = Grid { cols: 80, rows: 4, cell_w: 8, cell_h: 16 };
        let mut term = Term::new(Config::default(), &grid, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"see https://example.org/a here");
        let at = |col| CellAt { col, row: 0, right: false };
        let mut snapshot = Snapshot::default();
        snapshot.copy(&term, Some(at(8)));
        assert!(snapshot.linked());
        snapshot.keep(Some(at(8)));
        assert!(snapshot.linked(), "the same cell asked about again");
        snapshot.keep(Some(at(9)));
        assert!(!snapshot.linked(), "another cell: what is under it is not known");
        snapshot.copy(&term, Some(at(9)));
        assert!(snapshot.linked());
        snapshot.keep(None);
        assert!(!snapshot.linked(), "Control let go");
        // And a copy that is asked for none has none.
        snapshot.copy(&term, Some(at(1)));
        assert!(!snapshot.linked());
    }
}
