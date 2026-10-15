//! Changing the text of a file: what a key asks of it, and what is kept
//! of every change to take it back.
//!
//! Every change goes through `Document::splice`, also one that is taken
//! back. What is kept is the change, never a copy of the text.

use std::time::{Duration, Instant};

use super::files::MAX_FILE;
use super::text::{self, Change, Document, Pos, Refused, View};

/// Most bytes the changes that can be taken back hold together: a text of
/// the largest size taken out, another put in, and some typing.
const HELD: usize = 3 * MAX_FILE;
/// Keys further apart than this are not one step of typing.
const PAUSE: Duration = Duration::from_millis(800);

/// What made a change, for joining it with the one before.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Typed,
    /// Taken out before the caret, and behind it.
    Back,
    Forward,
    /// A step of its own: a new line, a paste, what replaced a selection.
    Alone,
}

struct Step {
    /// Where in the bytes of the text, what went out and what went in.
    at: usize,
    out: Vec<u8>,
    put: Vec<u8>,
    /// The caret before it, and where a selection began.
    was: (Pos, Option<Pos>),
    /// The marks of the states of the text before and after.
    from: u64,
    to: u64,
    kind: Kind,
}

impl Step {
    fn held(&self) -> usize {
        self.out.len() + self.put.len()
    }
}

/// The changes made to a text, and which state of it is the saved one.
///
/// Every state of the text has a mark that is never used twice. A count
/// of changes would not do: saved at five, two taken back, two others
/// made, and the count says five again over another text.
pub struct History {
    done: Vec<Step>,
    undone: Vec<Step>,
    /// How many bytes the steps hold, and how many they may.
    held: usize,
    limit: usize,
    /// The mark of the state the text is in, and the last one given.
    mark: u64,
    given: u64,
    /// The mark of the state the file on the disk has, if it has one of
    /// them.
    saved: Option<u64>,
    /// When the last step was made, while typing may still join it.
    open: Option<Instant>,
}

/// What a step that was taken back or made again did.
struct Moved {
    lines: Lines,
    caret: Pos,
    anchor: Option<Pos>,
}

/// Which lines a change touched: the first one that is another now, and
/// how many ends of lines went out behind it and came in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lines {
    pub first: usize,
    pub out: usize,
    pub put: usize,
}

