//! The pane for a file: its text by cells, with line numbers, a caret
//! and a selection, or its picture. A file of Markdown as it is meant is
//! drawn by `preview`.

use winit::keyboard::NamedKey;

use super::{Hit, Hits, PAD, PaneLayout, Ui, pane, preview, theme};
use crate::action::Action;
use crate::core::colors::End;
use crate::core::picture::{Decoded, Look, Notes, Seen};
use crate::core::preview::Why;
use crate::core::text::{self, Document, Move, Pos, Refused};
use crate::core::{FileView, Said, Shown};
use crate::render::font::UI;
use crate::render::{Canvas, Color, Rect};

/// The height of a line of what is said at the bottom of a file, and the
/// room above and below all of it together.
const NOTE_LINE: f32 = 20.0;
const NOTE_PAD: f32 = 14.0;

/// How high what is said at the bottom is, with that many lines.
pub fn note_height(ui: &Ui, lines: usize) -> f32 {
    if lines == 0 { 0.0 } else { ui.px(NOTE_PAD) + lines as f32 * ui.px(NOTE_LINE) }
}

/// Where the text of a file lies in a pane.
pub struct Place {
    /// Left edge of the first column that is shown, top of the first row.
    pub x: f32,
    pub y: f32,
    pub rows: usize,
    pub cols: usize,
    /// How many digits the line numbers have room for.
    digits: usize,
}

/// Where the text lies in the body of a pane, for a text of `lines`
/// lines. `noted` is how many lines at the bottom say something of the
/// file, see `noted`: they take the room of the last rows.
pub fn place(ui: &Ui, body: Rect, lines: usize, noted: usize) -> Place {
    let (cw, pad) = (ui.cell_w, ui.px(PAD));
    // The numbers are as wide as the largest of them, and three at least,
    // so that a short file does not begin further left than a long one.
    let digits = (lines.max(1).ilog10() as usize + 1).max(3);
    let x = body.x + pad + (digits + 2) as f32 * cw;
    let height = body.h - note_height(ui, noted);
    Place {
        x,
        y: body.y,
        rows: (height / ui.cell_h).floor().max(1.0) as usize,
        cols: ((body.right() - pad - x) / cw).floor().max(1.0) as usize,
        digits,
    }
}

/// Where the text of the file a pane shows lies in the body of the pane:
/// `place` for that file, with the lines it has and what is said of it.
pub fn room(ui: &Ui, body: Rect, view: &FileView) -> Place {
    place(ui, body, view.text().map_or(1, Document::lines), noted(view))
}

/// What is said of a picture in the line under it, by what was noted
/// when it was decoded.
const NOTED: [(u16, &str); 5] = [
    (Notes::EARLY, "The file ends early: what is there of it is shown."),
    (Notes::DAMAGED, "The file is damaged: what could be read of it is shown."),
    (Notes::UNAPPLIED, "It says how its colors are meant, which is not applied."),
    (Notes::ANIMATION, "It holds an animation, which is not played."),
    (Notes::SMALLER, "It is too large to be shown any finer."),
];

/// What is said at the bottom of a pane that shows a picture: its size,
/// and what there is to say of it, or why there is none.
fn picture_says(seen: &Seen, length: u64, zoom: Option<f32>) -> Vec<String> {
    match (&seen.shown, &seen.why) {
        (Some((decoded, _)), _) => {
            let (w, h) = decoded.picture;
            let noted = NOTED.iter().filter(|(note, _)| decoded.notes.has(*note));
            // Where it is zoomed, how much: pixels of the screen for one
            // of the picture, as a percentage.
            let zoom = zoom.map(|zoom| format!(", {} %", (zoom * 100.0).round()));
            std::iter::once(format!("{w} × {h} pixels, {}{}", size(length), zoom.unwrap_or_default()))
                .chain(noted.map(|(_, line)| (*line).to_owned()))
                .collect()
        }
        (None, Some(why)) => vec![why.clone()],
        (None, None) => Vec::new(),
    }
}

/// How many lines `picture_says` has, without writing them: asked
/// between events, where it has to cost nothing.
fn picture_lines(seen: &Seen) -> usize {
    match (&seen.shown, &seen.why) {
        (Some((decoded, _)), _) => 1 + NOTED.iter().filter(|(note, _)| decoded.notes.has(*note)).count(),
        (None, Some(_)) => 1,
        (None, None) => 0,
    }
}

