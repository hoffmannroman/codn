//! Pointer input: hover, clicks, text selection, mouse reports, the wheel.

use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;
use winit::window::CursorIcon;

use super::App;
use crate::action::Action;
use crate::core::SessionId;
use crate::platform::open;
use crate::term::{CellAt, Select, Terminal};
use crate::ui::{Hit, chrome, pane, picker, sidebar, tree};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// What the pointer is in the middle of.
pub(super) struct Pointer {
    /// What the left button went down on.
    pressed: Hit,
    /// Pane in which a text selection is being dragged, and the cell the
    /// selection reaches to.
    selecting: Option<usize>,
    selected: Option<CellAt>,
    /// Pane of a picture that is being moved by dragging, and where the
    /// pointer was last.
    panning: Option<(usize, (f32, f32))>,
    /// When and where the last press on a picture was, unless it is
    /// taken up by a double click or was the start of a drag.
    picture_click: Press,
    /// Pane and button of a press that went to a program tracking the
    /// mouse, and the last cell reported to it.
    tracking: Option<(usize, u8)>,
    tracked: Option<CellAt>,
    /// Where a press on the window's header began. The window is moved once
    /// the pointer has left that spot, so a plain click moves nothing.
    drag_from: Option<(f32, f32)>,
    /// Time, count, target and place of the latest run of clicks.
    clicks: (Instant, u8, Hit, (f32, f32)),
    /// When and where an element was last clicked, and which.
    done: Option<(Instant, (f32, f32), Hit)>,
    /// When an address was last opened by a click, and which.
    opened: Option<(Instant, String)>,
    /// The shape the pointer was given last.
    shape: CursorIcon,
    /// Scroll distance not yet turned into whole lines.
    wheel: f32,
    /// The same sideways, not yet turned into whole columns.
    sideways: f32,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            pressed: Hit::None,
            selecting: None,
            selected: None,
            panning: None,
            picture_click: None,
            tracking: None,
            tracked: None,
            drag_from: None,
            clicks: (Instant::now(), 0, Hit::None, (0.0, 0.0)),
            done: None,
            opened: None,
            shape: CursorIcon::Default,
            wheel: 0.0,
            sideways: 0.0,
        }
    }
}

/// Whether a click on `hit` comes right after one on another element, from
/// a pointer that has not moved. What a click closes or opens can put
/// another element under the pointer, and the second click of a double
/// click would act on that one: close the next session, split once more.
fn follows_another(
    done: Option<(Instant, (f32, f32), Hit)>,
    now: Instant,
    mouse: (f32, f32),
    hit: Hit,
    slack: f32,
) -> bool {
    done.is_some_and(|(at, (x, y), last)| {
        last != hit && now.duration_since(at) < DOUBLE_CLICK && (mouse.0 - x).hypot(mouse.1 - y) <= slack
    })
}

/// Which click of a run this one is: the first, or the next of up to
/// three on the same element, at the same place and soon after the one
/// before. Two quick clicks at two places of a text are two clicks.
fn click_count(
    last: (Instant, u8, Hit, (f32, f32)),
    now: Instant,
    hit: Hit,
    mouse: (f32, f32),
    slack: f32,
) -> u8 {
    let (at, count, target, (x, y)) = last;
    let near = (mouse.0 - x).hypot(mouse.1 - y) <= slack;
    let repeat = hit == target && near && now.duration_since(at) < DOUBLE_CLICK;
    if repeat { count % 3 + 1 } else { 1 }
}

/// Whether the keys are held with which a click opens an address:
/// Control, with or without Shift. A plain click selects text, and with
/// Alt or the key of the desktop it is the window manager's.
fn opens_links(mods: ModifiersState) -> bool {
    mods.control_key() && !mods.alt_key() && !mods.super_key()
}

/// Whether a click on an address comes right after one that opened the
/// same address: the next of a run of clicks, which opens nothing again.
/// A click on another address soon after is a click of its own.
fn opened_just_now(last: &Option<(Instant, String)>, now: Instant, address: &str) -> bool {
    last.as_ref().is_some_and(|(at, which)| which == address && now.duration_since(*at) < DOUBLE_CLICK)
}

/// When and where a press on a picture was, if a double click may follow.
type Press = Option<(Instant, (f32, f32))>;

