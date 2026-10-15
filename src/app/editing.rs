//! Keys and the pointer in the pane that shows a file: what a key asks
//! of its text, where the caret goes, and what the wheel moves.

use std::time::Instant;

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use super::App;
use crate::action::{Action, shortcut_key};
use crate::core::Said;
use crate::core::edit::Edit;
use crate::core::files::Request;
use crate::core::picture::{Look, ZOOM_STEP};
use crate::core::text::Move;
use crate::ui::file;

impl App {
    /// A key for the pane that shows a file, with the modifiers it had.
    pub(super) fn file_key(&mut self, pane: usize, event: &KeyEvent, mods: ModifiersState) {
        let (ctrl, shift, alt) = (mods.control_key(), mods.shift_key(), mods.alt_key());
        // The letter of a key as the shortcuts of the app take it: by the
        // place of the key where the layout has no Latin letter on it.
        let letter = shortcut_key(&event.key_without_modifiers(), event.physical_key);
        let named = match &event.logical_key {
            Key::Named(named) => Some(*named),
            _ => None,
        };
        // A picture has no text: its keys zoom and move it.
        if !ctrl && !alt && self.shows_picture(pane) && self.picture_key(pane, named, event.text.as_deref()) {
            return;
        }
        let said = self.ws.file().and_then(|(_, view)| view.said().cloned());
        let edit = match (named, letter.as_str()) {
            // While the line at the bottom asks something, Escape answers
            // it: "stay". Else it closes the pane.
            (Some(NamedKey::Escape), _) if said.as_ref().is_some_and(Said::asks) => return self.stay(),
            (Some(NamedKey::Escape), _) => return self.run(Action::ClosePane(pane), false),
            (None, "s") if ctrl && !shift && !alt => {
                self.file_save(false);
                return;
            }
            // What the line at the bottom names, and only while it does.
            (None, "s") if ctrl && shift && !alt => {
                if matches!(said, Some(Said::Other | Said::Gone | Said::Damaged(_))) {
                    self.file_save(true);
                }
                return;
            }
            (None, "r") if ctrl && !shift && !alt => {
                if matches!(said, Some(Said::Other | Said::NoWayBack)) {
                    self.file_take(pane);
                }
                return;
            }
            // Nothing is typed into a file that is shown as it is meant:
            // the keys that move a caret move the view, and no other
            // key does anything. Changing it is done in its text.
            _ if self.shows_preview(pane) => {
                let height = self.preview_height(pane);
                if let Some(to) = named.and_then(|named| file::key(named, true, 0))
                    && let Some(view) = self.ws.pane_file(pane)
                {
                    self.dirty |= view.preview.go(to, height);
                }
                return;
            }
            // No program runs here that these keys would be taken from.
            (None, "c") if ctrl && !shift && !alt => return self.copy(false),
            (None, "v") if ctrl && !shift && !alt => return self.paste(false),
            (None, "a") if ctrl && !shift && !alt => {
                if let Some((doc, view)) = self.ws.pane_file(pane).and_then(|view| view.parts()) {
                    view.select_all(doc);
                    self.dirty = true;
                }
                return;
            }
            (None, "x") if ctrl && !shift && !alt => {
                self.copy(false);
                Edit::Cut
            }
            (None, "z") if ctrl && !alt => {
                if shift {
                    Edit::Redo
                } else {
                    Edit::Undo
                }
            }
            (Some(NamedKey::Enter), _) => Edit::Enter,
            (Some(NamedKey::Tab), _) if !shift => Edit::Type("\t"),
            (Some(NamedKey::Backspace), _) => Edit::Back { word: ctrl },
            (Some(NamedKey::Delete), _) => Edit::Forward { word: ctrl },
            _ => {
                let rows = self.file_room(pane).0;
                if let Some(to) = named.and_then(|named| file::key(named, ctrl, rows)) {
                    return self.file_move(pane, to, shift);
                }
                // Text, unless the key was held with Control or Alt
                // alone: with both it is the third level of a layout.
                match event.text.as_deref() {
                    Some(text) if ctrl == alt => Edit::Type(text),
                    _ => return,
                }
            }
        };
        self.file_edit(edit);
    }

    /// Takes what the disk holds over the text that was changed here, as
    /// the line at the bottom offered.
    fn file_take(&mut self, pane: usize) {
        if self.quit {
            return;
        }
        let rows = self.file_room(pane).0;
        let Some(view) = self.ws.file_mut() else { return };
        let of = view.edition();
        if view.take_theirs(rows) {
            if let Some(path) = self.ws.file_path() {
                self.files.send(Request::File { path, known: None, of });
            }
        } else {
            self.ask_marks();
        }
        self.dirty = true;
    }