/// How long the file of a picture is, as far as that is known.
fn picture_length(view: &FileView, seen: &Seen) -> u64 {
    view.stamp().map_or(seen.length, |stamp| stamp.size)
}

/// The room a picture is fitted into in the body of a pane: what the
/// lines at the bottom leave, with a margin around it.
pub fn picture_room(ui: &Ui, body: Rect, view: &FileView) -> Rect {
    let lines = view.picture().map_or(0, picture_lines);
    let pad = ui.px(PAD);
    let height = body.h - note_height(ui, lines.max(1)) - 2.0 * pad;
    // Whole pixels, so that what is asked of the decoder and what is
    // drawn are reckoned for the same room.
    Rect::new(body.x + pad, body.y + pad, (body.w - 2.0 * pad).floor().max(1.0), height.floor().max(1.0))
}

/// What of the body of a pane lies under the veil when the keyboard is
/// with another pane: all of it, but of a picture only the lines under
/// it. A picture is looked at beside the session that made it, and
/// under the veil its colors are not to be judged.
pub fn veiled(ui: &Ui, body: Rect, view: &FileView) -> Rect {
    match view.picture() {
        Some(seen) if seen.shown.is_some() => lines_of(ui, body, picture_lines(seen)),
        _ => body,
    }
}

/// The room of so many lines at the bottom of a body.
fn lines_of(ui: &Ui, body: Rect, lines: usize) -> Rect {
    let height = note_height(ui, lines).min(body.h);
    Rect::new(body.x, body.bottom() - height, body.w, height)
}

/// Where the part of a picture that is decoded lies in its room, cut
/// to the room, and which part of the texture that is: the rectangle
/// from the top left of the room, and where it begins and ends in the
/// texture. Nothing where the part lies outside the room.
pub fn picture_place(decoded: &Decoded, look: &Look, room: (f32, f32)) -> Option<(Rect, [f32; 4])> {
    let frame = look.frame(decoded.picture, room);
    let [px, py, pw, ph] = decoded.part.map(|n| n as f32);
    let (x0, y0) = frame.screen((px, py));
    let (x1, y1) = frame.screen((px + pw, py + ph));
    // On whole pixels of the screen, and at least one.
    let (x0, y0) = (x0.round(), y0.round());
    let (x1, y1) = (x1.round().max(x0 + 1.0), y1.round().max(y0 + 1.0));
    let (cx0, cy0, cx1, cy1) = (x0.max(0.0), y0.max(0.0), x1.min(room.0), y1.min(room.1));
    if cx1 <= cx0 || cy1 <= cy0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let uv = [(cx0 - x0) / w, (cy0 - y0) / h, (cx1 - x0) / w, (cy1 - y0) / h];
    Some((Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0), uv))
}

/// Draws the picture of a file: fitted into its room and never larger
/// than it is, as it is opened, or at the scale and the place it was
/// zoomed and moved to. One pixel of it is one of the screen at the
/// most where fitted.
fn picture(cv: &mut Canvas, ui: &Ui, view: &FileView, seen: &Seen, area: Rect) {
    let length = picture_length(view, seen);
    match seen.shown {
        Some((decoded, id)) => {
            let room = picture_room(ui, area, view);
            // What of it is transparent shows the ground under it.
            let ground = (theme::GROUND, ui.px(theme::GROUND_CELL), theme::GROUND_LIGHTER);
            let ground = decoded.notes.has(Notes::ALPHA).then_some(ground);
            let placed = picture_place(&decoded, &seen.look, (room.w, room.h));
            // The texture is another picture's for a moment after a
            // file took the place of another. Where the part that is
            // decoded lies outside the room, the ground of the pane is
            // what there is until the part for it came.
            let drawn = placed.is_none_or(|(at, uv)| {
                let at = Rect::new((room.x + at.x).round(), (room.y + at.y).round(), at.w, at.h);
                cv.picture(id, at, uv, ground)
            });
            if !drawn {
                pane::centered(cv, ui, area, "Reading…", theme::FAINT);
            }
        }
        None if seen.why.is_some() => {
            pane::centered(cv, ui, area, &format!("{}, {}", seen.format.name(), size(length)), theme::FAINT);
        }
        None => pane::centered(cv, ui, area, "Reading…", theme::FAINT),
    }
    let zoom = seen.shown.and_then(|(decoded, _)| {
        let room = picture_room(ui, area, view);
        seen.look.shown(decoded.picture, (room.w, room.h))
    });
    bottom(cv, ui, area, &picture_says(seen, length, zoom), theme::DIM);
}

