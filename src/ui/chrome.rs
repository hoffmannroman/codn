//! The window's own frame: buttons, resize edges, pointer shapes. And the
//! small drawings the other elements share: icons, marks, the plus.

use winit::window::{CursorIcon, ResizeDirection};

use super::{Hit, Hits, Ui, theme};
use crate::action::Action;
use crate::core::{Doing, Session, Workspace};
use crate::render::{Canvas, Color, Icon, Rect};

/// Width of a window button in logical pixels.
const BUTTON: f32 = 44.0;
/// Width of one of the icons at the top right of the window that stand
/// for something to do, and how many of them there are. With the buttons
/// of the window they are as wide as the column with the files, so that
/// none of them stands across the line before it.
pub const CONTROL: f32 = 26.0;
pub const CONTROLS: usize = 4;
/// Size of an icon in logical pixels.
pub const ICON: f32 = 16.0;
/// The room behind the mark of an agent, in the column the marks stand in.
pub const MARK_GAP: f32 = 7.0;
/// How far the line of minimize is moved down, on the square of 16 it is
/// drawn on. Codicons has it in the middle, here it lies where the frame of
/// maximize ends.
const MINIMIZE_DROP: f32 = 6.0;
/// Width of the zone at the window edge that resizes.
const EDGE: f32 = 5.0;

/// A square of `size` centered in `area`, on the pixel grid.
fn icon(area: Rect, size: f32) -> Rect {
    Rect::new((area.x + (area.w - size) / 2.0).round(), (area.y + (area.h - size) / 2.0).round(), size, size)
}

/// Width of the mark in the header in logical pixels. It is as wide as the
/// square it is drawn on and about two thirds of that high: 15 pixels.
const MARK: f32 = 24.0;

/// The mark of codn in the header of the sidebar, see `scripts/mark.py`,
/// with `middle` in the middle of it.
pub fn logo(cv: &mut Canvas, ui: &Ui, middle: f32, header: f32) {
    let size = logo_width(ui);
    cv.icon(Icon::Mark, logo_x(ui, middle), ((header - size) / 2.0).round(), size, theme::TEXT);
}

/// Where that mark begins.
pub fn logo_x(ui: &Ui, middle: f32) -> f32 {
    (middle - logo_width(ui) / 2.0).round()
}

/// How wide it is.
pub fn logo_width(ui: &Ui) -> f32 {
    ui.px(MARK).round()
}

/// Draws an icon centered in `area`.
pub fn symbol(cv: &mut Canvas, ui: &Ui, which: Icon, area: Rect, color: Color) {
    let at = icon(area, ui.px(ICON));
    cv.icon(which, at.x, at.y, at.w, color);
}

/// Draws what an agent is known by, its icon or else its mark, centered
/// in the column for it that begins at `x`.
pub fn agent(cv: &mut Canvas, ui: &Ui, agent: &str, x: f32, row: Rect) {
    let column = ui.mark_w - ui.px(MARK_GAP);
    match ui.mark(agent) {
        // The column has room for two characters, and in the strip of
        // a folded sidebar nothing may stand behind it.
        (None, mark) if mark.chars().count() > 2 => {
            let end = mark.char_indices().nth(2).map_or(mark.len(), |(at, _)| at);
            let (px, short) = (ui.text_px, &mark[..end]);
            let mut at = x + ((column - cv.text_width(short, 0, px)) / 2.0).max(0.0).round();
            for c in short.chars() {
                at += cv.glyph(c, 0, px, at, ui.baseline(row), theme::DIM);
            }
        }
        (Some(which), _) => symbol(cv, ui, which, Rect::new(x, row.y, column, row.h), theme::DIM),
        (None, mark) => {
            let px = ui.text_px;
            let width = cv.text_width(mark, 0, px);
            let mut at = x + ((column - width) / 2.0).max(0.0).round();
            for c in mark.chars() {
                at += cv.glyph(c, 0, px, at, ui.baseline(row), theme::DIM);
            }
        }
    }
}

/// Draws what a session is doing, centered in `area`: a mark that is told
/// by its shape and by its color, each alone. The one of a session at
/// work moves, by the step `ui.step` is at.
pub fn state(cv: &mut Canvas, ui: &Ui, session: &Session, area: Rect) {
    // The program is not running: it could not start, or ended with an
    // error.
    let doing = if session.ended.is_some() { Doing::Failed } else { session.doing() };
    mark(cv, ui, doing, area, ICON);
}

/// Draws the mark of what is being done, `size` wide and centered in
/// `area`: that of a session, or smaller that of a row below one.
pub fn mark(cv: &mut Canvas, ui: &Ui, doing: Doing, area: Rect, size: f32) {
    let (which, color) = match doing {
        Doing::Failed => (Icon::StateFailed, theme::DANGER),
        Doing::Working => (Icon::StateWorking, theme::WORKING),
        Doing::Blocked => (Icon::StateBlocked, theme::ATTENTION),
        Doing::Done => (Icon::StateDone, theme::DONE),
        Doing::Attention => (Icon::StateAttention, theme::ATTENTION),
        Doing::Idle => (Icon::StateIdle, theme::FAINT),
    };
    let at = icon(area, ui.px(size));
    let step = if which == Icon::StateWorking { ui.step } else { 0 };
    cv.icon_at(which, step, at.x, at.y, at.w, color);
}