    /// Moves the caret of a file, or with `select` the end of the selection.
    fn file_move(&mut self, pane: usize, to: Move, select: bool) {
        let (rows, cols) = self.file_room(pane);
        let Some(view) = self.ws.pane_file(pane) else { return };
        if let Some((doc, view)) = view.parts() {
            view.go(doc, to, select);
            view.reveal(doc, rows, cols);
            self.dirty = true;
        }
    }

    /// How many lines and columns of text the pane of a file shows.
    pub(super) fn file_room(&self, pane: usize) -> (usize, usize) {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let Some((_, view)) = self.ws.file() else { return (1, 1) };
        let place = file::room(&self.ui, file::body(&layout), view);
        (place.rows, place.cols)
    }

    /// Does to the text of the file that is shown what a key asks, and
    /// keeps the caret in view.
    pub(super) fn file_edit(&mut self, edit: Edit) {
        // Nothing goes into a text once the window is closing: nobody
        // would be asked about it. Nor while the line asks about leaving:
        // only its keys are taken, also from what was composed or pasted.
        if self.quit || self.asked.is_some() {
            return;
        }
        // Nor into a file that is shown as it is meant: what was typed
        // would go into a text nobody sees.
        if self.ws.file().is_some_and(|(_, view)| view.previews()) {
            return;
        }
        let Some((pane, was)) = self.ws.file().map(|(pane, view)| (pane, view.said().cloned())) else {
            return;
        };
        let Some(view) = self.ws.file_mut() else { return };
        let changed = view.edit(edit, Instant::now());
        let (said, saved) = (view.said() != was.as_ref(), !view.changed());
        if !changed && !said {
            return;
        }
        // The line at the bottom may have come or gone, and with it a row.
        let (rows, cols) = self.file_room(pane);
        if let Some(view) = self.ws.file_mut() {
            view.reveal(rows, cols);
        }
        self.dirty = true;
        // Back at the text the file holds: its marks are good again.
        if changed && saved {
            self.ask_marks();
        }
    }

    /// The left button went down in a pane that shows a file: the caret
    /// goes there, a second click takes the word, a third the line.
    /// Returns whether the pane shows a text.
    pub(super) fn file_press(&mut self, pane: usize, clicks: u8) -> bool {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let ((x, y), shift) = (self.ui.mouse, self.mods.shift_key());
        let Some(view) = self.ws.pane_file(pane) else { return false };
        let Some((pos, _)) = file::at(&self.ui, &layout, view, x, y, clicks > 1) else { return false };
        let Some((doc, view)) = view.parts() else { return false };
        match clicks {
            2 => view.select_word(doc, pos),
            3 => view.select_line(doc, pos),
            _ => view.place(doc, pos, shift),
        }
        self.dirty = true;
        true
    }

    /// The pointer moved with the button held: the selection goes with
    /// it, and the view too once the pointer has left the rows.
    pub(super) fn file_drag(&mut self, pane: usize) {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let (x, y) = self.ui.mouse;
        let Some(view) = self.ws.pane_file(pane) else { return };
        let Some((pos, outside)) = file::at(&self.ui, &layout, view, x, y, false) else { return };
        let rows = file::room(&self.ui, file::body(&layout), view).rows;
        let Some((doc, view)) = view.parts() else { return };
        if outside != 0 {
            view.scroll_by(doc, outside.signum(), rows);
        }
        if view.caret != pos || outside != 0 {
            view.drag(doc, pos);
            self.dirty = true;
        }
    }

    /// The wheel over a pane that shows a file. Returns whether it does.
    pub(super) fn file_wheel(&mut self, pane: usize, lines: i32) -> bool {
        let rows = self.file_room(pane).0;
        let height = self.preview_height(pane);
        let Some(view) = self.ws.pane_file(pane) else { return false };
        if view.previews() {
            self.dirty |= view.preview.scroll_by(-lines as isize, height);
            return true;
        }
        if let Some((doc, view)) = view.parts() {
            view.scroll_by(doc, -lines as isize, rows);
            self.dirty = true;
        }
        true
    }

    /// The wheel over a pane that shows a file, turned sideways: by `cols`
    /// columns, to the right if positive.
    pub(super) fn file_sideways(&mut self, pane: usize, cols: isize) {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let Some(view) = self.ws.pane_file(pane) else { return };
        let place = file::room(&self.ui, file::body(&layout), view);
        let Some((doc, view)) = view.parts() else { return };
        self.dirty |= view.sideways(doc, cols, place.rows, place.cols);
    }

    /// Whether a pane shows a picture that is there to be zoomed and
    /// moved.
    pub(super) fn shows_picture(&self, pane: usize) -> bool {
        self.ws
            .file()
            .is_some_and(|(at, view)| at == pane && view.picture().is_some_and(|seen| seen.shown.is_some()))
    }