/// Whether a press on a picture is the second of a double click, and
/// what is left of the run after it. The count of a text goes round
/// 1, 2, 3 and so would take every second double click on a picture
/// for a single one: a picture has two clicks, and a double click
/// ends the run.
fn picture_press(last: Press, now: Instant, mouse: (f32, f32), slack: f32) -> (bool, Press) {
    match last {
        Some((at, (x, y)))
            if (mouse.0 - x).hypot(mouse.1 - y) <= slack && now.duration_since(at) < DOUBLE_CLICK =>
        {
            (true, None)
        }
        _ => (false, Some((now, mouse))),
    }
}

/// Whether an action is one that the second click of a double click
/// must not do again: what opens something with one click and closes it
/// with the next would do both.
fn once_for_a_double_click(action: Action) -> bool {
    matches!(
        action,
        Action::AddProject
            | Action::ToggleTree
            | Action::ToggleSidebar
            | Action::DiffActive
            | Action::TreeRow(_)
    )
}

impl App {
    /// The cell under the pointer in a pane that shows a terminal.
    fn cell(&self, pane: usize) -> Option<CellAt> {
        let panes = self.ws.panes().len();
        let layout = (pane < panes).then(|| self.ui.pane(pane, panes))?;
        Some(self.ui.cell_at(&layout, self.ui.mouse.0, self.ui.mouse.1))
    }

    /// The terminal a pane shows, and the cell of it under the pointer.
    fn term_at(&self, pane: usize) -> Option<(&Terminal, CellAt)> {
        Some((self.pane_term(pane)?, self.cell(pane)?))
    }

    /// Whether the terminal the pointer is over drew an address under it
    /// in the last frame, as one that a click opens.
    fn linked(&self) -> bool {
        let session = self.ui.link.and_then(|(id, _)| self.ws.session(id));
        session.and_then(|session| session.term.as_ref()).is_some_and(Terminal::linked)
    }

    /// The session a pane shows and the cell of its terminal that the
    /// pointer is on, for an address there: none beside the grid, and
    /// none under the note of a program that has ended.
    fn link_cell(&self, index: usize) -> Option<(SessionId, CellAt)> {
        let panes = self.ws.panes().len();
        let layout = (index < panes).then(|| self.ui.pane(index, panes))?;
        let session = self.ws.pane_session(index).and_then(|id| self.ws.session(id))?;
        session.term.as_ref()?;
        let (x, y) = self.ui.mouse;
        pane::shown_cell(&self.ui, &layout, session.ended.is_some(), x, y).map(|at| (session.id, at))
    }

    /// Updates what the pointer is over, and its shape.
    pub(super) fn hover(&mut self) {
        let hit = self.ui.hit(self.ui.mouse.0, self.ui.mouse.1);
        if hit != self.ui.hover {
            self.ui.hover = hit;
            self.dirty = true;
        }
        // The cell of a terminal whose address the next frame looks for,
        // while the keys are held that open it. Whether there is one
        // is known once that frame is drawn, which ends by coming here.
        let link = match hit {
            Hit::Pane(pane) if opens_links(self.mods) && self.pointer.selecting.is_none() => {
                self.link_cell(pane)
            }
            _ => None,
        };
        if link != self.ui.link {
            // Nothing is drawn for a pointer that leaves a text in which
            // nothing was underlined.
            self.dirty |= link.is_some() || self.linked();
            self.ui.link = link;
        }
        let text = matches!(hit, Hit::Pane(pane) if self.shows_terminal(pane) || self.shows_file_text(pane));
        let shape = match self.linked() {
            true => CursorIcon::Pointer,
            false => chrome::cursor_icon(hit, text),
        };
        if shape != self.pointer.shape {
            self.pointer.shape = shape;
            if let Some(window) = self.window() {
                window.set_cursor(shape);
            }
        }
    }

    /// Opens the address under the pointer, if the keys for that are
    /// held and there is one. Says whether there is: the click is then
    /// codn's, and selects nothing.
    fn open_link(&mut self, pane: usize) -> bool {
        if !opens_links(self.mods) {
            return false;
        }
        let at = self.link_cell(pane).map(|(_, at)| at);
        let Some(address) = at.zip(self.pane_term(pane)).and_then(|(at, term)| term.link(at)) else {
            return false;
        };
        // The second click of a double click opens nothing again, nor
        // does a third: each click of a run is the last one for the next.
        let now = Instant::now();
        let again = opened_just_now(&self.pointer.opened, now, &address);
        if !again && !open::address(&address) {
            self.ui.notice = Some("The address was not opened: xdg-open could not be started.".into());
            self.dirty = true;
        }
        self.pointer.opened = Some((now, address));
        true
    }

