//! The clipboard: copying, pasting, and the keys that wait behind a paste.
//!
//! The clipboard is read on a worker, so what is typed after a paste could
//! reach the program before the pasted text: an Enter would send an empty
//! line and leave the text behind. Keys for a terminal wait behind the
//! paste they followed.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use winit::event::KeyEvent;
use winit::keyboard::ModifiersState;

use super::{App, UserEvent};
use crate::core::edit::Edit;
use crate::core::{Reading, SessionId, Workspace};
use crate::platform::clipboard;

/// Where pasted text goes, and what has to be as it was for it to go there.
#[derive(Clone, Copy)]
pub enum PasteTarget {
    Field(u64),
    /// The session, the generation of its program, and which paste it is.
    Session(SessionId, u64, u64),
    /// The text of a file, by its opening and its reading, and which
    /// paste it is.
    File(Reading, u64),
}

/// How long keys wait for a paste: longer than a clipboard takes to give up.
const HELD_FOR: Duration = Duration::from_secs(6);
/// With this much waiting, nothing waits any longer.
const MAX_HELD: usize = 1024;

/// Input for a terminal or a file that waits for the paste it was typed
/// behind.
pub(super) enum Held {
    /// A paste that the clipboard has not answered yet, and whether it
    /// is one for a file.
    Paste(u64, bool),
    /// One it has answered, at the place it was asked for: a paste that
    /// was asked for later and answered sooner does not overtake it.
    Pasted(PasteTarget, String),
    Key(SessionId, KeyEvent, ModifiersState),
    Text(SessionId, String),
    /// A key and text for a file, by the opening and the reading they
    /// were typed for: another file may be shown by the time they go on.
    FileKey(Reading, KeyEvent, ModifiersState),
    FileText(Reading, String),
}

/// Pastes the clipboard has not answered, and what was typed for a terminal
/// behind them, in the order it goes to the program.
#[derive(Default)]
pub(super) struct Waiting {
    held: VecDeque<Held>,
    /// Since when the oldest of them waits.
    since: Option<Instant>,
    serial: u64,
}

impl Waiting {
    /// The number of a new paste.
    fn serial(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    /// The paste `serial` is out, for a file or not. What is typed from
    /// now on waits for it.
    fn paste(&mut self, serial: u64, file: bool) {
        self.held.push_back(Held::Paste(serial, file));
        self.since.get_or_insert_with(Instant::now);
    }

    /// Takes out what waits for a file, in its order: its keys, its
    /// text, and the pastes into it that were answered. A paste into it
    /// that is still out is given up on. What waits for a terminal
    /// stays, behind the pastes it waits for.
    fn take_file(&mut self) -> Vec<Held> {
        let file = |held: &Held| {
            matches!(
                held,
                Held::Paste(_, true)
                    | Held::Pasted(PasteTarget::File(..), _)
                    | Held::FileKey(..)
                    | Held::FileText(..)
            )
        };
        let (taken, kept) = std::mem::take(&mut self.held).into_iter().partition(file);
        self.held = kept;
        if self.held.is_empty() {
            self.since = None;
        }
        Vec::from(taken)
    }

    /// Something typed for a terminal waits behind what is out.
    pub(super) fn push(&mut self, held: Held) {
        self.held.push_back(held);
    }

    fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// When what waits is let go at the latest.
    pub(super) fn due(&self) -> Option<Instant> {
        self.since.map(|since| since + HELD_FOR)
    }

    /// Whether what waits has waited long enough, or is too much.
    fn overdue(&self) -> bool {
        self.held.len() >= MAX_HELD || self.due().is_some_and(|due| Instant::now() >= due)
    }

    /// The clipboard answered the paste `serial`: the text takes its
    /// place among what waits. Gives the text back if nothing waits for
    /// it any more.
    fn answered(&mut self, serial: u64, target: PasteTarget, text: String) -> Option<String> {
        match self.held.iter_mut().find(|held| matches!(held, Held::Paste(s, _) if *s == serial)) {
            Some(held) => *held = Held::Pasted(target, text),
            None => return Some(text),
        }
        None
    }

    /// Gives up on the pastes that are out.
    fn give_up(&mut self) {
        self.held.retain(|held| !matches!(held, Held::Paste(..)));
    }

    /// What goes on to its program now: the oldest of what waits, unless
    /// that is a paste still out. With nothing left, nothing waits since
    /// any time.
    fn next(&mut self) -> Option<Held> {
        match self.held.front() {
            Some(Held::Paste(..)) => None,
            Some(_) => self.held.pop_front(),
            None => {
                self.since = None;
                None
            }
        }
    }
}

impl App {
    /// Whether keys for a terminal have to wait. A paste that never comes
    /// back holds nothing longer than `HELD_FOR`.
    pub(super) fn holding(&mut self) -> bool {
        if self.waiting.overdue() {
            self.release_all();
        }
        !self.waiting.is_empty()
    }

