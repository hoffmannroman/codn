//! Saving the text of a file, and what waits while a file has changes
//! that are not saved: the question in the line at its bottom, the keys
//! that answer it, and the closing of the window.

use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use super::{App, Asked};
use crate::action::{Action, shortcut_key};
use crate::core::Said;
use crate::core::save::{self, Outcome};

/// The keys that answer a line which asks about leaving.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Chord {
    Escape,
    /// Ctrl+S.
    Save,
    /// Ctrl+Shift+Backspace.
    LetGo,
    Other,
}

/// What a key answers to such a line.
#[derive(Debug, PartialEq)]
enum Answer {
    Stay,
    Save,
    LetGo,
    /// Close the window while a file is written.
    CloseNow,
}

/// Whether a line is one that stands while something waits: the question
/// about changes, the word that the file is written in place on the way
/// to saving them, and the word that a file is still being written.
fn asks_to_leave(said: &Said) -> bool {
    matches!(said, Said::Leave | Said::InPlace | Said::Writing)
}

/// What a key answers to the line that stands while something waits:
/// nothing, unless the line names the key. Escape always stays.
fn answers(said: Option<&Said>, chord: Chord) -> Option<Answer> {
    match (said, chord) {
        (_, Chord::Escape) => Some(Answer::Stay),
        (Some(Said::Leave | Said::InPlace), Chord::Save) => Some(Answer::Save),
        (Some(Said::Leave), Chord::LetGo) => Some(Answer::LetGo),
        (Some(Said::Writing), Chord::LetGo) => Some(Answer::CloseNow),
        _ => None,
    }
}

/// What the thread that writes did with a text, as far as what waits
/// depends on it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Done {
    Written,
    /// Nothing yet: the file is written in place, and the user is told
    /// so first.
    InPlace,
    Nothing,
}

impl Done {
    fn of(outcome: &Outcome) -> Self {
        match outcome {
            Outcome::Saved { .. } => Self::Written,
            Outcome::InPlace => Self::InPlace,
            _ => Self::Nothing,
        }
    }
}

/// What becomes of what waits for changes to be saved, once the thread
/// that writes has answered.
#[derive(Debug, PartialEq)]
enum Next {
    /// The text that is there is the one the file holds.
    GoOn,
    /// It was written, and typed into since: the question is the same,
    /// and is put again.
    Ask,
    /// The line that says the file is written in place stands, and what
    /// waited waits on for the save it names.
    Stand,
    /// Nothing was saved. What waited is dropped, and the line says what
    /// came of the save, with keys of its own.
    Stay,
}

/// `saved` is what `FileView::saved` said of the answer, nothing for an
/// answer that was for another file: such a one changes nothing here.
/// The text that is there decides first: taken back to what the file
/// holds while the thread was out, it goes on whatever this answer did.
fn next(saved: Option<bool>, done: Done) -> Option<Next> {
    Some(match (saved?, done) {
        (true, _) => Next::GoOn,
        (false, Done::Written) => Next::Ask,
        (false, Done::InPlace) => Next::Stand,
        (false, Done::Nothing) => Next::Stay,
    })
}

impl App {
    /// Hands the text of the file that is shown to the thread that
    /// writes, if it has changes, or with `force` over whatever the file
    /// holds by now. Says whether it was handed over.
    pub(super) fn file_save(&mut self, force: bool) -> bool {
        let Some((_, view)) = self.ws.file() else { return false };
        let Some(root) = self.ws.project(view.project).map(|project| project.path.clone()) else {
            return false;
        };
        // One at a time: a second one would find the file changed, by the
        // first. And none once the window is closing: nothing waits for
        // it any more.
        if self.quit || self.writing > 0 || !(force || view.changed()) {
            return false;
        }
        let Some(view) = self.ws.file_mut() else { return false };
        let sent = match view.job(&root, force) {
            Some((of, job)) => self.writer.send(of, job),
            None => false,
        };
        if sent {
            self.writing += 1;
        } else {
            view.say(Some(Said::Failed("Not saved. This text cannot be written.".into())));
        }
        self.dirty = true;
        sent
    }

