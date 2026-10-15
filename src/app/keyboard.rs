//! Keyboard input: shortcuts, the text fields, and keys for the panes.
//! What is pasted is in `paste`.

use std::time::Instant;

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use super::App;
use super::paste::Held;
use crate::action::{Action, shortcut, shortcut_key};
use crate::core::edit::Edit;
use crate::core::{Pane, SessionId, Workspace};
use crate::term::Terminal;
use crate::ui::{diff, field, picker};

impl App {
    pub(super) fn key(&mut self, event: KeyEvent) {
        self.input_epoch = self.input_epoch.wrapping_add(1);
        // A question about changes that are not saved takes every key.
        if self.asked.is_some() {
            return self.answer(&event);
        }
        if self.ui.editing() && self.is_paste(&event) {
            return self.paste(false);
        }
        if self.ui.renaming.is_some() {
            return self.rename_key(event);
        }
        if self.ui.picker.is_some() {
            return self.picker_key(event);
        }
        let base_key = event.key_without_modifiers();
        let named = match &event.logical_key {
            Key::Named(key) => Some(*key),
            _ => None,
        };
        let split = self.ws.panes().len() > 1;
        // The key of the switch is the app's only while a file is shown
        // that has the switch: else it is the program's.
        let switches = self.ws.file().is_some_and(|(_, view)| view.switches());
        if let Some(action) = shortcut(&shortcut_key(&base_key, event.physical_key), named, self.mods, split)
            .filter(|action| *action != Action::TogglePreview || switches)
        {
            // A key that is held down must not start a session for every
            // repeat, each with a program of its own.
            if event.repeat && action == Action::NewDefault {
                return;
            }
            return self.run(action, false);
        }

        let (focus, panes) = (self.ws.focus(), self.ws.panes().len());
        match &self.ws.panes()[focus] {
            Pane::Diff(view) => {
                let Some(named) = named else { return };
                if named == NamedKey::Escape {
                    let project = view.project;
                    return self.run(Action::ToggleDiff(project), false);
                }
                let page = diff::rows(&self.ui, &self.ui.pane(focus, panes));
                if let Some(view) = self.ws.pane_diff(focus) {
                    self.dirty |= diff::key(view, named, page);
                }
            }
            // What is typed behind a paste waits for it, as for a session,
            // and names the text it was typed for. A paste is asked for
            // at once, also behind another one: asked for when its turn
            // came, it would stand behind what was typed after it.
            Pane::File(view) => {
                let reading = view.reading();
                let (ctrl, plain) = (self.mods.control_key(), !self.mods.shift_key() && !self.mods.alt_key());
                if ctrl && plain && shortcut_key(&base_key, event.physical_key) == "v" {
                    // Nothing is pasted into a file that is shown as it
                    // is meant, and so the clipboard is not asked.
                    if !view.previews() {
                        self.paste(false);
                    }
                } else if self.holding() {
                    self.waiting.push(Held::FileKey(reading, event, self.mods));
                } else {
                    self.file_key(focus, &event, self.mods);
                }
            }
            Pane::Session(id) => {
                let id = *id;
                if self.holding() {
                    self.waiting.push(Held::Key(id, event, self.mods));
                } else {
                    self.session_key(id, &event, self.mods);
                }
            }
            Pane::Empty => {}
        }
    }

    /// A key for the program of a session, with the modifiers it had.
    pub(super) fn session_key(&mut self, id: SessionId, event: &KeyEvent, mods: ModifiersState) {
        let Some(session) = self.ws.session_mut(id) else { return };
        if session.ended.is_some() {
            let page = matches!(event.logical_key, Key::Named(NamedKey::PageUp | NamedKey::PageDown));
            if event.logical_key == Key::Named(NamedKey::Enter) {
                self.restart(id);
            } else if page && mods == ModifiersState::SHIFT {
                // What the program printed last stays readable further up.
                let scrolled = session
                    .term
                    .as_ref()
                    .and_then(|term| term.key(&event.logical_key, &event.logical_key, None, mods));
                self.dirty |= scrolled.unwrap_or(false);
            }
            return;
        }
        let Some(term) = &session.term else { return };
        let base_key = event.key_without_modifiers();
        // A frame drawn for the press alone would come before the
        // echo and make the echo wait for the frame after it.
        if let Some(changed) = term.key(&event.logical_key, &base_key, event.text_with_all_modifiers(), mods)
        {
            let now = Instant::now();
            session.input(now);
            self.dirty |= changed;
            self.awaited = Some((id, now));
            if let Some(timing) = &mut self.timing {
                timing.echo.key(id, now);
            }
        }
    }

