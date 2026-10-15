//! A file of a project, shown in a pane: what was read of it, the
//! caret and the view in it, its marks and its colors, what was changed
//! in it and whether that is saved, and what the line at the bottom says.
//! Which answers are for it is told by numbers, see `Edition`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::colors::{Colors, FILE_MAX};
use super::edit::{self, Edit, History};
use super::files::{Body, Content, Edition, Stamp, digest};
use super::picture::Seen;
use super::preview::{self, Preview, Why};
use super::save::{self, Job, Outcome};
use super::text::{Document, Pos, Refused, View};
use super::{ProjectId, git};

/// A file of a project, shown in a pane.
pub struct FileView {
    pub project: ProjectId,
    /// Where the file lies inside the folder of the project.
    pub path: PathBuf,
    /// Read through `shown`, `view` and `marks`, and changed only by the
    /// methods here: what is asked of a text names its change, and a
    /// text put there from outside would not have counted as one.
    shown: Shown,
    /// What of a text is seen, and the caret in it.
    view: View,
    /// What changed in the file since the last commit.
    marks: git::Marks,
    /// What is known of the colors of its text.
    pub colors: Colors,
    /// The file as it is meant, where it is one of Markdown: whether
    /// that is wanted, and what is drawn of it.
    pub preview: Preview,
    /// The state the file was in when it was read, to ask whether it
    /// changed since.
    stamp: Option<Stamp>,
    /// Which opening of a file this is, a number no other has, and how
    /// often its text was put anew, which only grows: see `Edition`.
    opening: u64,
    change: u64,
    /// The file was read again once because git saw another state of it
    /// than the text here is of. A second time waits for an occasion: a
    /// file that is written all the time would be read without end.
    chased: bool,
    /// How often a text was put in the place of the one before.
    readings: u64,
    /// What was changed in the text, and which state of it is saved.
    history: History,
    /// The digest of the bytes the text was read from or written as:
    /// what the file has to hold still for a save to write over it.
    digest: Option<u64>,
    /// Where the name of the file led when the text was read, and the
    /// permissions the file had: a save writes there and nowhere else,
    /// and a file that is made anew gets what it had.
    led: u64,
    mode: Option<u32>,
    /// The user was told that this file is written in place.
    told: bool,
    /// The state of the text whose changes the user let go: for exactly
    /// that state the file may lose its place.
    released: Option<u64>,
    /// What of the disk is to take the place of the text that was
    /// changed here.
    theirs: Theirs,
    /// A save that was to write over whatever the file holds was
    /// answered with the word that the file is written in place: the
    /// save that follows that word writes over it all the same.
    forced: bool,
    /// The bytes that were handed over to be written, by what a reading
    /// of them would say they are, while the answer of that save is not
    /// here: found on the disk meanwhile, they are no other text.
    wrote: Option<u64>,
    /// The change of the text from which on a reading is taken: one that
    /// was asked for before the last save says nothing of the file as
    /// it is since.
    since: u64,
    /// What the line at the bottom says of the file besides what its
    /// text has to say. Set by `say`, which lets go of what waited for
    /// another answer.
    said: Option<Said>,
}

/// A reading of a file, for what comes back later for its text, as a
/// paste does: the opening of the file, and how often a text was put in
/// the place of the one before. Not the change of an `Edition`: every
/// key is a change, and a paste is still for the text it was asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reading {
    pub opening: u64,
    pub reading: u64,
}

/// What of the disk is to take the place of a text that was changed
/// here: one of these, so that nothing is asked for while something waits.
enum Theirs {
    /// Nothing. The text stays.
    Left,
    /// The next reading is taken: the user asked for it.
    Asked,
    /// What was read, kept while the line at the bottom says that taking
    /// it cannot be taken back.
    Kept(Content),
}

/// What the line at the bottom of a file says.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// Why a key changed nothing.
    Refused(Refused),
    /// The text was changed here, and the file holds another text than
    /// the one that was read: one of the two has to give way.
    Other,
    /// What the disk holds cannot be taken as a step that is taken back.
    NoWayBack,
    /// The file is not there any more.
    Gone,
    /// The file is written in place, which the user is told once.
    InPlace,
    /// Something came to nothing, in the words for the line: a save that
    /// wrote nothing, a file that could not be read to take it.
    Failed(String),
    /// A save in place failed half way.
    Damaged(String),
    /// The file was written, and the disk did not confirm it.
    Unflushed,
    /// Changes that are not saved are about to lose their place.
    Leave,
    /// The window was asked to close, twice, while the file is written.
    Writing,
}

impl Said {
    /// Whether Escape answers this, and does not close the pane.
    pub fn asks(&self) -> bool {
        matches!(
            self,
            Self::Other | Self::NoWayBack | Self::Gone | Self::InPlace | Self::Leave | Self::Writing
        )
    }
}

/// What becomes of marks git read for a state of a file.
#[derive(Debug, PartialEq)]
pub enum Taken {
    /// They are those of the text that is shown. Whether they are others
    /// than before.
    Marked(bool),
    /// Git saw another state of the file: it is to be read again.
    Reread,
    /// Nothing is to be done with them.
    Left,
}

/// What a pane shows of a file.
pub enum Shown {
    Reading,
    Text(Document),
    /// A picture, with what is known of it.
    Picture(Seen),
    /// Not text: the name of its format, where it has one.
    Other(Option<&'static str>),
    /// Why it could not be read.
    Failed(String),
}

impl FileView {
    /// `grammar` is the name of the grammar that shows the file in
    /// colors, if its name says one.
    pub(super) fn new(
        project: ProjectId,
        path: PathBuf,
        opening: u64,
        grammar: Option<&'static str>,
    ) -> Self {
        Self {
            project,
            path,
            shown: Shown::Reading,
            view: View::default(),
            marks: git::Marks::default(),
            colors: Colors::new(grammar),
            preview: Preview::new(grammar == Some(preview::GRAMMAR)),
            stamp: None,
            opening,
            change: 0,
            chased: false,
            readings: 0,
            history: History::new(),
            digest: None,
            led: 0,
            mode: None,
            told: false,
            released: None,
            theirs: Theirs::Left,
            forced: false,
            wrote: None,
            since: 0,
            said: None,
        }
    }

    /// What the pane shows of the file.
    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// What of a text is seen, and the caret in it. It is moved through
    /// `parts`.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// What changed in the file since the last commit.
    pub fn marks(&self) -> &git::Marks {
        &self.marks
    }

    /// The state the file was in when it was read, to ask whether it
    /// changed since.
    pub fn stamp(&self) -> Option<Stamp> {
        self.stamp
    }

    /// The picture the pane shows, if it shows one.
    pub fn picture(&self) -> Option<&Seen> {
        match &self.shown {
            Shown::Picture(seen) => Some(seen),
            _ => None,
        }
    }

    pub fn picture_mut(&mut self) -> Option<&mut Seen> {
        match &mut self.shown {
            Shown::Picture(seen) => Some(seen),
            _ => None,
        }
    }

    /// What was made of the file is not what it will hold: it was being
    /// written. The state it was in is forgotten, so that the next
    /// occasion reads it again.
    pub fn unsettled(&mut self) {
        self.stamp = None;
    }

    /// The text with what is known of its colors and what is seen of it,
    /// to bring the colors in line with the text.
    pub fn coloring(&mut self) -> Option<(&Document, &mut Colors, &View)> {
        match &self.shown {
            Shown::Text(text) => Some((text, &mut self.colors, &self.view)),
            _ => None,
        }
    }

    /// Whether the file is shown as it is meant, and not as its text:
    /// it is one of Markdown, that is wanted, and nothing stands
    /// against it.
    pub fn previews(&self) -> bool {
        matches!(&self.shown, Shown::Text(doc) if self.preview.shows(doc.len()))
    }

    /// Why the file is shown as its text though it is wanted as it is
    /// meant.
    pub fn unpreviewed(&self) -> Option<Why> {
        match &self.shown {
            Shown::Text(doc) => self.preview.refused(doc.len()),
            _ => None,
        }
    }

    /// Whether the file has the switch between the two: a text of
    /// Markdown.
    pub fn switches(&self) -> bool {
        self.preview.can() && self.text().is_some()
    }