/// Draws a small plus centered in `area`. The one of the icons is made to
/// stand alone and is too large beside a word.
pub fn plus(cv: &mut Canvas, ui: &Ui, area: Rect, color: Color) {
    let (at, line) = (icon(area, ui.px(9.0)), ui.line());
    let middle = ((at.w - line) / 2.0).round();
    cv.rect(Rect::new(at.x, at.y + middle, at.w, line), color);
    cv.rect(Rect::new(at.x + middle, at.y, line, at.h), color);
}

/// Draws an icon centered in `area` and makes the area stand for `action`.
pub fn icon_button(cv: &mut Canvas, ui: &Ui, area: Rect, which: Icon, action: Action, hits: &mut Hits) {
    symbol(cv, ui, which, area, ui.lit(action, theme::FAINT));
    hits.push((area, Hit::Do(action)));
}

/// Draws a cross centered in `area` and makes the area stand for `action`.
pub fn close_button(
    cv: &mut Canvas,
    ui: &Ui,
    area: Rect,
    size: f32,
    idle: Color,
    action: Action,
    hits: &mut Hits,
) {
    cv.cross(icon(area, ui.px(size)), ui.line(), ui.lit(action, idle));
    hits.push((area, Hit::Do(action)));
}

/// The width the buttons of the window take at the right edge.
pub fn buttons_width(ui: &Ui) -> f32 {
    if ui.buttons { 3.0 * ui.px(BUTTON) } else { 0.0 }
}

/// What stands at the top right of the window, whatever is below it: the
/// buttons of the window, and left of them the icons for what is shown
/// beside the sessions, each at a place of its own that depends on
/// nothing but the width of the window (`Ui::control`). From the left:
/// the sidebar, the changes of the project that is worked in, a second
/// pane, the column with the files. One that has nothing to do right
/// now stays where it is, and steps back.
pub fn controls(cv: &mut Canvas, ui: &Ui, ws: &Workspace, hits: &mut Hits) {
    let header = Rect::new(0.0, 0.0, ui.width, ui.px(super::HEADER));
    window_buttons(cv, ui, header, hits);
    for (n, (which, action)) in offered(ui, ws).into_iter().enumerate() {
        let place = ui.control(CONTROLS - 1 - n);
        match action {
            Some(action) => icon_button(cv, ui, place, which, action, hits),
            // Its place is its own, also while it does nothing: a second
            // click on what was there a moment ago is not a double click
            // on the header behind it, which would make the window large.
            None => {
                symbol(cv, ui, which, place, theme::LINE);
                hits.push((place, Hit::None));
            }
        }
    }
}

/// The icons at the top right from the left, each with what a click on
/// it does, or with nothing while it has nothing to do.
fn offered(ui: &Ui, ws: &Workspace) -> [(Icon, Option<Action>); CONTROLS] {
    let panes = ws.panes().len();
    let git = ws.active_project().and_then(|id| ws.project(id)).is_some_and(|project| project.git.is_some());
    // A second pane is put beside a session that stands alone.
    let alone = panes == 1 && ws.pane_session(0).is_some();
    [
        (
            if ui.sidebar_folded { Icon::SidebarLeftOff } else { Icon::SidebarLeft },
            Some(Action::ToggleSidebar),
        ),
        (Icon::FileDiff, git.then_some(Action::DiffActive)),
        (Icon::Columns, alone.then_some(Action::Split(0))),
        (
            if ui.tree_open { Icon::SidebarRight } else { Icon::SidebarRightOff },
            ui.tree_fits(panes).then_some(Action::ToggleTree),
        ),
    ]
}

/// Draws minimize, maximize and close at the right end of a header.
/// Returns where the buttons begin.
fn window_buttons(cv: &mut Canvas, ui: &Ui, header: Rect, hits: &mut Hits) -> f32 {
    let mut right = header.right();
    if !ui.buttons {
        return right;
    }
    for action in [Action::Quit, Action::Maximize, Action::Minimize] {
        let button = Rect::new(right - ui.px(BUTTON), header.y, ui.px(BUTTON), header.h);
        let hovered = ui.hot(action);
        if hovered {
            cv.rect(button, if action == Action::Quit { theme::DANGER } else { theme::HOVER });
        }
        let color = if hovered { theme::TEXT } else { theme::DIM };
        let mut at = icon(button, ui.px(ICON));
        let which = match action {
            Action::Quit => Icon::WindowClose,
            Action::Maximize if ui.maximized => Icon::WindowRestore,
            Action::Maximize => Icon::WindowMaximize,
            _ => {
                at.y += (at.h * MINIMIZE_DROP / ICON).round();
                Icon::WindowMinimize
            }
        };
        cv.icon(which, at.x, at.y, at.w, color);
        hits.push((button, Hit::Do(action)));
        right = button.x;
    }
    right
}