    /// Gives up on the pastes that are out and sends on what waited.
    pub(super) fn release_all(&mut self) {
        self.waiting.give_up();
        while let Some(held) = self.waiting.next() {
            self.replay(held);
        }
    }

    fn replay(&mut self, held: Held) {
        match held {
            Held::Key(id, event, mods) => self.session_key(id, &event, mods),
            Held::Text(id, text) => self.send_to(id, |term| term.text(text)),
            // Only for the text it was typed for, and not while a line
            // asks about leaving it: only the keys of that line are taken.
            Held::FileKey(reading, event, mods) => {
                if let Some(pane) = self.typed_for(reading) {
                    self.file_key(pane, &event, mods);
                }
            }
            Held::FileText(reading, text) => {
                if self.typed_for(reading).is_some() {
                    self.file_edit(Edit::Type(&text));
                }
            }
            Held::Pasted(target, text) => self.pasted(target, text),
            Held::Paste(..) => {}
        }
    }

    /// The pane of the file something was typed for, if that text is
    /// still the one shown and nothing asks about leaving it.
    fn typed_for(&self, reading: Reading) -> Option<usize> {
        let shown = self.ws.file().filter(|(_, view)| view.reading() == reading);
        shown.filter(|_| self.asked.is_none()).map(|(pane, _)| pane)
    }

    /// What waits for a file goes into it now, and a paste into it that
    /// is still out is given up on: before anything is decided that
    /// depends on whether the file has changes. What was typed behind a
    /// paste is no change yet, and the file would give way with it.
    pub(super) fn settle_file(&mut self) {
        for held in self.waiting.take_file() {
            // A paste that was given up on is nothing to do.
            self.replay(held);
        }
    }

    /// Whether the pane with the keyboard shows a text of a file.
    fn in_file(&self) -> bool {
        !self.ui.editing() && self.shows_file_text(self.ws.focus())
    }

    pub(super) fn copy(&mut self, primary: bool) {
        let text = self.focused_term().and_then(|t| t.selection_text()).or_else(|| self.file_selection());
        if let (Some(text), Some(clipboard)) = (text, &mut self.clipboard) {
            clipboard.set(primary, text);
        }
    }

    pub(super) fn paste(&mut self, primary: bool) {
        let target = if self.ui.editing() {
            PasteTarget::Field(self.input_epoch)
        } else if self.in_file() {
            let Some((_, view)) = self.ws.file() else { return };
            PasteTarget::File(view.reading(), self.waiting.serial())
        } else {
            let Some(session) = self.ws.focused_session().and_then(|id| self.ws.session(id)) else {
                return;
            };
            if session.ended.is_some() || session.term.is_none() {
                return;
            }
            PasteTarget::Session(session.id, session.generation, self.waiting.serial())
        };
        let proxy = self.proxy.clone();
        if let Some(clipboard) = &mut self.clipboard {
            // The clipboard answers every read, with nothing if need be.
            match target {
                PasteTarget::Session(.., serial) => self.waiting.paste(serial, false),
                PasteTarget::File(_, serial) => self.waiting.paste(serial, true),
                PasteTarget::Field(_) => {}
            }
            clipboard.get(primary, move |read| {
                let _ = proxy.send_event(UserEvent::Pasted(target, read));
            });
        }
    }