    /// Sends a button report to the program in a pane if it tracks the
    /// mouse. Returns whether it does.
    fn report(&mut self, pane: usize, button: u8, pressed: bool) -> bool {
        let Some((term, at)) = self.term_at(pane) else { return false };
        if !term.tracks_mouse(self.mods) {
            return false;
        }
        term.mouse(button, pressed, at, self.mods);
        self.pointer.tracked = Some(at);
        true
    }

    pub(super) fn mouse_moved(&mut self) {
        if let Some((x, y)) = self.pointer.drag_from {
            let (dx, dy) = (self.ui.mouse.0 - x, self.ui.mouse.1 - y);
            if dx.hypot(dy) > self.ui.px(4.0) {
                self.pointer.drag_from = None;
                if let Some(window) = self.window() {
                    let _ = window.drag_window();
                }
            }
            return;
        }
        if let Some((pane, last)) = self.pointer.panning {
            let now = self.ui.mouse;
            self.pointer.panning = Some((pane, now));
            // A press that turns into a drag is no first click.
            let slack = self.ui.px(4.0);
            if let Some((_, (x, y))) = self.pointer.picture_click
                && (now.0 - x).hypot(now.1 - y) > slack
            {
                self.pointer.picture_click = None;
            }
            self.picture_pan(pane, (now.0 - last.0, now.1 - last.1));
            return;
        }
        if let Some(pane) = self.pointer.selecting {
            if self.shows_file_text(pane) {
                return self.file_drag(pane);
            }
            // The pointer reports many positions within one cell.
            if let Some((term, at)) = self.term_at(pane)
                && self.pointer.selected != Some(at)
            {
                term.select_to(at);
                self.pointer.selected = Some(at);
                self.dirty = true;
            }
            return;
        }
        self.hover();
        // Motion reports for programs that track the mouse: to the pane a
        // button was pressed in, else to the one under the pointer.
        let held = self.pointer.tracking;
        let pane = held.map(|(pane, _)| pane).or(match self.ui.hover {
            Hit::Pane(pane) => Some(pane),
            _ => None,
        });
        if let Some(pane) = pane
            && let Some((term, at)) = self.term_at(pane)
            && term.wants_motion(held.map(|(_, button)| button))
            && !self.mods.shift_key()
            && self.pointer.tracked != Some(at)
        {
            term.mouse(32 + held.map_or(3, |(_, button)| button), true, at, self.mods);
            self.pointer.tracked = Some(at);
        }
    }