    /// Switches between the file as it is meant and its text. The view
    /// stays at the line it stood at. `rows` is how many lines of text
    /// the pane shows.
    pub fn switch_preview(&mut self, rows: usize) {
        if self.text().is_none() {
            return;
        }
        if self.previews()
            && let Some(line) = self.preview.line()
        {
            self.stand_at(line, rows);
        }
        let bytes = self.text().map_or(0, Document::len);
        self.preview.switch(self.view.top as u32, bytes);
    }

    /// Puts the view of the text at a line, where the preview stood.
    fn stand_at(&mut self, line: usize, rows: usize) {
        let Shown::Text(doc) = &self.shown else { return };
        // A caret that is not seen there comes along: the next key
        // would else take the view back to where it stands.
        if !(line..line.saturating_add(rows)).contains(&self.view.caret.line) {
            self.view.place(doc, Pos { line, byte: 0 }, false);
        }
        self.view.top = line;
        self.view.fit(doc, rows);
    }

    /// Whether a preview is asked for while the file is shown as its
    /// text, see `Preview::tries`.
    pub fn tries_preview(&self) -> bool {
        matches!(&self.shown, Shown::Text(doc) if self.preview.tries(doc.len()))
    }

    /// The text `of` has no preview, and why. Where the file was shown
    /// as it is meant until now, its text stands at the line the
    /// preview stood at. Says whether that was news.
    pub fn no_preview(&mut self, of: Edition, why: Why, rows: usize) -> bool {
        let stood = self.preview.line().filter(|_| self.previews());
        if !self.preview.fail(of, why) {
            return false;
        }
        if let Some(line) = stood {
            self.stand_at(line, rows);
        }
        true
    }

    /// The lines of the text that are seen: of its rows, or of its
    /// preview in a room that high. The colors of these are asked for.
    pub fn seen(&self, rows: usize, height: f32) -> (usize, usize) {
        match self.preview.lines(height).filter(|_| self.previews()) {
            Some(lines) => lines,
            None => (self.view.top, rows),
        }
    }

    /// Whether the text is another than the file holds.
    pub fn changed(&self) -> bool {
        self.history.changed()
    }

    /// Whether the file must not lose its place: its text has changes
    /// that are not saved, and the user did not let them go.
    pub fn holds(&self) -> bool {
        self.changed() && self.released != Some(self.history.mark())
    }

    /// What the line at the bottom says of the file besides what its
    /// text has to say.
    pub fn said(&self) -> Option<&Said> {
        self.said.as_ref()
    }

    /// Sets what the line at the bottom says. What waited for the answer
    /// to another line goes with that line: the text of the disk that was
    /// kept to be taken, and the word to write over whatever is there.
    /// An answer is for the question that stands, never for an older one.
    pub fn say(&mut self, said: Option<Said>) {
        if said != Some(Said::NoWayBack) && matches!(self.theirs, Theirs::Kept(_)) {
            self.theirs = Theirs::Left;
        }
        if said != Some(Said::InPlace) {
            self.forced = false;
        }
        self.said = said;
    }

    /// The answer to whatever the line at the bottom asked is "stay":
    /// the line goes, and with it what waited for another answer.
    pub fn stay(&mut self) {
        self.say(None);
        self.theirs = Theirs::Left;
    }

    /// The user lets the changes of the text go, as it is now.
    pub fn let_go(&mut self) {
        self.released = Some(self.history.mark());
    }

    /// Which opening of a file this is and which reading of it the text
    /// is from. What comes back later for the text, as a paste does,
    /// names this and not the change: every key is a change.
    pub fn reading(&self) -> Reading {
        Reading { opening: self.opening, reading: self.readings }
    }

    /// Another text was put in the place of the one that was shown.
    /// Whatever was asked of the one before is not an answer about this
    /// one, and a paste that was asked for it is not for this one: both
    /// numbers grow, and the second never without the first.
    fn replaced(&mut self) {
        self.change += 1;
        self.readings += 1;
    }

    /// The text as it is to be written to the file at `root`, and what
    /// its answer will name: nothing for a text that is only shown. With
    /// `force` over whatever the file holds by now.
    pub fn job(&mut self, root: &Path, force: bool) -> Option<(save::Of, Job)> {
        let bytes = self.text()?.written()?;
        // What is typed after this is not a part of what was written.
        self.history.rest();
        let of = save::Of { opening: self.opening, mark: self.history.mark() };
        // The word to write over what is there holds through the line
        // that says the file is written in place.
        let force = force || self.forced;
        self.forced = force;
        self.wrote = Some(digest(&bytes));
        let (path, known, in_place) = (root.join(&self.path), self.digest, self.told);
        Some((of, Job { path, bytes, known, force, in_place, led: self.led, mode: self.mode }))
    }

    /// Takes the answer of a save of the state `of`. Says whether the
    /// text that is there now is the one the file holds, or nothing for
    /// an answer that is for another opening of a file.
    pub fn saved(&mut self, of: save::Of, outcome: Outcome) -> Option<bool> {
        if of.opening != self.opening {
            return None;
        }
        self.wrote = None;
        let said = match outcome {
            Outcome::Saved { stamp, digest, flushed } => {
                // The state of the file is taken from what was written,
                // or the next look at it would call the own save a
                // change by someone else. A reading that was asked for
                // before this says nothing of the file as it is now: its
                // answer would put an older text in the place of the one
                // that was just written. So the text counts as another
                // from here on, and what was asked of it is asked again.
                self.history.saved(of.mark);
                (self.stamp, self.digest, self.chased) = (stamp, Some(digest), false);
                self.change += 1;
                self.since = self.change;
                // Nor is a word to take what the disk holds one for a
                // reading after this: the one it was given for is not
                // taken, and it would take the next, whenever that
                // comes, over what was typed since.
                self.theirs = Theirs::Left;
                self.colors.lost();
                (!flushed).then_some(Said::Unflushed)
            }
            Outcome::Other => Some(Said::Other),
            Outcome::Gone => Some(Said::Gone),
            Outcome::InPlace => {
                self.told = true;
                Some(Said::InPlace)
            }
            Outcome::Failed(why) => Some(Said::Failed(format!("Not saved. {why}"))),
            // What the file holds now is not known, and no state of the
            // text is the one on the disk.
            Outcome::Damaged(why) => {
                self.history.unsaved();
                self.digest = None;
                Some(Said::Damaged(why))
            }
        };
        // The word to write over what is there holds through the word
        // about writing in place and no further: `say` keeps it for that
        // line alone.
        self.say(said);
        Some(!self.changed())
    }

    /// Whether a reading that was asked for the text `of` is still one
    /// to take: of this opening, and not asked before the last save.
    pub fn takes(&self, of: Edition) -> bool {
        of.opening == self.opening && of.change >= self.since
    }

    /// The user asked for what the disk holds, over the text that was
    /// changed here: the next reading is taken. Says whether there is
    /// something to read for it, or it was taken already.
    pub fn take_theirs(&mut self, rows: usize) -> bool {
        // Only while the line says so: what was kept for an older line
        // is not what this one offers.
        match std::mem::replace(&mut self.theirs, Theirs::Left) {
            // Said once more, after the line said there is no way back.
            Theirs::Kept(content) if self.said == Some(Said::NoWayBack) => {
                self.history.forget();
                self.say(None);
                // What is shown is what was typed, whatever it was read
                // from: where the disk holds those bytes again, the
                // reading is not one of the text that is there.
                self.digest = None;
                self.read(Ok(content), rows);
                false
            }
            _ => {
                self.theirs = Theirs::Asked;
                true
            }
        }
    }

    /// Whether what was read is what a save has handed over to be
    /// written, the answer of which is not here yet.
    fn wrote_as(&self, content: &Content) -> bool {
        content.digest.is_some() && content.digest == self.wrote
    }

    /// Whether the reading that has come is one the user asked for, to be
    /// taken over the text that was changed here. Asked once, it is taken
    /// once: this uses the word up.
    fn asked_for(&mut self) -> bool {
        let asked = matches!(self.theirs, Theirs::Asked);
        if asked {
            self.theirs = Theirs::Left;
        }
        asked
    }

