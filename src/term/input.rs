//! What the user does to a terminal with the keyboard and the pointer.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::{Term, TermMode};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::{CellAt, Listener, Terminal, keys};

impl Terminal {
    /// What input does to the view: back to the bottom, and the selection
    /// goes. Returns whether that changed what is on screen.
    fn typed(term: &mut Term<Listener>) -> bool {
        let changed = term.grid().display_offset() != 0 || term.selection.is_some();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        changed
    }

    fn mode(&self) -> TermMode {
        let mode = *self.term.lock().mode();
        self.mode.set(mode);
        mode
    }

    // ----- keyboard -----

    /// Sends a key press to the program. `base` is the key without
    /// modifiers, `text` what the press types with all modifiers applied.
    /// Returns nothing if the key meant nothing, and otherwise whether the
    /// press itself changed what is on screen. What the program answers
    /// arrives as output.
    pub fn key(&self, key: &Key, base: &Key, text: Option<&str>, mods: ModifiersState) -> Option<bool> {
        let mut term = self.term.lock();
        let mode = *term.mode();
        self.mode.set(mode);
        // Shift+Page scrolls back, unless a full-screen program runs.
        if mods.shift_key() && !mods.control_key() && !mode.contains(TermMode::ALT_SCREEN) {
            let scroll = match key {
                Key::Named(NamedKey::PageUp) => Some(Scroll::PageUp),
                Key::Named(NamedKey::PageDown) => Some(Scroll::PageDown),
                _ => None,
            };
            if let Some(scroll) = scroll {
                term.scroll_display(scroll);
                return Some(true);
            }
        }
        let legacy = !mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
        let shift_enter = matches!(key, Key::Named(NamedKey::Enter)) && mods == ModifiersState::SHIFT;
        let bytes = if shift_enter && legacy && self.shift_enter {
            b"\x1b\r".to_vec()
        } else {
            keys::encode(key, base, text, mods, mode)?
        };
        let changed = Self::typed(&mut term);
        drop(term);
        self.write(bytes);
        Some(changed)
    }

    /// Sends text as typed, for example from an input method. Returns
    /// whether that changed what is on screen, like `key`.
    pub fn text(&self, text: String) -> bool {
        if text.is_empty() {
            return false;
        }
        let changed = Self::typed(&mut self.term.lock());
        self.write(text.into_bytes());
        changed
    }

    /// Pastes text. Returns whether that changed what is on screen.
    pub fn paste(&self, text: &str) -> bool {
        let (mode, changed) = {
            let mut term = self.term.lock();
            (*term.mode(), Self::typed(&mut term))
        };
        self.mode.set(mode);
        self.write(keys::paste(text, mode));
        changed
    }

    /// Tells the program that it gained or lost the focus, if it asked.
    pub fn focus(&self, focused: bool) {
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.write(if focused { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] });
        }
    }

    // ----- pointer -----

    /// Whether the program wants mouse reports. Shift always keeps the mouse
    /// for selecting text.
    pub fn tracks_mouse(&self, mods: ModifiersState) -> bool {
        !self.ended() && self.mode().intersects(TermMode::MOUSE_MODE) && !mods.shift_key()
    }

    /// The program has ended. What it left on screen can still be read,
    /// scrolled and selected: the modes it had set ask for nothing any more.
    fn ended(&self) -> bool {
        self.exit_code.is_some()
    }

    /// Reports a button press or release. See `keys::mouse` for the codes.
    pub fn mouse(&self, button: u8, pressed: bool, at: CellAt, mods: ModifiersState) {
        if let Some(bytes) = keys::mouse(button, pressed, at.col, at.row, mods, self.mode.get()) {
            self.write(bytes);
        }
    }

    /// Whether pointer motion is reported, with `held` the button that is
    /// down. Uses the modes of the last draw and takes no lock.
    pub fn wants_motion(&self, held: Option<u8>) -> bool {
        let mode = self.mode.get();
        !self.ended()
            && (mode.contains(TermMode::MOUSE_MOTION)
                || (mode.contains(TermMode::MOUSE_DRAG) && held.is_some()))
    }

    /// Scrolls by `lines`, positive is up: as mouse reports, as arrow keys
    /// for full-screen programs, or through the scrollback. Returns whether
    /// that changed what is on screen; what a program does with a report
    /// arrives as output.
    pub fn wheel(&self, lines: i32, at: CellAt, mods: ModifiersState) -> bool {
        let mut term = self.term.lock();
        let mode = if self.ended() { TermMode::empty() } else { *term.mode() };
        self.mode.set(mode);
        let count = lines.unsigned_abs().min(20) as usize;
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift_key() {
            drop(term);
            // All steps in one write: each write wakes the PTY thread.
            let button = if lines > 0 { 64 } else { 65 };
            let report = keys::mouse(button, true, at.col, at.row, mods, mode).unwrap_or_default();
            self.write(report.repeat(count));
            false
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !mods.shift_key() {
            drop(term);
            let intro = if mode.contains(TermMode::APP_CURSOR) { 'O' } else { '[' };
            let key = format!("\x1b{intro}{}", if lines > 0 { 'A' } else { 'B' });
            self.write(key.repeat(count).into_bytes());
            false
        } else {
            let before = term.grid().display_offset();
            term.scroll_display(Scroll::Delta(lines));
            term.grid().display_offset() != before
        }
    }
}