    /// The thread that writes has answered.
    pub(super) fn wrote(&mut self, reply: save::Reply) {
        self.writing = self.writing.saturating_sub(1);
        self.dirty = true;
        let done = Done::of(&reply.outcome);
        // Nothing of an answer for a file that is not shown any more.
        let shown =
            self.ws.file_mut().and_then(|view| Some((view.saved(reply.of, reply.outcome)?, view.project)));
        if let Some((_, project)) = shown.filter(|_| done == Done::Written) {
            // The file is another now: for git, for the marks of the
            // text, and for the folder it lies in.
            self.refresh_git(project);
            self.refresh_files();
            self.fit();
        }
        if self.asked.is_some() {
            match next(shown.map(|(saved, _)| saved), done) {
                Some(Next::GoOn) => {
                    if let Some(asked) = self.asked.take() {
                        self.go_on(asked);
                    }
                }
                Some(Next::Ask) => self.ask_again(),
                Some(Next::Stand) | None => {}
                Some(Next::Stay) => (self.asked, self.leaving) = (None, false),
            }
        }
        if self.leaving && self.writing == 0 {
            self.close();
        }
    }

    /// A file would lose its place with changes that are not saved: the
    /// line at the bottom of it asks, and what was asked for waits. The
    /// two are one state: while something waits, a line that names the
    /// keys stands, and those keys are the ones that are taken.
    pub(super) fn ask(&mut self, asked: Asked) {
        self.asked = Some(asked);
        self.ask_again();
    }

    /// Puts the question about changes that are not saved in the line.
    fn ask_again(&mut self) {
        if let Some(view) = self.ws.file_mut() {
            view.say(Some(Said::Leave));
        }
        self.dirty = true;
    }

    /// "Stay": what was asked for is dropped, and the line goes.
    pub(super) fn stay(&mut self) {
        (self.asked, self.leaving) = (None, false);
        if let Some(view) = self.ws.file_mut() {
            view.stay();
        }
        self.dirty = true;
    }

    /// Does what waited for an answer.
    fn go_on(&mut self, asked: Asked) {
        match asked {
            Asked::Do(action, split) => self.run(action, split),
            Asked::File(path) => self.show_file(&path),
            // The pane the file is in now, not the one it was in when
            // the question was asked: a session beside it may have ended.
            Asked::CloseFile => {
                if let Some((pane, _)) = self.ws.file() {
                    self.run(Action::ClosePane(pane), false);
                }
            }
        }
    }

    /// A key while something waits for the answer about changes that are
    /// not saved. Only the keys the line names do something, and Escape,
    /// which stays.
    pub(super) fn answer(&mut self, event: &KeyEvent) {
        let (ctrl, shift, alt) = (self.mods.control_key(), self.mods.shift_key(), self.mods.alt_key());
        let letter = shortcut_key(&event.key_without_modifiers(), event.physical_key);
        let chord = match &event.logical_key {
            Key::Named(NamedKey::Escape) => Chord::Escape,
            Key::Named(NamedKey::Backspace) if ctrl && shift && !alt => Chord::LetGo,
            _ if ctrl && !shift && !alt && letter == "s" => Chord::Save,
            _ => Chord::Other,
        };
        let Some(said) = self.ws.file().map(|(_, view)| view.said().cloned()) else {
            // No file is shown that could hold what waited.
            return self.stay();
        };
        match answers(said.as_ref(), chord) {
            Some(Answer::Stay) => self.stay(),
            // Closing while a file is written: this key closes all the
            // same, at the price of what is being written.
            Some(Answer::CloseNow) => self.quit = true,
            Some(Answer::LetGo) => {
                if let Some(view) = self.ws.file_mut() {
                    view.let_go();
                    view.say(None);
                }
                self.dirty = true;
                if let Some(asked) = self.asked.take() {
                    self.go_on(asked);
                }
            }
            // It goes on when the thread has answered, see `wrote`.
            Some(Answer::Save) => {
                let sent = self.file_save(false);
                if !sent && self.writing == 0 {
                    self.stay();
                }
            }
            // A key the line does not name does nothing. A line that
            // names none, with something waiting, is put right.
            None if !said.as_ref().is_some_and(asks_to_leave) => self.ask_again(),
            None => {}
        }
    }