    pub(super) fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        if pressed {
            self.input_epoch = self.input_epoch.wrapping_add(1);
            // Whoever clicks anything while the line at the bottom of a
            // file asks about its changes has answered "stay": a click
            // into the session beside it would otherwise leave the keys
            // of that session with the question.
            if self.asked.is_some() {
                self.stay();
            }
        }
        let hit = self.ui.hit(self.ui.mouse.0, self.ui.mouse.1);
        let code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };

        // A press that went to a program is followed by its release, also
        // when the pointer has left the pane in between. The cell is clamped.
        if let Some((pane, held)) = self.pointer.tracking
            && !pressed
            && held == code
        {
            self.pointer.tracking = None;
            if let Some((term, at)) = self.term_at(pane) {
                term.mouse(code, false, at, self.mods);
            }
            return;
        }
        if let Hit::Pane(pane) = hit
            && pressed
        {
            self.ws.focus_pane(pane);
            self.ui.picker = None;
            self.ui.renaming = None;
            self.dirty = true;
            // Before the program is asked: an address is opened by codn
            // also where the program tracks the mouse. A click beside
            // one is the program's.
            if button == MouseButton::Left && self.open_link(pane) {
                return;
            }
            if self.report(pane, code, true) {
                self.pointer.tracking = Some((pane, code));
                return;
            }
        }

        match (button, pressed) {
            (MouseButton::Left, true) => self.press(hit),
            (MouseButton::Left, false) => self.let_go(hit),
            (MouseButton::Middle, true) if matches!(hit, Hit::Pane(_)) => self.paste(true),
            (MouseButton::Right, true) if hit == Hit::Drag => {
                if let Some(window) = self.window() {
                    window.show_window_menu(PhysicalPosition::new(
                        self.ui.mouse.0 as f64,
                        self.ui.mouse.1 as f64,
                    ));
                }
            }
            _ => {}
        }
    }

    /// Left button down: things that start with the press itself.
    fn press(&mut self, hit: Hit) {
        let now = Instant::now();
        let count = click_count(self.pointer.clicks, now, hit, self.ui.mouse, self.ui.px(4.0));
        self.pointer.clicks = (now, count, hit, self.ui.mouse);
        self.pointer.pressed = hit;
        // A sidebar that lies over the panes stays there until the button
        // is let go: what closes below gives it back to the strip, and
        // the release would find another thing under the pointer than
        // the press did. The click did nothing.
        self.ui.sidebar_held = self.ui.sidebar_folded && !self.ui.rail();

        // A click outside an open text field closes it.
        if !picker::keeps_open(hit) && self.ui.picker.take().is_some() {
            self.dirty = true;
        }
        if hit != Hit::Field && self.ui.renaming.take().is_some() {
            self.dirty = true;
        }

        match hit {
            Hit::Resize(direction) => {
                if let Some(window) = self.window() {
                    let _ = window.drag_resize_window(direction);
                }
            }
            Hit::Drag => {
                if let Some(window) = self.window() {
                    if count == 2 {
                        window.set_maximized(!window.is_maximized());
                    } else {
                        self.pointer.drag_from = Some(self.ui.mouse);
                    }
                }
            }
            Hit::Pane(pane) => {
                let kind = match count {
                    2 => Select::Words,
                    3 => Select::Lines,
                    _ => Select::Cells,
                };
                if let Some((term, at)) = self.term_at(pane) {
                    term.select(kind, at);
                    self.pointer.selecting = Some(pane);
                    self.pointer.selected = None;
                    self.dirty = true;
                } else if self.shows_picture(pane) {
                    // Holding the button moves a picture, and a second
                    // click fits it or shows it at its own size. Nothing
                    // is selected, and nothing goes to the clipboard.
                    self.pointer.panning = Some((pane, self.ui.mouse));
                    let slack = self.ui.px(4.0);
                    let (double, run) = picture_press(self.pointer.picture_click, now, self.ui.mouse, slack);
                    self.pointer.picture_click = run;
                    if double {
                        self.picture_toggle(pane);
                    }
                } else if self.file_press(pane, count) {
                    self.pointer.selecting = Some(pane);
                }
            }
            Hit::None | Hit::Field | Hit::Do(_) => {}
        }
    }

    /// Left button up: the end of a selection, which goes to where the
    /// middle button pastes from, or a click on what the button went down on.
    fn let_go(&mut self, hit: Hit) {
        self.pointer.drag_from = None;
        self.pointer.panning = None;
        if let Some(pane) = self.pointer.selecting.take() {
            if let Some(term) = self.pane_term(pane) {
                term.select_end();
            }
            self.copy(true);
            self.dirty = true;
        } else if hit == self.pointer.pressed {
            self.activate(hit);
        }
        self.pointer.pressed = Hit::None;
        if std::mem::take(&mut self.ui.sidebar_held) {
            self.dirty = true;
        }
    }

    /// Left button released on the element it went down on.
    fn activate(&mut self, hit: Hit) {
        let Hit::Do(action) = hit else { return };
        let now = Instant::now();
        if follows_another(self.pointer.done, now, self.ui.mouse, hit, self.ui.px(4.0)) {
            return;
        }
        self.pointer.done = Some((now, self.ui.mouse, hit));
        // The first click of a double click has opened the session already,
        // the second renames it.
        if let Action::Open(id) = action
            && self.pointer.clicks.1 == 2
        {
            let label = self.ws.session(id).map(|s| self.ws.label(s)).unwrap_or_default();
            self.ui.renaming = Some((id, label));
            self.dirty = true;
            return;
        }
        if once_for_a_double_click(action) && self.pointer.clicks.1 > 1 {
            return;
        }
        self.run(action, self.mods.shift_key());
    }

    /// Moves the text of a file under the pointer sideways, if the wheel
    /// was turned for that: sideways, or the other way while Shift is
    /// held. Of a turn that goes both ways the greater part counts, so
    /// that a finger that drifts on its way down does not move the text.
    /// Returns whether the turn was taken.
    fn sideways(&mut self, across: f32, along: f32) -> bool {
        let (x, y) = self.ui.mouse;
        // Not a file under the sidebar, which lies over a pane while it
        // is folded and a list is open in it.
        if self.ui.sidebar().contains(x, y) {
            return false;
        }
        let layouts = self.ui.panes(self.ws.panes().len());
        let Some(pane) = layouts.iter().position(|l| l.area.contains(x, y)) else { return false };
        if !self.shows_file_text(pane) {
            return false;
        }
        let across = match self.mods.shift_key() {
            true => across + along,
            false if across.abs() > along.abs() => across,
            false => return false,
        };
        self.pointer.sideways += across;
        let cols = (self.pointer.sideways / self.ui.cell_w).trunc();
        self.pointer.sideways -= cols * self.ui.cell_w;
        // Positive is to the left, as it is up for lines.
        self.file_sideways(pane, -cols as isize);
        true
    }

    /// Zooms the picture under the pointer by the wheel while Control is
    /// held, and moves it by the wheel otherwise, sideways by Shift.
    /// Returns whether the wheel was over a picture.
    fn picture_wheel(&mut self, across: f32, along: f32) -> bool {
        let (x, y) = self.ui.mouse;
        if self.ui.sidebar().contains(x, y) {
            return false;
        }
        let layouts = self.ui.panes(self.ws.panes().len());
        let Some(pane) = layouts.iter().position(|l| l.area.contains(x, y)) else { return false };
        if !self.shows_picture(pane) {
            return false;
        }
        if self.mods.control_key() {
            // A notch of a wheel is three lines.
            self.picture_zoom(pane, along / (3.0 * self.ui.cell_h), true);
        } else if self.mods.shift_key() {
            self.picture_pan(pane, (across + along, 0.0));
        } else {
            self.picture_pan(pane, (across, along));
        }
        true
    }

    pub(super) fn wheel(&mut self, delta: MouseScrollDelta) {
        let at = Instant::now();
        let changed = self.changed_by(|app| app.scroll(delta));
        if let Some(timing) = &mut self.timing {
            timing.echo.wheel(changed.then_some(at));
        }
    }

    fn scroll(&mut self, delta: MouseScrollDelta) {
        // Both kinds of delta are summed as a distance, so the small steps
        // of smooth scrolling add up instead of being rounded away.
        let (across, along) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * 3.0 * self.ui.cell_h, y * 3.0 * self.ui.cell_h),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
        };
        if self.picture_wheel(across, along) {
            return;
        }
        if self.sideways(across, along) {
            return;
        }
        // Whole lines to scroll, positive is up.
        self.pointer.wheel += along;
        let lines = (self.pointer.wheel / self.ui.cell_h).trunc();
        self.pointer.wheel -= lines * self.ui.cell_h;
        let lines = lines as i32;
        if lines == 0 {
            return;
        }
        let (x, y) = self.ui.mouse;
        if self.ui.sidebar().contains(x, y) {
            self.dirty = true;
            // The open list scrolls instead of the projects it covers.
            let capacity = picker::capacity(&self.ui);
            match &mut self.ui.picker {
                Some(picker) => picker.wheel(lines, capacity),
                None => sidebar::wheel(&mut self.ui, lines),
            }
            return;
        }
        let layouts = self.ui.panes(self.ws.panes().len());
        if let Some(column) = self.ui.tree(layouts.len()).filter(|column| column.contains(x, y)) {
            self.dirty = true;
            return tree::wheel(&self.ui, &mut self.ws, column, lines);
        }
        let Some(pane) = layouts.iter().position(|l| l.area.contains(x, y)) else { return };
        if let Some(view) = self.ws.pane_diff(pane) {
            self.dirty = true;
            return view.scroll_by(-lines as isize);
        }
        if self.file_wheel(pane, lines) {
            return;
        }
        // Scrolling under a drag moves what the pointer's cell stands for.
        self.pointer.selected = None;
        if let Some((term, at)) = self.term_at(pane) {
            self.dirty |= term.wheel(lines, at, self.mods);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_opens_an_address_and_not_with_the_keys_of_the_desktop() {
        use ModifiersState as Mods;
        assert!(opens_links(Mods::CONTROL) && opens_links(Mods::CONTROL | Mods::SHIFT));
        assert!(!opens_links(Mods::empty()) && !opens_links(Mods::SHIFT));
        assert!(!opens_links(Mods::CONTROL | Mods::ALT) && !opens_links(Mods::CONTROL | Mods::SUPER));
    }

    #[test]
    fn a_run_of_clicks_opens_an_address_once_and_another_address_as_well() {
        // The guard was by time alone, and a click on a second address soon
        // after one on a first opened nothing.
        let (at, a, b) = (Instant::now(), "https://example.org/a", "https://example.org/b");
        let last = Some((at, a.to_owned()));
        let soon = at + Duration::from_millis(150);
        assert!(opened_just_now(&last, soon, a));
        assert!(!opened_just_now(&last, soon, b));
        assert!(!opened_just_now(&last, at + DOUBLE_CLICK, a));
        assert!(!opened_just_now(&None, soon, a));
    }

    #[test]
    fn two_quick_double_clicks_on_a_picture_are_two() {
        // The count of a text goes 1, 2, 3, 1 and so took the second double
        // click for one.
        let (start, here) = (Instant::now(), (50.0, 50.0));
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut run = None;
        let mut doubles = 0;
        for millis in [0, 80, 220, 300] {
            let (double, next) = picture_press(run, at(millis), here, 4.0);
            run = next;
            doubles += u8::from(double);
        }
        assert_eq!(doubles, 2);
        // Too late, or somewhere else, is a first click again.
        let (_, run) = picture_press(None, at(0), here, 4.0);
        assert!(!picture_press(run, at(5000), here, 4.0).0);
        assert!(!picture_press(run, at(100), (80.0, 50.0), 4.0).0);
    }

    #[test]
    fn a_second_click_does_not_act_on_what_the_first_put_under_the_pointer() {
        let (close, split) = (Hit::Do(Action::ClosePane(1)), Hit::Do(Action::Split(0)));
        let (at, here) = (Instant::now(), (100.0, 10.0));
        let done = Some((at, here, close));
        let soon = at + Duration::from_millis(150);
        // Closing the pane puts the button that splits where its own was.
        assert!(follows_another(done, soon, here, split, 4.0));
        assert!(follows_another(done, soon, (102.0, 11.0), split, 4.0));
        // The same element again, a pointer that moved, or a click later on.
        assert!(!follows_another(done, soon, here, close, 4.0));
        assert!(!follows_another(done, soon, (120.0, 10.0), split, 4.0));
        assert!(!follows_another(done, at + DOUBLE_CLICK, here, split, 4.0));
        assert!(!follows_another(None, soon, here, split, 4.0));
    }

    #[test]
    fn clicks_count_together_only_on_one_element_at_one_place_and_soon() {
        let (at, here, pane) = (Instant::now(), (300.0, 200.0), Hit::Pane(1));
        let soon = at + Duration::from_millis(150);
        let run = |count| (at, count, pane, here);
        assert_eq!(click_count(run(1), soon, pane, here, 4.0), 2);
        assert_eq!(click_count(run(2), soon, pane, (302.0, 201.0), 4.0), 3);
        assert_eq!(click_count(run(3), soon, pane, here, 4.0), 1, "after three it begins anew");
        // Another place in the same text, another element, or later.
        assert_eq!(click_count(run(1), soon, pane, (340.0, 200.0), 4.0), 1);
        assert_eq!(click_count(run(1), soon, Hit::Pane(0), here, 4.0), 1);
        assert_eq!(click_count(run(1), at + DOUBLE_CLICK, pane, here, 4.0), 1);
    }

    #[test]
    fn what_a_click_opens_and_the_next_closes_is_done_once_for_a_double_click() {
        // A folder in the column stays under the pointer when it opens.
        assert!(once_for_a_double_click(Action::TreeRow(7)));
        assert!(once_for_a_double_click(Action::ToggleTree) && once_for_a_double_click(Action::AddProject));
        assert!(once_for_a_double_click(Action::ToggleSidebar));
        // The icon of the changes stands still since it has a place at
        // the top right: the second click would take back the first.
        assert!(once_for_a_double_click(Action::DiffActive));
        // The second click on a session renames it, and is handled before.
        assert!(!once_for_a_double_click(Action::ClosePane(1)));
        assert!(!once_for_a_double_click(Action::Quit));
    }
}
