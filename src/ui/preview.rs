//! A file of Markdown as it is meant: the rows that `core::preview` made
//! of it, drawn. A row is drawn whole or not at all, since nothing here
//! cuts a quad at the edge of a pane.

use super::{PAD, Ui, file, pane, theme};
use crate::core::colors::{Kind, Piece};
use crate::core::preview::{self, Drawn, Ground, Laid, Meant, Row, Sizes, Tone, Type};
use crate::core::{FileView, text};
use crate::render::font::{self, Fonts, UI};
use crate::render::{Canvas, Color, Icon, Rect};

/// The font flowing text is set in: that of the interface, as a page is
/// read. Headings, lists and tables are set in it too, code in that of
/// the terminal.
pub const PROSE: u8 = UI;
/// How wide the rows are at the most, in sizes of their type: a line
/// that is longer is hard to read.
const MEASURE: f32 = 52.0;
/// By how much the six levels of a heading are larger than the text.
const HEADS: [f32; 6] = [1.6, 1.35, 1.15, 1.0, 1.0, 1.0];

/// The style of the renderer for the look of a run.
pub fn style(look: u8) -> u8 {
    let bit = |has: u8, bit: u8| if look & has != 0 { bit } else { 0 };
    (if look & preview::MONO != 0 { 0 } else { PROSE })
        | bit(preview::BOLD, font::BOLD)
        | bit(preview::ITALIC, font::ITALIC)
}

/// The sizes a preview is set in, for a size of the terminal font and
/// its cell: how wide, how high, and where its characters stand.
pub fn sizes(fonts: &mut Fonts, mono_px: f32, cell: (f32, f32, f32)) -> Sizes {
    let mut kind = |style: u8, px: f32| {
        let metrics = fonts.metrics(style, px);
        let high = metrics.ascent + metrics.descent;
        let h = (high * 1.3).ceil().max(1.0);
        Type { px, h, base: ((h - high) / 2.0 + metrics.ascent).round() }
    };
    let heads = HEADS.map(|by| kind(PROSE | font::BOLD, (mono_px * by).round()));
    Sizes {
        body: kind(PROSE, mono_px),
        heads,
        code: Type { px: mono_px, h: cell.1, base: cell.2 },
        cell: cell.0,
    }
}

/// Where the rows lie in the body of a pane. `noted` is how many lines
/// at the bottom say something of the file: they take the room of the
/// last rows.
pub fn room(ui: &Ui, body: Rect, noted: usize) -> Rect {
    let (pad, top) = (ui.px(PAD + 4.0), ui.px(10.0));
    let width = (body.w - 2.0 * pad).min((MEASURE * ui.meant.body.px).round()).max(1.0);
    let height = body.h - top - file::note_height(ui, noted);
    Rect::new(body.x + pad, body.y + top, width, height.max(0.0))
}

fn tone(tone: Tone) -> Color {
    match tone {
        Tone::Text | Tone::Code => theme::TEXT,
        Tone::Dim => theme::DIM,
        Tone::Faint => theme::FAINT,
        Tone::Link => theme::LINK,
    }
}

/// Draws the preview of a file into the body of its pane.
pub fn draw(cv: &mut Canvas, ui: &Ui, view: &FileView, area: Rect) {
    let Some((meant, laid, top, of)) = view.preview.made() else {
        return pane::centered(cv, ui, area, "Reading…", theme::FAINT);
    };
    let room = room(ui, area, file::noted(view));
    // The colors of the file are those of its text as it is now. Rows
    // of a text before it have none until theirs are made.
    let colored = of == view.edition();
    let mut y = room.y;
    for (at, row) in laid.rows().iter().enumerate().skip(top) {
        // The row at the top stands there without the room above it.
        let above = if at == top { 0.0 } else { row.above };
        if y + above + row.h > room.bottom() {
            break;
        }
        y += above;
        let pieces = if colored { view.colors.line(row.line as usize) } else { &[] };
        draw_row(cv, ui, (meant, laid), row, room, y, pieces);
        y += row.h;
    }
}