/// The body of a pane: what is below its header.
pub fn body(l: &PaneLayout) -> Rect {
    Rect::new(l.area.x, l.header.bottom(), l.area.w, l.area.h - l.header.bottom())
}

/// The place in the text under a point of the pane, and how far above or
/// below the rows the point lies, in rows: for a selection that is
/// dragged out of them. `under` asks for the character under the point,
/// not for the gap between two that is nearest.
pub fn at(ui: &Ui, l: &PaneLayout, view: &FileView, x: f32, y: f32, under: bool) -> Option<(Pos, isize)> {
    // A file that is shown as it is meant has no place of its text
    // under a point: its rows are not its lines.
    let doc = view.text().filter(|_| !view.previews())?;
    let place = room(ui, body(l), view);
    let row = ((y - place.y) / ui.cell_h).floor() as isize;
    let outside = if row < 0 { row } else { (row - place.rows as isize + 1).max(0) };
    let line = (view.view().top as isize + row.clamp(0, place.rows as isize - 1)).max(0) as usize;
    let line = line.min(doc.lines() - 1);
    let cells = (x - place.x) / ui.cell_w + view.view().left as f32;
    let bytes = doc.line(line);
    let byte = if under { text::byte_under(bytes, cells) } else { text::byte_near(bytes, cells) };
    Some((Pos { line, byte }, outside))
}

/// Where the caret of a file stands in the window, if it is in view: the
/// window of an input method is put there.
pub fn caret(ui: &Ui, l: &PaneLayout, view: &FileView) -> Option<Rect> {
    let doc = view.text().filter(|_| !view.previews())?;
    let place = room(ui, body(l), view);
    let (caret, view) = (view.view().caret, view.view());
    let row = caret.line.checked_sub(view.top).filter(|row| *row < place.rows)?;
    let col = text::col_of(doc.line(caret.line), caret.byte);
    let col = col.checked_sub(view.left).filter(|col| *col <= place.cols)?;
    Some(Rect::new(place.x + col as f32 * ui.cell_w, place.y + row as f32 * ui.cell_h, ui.cell_w, ui.cell_h))
}

/// What a key that types nothing means in a file. `ctrl` is held or not,
/// `rows` is how many lines the pane shows. With Shift the same move
/// takes the end of the selection along, which is for who calls to do.
pub fn key(key: NamedKey, ctrl: bool, rows: usize) -> Option<Move> {
    Some(match key {
        NamedKey::ArrowLeft if ctrl => Move::WordLeft,
        NamedKey::ArrowRight if ctrl => Move::WordRight,
        NamedKey::ArrowLeft => Move::Left,
        NamedKey::ArrowRight => Move::Right,
        NamedKey::ArrowUp => Move::Up,
        NamedKey::ArrowDown => Move::Down,
        NamedKey::Home if ctrl => Move::Top,
        NamedKey::End if ctrl => Move::Bottom,
        NamedKey::Home => Move::Home,
        NamedKey::End => Move::End,
        NamedKey::PageUp => Move::PageUp(rows.saturating_sub(1)),
        NamedKey::PageDown => Move::PageDown(rows.saturating_sub(1)),
        _ => return None,
    })
}

/// A size in the unit that fits it.
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} bytes"),
        1024..1_048_576 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
    }
}

/// What a row of text is drawn with.
struct Row<'a> {
    bytes: &'a [u8],
    /// The columns that are selected, from the first to before the last.
    selected: Option<(usize, usize)>,
    /// The color of the text up to each byte, in the order of the bytes.
    colors: &'a [(usize, Color)],
    /// The byte the caret stands before.
    caret: Option<usize>,
}