    /// Takes what the disk holds over the text that was changed here: as
    /// a step that can be taken back, where both are texts that can be
    /// changed. Else the line at the bottom says that there is no way
    /// back, and what was read waits for the word.
    fn take(&mut self, content: Content, rows: usize) {
        let Content { stamp, digest, body } = content;
        let step = match (&mut self.shown, &body) {
            (Shown::Text(doc), Body::Text(theirs))
                if theirs.editable() && theirs.marked() == doc.marked() =>
            {
                let same = doc.same_lines(theirs);
                edit::replace_all(doc, &mut self.view, &mut self.history, theirs.bytes(), Instant::now())
                    .ok()
                    .map(|_| same)
            }
            _ => None,
        };
        match step {
            Some(same) => {
                // The text is the one on the disk, and that state is the
                // saved one.
                self.history.saved(self.history.mark());
                (self.stamp, self.digest) = (Some(stamp), digest);
                (self.led, self.mode) = (stamp.led(), Some(stamp.mode()));
                self.replaced();
                // None of them where the text that was taken is too
                // large for colors, as where one is read.
                let large = self.text().is_some_and(|text| text.len() > FILE_MAX);
                self.colors.changed(if large { 0 } else { same });
                self.preview.again(self.text().map_or(0, Document::len));
                self.marks = git::Marks::default();
                self.say(None);
                if let Shown::Text(doc) = &self.shown {
                    self.view.fit(doc, rows);
                }
            }
            None => {
                self.say(Some(Said::NoWayBack));
                self.theirs = Theirs::Kept(Content { stamp, digest, body });
            }
        }
    }

    /// Moves the view so far that the caret is in it, in a pane of
    /// `rows` lines and `cols` columns.
    pub fn reveal(&mut self, rows: usize, cols: usize) {
        if let Shown::Text(doc) = &self.shown {
            self.view.reveal(doc, rows, cols);
        }
    }

    /// Does to the text what a key asks, at the time the key came. Says
    /// whether the text is another now. A key that was refused says why
    /// in the line at the bottom.
    pub fn edit(&mut self, edit: Edit, now: Instant) -> bool {
        let Shown::Text(doc) = &mut self.shown else { return false };
        match edit::apply(doc, &mut self.view, &mut self.history, edit, now) {
            Ok(Some(lines)) => {
                // Another text: what was asked of the one before is no
                // answer about it, its colors are asked for again from
                // the line that changed, and the marks of the file stand
                // at lines that may have moved.
                self.change += 1;
                self.colors.edited(lines);
                // What was known of the colors stays until the answer
                // for the text as it is now, and a text that grew too
                // large for colors is not asked for: none would come.
                if doc.len() > FILE_MAX {
                    self.colors.changed(0);
                }
                self.marks = git::Marks::default();
                // A question about leaving is answered, not typed away.
                if !matches!(self.said, Some(Said::Leave | Said::Writing)) {
                    self.say(None);
                }
                true
            }
            Ok(None) => false,
            Err(why) => {
                self.say(Some(Said::Refused(why)));
                false
            }
        }
    }

    /// The text that is shown, for what is asked of it: an answer counts
    /// only while it names this.
    pub fn edition(&self) -> Edition {
        Edition { opening: self.opening, change: self.change }
    }

    /// Takes the marks git read for the text `of` while the file was in
    /// the state `stamp`, or nothing if it moved meanwhile. Only marks
    /// of the text that is shown, and of the state it was read in, are
    /// taken: others would stand at wrong lines.
    pub fn take_marks(&mut self, of: Edition, stamp: Option<Stamp>, marks: Option<git::Marks>) -> Taken {
        // Marks for another text: its own were asked for when it came.
        // And none over a text that was changed here: they are those of
        // the file, and stand at its lines.
        if of != self.edition() || self.changed() {
            return Taken::Left;
        }
        // A text whose file cannot be read now has no state to hold
        // marks against. The next occasion asks for the file again.
        let Some(known) = self.stamp else { return Taken::Left };
        if stamp == Some(known) {
            self.chased = false;
            let marks = marks.unwrap_or_default();
            let changed = self.marks != marks;
            self.marks = marks;
            Taken::Marked(changed)
        } else if std::mem::replace(&mut self.chased, true) {
            Taken::Left
        } else {
            Taken::Reread
        }
    }

    /// An occasion to look at the file again has come: the window got
    /// the focus, a session went quiet, someone clicked.
    pub fn occasion(&mut self) {
        self.chased = false;
    }

