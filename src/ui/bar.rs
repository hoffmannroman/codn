//! The bar along the bottom edge of the window: at the left what adds a
//! project, beside it what the program of the session in front reports
//! about itself, each figure a ring that fills, and before them the speed
//! of its model.

use std::borrow::Cow;

use super::{Hit, Hits, INSET, PAD, Ui, chrome, theme};
use crate::action::Action;
use crate::core::reported::Gauge;
use crate::core::{Session, Workspace};
use crate::render::font::{BOLD, UI};
use crate::render::{Canvas, Rect};

/// The ring of a figure: how wide it is, and its line.
const RING: f32 = 14.0;
const RING_LINE: f32 = 2.0;
/// The room between a ring and its figure, and between the figure and
/// what is written beside it.
const WITHIN: f32 = 6.0;
/// The room between two figures.
const BETWEEN: f32 = 20.0;
/// The widest a figure usually is. Every figure gets this much room, so a
/// ring stays where it is when one goes from 9% to 10%.
const WIDEST: &str = "100%";
/// What the speed of a model is counted in: tokens a second.
const SPEED: &str = "tok/s";

/// How much is written beside a ring: all there is to say, the name alone,
/// or the figure alone.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Told {
    All,
    Name,
    Figure,
}

/// A figure as it is drawn: its texts, the room the figure gets, and the
/// room it takes in all, by how much is told.
struct Piece {
    /// How full its ring is, in percent. A figure that is no part of a
    /// whole has no ring.
    ring: Option<f64>,
    figure: String,
    /// What is written beside the figure, by how much is told.
    beside: [Option<Cow<'static, str>>; 3],
    slot: f32,
    widths: [f32; 3],
}

pub fn draw(cv: &mut Canvas, ui: &Ui, ws: &Workspace, hits: &mut Hits) {
    let bar = ui.bar();
    cv.rect(bar, theme::SIDEBAR);
    cv.rect(Rect::new(0.0, bar.y, bar.w, 1.0), theme::LINE);
    hits.push((bar, Hit::None));
    add(cv, ui, bar, hits);
    if let Some(session) = front(ws) {
        report(cv, ui, session, bar);
    }
}

/// What opens the list that adds a project, and closes it again.
fn add(cv: &mut Canvas, ui: &Ui, bar: Rect, hits: &mut Hits) {
    let (px, inset) = (ui.text_px, ui.px(INSET));
    let place = Rect::new(0.0, bar.y, ui.left(), bar.h);
    let color = ui.lit(Action::AddProject, theme::FAINT);
    chrome::plus(cv, ui, Rect::new(inset, bar.y, ui.px(9.0), bar.h), color);
    // Below a sidebar that is folded to a strip the plus stands alone.
    if !ui.sidebar_folded {
        cv.text("Add project", UI, px, inset + ui.px(14.0), ui.baseline(bar), color, place.w - 2.0 * inset);
    }
    hits.push((place, Hit::Do(Action::AddProject)));
}

/// The session the bar speaks of: the one with the keyboard, or beside a
/// pane that shows no session the one in the other.
fn front(ws: &Workspace) -> Option<&Session> {
    let shown = ws.focused_session().or_else(|| (0..ws.panes().len()).find_map(|pane| ws.pane_session(pane)));
    shown.and_then(|id| ws.session(id)).filter(|session| session.ended.is_none())
}

/// Below the panes: at the right the figures a program reports, before
/// them the speed of its model, at the left the model and the options it
/// reports to run with, or where it reports neither what was set for the
/// session before it started.
fn report(cv: &mut Canvas, ui: &Ui, session: &Session, bar: Rect) {
    let (px, pad, between) = (ui.text_px, ui.px(PAD), ui.px(BETWEEN));
    let left = ui.left() + 1.0 + pad;
    let right = bar.right() - pad;
    let room = right - left;
    let gauges = session.reported.as_ref().map_or_else(Vec::new, |reported| reported.gauges());
    let mut pieces: Vec<Piece> = gauges.iter().map(|gauge| piece(cv, ui, gauge)).collect();
    // The speed is the last that is given room, so it is the first to
    // go (`fit_speed`), and where it has room it stands before the rings.
    let speed = session.reported.as_ref().and_then(|reported| reported.speed);
    pieces.extend(speed.map(|speed| rate(cv, ui, speed)));
    let widths: Vec<[f32; 3]> = pieces.iter().map(|piece| piece.widths).collect();
    let (told, count, taken) =
        if speed.is_some() { fit_speed(&widths, between, room) } else { fit(&widths, between, room) };
    let mut shown: Vec<&Piece> = pieces[..count].iter().collect();
    if speed.is_some() && count == pieces.len() {
        shown.rotate_right(1);
    }
    let rooms: Vec<f32> = shown.iter().map(|piece| piece.widths[told as usize]).collect();
    for (piece, x) in shown.iter().zip(places(&rooms, between, right)) {
        show(cv, ui, piece, told, x, bar);
    }
    // The figures change, the name does not: it gives way to them.
    let name = room - taken - if count == 0 { 0.0 } else { between };
    if name > ui.px(24.0) {
        let runs = session.reported.as_ref().and_then(|reported| reported.runs());
        let text = runs.unwrap_or_else(|| ui.set(session));
        cv.text(&text, UI, px, left, ui.baseline(bar), theme::DIM, name);
    }
}

/// Measures a gauge once for the frame.
fn piece(cv: &mut Canvas, ui: &Ui, gauge: &Gauge) -> Piece {
    let (px, within) = (ui.text_px, ui.px(WITHIN));
    let figure = gauge.figure();
    let all = gauge
        .detail
        .as_ref()
        .map_or_else(|| gauge.name.clone(), |detail| format!("{} · {detail}", gauge.name));
    let slot = cv.text_width(&figure, UI | BOLD, px).max(cv.text_width(WIDEST, UI | BOLD, px));
    let alone = ui.px(RING) + within + slot;
    let beside = |cv: &mut Canvas, text: &str| alone + within + cv.text_width(text, UI, px);
    let widths = [beside(cv, &all), beside(cv, &gauge.name), alone];
    Piece {
        ring: Some(gauge.whole()),
        figure,
        beside: [Some(all.into()), Some(gauge.name.clone().into()), None],
        slot,
        widths,
    }
}

/// Measures the speed of a model once for the frame. It has no ring, and
/// it keeps what it is counted in however little is told: the number
/// alone would say nothing. Its figure gets the room it takes and no
/// more, so the two stand together, and it is the left one of the row
/// that grows.
fn rate(cv: &mut Canvas, ui: &Ui, speed: u32) -> Piece {
    let (px, within) = (ui.text_px, ui.px(WITHIN));
    let figure = speed.to_string();
    let slot = cv.text_width(&figure, UI | BOLD, px);
    let width = slot + within + cv.text_width(SPEED, UI, px);
    Piece { ring: None, figure, beside: [(); 3].map(|()| Some(SPEED.into())), slot, widths: [width; 3] }
}

/// How much is told beside the rings so that all of them fit into `room`,
/// with how many are drawn and the room they take. Where not even the
/// figures alone fit, the first ones that do are drawn. `widths` holds
/// for each gauge the room it takes by how much is told.
fn fit(widths: &[[f32; 3]], between: f32, room: f32) -> (Told, usize, f32) {
    let taken = |told: Told, count: usize| -> f32 {
        let all: f32 = widths[..count].iter().map(|widths| widths[told as usize]).sum();
        all + count.saturating_sub(1) as f32 * between
    };
    for told in [Told::All, Told::Name, Told::Figure] {
        if taken(told, widths.len()) <= room {
            return (told, widths.len(), taken(told, widths.len()));
        }
    }
    let count = (0..widths.len()).rev().find(|count| taken(Told::Figure, *count) <= room).unwrap_or(0);
    (Told::Figure, count, taken(Told::Figure, count))
}

/// As `fit`, where the last of `widths` is the speed. It is the first to
/// go, and the rings then have its room: as much is told beside them as
/// would be without a speed, not as little as was tried to keep it.
fn fit_speed(widths: &[[f32; 3]], between: f32, room: f32) -> (Told, usize, f32) {
    let all = fit(widths, between, room);
    if all.1 == widths.len() {
        return all;
    }
    fit(&widths[..widths.len() - 1], between, room)
}

/// The left edges of things of these widths, `between` apart, the last
/// one ending at `right`. Not rounded: each is put on a pixel where it is
/// drawn, and a rounding carried from one to the next would add up.
fn places(widths: &[f32], between: f32, right: f32) -> Vec<f32> {
    let all: f32 = widths.iter().sum::<f32>() + widths.len().saturating_sub(1) as f32 * between;
    let mut x = right - all;
    let place = |width: &f32| {
        let at = x;
        x += width + between;
        at
    };
    widths.iter().map(place).collect()
}

/// Draws a figure with its left edge at `x`: its ring where it has one,
/// the figure, and what is told beside it.
fn show(cv: &mut Canvas, ui: &Ui, piece: &Piece, told: Told, x: f32, bar: Rect) {
    let (px, base, size, within) = (ui.text_px, ui.baseline(bar), ui.px(RING), ui.px(WITHIN));
    let figure = match piece.ring {
        Some(percent) => {
            let ring = Rect::new(x.round(), bar.y + ((bar.h - size) / 2.0).round(), size, size);
            let line = ui.px(RING_LINE).max(1.0);
            cv.outline(ring, size, line, theme::ACTIVE);
            // As much of the ring as the figure says: no trace of an arc at 0%.
            cv.ring(ring, line, (percent / 100.0) as f32, theme::ACCENT);
            ring.right() + within
        }
        None => x.round(),
    };
    cv.text(&piece.figure, UI | BOLD, px, figure, base, theme::TEXT, f32::MAX);
    if let Some(beside) = &piece.beside[told as usize] {
        cv.text(beside, UI, px, figure + piece.slot + within, base, theme::DIM, f32::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three gauges as a font makes them: no width is a whole pixel.
    const THREE: [[f32; 3]; 3] = [[168.649, 98.3, 54.7], [152.1, 70.45, 54.7], [178.646, 70.45, 54.7]];

    fn all(widths: &[[f32; 3]], told: Told, between: f32) -> f32 {
        widths.iter().map(|widths| widths[told as usize]).sum::<f32>() + (widths.len() - 1) as f32 * between
    }

    #[test]
    fn as_much_is_told_as_fits_for_every_ring() {
        let (everything, names, figures) =
            (all(&THREE, Told::All, 20.0), all(&THREE, Told::Name, 20.0), all(&THREE, Told::Figure, 20.0));
        assert_eq!(fit(&THREE, 20.0, 900.0), (Told::All, 3, everything));
        assert_eq!(fit(&THREE, 20.0, everything), (Told::All, 3, everything));
        assert_eq!(fit(&THREE, 20.0, everything - 0.01), (Told::Name, 3, names));
        assert_eq!(fit(&THREE, 20.0, names - 0.01), (Told::Figure, 3, figures));
        // Not even the figures fit: the first ones that do are drawn, and
        // only the room they take is theirs.
        assert_eq!(fit(&THREE, 20.0, figures - 0.01), (Told::Figure, 2, 54.7 + 20.0 + 54.7));
        assert_eq!(fit(&THREE, 20.0, 60.0), (Told::Figure, 1, 54.7));
        assert_eq!(fit(&THREE, 20.0, 10.0), (Told::Figure, 0, 0.0));
        assert_eq!(fit(&THREE, 20.0, -5.0), (Told::Figure, 0, 0.0));
        assert_eq!(fit(&[], 20.0, 100.0), (Told::All, 0, 0.0));
    }

    #[test]
    fn the_speed_is_the_first_to_go_and_leaves_its_room_to_the_rings() {
        // Three rings and, last, a speed.
        let speed = [60.0; 3];
        let with: Vec<[f32; 3]> = THREE.iter().copied().chain([speed]).collect();
        let (everything, names, figures) =
            (all(&THREE, Told::All, 20.0), all(&THREE, Told::Name, 20.0), all(&THREE, Told::Figure, 20.0));
        // While it fits it stays, and the rings give up what stands beside
        // them for it, as they do for each other.
        assert_eq!(fit_speed(&with, 20.0, everything + 80.0), (Told::All, 4, everything + 80.0));
        assert_eq!(fit_speed(&with, 20.0, everything + 79.9), (Told::Name, 4, names + 80.0));
        assert_eq!(fit_speed(&with, 20.0, figures + 80.0), (Told::Figure, 4, figures + 80.0));
        // Where it does not fit beside the figures alone it goes, and the
        // rings are told of as without it: here their names fit again.
        assert!(names < figures + 80.0);
        assert_eq!(fit_speed(&with, 20.0, names), fit(&THREE, 20.0, names));
        assert_eq!(fit_speed(&with, 20.0, names), (Told::Name, 3, names));
        assert_eq!(fit_speed(&with, 20.0, names - 0.01), (Told::Figure, 3, figures));
        // Then the last rings go, as without a speed.
        assert_eq!(fit_speed(&with, 20.0, figures - 0.01), fit(&THREE, 20.0, figures - 0.01));
        assert_eq!(fit_speed(&with, 20.0, 10.0), (Told::Figure, 0, 0.0));
        // A speed alone, and no room for it.
        assert_eq!(fit_speed(&[speed], 20.0, 60.0), (Told::All, 1, 60.0));
        assert_eq!(fit_speed(&[speed], 20.0, 59.9), (Told::All, 0, 0.0));
    }

    #[test]
    fn every_ring_that_fits_is_placed_and_the_last_ends_at_the_edge() {
        // In a window of 1200 pixels: the room beside the sidebar.
        let (right, room) = (1188.0, 1188.0 - 249.0);
        let (told, count, taken) = fit(&THREE, 20.0, room);
        assert_eq!((told, count), (Told::All, 3));
        let widths: Vec<f32> = THREE.iter().map(|widths| widths[told as usize]).collect();
        let at = places(&widths, 20.0, right);
        assert_eq!(at.len(), 3);
        // Each begins where the one before ended, whatever pixel that one
        // was drawn on, and the last one ends at the edge.
        assert!((at[0] - (right - taken)).abs() < 0.01);
        assert!((at[1] - (at[0] + widths[0] + 20.0)).abs() < 0.01);
        assert!((at[2] + widths[2] - right).abs() < 0.01, "{at:?}");
        assert!(places(&[], 20.0, right).is_empty());
        assert_eq!(places(&[50.0], 20.0, right), [right - 50.0]);
    }
}