    /// Typed, composed or pasted text, for whatever has the keyboard.
    pub(super) fn type_text(&mut self, text: String) {
        self.input_epoch = self.input_epoch.wrapping_add(1);
        if let Some((_, name)) = &mut self.ui.renaming {
            field::insert(name, &text);
            self.dirty = true;
        } else if self.ui.picker.is_some() {
            self.picker_outcome(|picker, known| {
                picker.type_text(&text, known);
                picker::Outcome::Stay
            });
        } else if let Some(id) = self.ws.pane_session(self.ws.focus()) {
            if self.holding() {
                self.waiting.push(Held::Text(id, text));
            } else {
                self.send_to(id, |term| term.text(text));
            }
        } else if let Some((_, view)) = self.ws.file().filter(|(pane, _)| self.shows_file_text(*pane)) {
            let reading = view.reading();
            if self.holding() {
                self.waiting.push(Held::FileText(reading, text));
            } else {
                self.file_edit(Edit::Type(&text));
            }
        }
    }

    /// Passes input to the terminal of a session. Like a key, it counts as
    /// input, and draws a frame only if it changed the screen.
    pub(super) fn send_to(&mut self, id: SessionId, send: impl FnOnce(&Terminal) -> bool) {
        let Some(session) = self.ws.session_mut(id) else { return };
        if let Some(term) = &session.term {
            self.dirty |= send(term);
            let now = Instant::now();
            session.input(now);
            self.awaited = Some((id, now));
        }
    }

    fn is_paste(&self, event: &KeyEvent) -> bool {
        self.mods.control_key()
            && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("v"))
    }

    /// Keys while the "add project" field is open.
    fn picker_key(&mut self, event: KeyEvent) {
        let ctrl = self.mods.control_key();
        let capacity = picker::capacity(&self.ui);
        self.picker_outcome(|picker, known| {
            picker.key(&event.logical_key, event.text.as_deref(), ctrl, capacity, known)
        });
    }

    /// Keys while a session is being renamed.
    fn rename_key(&mut self, event: KeyEvent) {
        let ctrl = self.mods.control_key();
        let Some((id, name)) = &mut self.ui.renaming else { return };
        let id = *id;
        self.dirty = true;
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.ui.renaming = None,
            Key::Named(NamedKey::Backspace) => {
                name.pop();
            }
            Key::Named(NamedKey::Enter) => {
                let name = renamed(&self.ws, id, name);
                self.ui.renaming = None;
                let Some(name) = name else { return };
                if let Some(session) = self.ws.session_mut(id) {
                    session.name = name;
                }
                self.save();
            }
            _ if !ctrl => {
                if let Some(text) = event.text.as_ref().map(|t| t.to_string()) {
                    self.type_text(text);
                }
            }
            _ => {}
        }
    }
}

/// What the name of a session becomes when `typed` is confirmed for it:
/// nothing if the field was empty, which gives the automatic name back.
/// The field starts out with the automatic name, and confirming that
/// unchanged must not freeze it: then there is nothing to do.
fn renamed(ws: &Workspace, id: SessionId, typed: &str) -> Option<Option<String>> {
    let (name, session) = (typed.trim(), ws.session(id)?);
    if name.is_empty() {
        Some(None)
    } else if session.name.is_none() && name == ws.label(session) {
        None
    } else {
        Some(Some(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirming_the_automatic_name_does_not_make_it_the_name() {
        let mut ws = Workspace::new();
        let project = ws.add_project("/throwaway".into());
        let id = ws.new_session(project, "shell");
        assert_eq!(renamed(&ws, id, " shell "), None);
        assert_eq!(renamed(&ws, id, "mine"), Some(Some("mine".into())));
        assert_eq!(renamed(&ws, id, "  "), Some(None));
        // A name that was given stays one, also when it reads like the agent.
        ws.session_mut(id).unwrap().name = Some("shell".into());
        assert_eq!(renamed(&ws, id, "shell"), Some(Some("shell".into())));
        assert_eq!(renamed(&ws, SessionId(99), "x"), None);
    }
}