/// Draws a row of text at `y`, always in this order: what is selected,
/// the characters, the caret. The surface of a line that is marked lies
/// under all of it and under the number, and is drawn before. `left` is
/// the first column shown.
fn row(cv: &mut Canvas, ui: &Ui, place: &Place, left: usize, y: f32, row: Row) {
    let (cw, ch, px) = (ui.cell_w, ui.cell_h, ui.mono_px);
    let (base, end) = (y + ui.cell_base, left + place.cols);
    let x = |col: usize| place.x + (col - left) as f32 * cw;
    if let Some((from, to)) = row.selected.map(|(from, to)| (from.max(left), to.min(end)))
        && from < to
    {
        cv.rect(Rect::new(x(from), y, (to - from) as f32 * cw, ch), theme::SELECTION);
    }
    // A character takes the cells it is wide, a mark none: it goes onto
    // the character before it. Nothing is drawn outside the columns that
    // are shown.
    let mut colors = row.colors.iter().peekable();
    for cell in text::shown(row.bytes, end) {
        while colors.next_if(|(until, _)| *until <= cell.at).is_some() {}
        if cell.base < left || cell.col + cell.wide > end || cell.c == '\t' || cell.c == ' ' {
            continue;
        }
        let color = colors.peek().map_or(theme::TEXT, |(_, color)| *color);
        cv.glyph(cell.c, 0, px, x(cell.base), base, color);
    }
    if let Some(col) = row.caret.map(|byte| text::col_of(row.bytes, byte))
        && (left..=end).contains(&col)
    {
        cv.rect(Rect::new(x(col), y, (ui.scale * 1.5).round().max(1.0), ch), theme::TEXT);
    }
}

/// Draws the number of a line left of its text.
fn number(cv: &mut Canvas, ui: &Ui, place: &Place, y: f32, line: usize, color: Color) {
    let text = (line + 1).to_string();
    let mut x = place.x - (2 + text.len().min(place.digits)) as f32 * ui.cell_w;
    for c in text.chars() {
        cv.glyph(c, 0, ui.mono_px, x, y + ui.cell_base, color);
        x += ui.cell_w;
    }
}

/// `active` says whether the pane has the keyboard, and so shows its caret.
pub fn draw(cv: &mut Canvas, ui: &Ui, view: &FileView, area: Rect, active: bool, hits: &mut Hits) {
    let length = view.stamp().map_or(0, |stamp| stamp.size);
    let doc = match view.shown() {
        Shown::Text(doc) => doc,
        Shown::Reading => return pane::centered(cv, ui, area, "Reading…", theme::FAINT),
        Shown::Failed(why) => return pane::centered(cv, ui, area, why, theme::DIM),
        Shown::Picture(seen) => return picture(cv, ui, view, seen, area),
        Shown::Other(format) => {
            let what = format.unwrap_or("Not text");
            return pane::centered(cv, ui, area, &format!("{what}, {}", size(length)), theme::FAINT);
        }
    };
    if view.previews() {
        preview::draw(cv, ui, view, area);
        return note(cv, ui, area, view, doc, length);
    }
    let place = room(ui, area, view);
    let (top, left) = (view.view().top, view.view().left);
    // The colors of a row, made anew for each in the same room.
    let mut colors: Vec<(usize, Color)> = Vec::new();
    let selection = view.view().selection();
    let caret = view.view().caret;
    let (marks, last) = (view.marks(), doc.lines() - 1);

    for (i, line) in (top..doc.lines()).take(place.rows).enumerate() {
        let y = place.y + i as f32 * ui.cell_h;
        let bytes = doc.line(line);
        let selected =
            selection.filter(|(from, to)| from.line <= line && line <= to.line).map(|(from, to)| {
                let begin = if from.line == line { text::col_of(bytes, from.byte) } else { 0 };
                // A line that is selected through to the next shows that with
                // the room of one more cell: its end is selected too. Where
                // it ends is looked for no further than is shown.
                let through = text::col_until(bytes, left + place.cols) + 1;
                (begin, if to.line == line { text::col_of(bytes, to.byte) } else { through })
            });
        let here = active && caret.line == line;
        // What changed since the last commit: a line that is new or
        // changed is told by its surface, a place where lines went by
        // the color of the number of the line behind it. Lines that went
        // at the end are told by the last line.
        let changed = marks.is_changed(line);
        let gone = marks.is_gone_before(line)
            || (line == last && marks.gone.last().is_some_and(|at| *at as usize > last));
        if changed {
            cv.rect(Rect::new(area.x, y, area.w, ui.cell_h), theme::ADDED_BG);
        }
        let mut color = if here { theme::DIM } else { theme::FAINT };
        if changed || gone {
            // The number of a marked line leads to the changes there.
            let show = Action::FileChanges(view.edition(), line as u32 + 1);
            let number = Rect::new(area.x, y, place.x - ui.cell_w - area.x, ui.cell_h);
            hits.push((number, Hit::Do(show)));
            color = match (ui.hot(show), gone) {
                (true, _) => theme::TEXT,
                (false, true) => theme::REMOVED,
                (false, false) => theme::DIM,
            };
        }
        number(cv, ui, &place, y, line, color);
        // What is known of the pieces of the line, and text behind them:
        // a line whose pieces are not known has the one color.
        colors.clear();
        colors.extend(
            view.colors.line(line).iter().map(|(until, kind)| (*until as usize, theme::syntax(*kind))),
        );
        colors.push((usize::MAX, theme::TEXT));
        let caret = here.then_some(caret.byte);
        row(cv, ui, &place, left, y, Row { bytes, selected, colors: &colors, caret });
    }

    note(cv, ui, area, view, doc, length);
}