fn ends(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

impl History {
    /// For a text as it was read: that state is the saved one.
    pub fn new() -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            held: 0,
            limit: HELD,
            mark: 0,
            given: 0,
            saved: Some(0),
            open: None,
        }
    }

    /// The mark of the state the text is in.
    pub fn mark(&self) -> u64 {
        self.mark
    }

    /// Whether the text is another than the file holds.
    pub fn changed(&self) -> bool {
        self.saved != Some(self.mark)
    }

    /// The state with this mark was written to the file.
    pub fn saved(&mut self, mark: u64) {
        self.saved = Some(mark);
        self.rest();
    }

    /// No state that can be come back to is the one on the disk: the file
    /// was written by another, or a save failed half way.
    pub fn unsaved(&mut self) {
        self.saved = None;
    }

    /// Another text was read in the place of this one: what was changed
    /// in the old one would lie over the new. The new one is as saved.
    pub fn forget(&mut self) {
        let given = self.given + 1;
        *self = Self { mark: given, given, saved: Some(given), limit: self.limit, ..Self::new() };
    }

    /// The typing that follows is a step of its own: the caret was moved,
    /// the text was saved.
    pub fn rest(&mut self) {
        self.open = None;
    }

    #[cfg(test)]
    fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    fn record(&mut self, change: Change, put: &[u8], was: (Pos, Option<Pos>), kind: Kind, now: Instant) {
        // Typing over steps that were taken back begins anew.
        let fresh = self.undone.is_empty();
        self.held -= self.undone.drain(..).map(|step| step.held()).sum::<usize>();
        self.given += 1;
        let (from, to) = (self.mark, self.given);
        self.mark = to;
        // The state that was saved stays one that can be come back to.
        let soon = self.open.is_some_and(|at| now.saturating_duration_since(at) < PAUSE);
        let joins = fresh && soon && self.saved != Some(from) && kind != Kind::Alone;
        self.open = (kind != Kind::Alone).then_some(now);
        self.held += change.out.len() + put.len();
        if let Some(last) = self.done.last_mut().filter(|last| joins && last.kind == kind) {
            let joined = match kind {
                Kind::Typed if change.at == last.at + last.put.len() => {
                    last.put.extend_from_slice(put);
                    true
                }
                Kind::Back if change.at + change.out.len() == last.at => {
                    last.at = change.at;
                    last.out.splice(..0, change.out.iter().copied());
                    true
                }
                Kind::Forward if change.at == last.at => {
                    last.out.extend_from_slice(&change.out);
                    true
                }
                _ => false,
            };
            if joined {
                last.to = to;
                return self.trim();
            }
        }
        self.done.push(Step { at: change.at, out: change.out, put: put.to_vec(), was, from, to, kind });
        self.trim();
    }

    /// Keeps what is held within the limit, after every step that was
    /// kept or grew. The oldest go first. A step that is alone over the
    /// limit goes too: it cannot be taken back.
    fn trim(&mut self) {
        let mut gone = 0;
        while self.held > self.limit && gone < self.done.len() {
            self.held -= self.done[gone].held();
            gone += 1;
        }
        self.done.drain(..gone);
    }

    /// Takes the last step back. A step the text refuses stays where it
    /// is, and the refusal is said: no key makes one, and what is kept
    /// is not thrown away over it without a word.
    fn undo(&mut self, doc: &mut Document) -> Result<Option<Moved>, Refused> {
        self.rest();
        let Some(step) = self.done.pop() else { return Ok(None) };
        let change = match doc.splice(step.at, step.put.len(), &step.out) {
            Ok(change) => change,
            Err(why) => {
                self.done.push(step);
                return Err(why);
            }
        };
        self.mark = step.from;
        let lines = Lines { first: change.line, out: ends(&step.put), put: ends(&step.out) };
        let moved = Moved { lines, caret: step.was.0, anchor: step.was.1 };
        self.undone.push(step);
        Ok(Some(moved))
    }

    /// Makes the step again that was taken back last.
    fn redo(&mut self, doc: &mut Document) -> Result<Option<Moved>, Refused> {
        self.rest();
        let Some(step) = self.undone.pop() else { return Ok(None) };
        let change = match doc.splice(step.at, step.out.len(), &step.put) {
            Ok(change) => change,
            Err(why) => {
                self.undone.push(step);
                return Err(why);
            }
        };
        self.mark = step.to;
        let lines = Lines { first: change.line, out: ends(&step.out), put: ends(&step.put) };
        self.done.push(step);
        Ok(Some(Moved { lines, caret: change.end, anchor: None }))
    }
}

/// What a key asks of a text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit<'a> {
    /// Text that was typed or composed.
    Type(&'a str),
    /// Text from the clipboard: it takes the line ends of the file.
    Paste(&'a str),
    /// A new line, with the indent of the line it is made in.
    Enter,
    /// Takes out what is selected, or else what stands before the caret
    /// or behind it: a character, or with `word` up to where a word
    /// begins or ends.
    Back {
        word: bool,
    },
    Forward {
        word: bool,
    },
    /// Takes out what is selected and nothing else.
    Cut,
    Undo,
    Redo,
}

/// The end the line `line` has in the file, or for the last line, which
/// has none, that of the line before it.
fn line_end(doc: &Document, line: usize) -> &'static [u8] {
    let ended = |line: usize| {
        let bytes = doc.ended(line);
        bytes.ends_with(b"\r\n").then_some(&b"\r\n"[..]).or(bytes.ends_with(b"\n").then_some(&b"\n"[..]))
    };
    ended(line).or_else(|| line.checked_sub(1).and_then(ended)).unwrap_or(b"\n")
}

/// Text as it goes into a file: without zero bytes, which would make the
/// file binary at its next reading, and with `end` for every end of a line.
fn pasted(text: &str, end: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((byte, more)) = rest.split_first() {
        rest = more;
        match byte {
            0 => {}
            b'\r' => {
                rest = rest.strip_prefix(b"\n").unwrap_or(rest);
                bytes.extend_from_slice(end);
            }
            b'\n' => bytes.extend_from_slice(end),
            byte => bytes.push(*byte),
        }
    }
    bytes
}