fn draw_row(cv: &mut Canvas, ui: &Ui, of: (&Meant, &Laid), row: &Row, room: Rect, y: f32, pieces: &[Piece]) {
    let (meant, laid) = of;
    let (left, line) = (room.x + row.x, ui.line());
    let across = (room.right() - left).max(0.0);
    let across = if row.w > 0.0 { row.w.min(across) } else { across };
    match row.ground {
        Ground::Code | Ground::Head => cv.rect(Rect::new(left, y, across, row.h), theme::SIDEBAR),
        Ground::Rule => cv.rect(Rect::new(left, (y + row.h / 2.0).round(), across, line), theme::LINE),
        Ground::None => {}
    }
    if row.under {
        cv.rect(Rect::new(left, y + row.h - line, across, line), theme::LINE);
    }
    // What of the row is still drawn: its runs and their signs, counted
    // together.
    let mut left = preview::ROW_DRAWN;
    for run in laid.runs(row) {
        let (x, base, kind) = (room.x + run.x, y + row.base, ui.meant.of(run.class));
        if left == 0 {
            break;
        }
        left -= 1;
        if x >= room.right() {
            continue;
        }
        let color = tone(run.tone);
        let text = match run.what {
            Drawn::Text(from, to) => meant.text().get(from as usize..to as usize),
            Drawn::Own(from, to) => laid.own().get(from as usize..to as usize),
            Drawn::Task(checked) => {
                let side = (0.8 * kind.px).round();
                let frame = Rect::new(x.round(), (base - side).round(), side, side);
                cv.outline(frame, ui.px(2.0), line, color);
                if checked {
                    let inset = (side * 0.25).round();
                    let mark =
                        Rect::new(frame.x + inset, frame.y + inset, side - 2.0 * inset, side - 2.0 * inset);
                    cv.rect(mark, theme::TEXT);
                }
                continue;
            }
            Drawn::Picture => {
                let size = ui.px(16.0);
                cv.icon(Icon::FileMedia, x.round(), (y + (row.h - size) / 2.0).round(), size, color);
                continue;
            }
        };
        let Some(text) = text else { continue };
        if run.class == preview::CODE {
            // Where the run begins in its line, to find its colors.
            let offset = match run.what {
                Drawn::Text(from, _) if run.tone == Tone::Code => {
                    (from as usize).checked_sub(meant.start(row.line))
                }
                _ => None,
            };
            cells(
                cv,
                ui,
                text,
                (x, base),
                room.right(),
                offset.map_or(&[], |_| pieces),
                offset.unwrap_or(0),
                color,
            );
            continue;
        }
        if run.look & preview::RAISED != 0 {
            let (out, inset) = (ui.px(2.0), ui.px(2.0));
            let surface = Rect::new(x - out, y + inset, run.w + 2.0 * out, row.h - 2.0 * inset);
            cv.rounded(surface, ui.px(3.0), theme::HOVER);
        }
        let (style, mut pen) = (style(run.look), x);
        for c in text.chars().filter_map(preview::shown) {
            if pen >= room.right() || left == 0 {
                break;
            }
            left -= 1;
            pen += cv.glyph(c, style, kind.px, pen, base, color);
        }
        if run.look & preview::STRUCK != 0 {
            cv.rect(Rect::new(x, (base - 0.3 * kind.px).round(), run.w, line), color);
        }
    }
}

/// Draws a run of a row of code by cells, as a row of a text is drawn,
/// as far as `right`. `pieces` are the kinds of the line it stands in,
/// and `offset` is where in that line it begins.
#[allow(clippy::too_many_arguments)]
fn cells(
    cv: &mut Canvas,
    ui: &Ui,
    text: &str,
    (x, base): (f32, f32),
    right: f32,
    pieces: &[Piece],
    offset: usize,
    plain: Color,
) {
    let (cell, px) = (ui.meant.cell.max(1.0), ui.meant.code.px);
    let cols = ((right - x) / cell).floor().max(0.0) as usize;
    // A block that names no language is one string to its grammar, from
    // the start of every line to its end: that is text here, not a string.
    let whole = matches!(pieces, [(_, Kind::String)]);
    let mut pieces = pieces.iter().filter(|_| !whole).peekable();
    for at in text::shown(text.as_bytes(), cols) {
        while pieces.next_if(|(until, _)| *until as usize <= offset + at.at).is_some() {}
        if at.c == '\t' || at.c == ' ' || at.col + at.wide > cols {
            continue;
        }
        let color = pieces.peek().map_or(plain, |(_, kind)| theme::syntax(*kind));
        cv.glyph(at.c, 0, px, x + at.base as f32 * cell, base, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_is_a_style_of_the_renderer() {
        assert_eq!(style(0), PROSE);
        assert_eq!(style(preview::BOLD | preview::ITALIC), PROSE | font::BOLD | font::ITALIC);
        assert_eq!(style(preview::MONO | preview::BOLD | preview::RAISED | preview::STRUCK), font::BOLD);
        // Every style a preview is drawn in is among the faces that are
        // looked up at the start: one that is not would start
        // `fc-match` in the middle of a frame.
        for look in 0..32 {
            assert!(style(look) < 8);
        }
    }

    #[test]
    fn the_rows_are_no_wider_than_a_line_is_read_and_leave_the_lines_at_the_bottom() {
        let mut ui = Ui::new(Vec::new());
        ui.meant.body.px = 10.0;
        let body = Rect::new(100.0, 50.0, 2000.0, 400.0);
        let wide = room(&ui, body, 0);
        assert_eq!((wide.x, wide.w), (100.0 + ui.px(PAD + 4.0), 520.0));
        let narrow = room(&ui, Rect::new(100.0, 50.0, 200.0, 400.0), 2);
        assert_eq!(narrow.w, 200.0 - 2.0 * ui.px(PAD + 4.0));
        assert_eq!(narrow.bottom(), 450.0 - file::note_height(&ui, 2));
        assert_eq!(room(&ui, Rect::new(0.0, 0.0, 5.0, 5.0), 9).h, 0.0);
    }
}