/// Why a file is shown in one color, in the words for the line at the
/// bottom.
fn gave_up(why: End) -> &'static [&'static str] {
    match why {
        End::TookTooLong => &["This file is shown in one color: finding its colors took too long."],
        End::Ended => &["This file is shown in one color: what finds its colors ended over it."],
        End::Broken => &["This file is shown in one color: its colors could not be made out."],
    }
}

/// Why a file of Markdown is shown as its text, in the words for the
/// line at the bottom.
fn no_preview(why: Why) -> &'static [&'static str] {
    match why {
        Why::Long => &["This file is shown as its text:", "it is larger than 1 MiB."],
        Why::Large => &["This file is shown as its text: there is", "more to it than a preview is made of."],
        Why::NotText => &["This file is shown as its text:", "not all of it is text in UTF-8."],
        Why::TookTooLong => &["This file is shown as its text:", "reading what it is made of took too long."],
        Why::Ended => &["This file is shown as its text:", "what reads Markdown ended over it."],
        Why::Broken => &["This file is shown as its text:", "what it is made of could not be made out."],
    }
}

/// What is said at the bottom of a file. How many lines it has and what
/// they say are read from this one value, so that the room that is
/// reckoned for them is the room they take.
enum Note<'a> {
    /// Lines that stand as they are.
    Lines(&'static [&'static str]),
    /// Something came to nothing, in a sentence that says so and the
    /// rest that says why.
    Failed(&'a str),
    /// A save in place failed half way, and why.
    Damaged(&'a str),
    /// The file goes on behind what is held of it.
    GoesOn,
}

impl Note<'_> {
    fn lines(&self) -> usize {
        match self {
            Self::Lines(lines) => lines.len(),
            Self::Failed(_) => 2,
            Self::Damaged(_) => 4,
            Self::GoesOn => 1,
        }
    }
}

/// What is said of a file: what a key was refused for, what came of a
/// save, what is asked, or else that the file goes on behind what is
/// held, or that its colors were given up. A pane beside a session is
/// narrow, so what is said is set in lines that are short, and the keys
/// that answer stand each where they can be read.
fn note_of(view: &FileView) -> Note<'_> {
    let cut = view.text().is_some_and(|text| text.cut());
    Note::Lines(match view.said() {
        Some(Said::Refused(Refused::Shown)) if cut => {
            &["This file is only shown:", "it goes on behind what is held of it."]
        }
        Some(Said::Refused(Refused::Shown)) => {
            &["This file is only shown:", "not all of it is text in UTF-8."]
        }
        Some(Said::Refused(Refused::Line)) => {
            &["Nothing was changed: a line there is", "longer than 64 KiB, or would be."]
        }
        Some(Said::Refused(Refused::Limit)) => {
            &["Nothing was changed: the text would be", "longer than 8 MiB or two million lines."]
        }
        Some(Said::Refused(Refused::NoText)) => &["Nothing was changed:", "what was to go in is not text."],
        Some(Said::Refused(Refused::Place)) => {
            &["Nothing was changed:", "the caret stands inside a character."]
        }
        Some(Said::Other) => &[
            "The file changed on the disk.",
            "Ctrl+R takes what is there,",
            "Ctrl+Shift+S writes what is here.",
        ],
        Some(Said::NoWayBack) => {
            &["Taking what is on the disk cannot be", "taken back. Ctrl+R takes it, Escape stays."]
        }
        Some(Said::Gone) => &["The file is gone.", "Ctrl+Shift+S writes it anew."],
        Some(Said::InPlace) => &[
            "This file is written in place: a crash",
            "in the middle leaves half of it.",
            "Ctrl+S writes it, Escape stays.",
        ],
        Some(Said::Unflushed) => &["Saved, but the disk did not confirm", "that it holds it."],
        Some(Said::Leave) => {
            &["Not saved.", "Ctrl+S saves, Escape stays,", "Ctrl+Shift+Backspace lets the changes go."]
        }
        Some(Said::Writing) => &[
            "The file is still being written.",
            "Ctrl+Shift+Backspace closes all the same,",
            "Escape stays.",
        ],
        Some(Said::Failed(why)) => return Note::Failed(why),
        Some(Said::Damaged(why)) => return Note::Damaged(why),
        None if cut => return Note::GoesOn,
        None => match view.unpreviewed() {
            Some(why) => no_preview(why),
            // What is said of the colors is said of the text.
            None if view.previews() => &[],
            None => view.colors.ended().map_or(&[], gave_up),
        },
    })
}