    /// Changes how the picture of a pane lies. `change` is given its
    /// look, the size of the picture, the room it lies in, and where on
    /// the window the middle of that room is.
    fn picture_look(
        &mut self,
        pane: usize,
        change: impl FnOnce(&mut Look, (u32, u32), (f32, f32), (f32, f32)),
    ) {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let Some((_, view)) = self.ws.file() else { return };
        let room = file::picture_room(&self.ui, file::body(&layout), view);
        let Some((decoded, _)) = view.picture().and_then(|seen| seen.shown) else { return };
        let middle = (room.x + room.w / 2.0, room.y + room.h / 2.0);
        let Some(seen) = self.ws.file_mut().and_then(|view| view.picture_mut()) else { return };
        let before = seen.look;
        seen.look.settle(decoded.picture, (room.w, room.h));
        change(&mut seen.look, decoded.picture, (room.w, room.h), middle);
        self.dirty |= seen.look != before;
    }

    /// Moves the picture of a pane by pixels of the screen.
    pub(super) fn picture_pan(&mut self, pane: usize, by: (f32, f32)) {
        self.picture_look(pane, |look, picture, room, _| look.pan(by, picture, room));
    }

    /// Zooms the picture of a pane by that many steps, in if positive,
    /// about the place of the pointer, or the middle where the pointer
    /// is not over the pane.
    pub(super) fn picture_zoom(&mut self, pane: usize, steps: f32, at_pointer: bool) {
        let mouse = self.ui.mouse;
        self.picture_look(pane, |look, picture, room, middle| {
            let anchor = if at_pointer { (mouse.0 - middle.0, mouse.1 - middle.1) } else { (0.0, 0.0) };
            look.zoom_by(ZOOM_STEP.powf(steps), picture, room, anchor);
        });
    }

    /// A double click: fitted gives way to its own size about the place
    /// that was clicked, and anything else goes back to fitted.
    pub(super) fn picture_toggle(&mut self, pane: usize) {
        let mouse = self.ui.mouse;
        self.picture_look(pane, |look, picture, room, middle| {
            look.toggle(picture, room, (mouse.0 - middle.0, mouse.1 - middle.1));
        });
    }

    /// A key for a picture: plus and minus zoom, 0 fits it again, the
    /// arrows move it by an eighth of its room. Returns whether it was
    /// one of them.
    fn picture_key(&mut self, pane: usize, named: Option<NamedKey>, text: Option<&str>) -> bool {
        let room = self.picture_pixels(pane);
        let step = |along: f32| (room.0 * along / 8.0, room.1 * along / 8.0);
        match (named, text) {
            (Some(NamedKey::ArrowLeft), _) => self.picture_pan(pane, (step(1.0).0, 0.0)),
            (Some(NamedKey::ArrowRight), _) => self.picture_pan(pane, (-step(1.0).0, 0.0)),
            (Some(NamedKey::ArrowUp), _) => self.picture_pan(pane, (0.0, step(1.0).1)),
            (Some(NamedKey::ArrowDown), _) => self.picture_pan(pane, (0.0, -step(1.0).1)),
            (_, Some("+" | "=")) => self.picture_zoom(pane, 1.0, false),
            (_, Some("-" | "_")) => self.picture_zoom(pane, -1.0, false),
            (_, Some("0")) => self.picture_look(pane, |look, _, _, _| *look = Look::default()),
            _ => return false,
        }
        true
    }

    /// The room of the picture of a pane in pixels of the screen.
    fn picture_pixels(&self, pane: usize) -> (f32, f32) {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let Some((_, view)) = self.ws.file() else { return (1.0, 1.0) };
        let room = file::picture_room(&self.ui, file::body(&layout), view);
        (room.w, room.h)
    }

    /// Whether a pane shows text of a file that can be selected: not a
    /// file that is shown as it is meant, in which nothing is selected
    /// and nothing typed.
    pub(super) fn shows_file_text(&self, pane: usize) -> bool {
        self.ws.file().is_some_and(|(at, view)| at == pane && view.text().is_some() && !view.previews())
    }

    /// Whether a pane shows a file as it is meant.
    pub(super) fn shows_preview(&self, pane: usize) -> bool {
        self.ws.file().is_some_and(|(at, view)| at == pane && view.previews())
    }

    /// The text selected in the file that has the keyboard. None in a
    /// file that is shown as it is meant: what is selected in its text
    /// is not seen there.
    pub(super) fn file_selection(&self) -> Option<String> {
        let (pane, view) = self.ws.file().filter(|(_, view)| !view.previews())?;
        (pane == self.ws.focus()).then(|| view.view().selected(view.text()?)).flatten()
    }
}
