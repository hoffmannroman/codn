//! A pane: its header with title and buttons, and its content.

use super::{Hit, Hits, PAD, PaneLayout, Ui, chrome, diff, file, theme};
use crate::action::Action;
use crate::core::{Pane, Session, Workspace, files};
use crate::render::font::{BOLD, UI};
use crate::render::{Canvas, Color, Icon, Rect};
use crate::term::CellAt;

/// Height of the note that lies over the last rows of a terminal whose
/// program has ended.
const NOTE: f32 = 34.0;

/// What a pane shows below its header.
fn body(l: &PaneLayout) -> Rect {
    let top = l.header.bottom();
    Rect::new(l.area.x, top, l.area.w, l.area.h - top)
}

/// Where the note of a program that has ended lies in the body of its pane.
fn note(ui: &Ui, body: Rect) -> Rect {
    Rect::new(body.x, body.bottom() - ui.px(NOTE), body.w, ui.px(NOTE))
}

/// The cell of a terminal that is shown at a point of its pane, for what
/// is done to the text there and nowhere else: none in the room around
/// the grid, where `Ui::cell_at` gives the nearest one for a selection
/// that is dragged out of it, and none in a row that the note of a
/// program that has ended lies over, `noted`. A click there would open
/// an address nobody reads.
pub fn shown_cell(ui: &Ui, l: &PaneLayout, noted: bool, x: f32, y: f32) -> Option<CellAt> {
    let grid = Rect::new(l.origin.0, l.origin.1, l.cols as f32 * ui.cell_w, l.rows as f32 * ui.cell_h);
    let at = grid.contains(x, y).then(|| ui.cell_at(l, x, y))?;
    let bottom = l.origin.1 + (at.row + 1) as f32 * ui.cell_h;
    (!noted || bottom <= note(ui, body(l)).y).then_some(at)
}