/// How many lines at the bottom of the pane say something of a file.
/// They take the room of the last rows.
pub fn noted(view: &FileView) -> usize {
    note_of(view).lines()
}

/// What is said at the bottom, line by line: as many as `noted` says.
fn says(view: &FileView, doc: &Document, length: u64) -> Vec<String> {
    match note_of(view) {
        Note::Lines(lines) => lines.iter().map(|line| (*line).to_owned()).collect(),
        // The first sentence says that it came to nothing, the rest why.
        Note::Failed(why) => {
            let (what, why) = why.split_once(". ").map_or((why, ""), |(what, why)| (what, why));
            let what = if why.is_empty() { what.to_owned() } else { format!("{what}.") };
            vec![what, why.to_owned()]
        }
        Note::Damaged(why) => vec![
            "Not saved, and the file may be damaged.".into(),
            why.to_owned(),
            "The text here is the only one.".into(),
            "Ctrl+Shift+S writes it once more.".into(),
        ],
        Note::GoesOn => vec![format!(
            "The file goes on: {} of {} are shown, {} lines.",
            size(doc.len() as u64),
            size(length.max(doc.len() as u64)),
            doc.lines()
        )],
    }
}

/// Draws what is said at the bottom of a file, over its last rows.
fn note(cv: &mut Canvas, ui: &Ui, area: Rect, view: &FileView, doc: &Document, length: u64) {
    // What asks for an answer is as plain as text, what went wrong has
    // the color of that.
    let color = match view.said() {
        Some(Said::Failed(_) | Said::Damaged(_)) => theme::DANGER,
        Some(said) if said.asks() => theme::TEXT,
        _ => theme::DIM,
    };
    bottom(cv, ui, area, &says(view, doc, length), color);
}