    /// The clipboard answered. What was asked for a terminal or a file
    /// goes on in the order it was asked, with what was typed behind it.
    pub(super) fn finish_paste(&mut self, target: PasteTarget, read: clipboard::Read) {
        // A paste that brings nothing where there was a text says why:
        // nothing else tells it from a key that did nothing.
        let text = read.unwrap_or_else(|failed| {
            self.ui.notice = Some(not_pasted(failed).into());
            self.dirty = true;
            String::new()
        });
        let late = match target {
            PasteTarget::Field(_) => Some(text),
            PasteTarget::Session(.., serial) | PasteTarget::File(_, serial) => {
                self.waiting.answered(serial, target, text)
            }
        };
        // Nothing waits for it: a field, or a paste that was given up on.
        if let Some(text) = late {
            self.pasted(target, text);
        }
        while let Some(held) = self.waiting.next() {
            self.replay(held);
        }
    }

    /// Puts pasted text where it was asked for, if that is still there.
    fn pasted(&mut self, target: PasteTarget, text: String) {
        if text.is_empty() || !paste_valid(target, self.input_epoch, self.ui.editing(), &self.ws) {
            return;
        }
        match target {
            PasteTarget::Field(_) => self.type_text(text.trim().to_owned()),
            PasteTarget::Session(id, ..) => self.send_to(id, |term| term.paste(&text)),
            PasteTarget::File(..) => self.file_edit(Edit::Paste(&text)),
        }
    }
}

/// What the sidebar says of a paste that brought nothing.
fn not_pasted(failed: clipboard::Failed) -> &'static str {
    match failed {
        clipboard::Failed::Late => "Nothing was pasted: the clipboard did not answer in time.",
        clipboard::Failed::TooLarge => "Nothing was pasted: the clipboard holds more than 8 MiB of text.",
    }
}