/// `last` is the pane at the right.
pub fn draw(
    cv: &mut Canvas,
    ui: &Ui,
    ws: &Workspace,
    index: usize,
    l: &PaneLayout,
    last: bool,
    hits: &mut Hits,
) {
    let focused = ws.focus() == index;
    let px = ui.text_px;
    hits.push((l.area, Hit::Pane(index)));
    // Down to the end of its header a pane moves the window, also where
    // that header stands a row lower.
    hits.push((Rect::new(l.area.x, 0.0, l.area.w, l.header.bottom()), Hit::Drag));
    if !last {
        cv.rect(Rect::new(l.area.right(), 0.0, 1.0, l.area.h), theme::LINE);
    }
    let right = ui.head_right(l);
    // Which of two panes has the keyboard is told by surfaces: its header
    // is lighter, as far as it is the pane's and not the window's, and
    // what the other pane shows steps back. The same key saves a file in
    // one and stops the output of a shell in the other.
    let marked = ws.panes().len() > 1;
    if marked && focused {
        cv.rect(Rect::new(l.header.x, l.header.y, right - l.header.x, l.header.h), theme::HOVER);
    }

    // Title and project, and at the right what closes the pane: while
    // there are two, and for a file and a diff, which have no session to
    // go back to otherwise. What else can be done stands at the top
    // right of the window, see `chrome::controls`.
    let (title, project, closable) = match &ws.panes()[index] {
        Pane::Session(id) => {
            let Some(session) = ws.session(*id) else { return };
            (ws.label(session), Some(session.project), ws.panes().len() > 1)
        }
        Pane::Diff(view) => ("Changes".to_owned(), Some(view.project), true),
        Pane::File(view) => {
            let name = view.path.file_name().map_or_else(String::new, files::shown);
            (name, Some(view.project), true)
        }
        Pane::Empty => (String::new(), None, false),
    };
    let project = project.and_then(|id| ws.project(id));
    // Beside the title, fainter: for a file the folder it lies in, for
    // the changes the project they are of. Not for a session: its row
    // in the sidebar stands under its project. Only while the sidebar
    // is folded to a strip, which has no names, the project is said
    // here.
    let beside = match &ws.panes()[index] {
        Pane::File(view) => view.path.parent().map(|folder| files::shown(folder.as_os_str())),
        Pane::Session(_) if !ui.sidebar_folded => None,
        _ => project.map(|project| project.name.clone()),
    };
    let mut x = l.area.x + ui.px(PAD);
    let base = ui.baseline(l.header);
    // Before the title of a session the mark of its program, the one
    // place that says which it is.
    if let Pane::Session(id) = &ws.panes()[index]
        && let Some(session) = ws.session(*id)
        && ui.mark_w > 0.0
        && l.header.w > 2.0 * ui.px(PAD) + ui.mark_w
    {
        chrome::agent(cv, ui, &session.agent, x, l.header);
        x += ui.mark_w;
    }
    let (gap, button) = (ui.px(10.0), ui.px(28.0));
    // What closes the pane is there whenever it fits at all: beside
    // the icons at the top right of the window, a narrow pane had
    // nothing to close it by.
    let mut right = right;
    if closable && right - button >= l.area.x {
        right -= button;
        let place = Rect::new(right, l.header.y, button, l.header.h);
        chrome::icon_button(cv, ui, place, Icon::Close, Action::ClosePane(index), hits);
    }
    // A file of Markdown has the switch between the file as it is meant
    // and its text beside that: it shows what a click leads to.
    if let Pane::File(view) = &ws.panes()[index]
        && view.switches()
        && right - button >= l.area.x
    {
        right -= button;
        let place = Rect::new(right, l.header.y, button, l.header.h);
        let which = if view.previews() { Icon::Code } else { Icon::OpenPreview };
        chrome::icon_button(cv, ui, place, which, Action::TogglePreview, hits);
    }
    let max = (right - x - ui.px(8.0)).max(0.0);
    // A file with changes that are not saved says so by the color of its
    // name.
    let changed = matches!(&ws.panes()[index], Pane::File(view) if view.changed());
    let color = match (changed, focused) {
        (true, _) => theme::WORKING,
        (false, true) => theme::TEXT,
        (false, false) => theme::DIM,
    };
    let used = cv.text(&title, UI | BOLD, px, x, base, color, max);
    let mut used = used;
    if let Some(beside) = beside.filter(|beside| !beside.is_empty())
        && max - used - gap > ui.px(24.0)
    {
        used += gap + cv.text(&beside, UI, px, x + used + gap, base, theme::FAINT, max - used - gap);
    }
    // A file that the last commit does not have says so, in the color
    // of what is new in the changes.
    if matches!(&ws.panes()[index], Pane::File(view) if view.marks().new) && max - used - gap > ui.px(24.0) {
        cv.text("new", UI, px, x + used + gap, base, theme::ADDED, max - used - gap);
    }

    let body = body(l);
    match &ws.panes()[index] {
        Pane::Session(id) => {
            let Some(session) = ws.session(*id) else { return };
            session_body(cv, ui, session, l, body, focused);
        }
        Pane::Diff(view) => diff::draw(cv, ui, view, body, hits),
        Pane::File(view) => file::draw(cv, ui, view, body, focused && ui.focused && !ui.editing(), hits),
        Pane::Empty => centered(cv, ui, body, empty_hint(ui, ws), theme::FAINT),
    }
    if marked && !focused {
        // Not over a picture, only over the lines under it.
        let veiled = match &ws.panes()[index] {
            Pane::File(view) => file::veiled(ui, body, view),
            _ => body,
        };
        cv.rect(veiled, theme::VEIL);
    }
}