/// Draws lines at the bottom of a pane, over what is there.
fn bottom(cv: &mut Canvas, ui: &Ui, area: Rect, lines: &[String], color: Color) {
    if lines.is_empty() {
        return;
    }
    let height = note_height(ui, lines.len());
    cv.rect(Rect::new(area.x, area.bottom() - height, area.w, height), theme::BG);
    let (x, top) = (area.x + ui.px(PAD), area.bottom() - height + ui.px(NOTE_PAD) / 2.0);
    for (at, line) in lines.iter().enumerate() {
        let row = Rect::new(area.x, top + at as f32 * ui.px(NOTE_LINE), area.w, ui.px(NOTE_LINE));
        cv.text(line, UI, ui.text_px, x, ui.baseline(row), color, area.w - 2.0 * ui.px(PAD));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_under_a_picture_are_the_bottom_of_its_pane() {
        let ui = Ui::new(Vec::new());
        let body = Rect::new(10.0, 20.0, 300.0, 200.0);
        let two = lines_of(&ui, body, 2);
        assert_eq!((two.x, two.w, two.bottom()), (10.0, 300.0, 220.0));
        assert_eq!(two.h, note_height(&ui, 2));
        assert!(two.h > 0.0 && two.h < body.h);
        // No lines, no room, and never more than the body has.
        assert_eq!(lines_of(&ui, body, 0).h, 0.0);
        assert_eq!(lines_of(&ui, body, 100).h, 200.0);
    }

    #[test]
    fn a_picture_says_its_size_and_what_was_noted_of_it() {
        use crate::core::picture::{Decoded, Format};
        let mut seen = Seen::new(Format::Png);
        assert!(picture_says(&seen, 100, None).is_empty(), "nothing while it is read");
        assert_eq!(picture_lines(&seen), 0);
        seen.why = Some("It is too long to be shown.".into());
        assert_eq!(picture_says(&seen, 100, None), ["It is too long to be shown."]);
        assert_eq!(picture_lines(&seen), 1);
        let notes = Notes(Notes::ALPHA | Notes::EARLY | Notes::SMALLER);
        seen.shown = Some((Decoded::whole((6238, 4162), (999, 667), notes), 3));
        assert_eq!(
            picture_says(&seen, 29_696_347, None),
            [
                "6238 × 4162 pixels, 28.3 MiB",
                "The file ends early: what is there of it is shown.",
                "It is too large to be shown any finer.",
            ]
        );
        // How many lines that is, reckoned without writing them.
        assert_eq!(picture_lines(&seen), 3);
    }

    #[test]
    fn sizes_are_said_in_the_unit_that_fits() {
        assert_eq!(
            [size(0), size(1023), size(1536), size(8 << 20)],
            ["0 bytes", "1023 bytes", "1.5 KiB", "8.0 MiB"]
        );
    }

    /// A workspace that shows a file with this text, all of it or only
    /// what is held of a longer one.
    fn showing(text: &str, cut: bool) -> crate::core::Workspace {
        use crate::core::files::{Body, Content, stamp_of};
        use std::sync::atomic::{AtomicU64, Ordering};
        // A name for every call: the tests run side by side in one
        // process, and two of them ask for the same text.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("codn-note-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, text).unwrap();
        let stamp = stamp_of(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut ws = crate::core::Workspace::new();
        let project = ws.add_project("/p/trail".into());
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        let body = Body::Text(Document::new(text.as_bytes().to_vec(), cut).ok().unwrap());
        ws.file_mut().unwrap().read(Ok(Content { stamp, digest: None, body }), 20);
        ws
    }

    #[test]
    fn what_is_said_at_the_bottom_has_the_lines_its_room_is_reckoned_with() {
        let mut ws = showing("one\n", false);
        let view = ws.file_mut().unwrap();
        assert_eq!(noted(view), 0);
        let refused = [Refused::Shown, Refused::Place, Refused::Line, Refused::Limit, Refused::NoText];
        let said = [
            Said::Other,
            Said::NoWayBack,
            Said::Gone,
            Said::InPlace,
            Said::Unflushed,
            Said::Leave,
            Said::Writing,
            Said::Failed("Not saved. It may not be written.".into()),
            Said::Failed("It is not there any more.".into()),
            Said::Damaged("There is no room for it on the disk.".into()),
        ];
        for said in refused.into_iter().map(Said::Refused).chain(said) {
            view.say(Some(said.clone()));
            let lines = says(view, view.text().unwrap(), 4);
            assert_eq!(lines.len(), noted(view), "{said:?}");
            // Short enough for a pane beside a session in a small window.
            assert!(lines.iter().all(|line| line.len() <= 44), "{said:?}: {lines:?}");
        }
        view.say(Some(Said::Failed("Not saved. It may not be written.".into())));
        assert_eq!(says(view, view.text().unwrap(), 4), ["Not saved.", "It may not be written."]);
        // Why a file of Markdown is shown as its text.
        for why in [Why::Long, Why::Large, Why::NotText, Why::TookTooLong, Why::Ended, Why::Broken] {
            let lines = no_preview(why);
            assert!(lines.len() == 2 && lines.iter().all(|line| line.len() <= 44), "{why:?}: {lines:?}");
        }
    }

    #[test]
    fn a_file_of_markdown_that_has_no_preview_says_why_under_its_text() {
        use crate::core::files::{Body, Content, stamp_of};
        let mut ws = crate::core::Workspace::new();
        let project = ws.add_project("/tmp/codn-preview-note".into());
        ws.open_file(project, "a.md".into(), |_| Some("Markdown")).unwrap();
        let path = std::env::temp_dir().join(format!("codn-preview-note-{}", std::process::id()));
        std::fs::write(&path, "# a\n").unwrap();
        let stamp = stamp_of(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let body = Body::Text(Document::new(b"# a\n".to_vec(), false).ok().unwrap());
        let view = ws.file_mut().unwrap();
        view.read(Ok(Content { stamp, digest: None, body }), 20);
        // Shown as it is meant, with nothing to say.
        assert!(view.previews() && view.switches() && noted(view) == 0);
        let of = view.edition();
        view.preview.asked(of);
        assert!(view.no_preview(of, Why::NotText, 20));
        assert!(!view.previews() && view.switches());
        let lines = says(view, view.text().unwrap(), 4);
        assert_eq!((lines.len(), noted(view)), (2, 2));
        assert!(lines[1].contains("UTF-8"), "{lines:?}");
        // The switch asks anew, and the text says why until an answer
        // comes. The process ended over it twice: then the switch turns
        // the wish off, and there is nothing to say for that.
        view.switch_preview(20);
        assert!(!view.previews() && view.tries_preview() && noted(view) == 2);
        for _ in 0..2 {
            view.preview.asked(of);
            assert!(view.no_preview(of, Why::Ended, 20));
            view.preview.again(0);
        }
        view.switch_preview(20);
        assert!(!view.previews() && !view.tries_preview() && noted(view) == 0);
        // And a file that is none of Markdown has no switch.
        ws.open_file(project, "b.rs".into(), |_| Some("Rust")).unwrap();
        assert!(!ws.file().unwrap().1.switches());
    }

    #[test]
    fn what_a_file_says_unasked_gives_way_to_what_its_line_says() {
        let said = |view: &FileView| says(view, view.text().unwrap(), 4096);
        // A text that goes on behind what is held says so, also where its
        // colors were given up as well: one line, and that one.
        let mut ws = showing("one\n", true);
        let view = ws.file_mut().unwrap();
        let lines = view.text().unwrap().lines();
        let goes_on = format!("The file goes on: 4 bytes of 4.0 KiB are shown, {lines} lines.");
        assert_eq!((noted(view), said(view)), (1, vec![goes_on.clone()]));
        view.colors.end(End::Broken);
        assert_eq!((noted(view), said(view)), (1, vec![goes_on.clone()]));
        // What the line says of the file stands in the place of it.
        view.say(Some(Said::Gone));
        assert_eq!(said(view), ["The file is gone.", "Ctrl+Shift+S writes it anew."]);
        assert_eq!(noted(view), 2);
        view.say(Some(Said::Refused(Refused::Shown)));
        assert_eq!(said(view), ["This file is only shown:", "it goes on behind what is held of it."]);
        view.say(None);
        assert_eq!((noted(view), said(view)), (1, vec![goes_on]));

        // A text that is held whole says nothing, until its colors end.
        let mut ws = showing("one\n", false);
        let view = ws.file_mut().unwrap();
        assert_eq!((noted(view), said(view)), (0, Vec::<String>::new()));
        for (why, text) in [
            (End::TookTooLong, "This file is shown in one color: finding its colors took too long."),
            (End::Ended, "This file is shown in one color: what finds its colors ended over it."),
            (End::Broken, "This file is shown in one color: its colors could not be made out."),
        ] {
            view.colors.end(why);
            assert_eq!((noted(view), said(view)), (1, vec![text.to_owned()]));
        }
        view.say(Some(Said::Refused(Refused::Shown)));
        assert_eq!(said(view), ["This file is only shown:", "not all of it is text in UTF-8."]);
        // What came to nothing has two lines, also where it says no why.
        view.say(Some(Said::Failed("It is not there any more.".into())));
        assert_eq!(
            (noted(view), said(view)),
            (2, vec!["It is not there any more.".to_owned(), String::new()])
        );
        view.say(Some(Said::Damaged("The disk is gone.".into())));
        assert_eq!(noted(view), 4);
        assert_eq!(
            said(view),
            [
                "Not saved, and the file may be damaged.",
                "The disk is gone.",
                "The text here is the only one.",
                "Ctrl+Shift+S writes it once more."
            ]
        );
    }

    #[test]
    fn a_pane_has_as_many_rows_however_long_the_text_is() {
        let mut ui = Ui::new(Vec::new());
        (ui.cell_w, ui.cell_h) = (8.0, 17.0);
        let body = Rect::new(240.0, 30.0, 500.0, 400.0);
        for noted in 0..5 {
            let rows = place(&ui, body, 1, noted).rows;
            assert!(rows >= 1);
            for lines in [0, 2, 999, 1000, 2_000_000] {
                assert_eq!(place(&ui, body, lines, noted).rows, rows, "{noted} {lines}");
            }
        }
    }

    #[test]
    fn keys_move_the_caret_and_control_moves_it_further() {
        assert_eq!(key(NamedKey::ArrowLeft, false, 40), Some(Move::Left));
        assert_eq!(key(NamedKey::ArrowLeft, true, 40), Some(Move::WordLeft));
        assert_eq!(key(NamedKey::End, true, 40), Some(Move::Bottom));
        assert_eq!(key(NamedKey::PageDown, false, 40), Some(Move::PageDown(39)));
        assert_eq!(key(NamedKey::Enter, false, 40), None);
    }
}