/// The outline of a floating window and the zones that resize it.
pub fn edges(cv: &mut Canvas, ui: &Ui, hits: &mut Hits) {
    if !ui.floating {
        return;
    }
    // Four strips. One outlined rectangle would run the shader over every
    // pixel of the window to draw a line of one.
    let (w, h) = (ui.width, ui.height);
    for strip in [
        Rect::new(0.0, 0.0, w, 1.0),
        Rect::new(0.0, h - 1.0, w, 1.0),
        Rect::new(0.0, 1.0, 1.0, h - 2.0),
        Rect::new(w - 1.0, 1.0, 1.0, h - 2.0),
    ] {
        cv.rect(strip, theme::LINE);
    }
    // Corners are listed last, so they win over the sides.
    let (e, w, h) = (ui.px(EDGE), ui.width, ui.height);
    use ResizeDirection::*;
    for (rect, direction) in [
        (Rect::new(0.0, 0.0, w, e), North),
        (Rect::new(0.0, h - e, w, e), South),
        (Rect::new(0.0, 0.0, e, h), West),
        (Rect::new(w - e, 0.0, e, h), East),
        (Rect::new(0.0, 0.0, 2.0 * e, 2.0 * e), NorthWest),
        (Rect::new(w - 2.0 * e, 0.0, 2.0 * e, 2.0 * e), NorthEast),
        (Rect::new(0.0, h - 2.0 * e, 2.0 * e, 2.0 * e), SouthWest),
        (Rect::new(w - 2.0 * e, h - 2.0 * e, 2.0 * e, 2.0 * e), SouthEast),
    ] {
        hits.push((rect, Hit::Resize(direction)));
    }
}

/// The pointer shape over a hit. `text` says whether a pane shows text
/// that can be selected.
pub fn cursor_icon(hit: Hit, text: bool) -> CursorIcon {
    use ResizeDirection::*;
    match hit {
        Hit::Resize(North | South) => CursorIcon::NsResize,
        Hit::Resize(East | West) => CursorIcon::EwResize,
        Hit::Resize(NorthEast | SouthWest) => CursorIcon::NeswResize,
        Hit::Resize(NorthWest | SouthEast) => CursorIcon::NwseResize,
        Hit::Pane(_) if text => CursorIcon::Text,
        Hit::None | Hit::Drag | Hit::Pane(_) | Hit::Field => CursorIcon::Default,
        Hit::Do(_) => CursorIcon::Pointer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_at_the_top_right_does_something_only_while_there_is_something_to_do() {
        let mut ui = Ui::new(Vec::new());
        (ui.width, ui.height, ui.cell_w, ui.cell_h) = (1200.0, 760.0, 8.0, 17.0);
        let mut ws = Workspace::restore(&Default::default());
        let does = |ui: &Ui, ws: &Workspace| offered(ui, ws).map(|(_, action)| action);
        // Nothing is shown: the sidebar folds, the column has room.
        let nothing = [Some(Action::ToggleSidebar), None, None, Some(Action::ToggleTree)];
        assert_eq!(does(&ui, &ws), nothing);
        // A session alone: a second pane can be put beside it. Its
        // project is no repository, so there are no changes to show.
        let project = ws.add_project("/p/a".into());
        let one = ws.new_session(project, "shell");
        ws.show(one, false).unwrap();
        assert_eq!(does(&ui, &ws)[2], Some(Action::Split(0)));
        assert_eq!(does(&ui, &ws)[1], None);
        ws.projects[0].git = Some(crate::core::git::Info { branch: "main".into(), changed: 0 });
        assert_eq!(does(&ui, &ws)[1], Some(Action::DiffActive));
        // Two panes: there is no third.
        let two = ws.new_session(project, "shell");
        ws.show(two, true).unwrap();
        assert_eq!(ws.panes().len(), 2);
        assert_eq!(does(&ui, &ws)[2], None);
        // A window too narrow for the column: its icon stays and does
        // nothing, as its key does.
        ui.width = 700.0;
        assert_eq!(does(&ui, &ws)[3], None);
        // Each shows the state it would change.
        let shown = |ui: &Ui| offered(ui, &ws).map(|(icon, _)| icon);
        assert_eq!((shown(&ui)[0], shown(&ui)[3]), (Icon::SidebarLeft, Icon::SidebarRightOff));
        (ui.sidebar_folded, ui.tree_open) = (true, true);
        assert_eq!((shown(&ui)[0], shown(&ui)[3]), (Icon::SidebarLeftOff, Icon::SidebarRight));
    }
}