/// What a pane shows of a session: its terminal, and what there is to say
/// about a program that is not running.
fn session_body(cv: &mut Canvas, ui: &Ui, session: &Session, l: &PaneLayout, body: Rect, focused: bool) {
    let (px, x) = (ui.text_px, l.area.x + ui.px(PAD));
    if let Some(term) = &session.term {
        let active = focused && ui.focused && !ui.editing() && session.ended.is_none();
        let link = ui.link.filter(|(of, _)| *of == session.id).map(|(_, at)| at);
        term.draw(cv, &ui.look(), l.origin, active, link);
    }
    match (&session.ended, &session.term) {
        // What the program printed last stays readable above the note.
        (Some(why), Some(_)) => {
            let row = note(ui, body);
            cv.rect(row, theme::BG);
            let note = format!("{why} Enter starts it again.");
            cv.text(&note, UI, px, x, ui.baseline(row), theme::DANGER, body.w - 2.0 * ui.px(PAD));
        }
        (Some(why), None) => {
            let upper = Rect::new(body.x, body.y, body.w, body.h - ui.px(28.0));
            let lower = Rect::new(body.x, body.y + ui.px(28.0), body.w, body.h - ui.px(28.0));
            centered(cv, ui, upper, why, theme::DANGER);
            centered(cv, ui, lower, "Enter tries again.", theme::FAINT);
        }
        (None, None) => centered(cv, ui, body, "Starting…", theme::FAINT),
        (None, Some(_)) => {}
    }
}

/// What a pane that shows nothing says there is to do.
fn empty_hint(ui: &Ui, ws: &Workspace) -> &'static str {
    if ws.projects.is_empty() {
        "Add a project to start"
    } else if !ui.agents.iter().any(|agent| agent.installed) {
        "No agent is installed. Check [[agent]] in config.toml."
    } else {
        "Start a session from the sidebar"
    }
}

/// One line of text in the middle of an area, cut off if it does not fit.
pub fn centered(cv: &mut Canvas, ui: &Ui, area: Rect, text: &str, color: Color) {
    let max = area.w - 2.0 * ui.px(PAD);
    let width = cv.text_width(text, UI, ui.text_px).min(max);
    let x = (area.x + (area.w - width) / 2.0).round();
    cv.text(text, UI, ui.text_px, x, ui.baseline(area), color, max);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_shown_only_within_the_grid_and_not_under_the_note() {
        // A click with Control in the room beside the grid opened the
        // address in the nearest cell, and one on the note of a program
        // that had ended opened what stood under the note.
        let mut ui = Ui::new(Vec::new());
        (ui.width, ui.height, ui.cell_w, ui.cell_h) = (1200.0, 760.0, 8.0, 20.0);
        let l = ui.pane(0, 1);
        let (x, y) = l.origin;
        let cell = |noted, x, y| shown_cell(&ui, &l, noted, x, y).map(|at| (at.col, at.row));
        assert_eq!(cell(false, x, y), Some((0, 0)));
        assert_eq!(cell(false, x + 3.0 * 8.0 + 5.0, y + 2.0 * 20.0 + 1.0), Some((3, 2)));
        let (right, bottom) = (x + l.cols as f32 * 8.0, y + l.rows as f32 * 20.0);
        assert_eq!(cell(false, right - 1.0, bottom - 1.0), Some((l.cols as usize - 1, l.rows as usize - 1)));
        // Around the grid there is room that is the pane's and no cell.
        for (x, y) in [(x - 1.0, y), (l.area.x + 2.0, y + 5.0), (right, y), (x, bottom), (x, y - 1.0)] {
            assert!(l.area.contains(x, y) || y < l.origin.1, "{x} {y}");
            assert_eq!(cell(false, x, y), None, "{x} {y}");
            // For a selection the nearest cell is still given.
            assert!(ui.cell_at(&l, x, y).row < l.rows as usize);
        }
        // The note: no row it lies over, not even one it cuts, and every
        // row above it.
        let top = note(&ui, body(&l)).y;
        let cut = ((top - y) / 20.0).floor();
        assert!(cut < l.rows as f32, "the note lies over a row of this grid");
        assert_eq!(cell(true, x, top + 10.0), None);
        assert_eq!(cell(true, x, y + cut * 20.0), None, "the row the note begins in");
        assert_eq!(cell(true, x, y + cut * 20.0 - 1.0), Some((0, cut as usize - 1)));
        assert_eq!(cell(false, x, y + cut * 20.0), Some((0, cut as usize)));
    }
}