    pub fn text(&self) -> Option<&Document> {
        match &self.shown {
            Shown::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The text with the view of it, to move the caret in. What is typed
    /// after that is a step of its own to take back.
    pub fn parts(&mut self) -> Option<(&Document, &mut View)> {
        self.history.rest();
        match &self.shown {
            Shown::Text(text) => Some((text, &mut self.view)),
            _ => None,
        }
    }

    /// Takes what was read of the file. A text that was shown before
    /// keeps its place as far as the new one reaches. `rows` is how many
    /// lines the pane shows.
    pub fn read(&mut self, content: Result<Content, String>, rows: usize) {
        // A text that was changed here is not replaced by what the disk
        // holds: whoever typed it would lose it without a word. Whether
        // it was changed is asked now, when the answer is here, not when
        // the file was asked for. The line at the bottom says that the
        // disk holds another text, and what the disk holds is taken when
        // the user said so.
        if self.changed() {
            match content {
                Ok(content) if self.asked_for() => self.take(content, rows),
                // A question about leaving stands, and its keys are
                // those that are taken: the save will say what the disk
                // holds.
                Ok(_) if matches!(self.said, Some(Said::Leave | Said::Writing)) => {}
                // Another text than the one that was read, and than the
                // one a save has handed over whose answer is not here
                // yet: until it is, the bytes of the own save are not
                // another program's. They are found like the text that
                // was read: nothing waits any more, and no line says so.
                Ok(content) if content.digest != self.digest && !self.wrote_as(&content) => {
                    // What waits to be taken waits only while it is what
                    // the disk holds.
                    let waits = self.said == Some(Said::NoWayBack)
                        && matches!(&self.theirs, Theirs::Kept(theirs) if theirs.digest == content.digest);
                    if !waits {
                        self.say(Some(Said::Other));
                    }
                }
                Ok(_) if matches!(self.said, Some(Said::Other | Said::NoWayBack)) => self.say(None),
                Err(why) if self.asked_for() => self.say(Some(Said::Failed(why))),
                _ => {}
            }
            return;
        }
        // Nothing was changed in the text that is there: no reading is
        // to be taken over it any more.
        self.asked_for();
        let again = self.text().is_some();
        // The bytes the text is of, in a file whose state is another: it
        // was touched, or written with what it held, or this is the
        // answer to a look at the file after the own save. The text
        // stays, with what can be taken back in it.
        if let Ok(content) = &content
            && again
            && content.digest.is_some()
            && content.digest == self.digest
        {
            if self.stamp != Some(content.stamp) {
                self.marks = git::Marks::default();
            }
            self.stamp = Some(content.stamp);
            return;
        }
        match content {
            // A picture that is shown does not give way to what is no
            // picture while its file was written just now: a program
            // that writes it anew empties it first, and a look at it
            // then finds a text of nothing. The next occasion looks
            // again.
            Ok(Content { stamp, body, .. })
                if stamp.fresh()
                    && !matches!(body, Body::Picture(_))
                    && self.picture().is_some_and(|seen| seen.shown.is_some()) =>
            {
                self.stamp = None;
            }
            Ok(Content { stamp, digest, body }) => {
                self.digest = digest;
                (self.led, self.mode) = (stamp.led(), Some(stamp.mode()));
                // The marks are those of the state they were read for.
                // Over another text they would stand at wrong lines
                // until git has answered, so that text has none until then.
                if self.stamp != Some(stamp) {
                    self.marks = git::Marks::default();
                }
                self.stamp = Some(stamp);
                // Also where the bytes are the same. And what was changed
                // in the text before would lie over this one.
                self.replaced();
                self.history.forget();
                // The colors above the first line that is another stay,
                // so that a file an agent writes to does not lose all of
                // them at every reading. None stay where the text before
                // was no text, or this one is too large for colors: no
                // answer would come to take their place.
                let same = match (&self.shown, &body) {
                    (Shown::Text(before), Body::Text(text)) if text.len() <= FILE_MAX => {
                        before.same_lines(text)
                    }
                    _ => 0,
                };
                self.colors.changed(same);
                // And what stood against a preview of the text before
                // may not stand against this one.
                self.preview.again(match &body {
                    Body::Text(text) => text.len(),
                    _ => 0,
                });
                self.shown = match (body, std::mem::replace(&mut self.shown, Shown::Reading)) {
                    (Body::Text(text), _) => Shown::Text(text),
                    // The picture that is shown stays until the file is
                    // decoded anew: one that an agent is writing is half
                    // a picture.
                    (Body::Picture(format), Shown::Picture(seen)) if seen.format == format => {
                        Shown::Picture(Seen { stale: true, ..seen })
                    }
                    (Body::Picture(format), _) => Shown::Picture(Seen::new(format)),
                    (Body::Binary(format), _) => Shown::Other(format),
                };
            }
            // A file that is gone while its text or its picture is shown:
            // what was read stays, and it is asked for again at the next
            // occasion.
            Err(_) if again || self.picture().is_some_and(|seen| seen.shown.is_some()) => self.stamp = None,
            Err(why) => {
                (self.shown, self.stamp) = (Shown::Failed(why), None);
                self.replaced();
                self.history.forget();
            }
        }
        match &self.shown {
            Shown::Text(text) if again => self.view.fit(text, rows),
            _ => (self.view, self.marks) = (View::default(), git::Marks::default()),
        }
    }
}

/// What the tests of a file in a pane and of the workspace share.
#[cfg(test)]
pub(super) mod testing {
    use super::*;

    /// What was read of a file, as a worker hands it over.
    pub fn content(name: &str, text: &str) -> Content {
        use crate::core::files::{digest, stamp_of};
        use std::sync::atomic::{AtomicU64, Ordering};
        // A file for every call: the tests run side by side in one
        // process, and many of them ask under the same name.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nth = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("codn-ws-{name}-{}-{nth}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        let stamp = stamp_of(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let body = Body::Text(Document::new(text.as_bytes().to_vec(), false).ok().unwrap());
        Content { stamp, digest: Some(digest(text.as_bytes())), body }
    }

    pub fn shown(view: &FileView) -> String {
        String::from_utf8(view.text().unwrap().bytes().to_vec()).unwrap()
    }

    /// A file that is no text, as a worker hands it over.
    pub fn binary(digest: u64) -> Content {
        Content { body: Body::Binary(None), digest: Some(digest), ..content("binary", "x") }
    }

    /// A file just opened in a pane, nothing read of it yet.
    pub fn opened() -> FileView {
        FileView::new(ProjectId(1), "a.rs".into(), 1, None)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn a_file_of_markdown_is_shown_as_it_is_meant_and_keeps_its_line_through_the_switch() {
        use crate::core::preview::{Entry, Key, Meant, Sizes, Type, What, Word};
        let mut view = FileView::new(ProjectId(1), "a.md".into(), 1, Some("Markdown"));
        assert!(!view.previews() && !view.switches(), "nothing is read of it yet");
        let text = "one\n\ntwo\n\nthree\n\nfour\n";
        view.read(Ok(content("meant", text)), 2);
        assert!(view.previews() && view.switches() && view.unpreviewed().is_none());
        // Four paragraphs, each a row of 20 with 11 above it.
        let meant = || {
            let paragraph = |word: &str| {
                let (from, to) =
                    (text.find(word).unwrap() as u32, (text.find(word).unwrap() + word.len()) as u32);
                [
                    Entry { what: What::Paragraph, from, to },
                    Entry { what: What::Text(Word::Here(from, to)), from, to },
                    Entry { what: What::End, from, to },
                ]
            };
            let entries = ["one", "two", "three", "four"].into_iter().flat_map(paragraph).collect();
            Meant::new(text.as_bytes().to_vec(), entries).unwrap()
        };
        let kind = Type { px: 10.0, h: 20.0, base: 15.0 };
        let sizes = Sizes { body: kind, heads: [kind; 6], code: kind, cell: 10.0 };
        let key = Key { width: 400.0, sizes, fonts: 0 };
        let made = |view: &mut FileView| {
            let of = view.edition();
            assert!(view.preview.wants(of));
            view.preview.asked(of);
            assert!(view.preview.take(of, meant()));
            view.preview.lay(key, &mut |_, _, px| px);
        };
        made(&mut view);
        // Before there are rows, and in the text, the lines that are
        // seen are those of the text.
        assert_eq!(view.seen(2, 40.0), (0, 1));
        assert!(view.preview.scroll_by(2, 40.0));
        assert_eq!(view.seen(2, 40.0), (4, 1), "the third paragraph, which is the fifth line");
        // To the text: it stands at that line.
        view.switch_preview(2);
        assert!(!view.previews() && view.switches() && view.unpreviewed().is_none());
        assert_eq!((view.view().top, view.seen(2, 40.0)), (4, (4, 2)));
        // The caret, which stood at the first line, came along. One
        // that is seen there stays where it is.
        assert_eq!(view.view().caret, Pos { line: 4, byte: 0 });
        // Moved there, and back at once, while the rows are still held.
        view.view.top = 2;
        view.switch_preview(2);
        assert!(view.previews() && view.preview.line() == Some(2));
        // And back after what was held for the preview was given up.
        view.switch_preview(2);
        view.preview.rest();
        view.view.top = 6;
        view.switch_preview(2);
        made(&mut view);
        assert_eq!(view.preview.line(), Some(6));
        // Another text is asked for anew, and the rows of the one
        // before stay until its own are made.
        view.read(Ok(content("meant-2", "one\n")), 2);
        assert!(view.previews() && view.preview.wants(view.edition()) && view.preview.made().is_some());
        // A text that has no preview is shown as its text, and stays
        // that while it is typed into: a key does not ask again, and
        // nothing takes the text out from under what is typed.
        let of = view.edition();
        view.preview.asked(of);
        assert!(view.preview.fail(of, Why::Large));
        assert!(!view.previews() && view.unpreviewed() == Some(Why::Large));
        assert!(view.edit(Edit::Type("x"), Instant::now()));
        assert!(view.edition() != of && !view.previews());
        // What is read of the file after that is asked for again, and
        // the text stays shown until an answer says otherwise.
        view.history.forget();
        view.read(Ok(content("meant-3", "one\n\ntwo\n\nthree\n\nfour\n\nfive\n")), 2);
        assert!(!view.previews() && view.tries_preview());
        made(&mut view);
        assert!(view.previews() && !view.tries_preview());
        // A preview that stood further down gives way to the text at that
        // line, not at the line the text was left at before.
        view.preview.scroll_by(3, 40.0);
        view.view.top = 0;
        assert_eq!(view.preview.line(), Some(6));
        let of = view.edition();
        view.preview.asked(Edition { change: of.change + 1, ..of });
        assert!(!view.no_preview(of, Why::Ended, 2), "not the text that was asked for");
        assert!(view.no_preview(Edition { change: of.change + 1, ..of }, Why::Ended, 2));
        assert!(!view.previews() && view.view().top == 6 && view.view().caret.line == 6);
        // What is no text has no preview and no switch.
        view.read(Ok(binary(7)), 2);
        assert!(!view.previews() && !view.switches());
        // Nor has a file that is none of Markdown.
        let mut other = opened();
        other.read(Ok(content("other", text)), 2);
        assert!(!other.previews() && !other.switches() && other.unpreviewed().is_none());
    }

    #[test]
    fn a_text_too_long_for_its_preview_does_not_give_way_to_it_under_a_key() {
        let mut view = FileView::new(ProjectId(1), "a.md".into(), 1, Some("Markdown"));
        // Two bytes more than a preview is made of.
        let long = "x\n".repeat(FILE_MAX / 2 + 1);
        view.read(Ok(content("long", &long)), 20);
        assert!(!view.previews() && view.unpreviewed() == Some(Why::Long));
        // A key takes two signs out: the text is short enough, and it
        // stays the text that is typed in, with what is not saved.
        let now = Instant::now();
        assert!(
            view.edit(Edit::Forward { word: false }, now) && view.edit(Edit::Forward { word: false }, now)
        );
        assert_eq!(view.text().map(Document::len), Some(FILE_MAX));
        assert!(view.changed() && !view.previews() && !view.tries_preview());
        assert_eq!(view.unpreviewed(), Some(Why::Long));
        // The switch asks for it as it is meant.
        view.switch_preview(20);
        assert!(view.previews() && view.unpreviewed().is_none());
        // And a text that is read shorter is one that has a preview.
        let mut view = FileView::new(ProjectId(1), "a.md".into(), 1, Some("Markdown"));
        view.read(Ok(content("long", &long)), 20);
        view.read(Ok(content("long", "short\n")), 20);
        assert!(view.previews());
        // What the disk holds, taken on the user's word, is held
        // against the length as well.
        view.edit(Edit::Type("mine"), now);
        assert!(view.take_theirs(20));
        view.read(Ok(content("long", &long)), 20);
        assert!(!view.changed() && view.unpreviewed() == Some(Why::Long));
        view.edit(Edit::Forward { word: false }, now);
        view.edit(Edit::Forward { word: false }, now);
        assert!(!view.previews() && view.unpreviewed() == Some(Why::Long));
        // Switched to its text, a short one typed in until it is too
        // long, and switched back: held from then on as well.
        let mut view = FileView::new(ProjectId(1), "a.md".into(), 1, Some("Markdown"));
        view.read(Ok(content("long", "short\n")), 20);
        view.switch_preview(20);
        assert!(!view.previews() && view.edit(Edit::Paste(&long), now));
        view.switch_preview(20);
        assert_eq!(view.unpreviewed(), Some(Why::Long));
        for _ in 0..8 {
            view.edit(Edit::Back { word: false }, now);
        }
        assert!(view.text().is_some_and(|text| text.len() <= FILE_MAX));
        assert!(!view.previews() && view.unpreviewed() == Some(Why::Long));
    }

    #[test]
    fn a_picture_that_is_shown_stays_while_its_file_is_read_again() {
        use crate::core::picture::{Decoded, Format, Notes};
        let picture =
            |name: &str| Content { body: Body::Picture(Format::Png), digest: None, ..content(name, name) };
        let mut view = opened();
        view.read(Ok(picture("one")), 20);
        // Nothing was made of it yet: it is to be decoded.
        let seen = view.picture().unwrap();
        assert!(seen.stale && seen.shown.is_none() && seen.asked.is_none());
        assert!(view.text().is_none() && view.coloring().is_none());

        // The first answer came. The file is in another state: the
        // picture stays, and is to be decoded anew.
        let decoded = Decoded::whole((4, 4), (4, 4), Notes(0));
        *view.picture_mut().unwrap() =
            Seen { shown: Some((decoded, 7)), stale: false, ..Seen::new(Format::Png) };
        view.read(Ok(picture("two")), 20);
        let seen = view.picture().unwrap();
        assert!(seen.stale && seen.shown == Some((decoded, 7)));
        // The file is gone: what is shown stays, and the next occasion
        // asks again.
        view.read(Err("It is not there any more.".into()), 20);
        assert!(view.picture().is_some_and(|seen| seen.shown.is_some()) && view.stamp().is_none());
        // It was being written: its state is forgotten for the same.
        view.read(Ok(picture("three")), 20);
        assert!(view.stamp().is_some());
        view.unsettled();
        assert!(view.stamp().is_none() && view.picture().is_some());

        // A program that writes the file anew empties it first, and a look
        // at it then finds no picture, but a text of nothing. The picture
        // stays while the file was written just now, and the next occasion
        // looks again.
        let empty = content("empty", "");
        assert!(empty.stamp.fresh());
        view.read(Ok(empty), 20);
        assert!(view.picture().is_some_and(|seen| seen.shown.is_some()) && view.stamp().is_none());
        // A file that stood for a while as what is no picture is that.
        let old = {
            let path = std::env::temp_dir().join(format!("codn-ws-old-{}", std::process::id()));
            std::fs::write(&path, "text\n").unwrap();
            let before = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(before).unwrap();
            let stamp = crate::core::files::stamp_of(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            Content { stamp, ..content("old", "text\n") }
        };
        assert!(!old.stamp.fresh());
        let mut stood = opened();
        stood.read(Ok(picture("six")), 20);
        *stood.picture_mut().unwrap() =
            Seen { shown: Some((decoded, 7)), stale: false, ..Seen::new(Format::Png) };
        stood.read(Ok(old), 20);
        assert!(stood.picture().is_none() && shown(&stood) == "text\n");

        // A picture of which nothing is shown gives way to why its file
        // cannot be read, and any picture to a text.
        let mut bare = opened();
        bare.read(Ok(picture("four")), 20);
        bare.read(Err("It may not be read.".into()), 20);
        assert!(matches!(bare.shown(), Shown::Failed(_)));
        let mut bare = opened();
        bare.read(Ok(picture("five")), 20);
        bare.read(Ok(content("five", "text\n")), 20);
        assert!(bare.picture().is_none() && shown(&bare) == "text\n");
    }

    #[test]
    fn marks_of_another_state_have_the_file_read_once_and_then_wait() {
        use crate::core::files::{Body, Content, stamp_of};
        let dir = std::env::temp_dir().join(format!("codn-marks-chase-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "one\n").unwrap();
        std::fs::write(dir.join("b"), "one\ntwo\n").unwrap();
        let (shown, other) = (stamp_of(&dir.join("a")).unwrap(), stamp_of(&dir.join("b")).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        let text = || Body::Text(Document::new(b"one\n".to_vec(), false).ok().unwrap());
        let some = || Some(git::Marks { changed: vec![(0, 1)], ..Default::default() });

        let mut view = opened();
        let view = &mut view;
        // Nothing was read yet: there is no state to hold marks against.
        assert_eq!(view.take_marks(view.edition(), Some(shown), some()), Taken::Left);
        view.read(Ok(Content { stamp: shown, digest: None, body: text() }), 20);
        assert_eq!(view.take_marks(view.edition(), Some(shown), some()), Taken::Marked(true));
        assert_eq!(view.take_marks(view.edition(), Some(shown), some()), Taken::Marked(false));

        // Git saw another state, or one that moved while it looked: the
        // file is read again, once. A file that is written all the time
        // would otherwise be read and asked about without end.
        assert_eq!(view.take_marks(view.edition(), Some(other), some()), Taken::Reread);
        assert_eq!(view.take_marks(view.edition(), Some(other), some()), Taken::Left);
        assert_eq!(view.take_marks(view.edition(), None, None), Taken::Left);
        // The next occasion allows it once more, and so do marks that fit.
        view.occasion();
        assert_eq!(view.take_marks(view.edition(), None, None), Taken::Reread);
        assert_eq!(view.take_marks(view.edition(), Some(shown), some()), Taken::Marked(false));
        assert_eq!(view.take_marks(view.edition(), Some(other), None), Taken::Reread);

        // The file is gone while its text is shown: nothing is asked of
        // anyone on account of marks, whatever git answers.
        view.occasion();
        view.read(Err("It is not there any more.".into()), 20);
        assert!(view.text().is_some() && view.stamp.is_none());
        assert_eq!(view.take_marks(view.edition(), None, None), Taken::Left);
        assert_eq!(view.take_marks(view.edition(), Some(shown), some()), Taken::Left);
        assert_eq!(view.take_marks(view.edition(), None, None), Taken::Left);
    }

    #[test]
    fn an_answer_is_for_one_opening_of_a_file_and_one_text_of_it() {
        use crate::core::files::{Body, Content, stamp_of};
        let dir = std::env::temp_dir().join(format!("codn-edition-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "one\n").unwrap();
        let stamp = stamp_of(&dir.join("a")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let text = || {
            let body = Body::Text(Document::new(b"one\n".to_vec(), false).ok().unwrap());
            Ok(Content { stamp, digest: None, body })
        };
        let some = || Some(git::Marks { changed: vec![(0, 1)], ..Default::default() });

        // Another opening of the same file before this one: a path and a
        // state do not tell the two apart, their numbers do.
        let first = opened().edition();
        let mut view = FileView::new(ProjectId(1), "a.rs".into(), 2, None);
        let view = &mut view;
        let second = view.edition();

        // Every text put in the place of another is a change, also the
        // same bytes in the same state, and a file that cannot be read
        // where none was shown. The number only grows.
        view.read(text(), 20);
        let read = view.edition();
        assert_eq!((read.opening, read.change > second.change), (second.opening, true));
        // Marks asked for before the text was there, or for the first
        // opening, are not those of this text, whatever state they name.
        assert_eq!(view.take_marks(second, Some(stamp), some()), Taken::Left);
        assert_eq!(view.take_marks(first, Some(stamp), some()), Taken::Left);
        assert_eq!(view.marks, git::Marks::default());
        assert_eq!(view.take_marks(read, Some(stamp), some()), Taken::Marked(true));
        view.read(text(), 20);
        assert!(view.edition().change > read.change);
        // A text that stays because its file is gone is the text it was.
        let kept = view.edition();
        view.read(Err("It is not there any more.".into()), 20);
        assert_eq!(view.edition(), kept);
        let mut view = FileView::new(ProjectId(1), "c.rs".into(), 3, None);
        let reading = view.edition();
        view.read(Err("It may not be read.".into()), 20);
        assert!(view.edition().change > reading.change);
    }

    #[test]
    fn colors_stay_only_over_lines_that_stayed_in_a_text_that_can_have_them() {
        use crate::core::colors::Kind;
        use crate::core::files::{Body, Content, stamp_of};
        let dir = std::env::temp_dir().join(format!("codn-colors-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "one\n").unwrap();
        let stamp = stamp_of(&dir.join("a")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let text = |text: &str| {
            Ok(Content {
                stamp,
                digest: None,
                body: Body::Text(Document::new(text.as_bytes().to_vec(), false).ok().unwrap()),
            })
        };
        let colored = |view: &mut FileView| {
            let FileView { shown: Shown::Text(doc), colors, .. } = view else { panic!("text") };
            let lines = (0..doc.lines() - 1).map(|_| vec![(2, Kind::Keyword)].into_boxed_slice()).collect();
            assert!(colors.take(doc, 0, lines, None));
        };
        let mut view = opened();
        let view = &mut view;
        view.read(text("fn a\nfn b\nfn c\n"), 20);
        colored(view);
        // A line in the middle is another: what is above it keeps its colors.
        view.read(text("fn a\nlet\nfn c\n"), 20);
        assert_eq!(
            (view.colors.line(0).len(), view.colors.line(1).len(), view.colors.line(2).len()),
            (1, 0, 0)
        );
        colored(view);
        // What is no text comes in its place, and then another text of
        // as many lines: nothing of the first text's colors is on it.
        view.read(Ok(Content { stamp, digest: None, body: Body::Binary(None) }), 20);
        view.read(text("xx a\nxx b\nxx c\n"), 20);
        assert_eq!((0..3).map(|line| view.colors.line(line).len()).sum::<usize>(), 0);
        colored(view);
        // The file grows over what is shown in colors: it has one color,
        // all of it, and gets its colors back when it is small again.
        let large = format!("xx a\nxx b\nxx c\n{}", "x".repeat(FILE_MAX));
        view.read(text(&large), 20);
        assert_eq!((0..3).map(|line| view.colors.line(line).len()).sum::<usize>(), 0);
        assert!(view.colors.grammar(view.text().unwrap()).is_none());
        view.read(text("xx a\nxx b\nxx c\n"), 20);
        assert_eq!(view.colors.wanted(0, 20, 4), Some((0, 4)));
        // A text that grows over it by a key has one color as well.
        // Nothing is asked for such a text, so no answer would come to
        // take away what was known: the line that was typed into kept
        // the colors of the line it had been, and those below theirs.
        let first = |view: &mut FileView, count: usize| {
            let FileView { shown: Shown::Text(doc), colors, .. } = view else { panic!("text") };
            let lines = (0..count).map(|_| vec![(2, Kind::Keyword)].into_boxed_slice()).collect();
            assert!(colors.take(doc, 0, lines, None));
        };
        let known = |view: &FileView| (0..3).map(|line| view.colors.line(line).len()).sum::<usize>();
        let near = format!("fn a\nfn b\nfn c\n{}", "x\n".repeat((FILE_MAX - 15) / 2));
        assert_eq!(near.len(), FILE_MAX - 1);
        let now = Instant::now();
        view.read(text(&near), 20);
        first(view, 3);
        assert!(view.edit(Edit::Type("l"), now) && known(view) == 3, "it stays while the text has colors");
        assert!(view.edit(Edit::Type("e"), now) && view.text().unwrap().len() > FILE_MAX);
        assert_eq!(known(view), 0);
        // And so has one that is taken from the disk on the user's word.
        // What is typed stands below the lines whose colors are known:
        // those lines are the same in what is taken.
        let mut view = opened();
        let view = &mut view;
        view.read(text("fn a\nfn b\nfn c\n"), 20);
        first(view, 3);
        let (doc, caret) = view.parts().unwrap();
        caret.go(doc, crate::core::text::Move::Bottom, false);
        assert!(view.edit(Edit::Type("mine"), now) && known(view) == 3);
        assert!(view.take_theirs(20));
        view.read(text(&format!("{near}xx\n")), 20);
        assert!(!view.changed() && view.text().unwrap().len() > FILE_MAX);
        assert_eq!(known(view), 0);
    }

    #[test]
    fn the_marks_of_a_file_are_those_of_the_text_that_is_shown() {
        use crate::core::files::{Body, Content, stamp_of};
        let dir = std::env::temp_dir().join(format!("codn-marks-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "one\n").unwrap();
        std::fs::write(dir.join("b"), "zero\none\n").unwrap();
        let (first, second) = (stamp_of(&dir.join("a")).unwrap(), stamp_of(&dir.join("b")).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        let text = |stamp, text: &str| {
            let body = Body::Text(Document::new(text.as_bytes().to_vec(), false).ok().unwrap());
            Ok(Content { stamp, digest: None, body })
        };
        let marked = || git::Marks { changed: vec![(0, 1)], ..Default::default() };
        let mut view = opened();
        let view = &mut view;
        view.read(text(first, "one\n"), 20);
        view.marks = marked();
        // The same state read once more keeps them.
        view.read(text(first, "one\n"), 20);
        assert_eq!(view.marks, marked());
        // A line was put in above: the marks would stand a line too
        // high until git has answered, so the new text has none till then.
        view.read(text(second, "zero\none\n"), 20);
        assert_eq!(view.marks, git::Marks::default());
        // A file that cannot be read keeps its text, and the marks of it.
        view.marks = marked();
        view.read(Err("It is not there any more.".into()), 20);
        assert_eq!(view.marks, marked());
        assert!(opened().marks == git::Marks::default(), "a file just opened has none");
    }

    #[test]
    fn a_save_is_held_against_where_the_name_led_when_the_text_was_read() {
        let mut view = opened();
        let read = content("led", "one\n");
        view.read(Ok(Content { stamp: read.stamp.leading(7), ..read }), 20);
        // A look at the file finds the bytes it had under a name that
        // leads elsewhere by now: the text stays, and what a save is
        // held against is still where it came from. Before a key was
        // typed, and after.
        let elsewhere = |view: &mut FileView| {
            let same = content("led-elsewhere", "one\n");
            view.read(Ok(Content { stamp: same.stamp.leading(9), ..same }), 20);
        };
        elsewhere(&mut view);
        view.edit(Edit::Type("x"), Instant::now());
        elsewhere(&mut view);
        let (_, job) = view.job(Path::new("/p/trail"), false).unwrap();
        assert_eq!((job.led, job.mode.is_some()), (7, true));
        // And other bytes there: they wait, and are not where it came from.
        let other = content("led-other", "two\n");
        view.read(Ok(Content { stamp: other.stamp.leading(9), ..other }), 20);
        assert_eq!(view.job(Path::new("/p/trail"), true).unwrap().1.led, 7);
    }

    #[test]
    fn a_save_is_answered_for_the_state_that_was_written() {
        let mut view = opened();
        let view = &mut view;
        // A file that is still being read has nothing to write.
        assert!(view.job(Path::new("/p/trail"), false).is_none());
        let read = content("save", "one\n");
        let held = read.digest;
        view.read(Ok(read), 20);
        let now = Instant::now();
        view.edit(Edit::Type("x"), now);
        let (of, job) = view.job(Path::new("/p/trail"), false).unwrap();
        assert_eq!(
            (&job.path, &job.bytes[..], job.known),
            (&PathBuf::from("/p/trail/a.rs"), &b"xone\n"[..], held)
        );
        assert!(!job.force && !job.in_place);
        // Typed on while the thread wrote: the file holds what was
        // written, and the text is still another.
        view.edit(Edit::Type("y"), now);
        let written = Outcome::Saved { stamp: None, digest: 7, flushed: true };
        assert_eq!(view.saved(of, written), Some(false));
        assert!(view.changed() && view.said.is_none());
        // Taken back to what was written, it is the saved one.
        view.edit(Edit::Undo, now);
        assert!(!view.changed());
        // The next save names what the file holds now.
        view.edit(Edit::Type("z"), now);
        let (of, job) = view.job(Path::new("/p/trail"), false).unwrap();
        assert_eq!(job.known, Some(7));
        // What a save says when it wrote nothing, and what it leaves.
        assert_eq!(view.saved(of, Outcome::Other), Some(false));
        assert_eq!(view.said, Some(Said::Other));
        assert_eq!(view.saved(of, Outcome::InPlace), Some(false));
        assert_eq!(view.said, Some(Said::InPlace));
        assert!(view.job(Path::new("/p/trail"), false).unwrap().1.in_place, "told once, it is written");
        assert_eq!(view.saved(of, Outcome::Failed("It may not be written.".into())), Some(false));
        assert_eq!(view.said, Some(Said::Failed("Not saved. It may not be written.".into())));
        // Written, and the disk did not confirm it: saved, and said.
        assert_eq!(view.saved(of, Outcome::Saved { stamp: None, digest: 8, flushed: false }), Some(true));
        assert_eq!(view.said, Some(Said::Unflushed));
        // Half written: no state of the text is the one on the disk, also
        // not the one that was saved before.
        view.edit(Edit::Type("!"), now);
        let (of, _) = view.job(Path::new("/p/trail"), false).unwrap();
        assert_eq!(view.saved(of, Outcome::Damaged("The disk is gone.".into())), Some(false));
        view.edit(Edit::Undo, now);
        assert!(view.changed() && view.job(Path::new("/p/trail"), false).unwrap().1.known.is_none());
        // An answer for another opening of the file is not for this one.
        assert_eq!(
            view.saved(
                save::Of { opening: of.opening + 1, ..of },
                Outcome::Saved { stamp: None, digest: 9, flushed: true }
            ),
            None
        );
        assert!(view.changed());
    }

    #[test]
    fn what_the_disk_holds_does_not_take_the_place_of_a_changed_text_unasked() {
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("theirs", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("mine "), now);
        let marked = Some(git::Marks { changed: vec![(0, 1)], ..Default::default() });
        // The file was written by another: the text stays, the line at
        // the bottom says so, and marks of the file are not taken.
        view.read(Ok(content("theirs", "two\nthree\n")), 20);
        assert_eq!((shown(view).as_str(), &view.said), ("mine one\n", &Some(Said::Other)));
        assert_eq!(view.take_marks(view.edition(), view.stamp, marked), Taken::Left);
        // Back as it was read: nothing to say any more.
        view.read(Ok(content("theirs", "one\n")), 20);
        assert_eq!(view.said, None);
        // Taken on the user's word, as a step that is taken back.
        view.read(Ok(content("theirs", "two\nthree\n")), 20);
        assert!(view.take_theirs(20), "it is to be read for that");
        view.read(Ok(content("theirs", "two\nthree\n")), 20);
        assert_eq!((shown(view).as_str(), view.changed(), &view.said), ("two\nthree\n", false, &None));
        view.edit(Edit::Undo, now);
        assert_eq!((shown(view).as_str(), view.changed()), ("mine one\n", true));
        // What cannot be a step waits for a second word, and "stay"
        // leaves the text and drops what waited.
        let long = "x".repeat(crate::core::text::ROW_BYTES + 1);
        assert!(view.take_theirs(20));
        view.read(Ok(content("theirs", &long)), 20);
        assert_eq!((shown(view).as_str(), &view.said), ("mine one\n", &Some(Said::NoWayBack)));
        view.stay();
        assert!(view.take_theirs(20), "nothing is kept from before");
        view.read(Ok(content("theirs", &long)), 20);
        assert!(!view.take_theirs(20), "taken now");
        assert!(shown(view) == long && !view.changed() && view.said.is_none());
        assert!(!view.edit(Edit::Undo, now), "and with no way back");
        // A file that could not be read to take it says so.
        view.read(Ok(content("theirs", "short\n")), 20);
        view.edit(Edit::Type("x"), now);
        assert!(view.take_theirs(20));
        view.read(Err("It is not there any more.".into()), 20);
        assert_eq!(view.said, Some(Said::Failed("It is not there any more.".into())));
        assert_eq!(shown(view), "shortx\n");
    }

    #[test]
    fn what_waited_to_be_taken_is_taken_for_its_own_line_only() {
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("waited", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("mine "), now);
        // The disk holds what is no text: taking it cannot be a step, so
        // it waits for a second word.
        assert!(view.take_theirs(20));
        view.read(Ok(binary(1)), 20);
        assert_eq!(view.said(), Some(&Said::NoWayBack));
        // Looked at again and still the same on the disk: it waits on.
        view.read(Ok(binary(1)), 20);
        assert_eq!(view.said(), Some(&Said::NoWayBack));
        // Typed on, the line goes, and what waited goes with it. Then an
        // agent writes a text, and the line offers that one.
        view.edit(Edit::Type("more "), now);
        assert_eq!(view.said(), None);
        view.read(Ok(content("waited", "two\n")), 20);
        assert_eq!(view.said(), Some(&Said::Other));
        assert!(view.take_theirs(20), "what is there now is read, what waited before is not taken");
        assert_eq!(shown(view), "mine more one\n");
        let before = view.reading();
        view.read(Ok(content("waited", "two\n")), 20);
        assert_eq!((shown(view).as_str(), view.changed()), ("two\n", false));
        // Another text, read anew: what was pasted for the one before
        // is not for this one.
        assert_ne!(view.reading(), before);
        assert!(view.edit(Edit::Undo, now) && shown(view) == "mine more one\n", "and it is a step");
        // While it waits, another state of the disk takes its place.
        assert!(view.take_theirs(20));
        view.read(Ok(binary(1)), 20);
        view.read(Ok(binary(2)), 20);
        assert_eq!(view.said(), Some(&Said::Other));
        assert!(view.take_theirs(20), "the first one is not taken for the second");
        // And a disk that holds the text the file was read from again
        // leaves nothing to take.
        view.read(Ok(binary(1)), 20);
        assert_eq!(view.said(), Some(&Said::NoWayBack));
        view.read(Ok(content("waited", "two\n")), 20);
        assert_eq!(view.said(), None);
        assert!(view.take_theirs(20));
    }

    #[test]
    fn what_was_asked_of_the_disk_is_taken_once_and_not_after_the_word_to_stay() {
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("asked", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("mine "), now);
        view.read(Ok(content("asked", "two\n")), 20);
        assert_eq!(view.said(), Some(&Said::Other));
        // Asked for, and then "stay": the reading that comes is not taken.
        assert!(view.take_theirs(20));
        view.stay();
        view.read(Ok(content("asked", "two\n")), 20);
        assert_eq!((shown(view).as_str(), view.said()), ("mine one\n", Some(&Said::Other)));
        // Asked for, and another line stands when the reading comes: the
        // word was given, and a line does not take it back.
        assert!(view.take_theirs(20));
        view.say(Some(Said::Gone));
        view.read(Ok(content("asked", "two\n")), 20);
        assert_eq!((shown(view).as_str(), view.changed(), view.said()), ("two\n", false, None));
        // It was for one reading: the next is one like any other.
        view.edit(Edit::Type("mine "), now);
        let mine = shown(view);
        view.read(Ok(content("asked", "three\n")), 20);
        assert_eq!((shown(view), view.said()), (mine, Some(&Said::Other)));
        // Asked for, and the text taken back to what the file held: the
        // reading finds nothing that was changed, and the word is used up.
        assert!(view.take_theirs(20));
        assert!(view.edit(Edit::Undo, now) && !view.changed());
        view.read(Ok(content("asked", "two\n")), 20);
        view.edit(Edit::Type("x"), now);
        let mine = shown(view);
        view.read(Ok(content("asked", "three\n")), 20);
        assert_eq!((shown(view), view.said()), (mine, Some(&Said::Other)));
    }

    #[test]
    fn no_key_takes_out_of_a_file_what_a_step_back_could_not_put_in_again() {
        let mut view = opened();
        let view = &mut view;
        // A zero byte far behind the start of a file that is text.
        let file = "a".repeat(9000) + "\nb\0c\nlast\n";
        view.read(Ok(content("zero", &file)), 20);
        let now = Instant::now();
        let (doc, caret) = view.parts().unwrap();
        caret.select_line(doc, Pos { line: 1, byte: 0 });
        assert!(!view.edit(Edit::Back { word: false }, now), "its line is not taken out");
        assert_eq!(view.said(), Some(&Said::Refused(Refused::Shown)));
        assert!(shown(view) == file && !view.changed());
    }

    #[test]
    fn the_second_word_takes_what_waited_also_where_the_text_was_read_from_those_bytes() {
        use crate::core::text::{Move, ROW_BYTES};
        let mut view = opened();
        let view = &mut view;
        // A file with a line too long to be changed in: what the disk
        // holds of it cannot be taken as a step.
        let file = "x".repeat(ROW_BYTES + 1) + "\nshort\n";
        view.read(Ok(content("again", &file)), 20);
        let now = Instant::now();
        let (doc, caret) = view.parts().unwrap();
        caret.go(doc, Move::Bottom, false);
        assert!(view.edit(Edit::Type("typed"), now) && view.changed());
        // Another program writes the file, which a look at it finds,
        // and has written it back as it was when it is read to be taken.
        view.read(Ok(content("again", "another\n")), 20);
        assert_eq!(view.said(), Some(&Said::Other));
        assert!(view.take_theirs(20));
        view.read(Ok(content("again", &file)), 20);
        assert_eq!(view.said(), Some(&Said::NoWayBack));
        // The second word: what is shown is what the disk holds, and
        // not what was typed under the name of a text that is saved.
        assert!(!view.take_theirs(20), "taken now");
        assert!(shown(view) == file && !view.changed() && view.said().is_none());
        assert!(!view.edit(Edit::Undo, now), "and with no way back");
    }

    #[test]
    fn the_own_save_is_no_other_text_on_the_disk_and_no_word_outlives_it() {
        use crate::core::files::digest;
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("word", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("a"), now);
        // The text is with the thread that writes it, which is slow to
        // answer. A look at the file meanwhile finds what was written.
        let (of, job) = view.job(Path::new("/p/trail"), false).unwrap();
        view.read(Ok(content("word", "aone\n")), 20);
        assert_eq!((shown(view).as_str(), view.said()), ("aone\n", None));
        // What another program wrote since is another text, and the
        // user asks for it. Then the answer of the save is here: the
        // reading that was asked for before it is not taken, and the
        // word is gone with it.
        view.read(Ok(content("word", "theirs\n")), 20);
        assert_eq!(view.said(), Some(&Said::Other));
        let asked = view.edition();
        assert!(view.take_theirs(20));
        let written = Outcome::Saved { stamp: None, digest: digest(&job.bytes), flushed: true };
        assert_eq!(view.saved(of, written), Some(true));
        assert!(!view.takes(asked) && view.said().is_none());
        // Typed on: a reading that comes later is one like any other.
        view.edit(Edit::Type("later "), now);
        let mine = shown(view);
        view.read(Ok(content("word", "an agent wrote this\n")), 20);
        assert_eq!((shown(view), view.said()), (mine, Some(&Said::Other)));
        // A save that wrote nothing leaves its bytes no text of ours:
        // found on the disk later, they are another program's.
        let (of, _) = view.job(Path::new("/p/trail"), false).unwrap();
        assert_eq!(view.saved(of, Outcome::Failed("It may not be written.".into())), Some(false));
        let mine = shown(view);
        view.read(Ok(content("word", &mine)), 20);
        assert_eq!(view.said(), Some(&Said::Other));
        // With no save under way, a file of which it is not known what
        // it holds is another text too.
        view.say(None);
        view.read(Ok(Content { digest: None, ..content("word", "one\n") }), 20);
        assert_eq!(view.said(), Some(&Said::Other));
    }

    #[test]
    fn what_waited_to_be_taken_goes_when_the_disk_holds_the_own_save() {
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("over", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("mine "), now);
        // The disk holds what cannot be taken as a step, and it waits
        // for a second word.
        assert!(view.take_theirs(20));
        view.read(Ok(binary(1)), 20);
        assert_eq!(view.said(), Some(&Said::NoWayBack));
        // The user writes the text over it instead, and a look at the
        // file finds that text before the save has answered: what
        // waited is not what the disk holds any more, and its line goes.
        assert!(view.job(Path::new("/p/trail"), true).is_some());
        view.read(Ok(content("over", "mine one\n")), 20);
        assert_eq!(view.said(), None);
        assert!(view.take_theirs(20), "nothing waits: the word is for the next reading");
        assert_eq!(shown(view), "mine one\n");
    }

    #[test]
    fn a_reading_does_not_put_an_older_text_in_the_place_of_one_that_was_saved() {
        use crate::core::files::digest;
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("late", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("x"), now);
        // A reading was asked for, and then the text was saved.
        let asked = view.edition();
        assert!(view.takes(asked));
        let (of, job) = view.job(Path::new("/p/trail"), false).unwrap();
        let written = Outcome::Saved { stamp: None, digest: digest(&job.bytes), flushed: true };
        assert_eq!(view.saved(of, written), Some(true));
        assert!(!view.takes(asked), "its answer may hold what the file held before");
        assert!(view.takes(view.edition()));
        assert!(
            !view.takes(Edition { opening: asked.opening + 1, change: u64::MAX }),
            "nor one for another file"
        );
        // The look at the file after the save finds the bytes that were
        // written: the text stays, and what can be taken back in it.
        let (reading, edition) = (view.reading(), view.edition());
        view.read(Ok(content("late", "xone\n")), 20);
        assert_eq!((view.reading(), view.edition()), (reading, edition));
        assert!(view.stamp.is_some(), "its state is known again");
        assert!(view.edit(Edit::Undo, now) && shown(view) == "one\n");
        view.edit(Edit::Redo, now);
        // Another text on the disk takes the place of an unchanged one.
        view.read(Ok(content("late", "theirs\n")), 20);
        assert!(shown(view) == "theirs\n" && view.reading() != reading);
        assert!(!view.edit(Edit::Undo, now));
    }

    #[test]
    fn the_word_to_write_over_holds_through_the_word_about_writing_in_place() {
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("forced", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("x"), now);
        let root = Path::new("/p/trail");
        // Over whatever is there, and the file is one written in place:
        // the save the line then names writes over it all the same.
        let (of, job) = view.job(root, true).unwrap();
        assert!(job.force && !job.in_place);
        assert_eq!(view.saved(of, Outcome::InPlace), Some(false));
        let (of, job) = view.job(root, false).unwrap();
        assert!(job.force && job.in_place);
        // Not for longer than that line stands: "stay", a key, or any
        // other answer of the thread ends it.
        assert_eq!(view.saved(of, Outcome::InPlace), Some(false));
        view.stay();
        assert!(!view.job(root, false).unwrap().1.force);
        let (of, _) = view.job(root, true).unwrap();
        view.saved(of, Outcome::InPlace);
        view.edit(Edit::Type("y"), now);
        assert!(!view.job(root, false).unwrap().1.force);
        let (of, _) = view.job(root, true).unwrap();
        view.saved(of, Outcome::Other);
        assert!(!view.job(root, false).unwrap().1.force);
        // A plain save that is told about writing in place stays plain.
        let (of, _) = view.job(root, false).unwrap();
        view.saved(of, Outcome::InPlace);
        assert!(!view.job(root, false).unwrap().1.force);
    }

    #[test]
    fn a_question_about_leaving_is_not_written_over_or_typed_away() {
        let mut view = opened();
        let view = &mut view;
        view.read(Ok(content("leave", "one\n")), 20);
        let now = Instant::now();
        view.edit(Edit::Type("x"), now);
        for asks in [Said::Leave, Said::Writing] {
            view.say(Some(asks.clone()));
            // An agent writes the file while the question stands.
            view.read(Ok(content("leave", "theirs\n")), 20);
            assert_eq!(view.said(), Some(&asks));
            view.edit(Edit::Type("y"), now);
            assert_eq!(view.said(), Some(&asks));
        }
        // Any other line goes with the next key.
        view.say(Some(Said::Gone));
        view.edit(Edit::Type("z"), now);
        assert_eq!(view.said(), None);
    }
}