    /// The window is asked to close. It does not while a file has changes
    /// that are not saved, which the line at the bottom asks about, nor
    /// while a file is being written: it closes when that is done. Asked
    /// again meanwhile, the line names the key that closes all the same,
    /// so that a drive that hangs does not keep the window for good.
    pub(super) fn close(&mut self) {
        // What was typed behind a paste that is still out is typed: it
        // is a part of what the question is about.
        self.settle_file();
        if self.ws.unsaved() {
            self.leaving = false;
            return self.ask(Asked::Do(Action::Quit, false));
        }
        if self.writing == 0 {
            self.quit = true;
            return;
        }
        let again = std::mem::replace(&mut self.leaving, true);
        match self.ws.file_mut().filter(|_| again) {
            Some(view) => {
                view.say(Some(Said::Writing));
                self.asked = Some(Asked::Do(Action::Quit, false));
                self.dirty = true;
            }
            // No pane to say it in: asked twice is asked enough.
            None if again => self.quit = true,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_keys_a_line_names_answer_it() {
        use Chord::*;
        let all = [Escape, Save, LetGo, Other];
        let to = |said: &Said| all.map(|chord| answers(Some(said), chord));
        assert_eq!(to(&Said::Leave), [Some(Answer::Stay), Some(Answer::Save), Some(Answer::LetGo), None]);
        // On the way to saving, the file is written in place: the line
        // names the key that saves, and not the one that lets go.
        assert_eq!(to(&Said::InPlace), [Some(Answer::Stay), Some(Answer::Save), None, None]);
        // While a file is written, one key closes all the same.
        assert_eq!(to(&Said::Writing), [Some(Answer::Stay), None, Some(Answer::CloseNow), None]);
        // A line that names other keys is answered by none of these, and
        // neither is no line at all. Escape gets out of everything.
        for said in [Said::Other, Said::Gone, Said::NoWayBack, Said::Unflushed] {
            assert_eq!(to(&said), [Some(Answer::Stay), None, None, None], "{said:?}");
            assert!(!asks_to_leave(&said));
        }
        assert_eq!(all.map(|chord| answers(None, chord)), [Some(Answer::Stay), None, None, None]);
        assert!([Said::Leave, Said::InPlace, Said::Writing].iter().all(asks_to_leave));
    }

    #[test]
    fn what_waits_goes_on_only_when_the_text_that_is_there_was_saved() {
        // Saved as it is: on.
        assert_eq!(next(Some(true), Done::Written), Some(Next::GoOn));
        // The text that is there is the one the file holds, whatever this
        // answer wrote: taken back to it while the thread was out.
        assert_eq!(next(Some(true), Done::InPlace), Some(Next::GoOn));
        assert_eq!(next(Some(true), Done::Nothing), Some(Next::GoOn));
        // Written, and typed into since: the question is put again.
        assert_eq!(next(Some(false), Done::Written), Some(Next::Ask));
        // Written in place, and the user is told first: the line stands,
        // so that the save that follows still goes on.
        assert_eq!(next(Some(false), Done::InPlace), Some(Next::Stand));
        // Another text on the disk, gone, failed: it stays.
        assert_eq!(next(Some(false), Done::Nothing), Some(Next::Stay));
        // An answer for a file that is not shown any more says nothing
        // of what waits for the one that is.
        assert_eq!(next(None, Done::Written), None);
        assert_eq!(next(None, Done::Nothing), None);
    }
}