/// Delayed replies belong to the original field edit or PTY generation.
fn paste_valid(target: PasteTarget, epoch: u64, editing: bool, ws: &Workspace) -> bool {
    match target {
        PasteTarget::Field(requested) => editing && epoch == requested,
        PasteTarget::Session(id, generation, _) => {
            ws.session(id).is_some_and(|session| session.generation == generation && session.ended.is_none())
        }
        // Another file, or another text of this one read meanwhile.
        PasteTarget::File(reading, _) => ws.file().is_some_and(|(_, view)| view.reading() == reading),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_behind_a_paste_waits_for_it_and_no_longer() {
        let mut waiting = Waiting::default();
        let text = |held: Option<Held>| match held {
            Some(Held::Text(_, text)) => Some(text),
            _ => None,
        };
        let (first, second) = (waiting.serial(), waiting.serial());
        waiting.paste(first, false);
        waiting.push(Held::Text(SessionId(1), "a".into()));
        waiting.paste(second, false);
        waiting.push(Held::Text(SessionId(1), "b".into()));
        assert!(waiting.next().is_none(), "the first paste is out");
        // The later paste came back first: it keeps its place, what is
        // behind the earlier one still waits, and so does all that follows.
        let target = PasteTarget::Session(SessionId(1), 0, 0);
        let pasted = |held: Option<Held>| match held {
            Some(Held::Pasted(_, text)) => Some(text),
            _ => None,
        };
        assert!(waiting.answered(second, target, "2".into()).is_none());
        assert!(waiting.next().is_none());
        assert!(waiting.answered(first, target, "1".into()).is_none());
        assert_eq!(pasted(waiting.next()).as_deref(), Some("1"));
        assert_eq!(text(waiting.next()).as_deref(), Some("a"));
        assert_eq!(pasted(waiting.next()).as_deref(), Some("2"));
        assert_eq!(text(waiting.next()).as_deref(), Some("b"));
        // An answer nothing waits for comes back to be dealt with at once.
        assert_eq!(waiting.answered(first, target, "late".into()).as_deref(), Some("late"));
        // The time runs from the oldest paste until nothing waits.
        assert!(waiting.due().is_some());
        assert!(waiting.next().is_none() && waiting.due().is_none());

        let third = waiting.serial();
        waiting.paste(third, false);
        waiting.push(Held::Text(SessionId(1), "c".into()));
        waiting.give_up();
        assert_eq!(text(waiting.next()).as_deref(), Some("c"));
        assert!(!waiting.overdue());
    }

    #[test]
    fn what_waits_for_a_file_is_taken_out_in_its_order_and_the_rest_stays() {
        let mut waiting = Waiting::default();
        let (session, file, later) = (waiting.serial(), waiting.serial(), waiting.serial());
        let reading = Reading { opening: 3, reading: 1 };
        waiting.paste(session, false);
        waiting.push(Held::Text(SessionId(1), "s".into()));
        waiting.paste(file, true);
        waiting.push(Held::FileText(reading, "a".into()));
        waiting.paste(later, true);
        waiting.push(Held::FileText(reading, "b".into()));
        // The second paste into the file was answered, the first not.
        assert!(waiting.answered(later, PasteTarget::File(reading, later), "p".into()).is_none());
        let taken: Vec<String> = waiting
            .take_file()
            .into_iter()
            .map(|held| match held {
                Held::FileText(_, text) => text,
                Held::Pasted(_, text) => format!("pasted {text}"),
                Held::Paste(..) => "given up".to_owned(),
                _ => panic!("not for a file"),
            })
            .collect();
        assert_eq!(taken, ["given up", "a", "pasted p", "b"]);
        // What waits for the terminal still waits for its paste.
        assert!(waiting.next().is_none() && waiting.due().is_some());
        let target = PasteTarget::Session(SessionId(1), 0, session);
        assert!(waiting.answered(session, target, "t".into()).is_none());
        assert!(matches!(waiting.next(), Some(Held::Pasted(..))));
        assert!(matches!(waiting.next(), Some(Held::Text(..))));
        // The answer to the paste that was given up on waits for nothing.
        assert_eq!(
            waiting.answered(file, PasteTarget::File(reading, file), "late".into()).as_deref(),
            Some("late")
        );
        // With nothing but what is for a file, nothing waits afterwards.
        let only = waiting.serial();
        waiting.paste(only, true);
        waiting.push(Held::FileText(reading, "c".into()));
        assert_eq!(waiting.take_file().len(), 2);
        assert!(waiting.is_empty() && waiting.due().is_none());
    }

    #[test]
    fn delayed_pastes_do_not_cross_fields_or_restarted_sessions() {
        let mut ws = Workspace::new();
        let project = ws.add_project("/throwaway".into());
        let id = ws.new_session(project, "shell");
        let field = PasteTarget::Field(5);
        assert!(paste_valid(field, 5, true, &ws));
        assert!(!paste_valid(field, 6, true, &ws), "another edit cancels the paste");
        assert!(!paste_valid(field, 5, false, &ws), "closing the field cancels it");
        let terminal = PasteTarget::Session(id, 0, 0);
        assert!(paste_valid(terminal, 9, false, &ws));
        ws.session_mut(id).unwrap().generation = 1;
        assert!(!paste_valid(terminal, 9, false, &ws));
        let restarted = PasteTarget::Session(id, 1, 0);
        assert!(paste_valid(restarted, 9, false, &ws));
        ws.session_mut(id).unwrap().ended = Some("Exited".into());
        assert!(!paste_valid(restarted, 9, false, &ws));
        ws.remove_session(id);
        assert!(!paste_valid(restarted, 9, false, &ws));
    }
}