/// Puts another text in the place of all of this one, as one step that
/// can be taken back: what the disk holds, taken over a text that was
/// changed here.
pub fn replace_all(
    doc: &mut Document,
    view: &mut View,
    history: &mut History,
    with: &[u8],
    now: Instant,
) -> Result<Lines, Refused> {
    let was = (view.caret, view.anchor);
    let change = doc.splice(0, doc.len(), with)?;
    let lines = Lines { first: 0, out: ends(&change.out), put: ends(with) };
    history.record(change, with, was, Kind::Alone, now);
    view.place(doc, was.0, false);
    Ok(lines)
}

/// Does to a text what a key asks. Returns the lines that are others
/// now, or nothing where nothing was changed: no step to take back, the
/// start of the text before the caret. `now` is when the key came.
pub fn apply(
    doc: &mut Document,
    view: &mut View,
    history: &mut History,
    edit: Edit,
    now: Instant,
) -> Result<Option<Lines>, Refused> {
    let caret = view.caret;
    let was = (caret, view.anchor);
    let selected = view.selection();
    let here = doc.line(caret.line);
    let at = |byte| Pos { line: caret.line, byte };
    // What goes out, what goes in, and what kind of step that is.
    let ((from, to), put, kind) = match edit {
        Edit::Undo | Edit::Redo => {
            let moved = if edit == Edit::Undo { history.undo(doc) } else { history.redo(doc) };
            let Some(moved) = moved? else { return Ok(None) };
            view.place(doc, moved.anchor.unwrap_or(moved.caret), false);
            if moved.anchor.is_some() {
                view.place(doc, moved.caret, true);
            }
            return Ok(Some(moved.lines));
        }
        Edit::Type(text) => {
            // What a key types besides text is the key's to say, not the
            // text's: a tab is text here, an escape is not.
            let put: Vec<u8> =
                text.chars().filter(|c| !c.is_control() || *c == '\t').collect::<String>().into_bytes();
            if put.is_empty() {
                return Ok(None);
            }
            let kind = if selected.is_some() { Kind::Alone } else { Kind::Typed };
            (selected.unwrap_or((caret, caret)), put, kind)
        }
        Edit::Paste(text) => {
            let put = pasted(text, line_end(doc, caret.line));
            if put.is_empty() && selected.is_none() {
                return Ok(None);
            }
            (selected.unwrap_or((caret, caret)), put, Kind::Alone)
        }
        Edit::Enter => {
            let indent = here[..caret.byte.min(here.len())].iter().take_while(|b| matches!(b, b' ' | b'\t'));
            let mut put = line_end(doc, caret.line).to_vec();
            put.extend(indent);
            (selected.unwrap_or((caret, caret)), put, Kind::Alone)
        }
        Edit::Cut | Edit::Back { .. } | Edit::Forward { .. } if selected.is_some() => {
            (selected.unwrap_or_default(), Vec::new(), Kind::Alone)
        }
        Edit::Cut => return Ok(None),
        Edit::Back { word } => {
            let from = match caret.byte {
                // The end of the line before goes, and the two are one.
                0 if caret.line == 0 => return Ok(None),
                0 => Pos { line: caret.line - 1, byte: doc.line(caret.line - 1).len() },
                byte if word => at(text::word_start(here, byte)),
                byte => at(text::prev_stop(here, byte).unwrap_or(0)),
            };
            ((from, caret), Vec::new(), Kind::Back)
        }
        Edit::Forward { word } => {
            let to = match text::next_stop(here, caret.byte) {
                Some(_) if word => at(text::word_end(here, caret.byte)),
                Some(next) => at(next),
                None if caret.line + 1 < doc.lines() && caret.byte == here.len() => {
                    Pos { line: caret.line + 1, byte: 0 }
                }
                None => return Ok(None),
            };
            ((caret, to), Vec::new(), Kind::Forward)
        }
    };
    let change = doc.replace(from, to, &put)?;
    let lines = Lines { first: change.line, out: ends(&change.out), put: ends(&put) };
    let end = change.end;
    history.record(change, &put, was, kind, now);
    view.place(doc, end, false);
    Ok(Some(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::new(text.as_bytes().to_vec(), false).ok().unwrap()
    }

    /// A text with its view and its history, and a clock that is set.
    struct Desk {
        doc: Document,
        view: View,
        history: History,
        now: Instant,
    }

    impl Desk {
        fn new(text: &str) -> Self {
            Self { doc: doc(text), view: View::default(), history: History::new(), now: Instant::now() }
        }

        fn at(mut self, line: usize, byte: usize) -> Self {
            self.view.place(&self.doc, Pos { line, byte }, false);
            self
        }

        fn key(&mut self, edit: Edit) -> Option<usize> {
            self.now += Duration::from_millis(100);
            let lines = apply(&mut self.doc, &mut self.view, &mut self.history, edit, self.now).unwrap();
            lines.map(|lines| lines.first)
        }

        fn text(&self) -> &str {
            std::str::from_utf8(self.doc.bytes()).unwrap()
        }

        fn caret(&self) -> (usize, usize) {
            (self.view.caret.line, self.view.caret.byte)
        }
    }

    #[test]
    fn keys_put_text_in_and_take_it_out() {
        let mut desk = Desk::new("one two\n\tthree\n").at(0, 3);
        assert_eq!(desk.key(Edit::Type("!")), Some(0));
        assert_eq!((desk.text(), desk.caret()), ("one! two\n\tthree\n", (0, 4)));
        // What a key types besides text does not go in, a tab does.
        assert_eq!(desk.key(Edit::Type("\u{1b}\r\u{8}")), None);
        desk.key(Edit::Type("\t"));
        assert_eq!(desk.text(), "one!\t two\n\tthree\n");
        desk.key(Edit::Back { word: false });
        desk.key(Edit::Back { word: false });
        assert_eq!((desk.text(), desk.caret()), ("one two\n\tthree\n", (0, 3)));
        desk.key(Edit::Forward { word: true });
        assert_eq!(desk.text(), "one\n\tthree\n");
        // Behind the last character the end of the line goes, before the
        // first one the end of the line above.
        assert_eq!(desk.key(Edit::Forward { word: false }), Some(0));
        assert_eq!((desk.text(), desk.caret()), ("one\tthree\n", (0, 3)));
        desk.key(Edit::Enter);
        assert_eq!((desk.text(), desk.caret()), ("one\n\tthree\n", (1, 0)));
        assert_eq!(desk.key(Edit::Back { word: true }), Some(0));
        assert_eq!((desk.text(), desk.caret()), ("one\tthree\n", (0, 3)));
        desk.key(Edit::Back { word: true });
        assert_eq!(desk.text(), "\tthree\n");
        // Nothing before the start of the text, nothing behind its end.
        assert_eq!(desk.key(Edit::Back { word: false }), None);
        let mut desk = Desk::new("a").at(0, 1);
        assert_eq!((desk.key(Edit::Forward { word: false }), desk.key(Edit::Cut)), (None, None));
        // A character goes with the marks that stand on it.
        let mut desk = Desk::new("e\u{301}x").at(0, 3);
        desk.key(Edit::Back { word: false });
        assert_eq!(desk.text(), "x");
    }

    #[test]
    fn what_is_selected_goes_when_something_is_typed_or_taken_out() {
        let mut desk = Desk::new("one two\nthree").at(0, 4);
        desk.view.place(&desk.doc, Pos { line: 1, byte: 2 }, true);
        desk.key(Edit::Type("X"));
        assert_eq!((desk.text(), desk.caret(), desk.view.selection()), ("one Xree", (0, 5), None));
        for edit in [Edit::Back { word: true }, Edit::Forward { word: false }, Edit::Cut] {
            let mut desk = Desk::new("one two\nthree").at(0, 4);
            desk.view.place(&desk.doc, Pos { line: 1, byte: 2 }, true);
            assert_eq!(desk.key(edit), Some(0));
            assert_eq!((desk.text(), desk.caret()), ("one ree", (0, 4)), "{edit:?}");
            // Taken back, it is selected as it was.
            desk.key(Edit::Undo);
            assert_eq!(desk.text(), "one two\nthree");
            assert_eq!(desk.view.selection(), Some((Pos { line: 0, byte: 4 }, Pos { line: 1, byte: 2 })));
        }
    }

    #[test]
    fn a_new_line_keeps_the_indent_and_the_line_end_of_its_line() {
        let mut desk = Desk::new("\t  let x;\r\nend\n").at(0, 9);
        assert_eq!(desk.key(Edit::Enter), Some(0));
        assert_eq!((desk.text(), desk.caret()), ("\t  let x;\r\n\t  \r\nend\n", (1, 3)));
        // In the middle of the indent only what stands before the caret.
        let mut desk = Desk::new("\t  let x;\nend").at(0, 1);
        desk.key(Edit::Enter);
        assert_eq!(desk.text(), "\t\n\t  let x;\nend");
        // The last line has no end: that of the line above, or a line feed.
        let mut desk = Desk::new("a\r\nb").at(1, 1);
        desk.key(Edit::Enter);
        assert_eq!(desk.text(), "a\r\nb\r\n");
        let mut desk = Desk::new("a").at(0, 1);
        desk.key(Edit::Enter);
        assert_eq!(desk.text(), "a\n");
    }

    #[test]
    fn what_is_pasted_takes_the_line_ends_of_the_file_and_has_no_zero_byte() {
        let mut desk = Desk::new("a\r\nb\r\n").at(1, 1);
        desk.key(Edit::Paste("1\n2\r3\r\n4\0"));
        assert_eq!((desk.text(), desk.caret()), ("a\r\nb1\r\n2\r\n3\r\n4\r\n", (4, 1)));
        let mut desk = Desk::new("a\nb").at(0, 0);
        desk.key(Edit::Paste("x\r\ny"));
        assert_eq!(desk.text(), "x\nya\nb");
        // Nothing but zero bytes is nothing.
        assert_eq!(desk.key(Edit::Paste("\0")), None);
        // One step, taken back at once.
        desk.key(Edit::Undo);
        assert_eq!((desk.text(), desk.caret()), ("a\nb", (0, 0)));
    }

    #[test]
    fn typing_is_taken_back_in_steps_and_made_again() {
        let mut desk = Desk::new("").at(0, 0);
        for letter in ["o", "n", "e"] {
            desk.key(Edit::Type(letter));
        }
        // A pause makes a new step, and so does a new line.
        desk.now += PAUSE;
        desk.key(Edit::Type(" "));
        desk.key(Edit::Type("2"));
        desk.key(Edit::Enter);
        desk.key(Edit::Type("x"));
        assert_eq!(desk.text(), "one 2\nx");
        let mut seen = Vec::new();
        while desk.key(Edit::Undo).is_some() {
            seen.push(desk.text().to_owned());
        }
        assert_eq!(seen, ["one 2\n", "one 2", "one", ""]);
        assert!(!desk.history.changed(), "back at the text that was read");
        while desk.key(Edit::Redo).is_some() {}
        assert_eq!((desk.text(), desk.caret()), ("one 2\nx", (1, 1)));
        // Typing after a step was taken back drops what could be made
        // again, and is not joined to what stands before.
        desk.key(Edit::Undo);
        desk.key(Edit::Type("y"));
        assert_eq!((desk.key(Edit::Redo), desk.text()), (None, "one 2\ny"));
        desk.key(Edit::Undo);
        assert_eq!(desk.text(), "one 2\n");
        // Taking out before the caret is one step, behind it another,
        // and a caret that was moved ends a step.
        let mut desk = Desk::new("abcdef").at(0, 3);
        desk.key(Edit::Back { word: false });
        desk.key(Edit::Back { word: false });
        desk.key(Edit::Forward { word: false });
        desk.key(Edit::Forward { word: false });
        assert_eq!(desk.text(), "af");
        desk.key(Edit::Undo);
        assert_eq!((desk.text(), desk.caret()), ("adef", (0, 1)));
        desk.key(Edit::Undo);
        assert_eq!((desk.text(), desk.caret()), ("abcdef", (0, 3)));
        let mut desk = Desk::new("").at(0, 0);
        desk.key(Edit::Type("a"));
        desk.history.rest();
        desk.key(Edit::Type("b"));
        desk.key(Edit::Undo);
        assert_eq!(desk.text(), "a");
    }

    #[test]
    fn the_saved_state_is_known_by_its_mark_and_not_by_a_count() {
        let mut desk = Desk::new("").at(0, 0);
        for letter in ["a", "b", "c"] {
            desk.key(Edit::Type(letter));
            desk.history.rest();
        }
        let saved = desk.history.mark();
        desk.history.saved(saved);
        assert!(!desk.history.changed());
        // Two taken back and two others made: as many changes, another text.
        desk.key(Edit::Undo);
        desk.key(Edit::Undo);
        desk.key(Edit::Type("x"));
        desk.history.rest();
        desk.key(Edit::Type("y"));
        assert!(desk.history.changed() && desk.history.mark() != saved);
        // Typing right after a save is not joined to what was saved:
        // taking it back leads to the saved state.
        let mut desk = Desk::new("").at(0, 0);
        desk.key(Edit::Type("a"));
        desk.history.saved(desk.history.mark());
        desk.key(Edit::Type("b"));
        desk.key(Edit::Undo);
        assert!(desk.text() == "a" && !desk.history.changed());
        // A text that was read anew has nothing to take back and is saved.
        desk.key(Edit::Redo);
        desk.history.forget();
        assert!(!desk.history.changed() && desk.key(Edit::Undo).is_none());
        // A save that failed half way leaves no state that is the saved one.
        desk.history.unsaved();
        assert!(desk.history.changed());
    }

    #[test]
    fn another_text_in_the_place_of_all_of_it_is_one_step() {
        let mut desk = Desk::new("one\ntwo\nthree").at(2, 3);
        desk.key(Edit::Type("!"));
        let lines = replace_all(&mut desk.doc, &mut desk.view, &mut desk.history, b"a\nb", desk.now).unwrap();
        assert_eq!(lines, Lines { first: 0, out: 2, put: 1 });
        // The caret stays as near to where it was as the text allows.
        assert_eq!((desk.text(), desk.caret()), ("a\nb", (1, 0)));
        desk.key(Edit::Undo);
        assert_eq!((desk.text(), desk.caret()), ("one\ntwo\nthr!ee", (2, 4)));
        // A text that cannot be changed is not put there this way.
        let long = vec![b'x'; text::ROW_BYTES + 1];
        let refused = replace_all(&mut desk.doc, &mut desk.view, &mut desk.history, &long, desk.now);
        assert_eq!((refused, desk.text()), (Err(Refused::Line), "one\ntwo\nthr!ee"));
    }

    #[test]
    fn a_line_with_an_end_of_two_bytes_is_taken_back_whole() {
        // As long as a line may be, cut with its end, and taken back.
        let first = format!("{}\r\nb", "x".repeat(text::ROW_BYTES));
        let mut desk = Desk::new(&first).at(0, 0);
        desk.view.place(&desk.doc, Pos { line: 1, byte: 0 }, true);
        assert_eq!(desk.key(Edit::Cut), Some(0));
        assert_eq!(desk.text(), "b");
        assert_eq!(desk.key(Edit::Undo), Some(0));
        assert!(desk.text() == first && !desk.history.changed());
    }

    #[test]
    fn a_step_that_cannot_be_taken_back_says_so_and_stays() {
        let mut desk = Desk::new("one").at(0, 3);
        desk.key(Edit::Type("!"));
        // The text is another than the step was made in: no key does
        // that, and nothing of what is kept is thrown away for it.
        desk.doc.splice(0, 4, b"").unwrap();
        let undone = apply(&mut desk.doc, &mut desk.view, &mut desk.history, Edit::Undo, desk.now);
        assert_eq!(undone, Err(Refused::Place));
        assert!(desk.history.can_undo() && desk.history.changed());
        desk.doc.splice(0, 0, b"one!").unwrap();
        assert_eq!((desk.key(Edit::Undo), desk.text()), (Some(0), "one"));
        // And one that cannot be made again.
        desk.doc.splice(0, 3, b"").unwrap();
        let redone = apply(&mut desk.doc, &mut desk.view, &mut desk.history, Edit::Redo, desk.now);
        assert_eq!(redone, Err(Refused::Place));
        desk.doc.splice(0, 0, b"one").unwrap();
        assert_eq!((desk.key(Edit::Redo), desk.text()), (Some(0), "one!"));
    }

    #[test]
    fn typing_that_joins_a_step_does_not_pass_the_limit() {
        let mut desk = Desk::new("").at(0, 0);
        desk.history.limit = 1000;
        desk.key(Edit::Paste(&"x".repeat(995)));
        for _ in 0..10 {
            desk.key(Edit::Type("a"));
        }
        assert!(desk.history.held <= 1000, "{}", desk.history.held);
        let held: usize = desk.history.done.iter().map(Step::held).sum();
        assert_eq!(held, desk.history.held);
    }

    #[test]
    fn what_is_kept_to_take_back_has_a_limit() {
        let mut desk = Desk::new("").at(0, 0);
        desk.history.limit = 1000;
        let block = "x".repeat(199) + "\n";
        for _ in 0..4 {
            desk.key(Edit::Paste(&block));
            desk.view.select_all(&desk.doc);
            desk.key(Edit::Cut);
        }
        assert!(desk.history.held <= 1000 && desk.history.can_undo());
        let held: usize = desk.history.done.iter().map(Step::held).sum();
        assert_eq!(held, desk.history.held);
        // The oldest went: not all of it can be taken back, and the text
        // is one that was there.
        let mut steps = 0;
        while desk.key(Edit::Undo).is_some() {
            steps += 1;
        }
        assert_eq!((steps, desk.text()), (5, &block[..]));
        assert!(desk.history.changed());
        // A step that is alone over the limit is not kept at all.
        desk.key(Edit::Paste(&block.repeat(6)));
        assert!(!desk.history.can_undo() && desk.history.held == 0);
        // What can be made again counts, and goes when something is typed.
        let mut desk = Desk::new("").at(0, 0);
        desk.key(Edit::Paste(&block));
        desk.key(Edit::Undo);
        assert_eq!(desk.history.held, 200);
        desk.key(Edit::Type("a"));
        assert_eq!(desk.history.held, 1);
        // A text of the largest size taken out and another put in.
        const { assert!(HELD >= 3 * MAX_FILE) };
    }

    #[test]
    fn rows_of_keys_at_random_are_taken_back_to_the_text_that_was_read() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut dice = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 16) as usize
        };
        let first = "fn main() {\r\n\tlet x = \"\u{e4}\u{6f22}\";\n}\n";
        for _ in 0..30 {
            let mut desk = Desk::new(first);
            let mut states = vec![desk.text().to_owned()];
            let mut saved = first.to_owned();
            for _ in 0..120 {
                // Now and then the text is saved as it is: whenever it is
                // called unchanged after that, it is the saved one.
                if dice() % 9 == 0 {
                    desk.history.saved(desk.history.mark());
                    saved = desk.text().to_owned();
                }
                assert!(desk.history.changed() || desk.text() == saved);
                let line = dice() % desk.doc.lines();
                // A place between two characters, as a click gives one.
                let here = std::str::from_utf8(desk.doc.line(line)).unwrap();
                let stops: Vec<usize> = here.char_indices().map(|(at, _)| at).chain([here.len()]).collect();
                let pos = Pos { line, byte: stops[dice() % stops.len()] };
                match dice() % 12 {
                    0 => desk.view.place(&desk.doc, pos, false),
                    1 => desk.view.place(&desk.doc, pos, true),
                    2 | 3 => drop(desk.key(Edit::Type(["a", "\u{e4}", " ", "\t"][dice() % 4]))),
                    4 => drop(desk.key(Edit::Enter)),
                    5 => drop(desk.key(Edit::Back { word: dice() % 2 == 0 })),
                    6 => drop(desk.key(Edit::Forward { word: dice() % 2 == 0 })),
                    7 => drop(desk.key(Edit::Paste("p\r\nq"))),
                    8 => drop(desk.key(Edit::Cut)),
                    9 => drop(desk.key(Edit::Undo)),
                    10 => drop(desk.key(Edit::Redo)),
                    _ => desk.now += PAUSE,
                }
                // The caret is in the text, at a place a caret can be.
                let caret = desk.view.caret;
                assert!(desk.doc.replace(caret, caret, b"").is_ok(), "{caret:?} in {:?}", desk.text());
                states.push(desk.text().to_owned());
            }
            // Every step back leads to a text that was there, and the
            // last one to the text that was read.
            while desk.key(Edit::Undo).is_some() {
                assert!(states.iter().any(|state| state == desk.text()), "{:?}", desk.text());
            }
            assert_eq!(desk.text(), first);
            assert_eq!(desk.history.changed(), saved != first);
        }
    }
}
