//! What is known of the colors of a text: for some of its lines, which
//! kind of thing each piece is. The pieces come from another process,
//! `src/syntax`, and nothing of an answer is taken before it was held
//! against the text it is for.

use super::edit::Lines;
use super::files::Edition;
use super::text::Document;

/// A file over this is shown in one color: its end would wait too long
/// for its colors.
pub const FILE_MAX: usize = 1 << 20;

/// What a piece of a line is, as far as a color tells: seven kinds, and
/// text that is none of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    Plain,
    Comment,
    String,
    /// A number, a value with a name of its own, an escape in a string.
    Constant,
    Keyword,
    Function,
    Type,
    /// What a markup names: a tag, an attribute, a property, a key.
    Name,
}

impl Kind {
    pub const ALL: [Self; 8] = [
        Self::Plain,
        Self::Comment,
        Self::String,
        Self::Constant,
        Self::Keyword,
        Self::Function,
        Self::Type,
        Self::Name,
    ];

    /// The kind a byte of a message stands for, if it stands for one.
    pub fn from_byte(byte: u8) -> Option<Self> {
        Self::ALL.get(byte as usize).copied()
    }
}

/// A piece of a line: up to this byte of the line, this kind. The pieces
/// of a line follow one another, each ending behind the one before.
pub type Piece = (u32, Kind);

/// How often the colors of a file are given up before it stays in one
/// color for as long as it is open. A text that is read after the first
/// time gets a second try: what took too long may be gone from it. Not a
/// try with every reading, and not a new count after colors that came:
/// a file whose top is cut and whose end is not, and that an agent
/// writes to, would keep a process busy at every reading.
const TRIES: u8 = 2;

/// Why the colors of a text were given up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    /// A piece of the work took longer than is waited for.
    TookTooLong,
    /// The process ended, or could not be started.
    Ended,
    /// An answer did not fit the text or the question.
    Broken,
}

/// The colors of the text a pane shows, as far as they are known: the
/// pieces of a run of lines around what is seen.
#[derive(Default)]
pub struct Colors {
    /// The grammar that shows the file, by its name. None, and the file
    /// has one color.
    grammar: Option<&'static str>,
    ended: Option<End>,
    /// How often they were given up since the file was opened.
    given_up: u8,
    /// The text the process was given last.
    sent: Option<Edition>,
    /// The lines asked for and not answered yet: from, and how many.
    asked: Option<(usize, usize)>,
    /// The first line whose pieces are known, and the pieces of it and of
    /// the lines behind it.
    from: usize,
    lines: Vec<Box<[Piece]>>,
    /// The line up to which an answer has said what there is to say:
    /// behind the pieces, or further where it had fewer than were asked
    /// for. Asking for the same again would be answered the same again.
    to: usize,
    /// The line from which on nothing can be known: a line there is too
    /// long, or no text, and what follows it depends on it.
    unknown: Option<usize>,
}

impl Colors {
    pub fn new(grammar: Option<&'static str>) -> Self {
        Self { grammar, ..Self::default() }
    }

    /// The grammar to cut the text with, unless it has none, is too large
    /// or was given up.
    pub fn grammar(&self, doc: &Document) -> Option<&'static str> {
        self.grammar.filter(|_| self.ended.is_none() && doc.len() <= FILE_MAX)
    }

    /// Why the colors were given up, if they were.
    pub fn ended(&self) -> Option<End> {
        self.ended
    }

    /// Whether the process has to be given the text `of` first.
    pub fn unsent(&self, of: Edition) -> bool {
        self.sent != Some(of)
    }

    pub fn sent(&mut self, of: Edition) {
        (self.sent, self.asked) = (Some(of), None);
    }

    /// The process does not hold the text after all: it is to be given
    /// again, and what was asked of it asked again.
    pub fn lost(&mut self) {
        (self.sent, self.asked) = (None, None);
    }

    /// The pieces of a line, none where they are not known.
    pub fn line(&self, line: usize) -> &[Piece] {
        let known = line.checked_sub(self.from).and_then(|at| self.lines.get(at));
        known.map_or(&[], |pieces| pieces)
    }

    /// Whether what can be known of these lines is known.
    fn covers(&self, from: usize, to: usize) -> bool {
        let to = self.unknown.map_or(to, |unknown| to.min(unknown));
        from >= to || (self.from <= from && to <= self.to)
    }

    /// The lines to ask for, from and how many, for a pane that shows
    /// `rows` lines from `top` of a text of `total` lines: nothing while
    /// what is seen and half a pane to either side is known or asked for,
    /// else a pane to either side, so that scrolling a little asks for
    /// nothing.
    pub fn wanted(&self, top: usize, rows: usize, total: usize) -> Option<(usize, usize)> {
        let near = (top.saturating_sub(rows / 2), (top + rows + rows / 2).min(total));
        if self.covers(near.0, near.1) {
            return None;
        }
        let (from, to) = (top.saturating_sub(rows), (top + 2 * rows).min(total));
        if self.asked.is_some_and(|(asked, count)| asked <= near.0 && near.1 <= asked + count) {
            return None;
        }
        (from < to).then_some((from, to - from))
    }

    pub fn asked(&mut self, from: usize, count: usize) {
        self.asked = Some((from, count));
    }

    /// Takes the pieces of the lines from `from` on, and the line from
    /// which on nothing is known. Every number of them is held against
    /// the text first: says false, and takes nothing, if one does not fit.
    pub fn take(
        &mut self,
        doc: &Document,
        from: usize,
        lines: Vec<Box<[Piece]>>,
        unknown: Option<usize>,
    ) -> bool {
        let fits = |(at, pieces): (usize, &[Piece])| {
            let length = doc.ended(from + at).len();
            let mut last = 0;
            pieces
                .iter()
                .all(|(until, _)| *until > std::mem::replace(&mut last, *until) && *until as usize <= length)
        };
        let within = from.checked_add(lines.len()).is_some_and(|to| to <= doc.lines());
        if !within
            || unknown.is_some_and(|line| line >= doc.lines())
            || !lines.iter().map(|line| &**line).enumerate().all(fits)
        {
            return false;
        }
        let asked = self.asked.take().filter(|(asked, _)| *asked == from);
        self.to = asked.map_or(0, |(asked, count)| asked + count).max(from + lines.len());
        (self.from, self.lines, self.unknown) = (from, lines, unknown);
        true
    }

    /// Another text is in the place of the one the pieces were of, alike
    /// in its first `same` lines. What is known of those stays, so that a
    /// file that is written to does not lose its colors above the place.
    pub fn changed(&mut self, same: usize) {
        self.lines.truncate(same.saturating_sub(self.from));
        self.to = self.to.min(same).max(self.from);
        self.unknown = self.unknown.filter(|line| *line < same);
        self.asked = None;
        // Colors that were given up over the text before are tried once
        // more with this one.
        if self.given_up < TRIES {
            self.ended = None;
        }
    }

    /// The text was changed by a key. Nothing is known for sure from the
    /// first line that is another, and all of it is asked for again. But
    /// what was known stays to be seen until the answer is there, moved
    /// with the lines: a text that lost its colors below the caret at
    /// every key and got them back a moment later would flicker.
    pub fn edited(&mut self, lines: Lines) {
        let Lines { first, out, put } = lines;
        match first.checked_sub(self.from) {
            // The lines that went out with their ends go, and those that
            // came in have no pieces yet.
            Some(at) if at < self.lines.len() => {
                let gone = (at + 1)..(at + 1 + out).min(self.lines.len());
                self.lines.splice(gone, (0..put).map(|_| Box::default()));
            }
            Some(_) => {}
            None => self.lines.clear(),
        }
        self.to = self.to.min(first).max(self.from);
        self.unknown = self.unknown.filter(|line| *line < first);
        self.asked = None;
    }

    /// The colors are given up, and those that were known go.
    pub fn end(&mut self, why: End) {
        let given_up = self.given_up.saturating_add(1);
        *self =
            Self { grammar: self.grammar, ended: Some(why), given_up, sent: self.sent, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::new(text.as_bytes().to_vec(), false).ok().unwrap()
    }

    fn pieces(lines: &[&[Piece]]) -> Vec<Box<[Piece]>> {
        lines.iter().map(|line| line.to_vec().into_boxed_slice()).collect()
    }

    #[test]
    fn a_byte_stands_for_a_kind_or_for_none() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_byte(kind as u8), Some(kind));
        }
        assert_eq!((Kind::from_byte(8), Kind::from_byte(255)), (None, None));
    }

    #[test]
    fn pieces_are_taken_only_if_they_fit_the_text() {
        let text = doc("let a = 1;\n// two\n\nlast");
        let mut colors = Colors::new(Some("Rust"));
        let good: [&[Piece]; 3] = [
            &[(3, Kind::Keyword), (8, Kind::Plain), (9, Kind::Constant), (11, Kind::Plain)],
            &[(7, Kind::Comment)],
            &[],
        ];
        assert!(colors.take(&text, 0, pieces(&good), None));
        assert_eq!(colors.line(1), &[(7, Kind::Comment)]);
        assert_eq!((colors.line(2), colors.line(3), colors.line(99)), (&[][..], &[][..], &[][..]));

        // A piece that ends behind its line, with its end counted in.
        assert!(!colors.take(&text, 0, pieces(&[&[(12, Kind::Plain)]]), None));
        assert!(colors.take(&text, 0, pieces(&[&[(11, Kind::Plain)]]), None));
        // Pieces that do not follow one another, and one that is empty.
        assert!(!colors.take(&text, 0, pieces(&[&[(5, Kind::Plain), (5, Kind::String)]]), None));
        assert!(!colors.take(&text, 0, pieces(&[&[(5, Kind::Plain), (2, Kind::String)]]), None));
        assert!(!colors.take(&text, 0, pieces(&[&[(0, Kind::Plain)]]), None));
        // Lines the text does not have, and a number that overflows.
        assert!(!colors.take(&text, 3, pieces(&[&[(4, Kind::Plain)], &[]]), None));
        assert!(!colors.take(&text, usize::MAX, pieces(&[&[]]), None));
        assert!(!colors.take(&text, 0, Vec::new(), Some(4)));
        // What was refused changed nothing.
        assert_eq!(colors.line(0), &[(11, Kind::Plain)]);
        assert!(colors.take(&text, 3, pieces(&[&[(4, Kind::Plain)]]), Some(3)));
    }

    #[test]
    fn what_is_seen_and_a_little_more_is_asked_for_once() {
        let mut colors = Colors::new(Some("Rust"));
        let text = doc(&"x\n".repeat(999));
        // Nothing is known: a pane to either side of what is seen.
        assert_eq!(colors.wanted(0, 40, 1000), Some((0, 80)));
        assert_eq!(colors.wanted(500, 40, 1000), Some((460, 120)));
        assert_eq!(colors.wanted(990, 40, 1000), Some((950, 50)));
        colors.asked(460, 120);
        assert_eq!(colors.wanted(500, 40, 1000), None, "asked for already");
        assert_eq!(colors.wanted(700, 40, 1000), Some((660, 120)), "elsewhere by now");
        let lines = || vec![Vec::new().into_boxed_slice(); 120];
        assert!(colors.take(&text, 460, lines(), None));
        // Scrolling by a few lines asks for nothing, by half a pane it does.
        assert_eq!((colors.wanted(500, 40, 1000), colors.wanted(515, 40, 1000)), (None, None));
        assert_eq!(colors.wanted(525, 40, 1000), Some((485, 120)));
        assert_eq!(colors.wanted(475, 40, 1000), Some((435, 120)));
        // An answer with fewer lines than were asked for is all there
        // is to know of them: the same is not asked for again.
        colors.asked(485, 120);
        assert!(colors.take(&text, 485, vec![Vec::new().into_boxed_slice(); 30], None));
        assert_eq!((colors.wanted(525, 40, 1000), colors.wanted(600, 40, 1000)), (None, Some((560, 120))));
        assert!(colors.take(&text, 460, lines(), None));
        // A text shorter than the pane, and an empty pane.
        assert_eq!(Colors::new(Some("Rust")).wanted(0, 40, 3), Some((0, 3)));
        assert_eq!(colors.wanted(0, 0, 1000), None);
        // Behind the line from which nothing is known there is nothing to ask.
        assert!(colors.take(&text, 460, lines(), Some(470)));
        assert_eq!((colors.wanted(480, 40, 1000), colors.wanted(900, 40, 1000)), (None, None));
        assert_eq!(colors.wanted(300, 40, 1000), Some((260, 120)));
    }

    #[test]
    fn a_text_read_anew_keeps_the_colors_above_where_it_changed() {
        let text = doc(&"x\n".repeat(99));
        let mut colors = Colors::new(Some("Rust"));
        let lines = vec![vec![(2, Kind::Keyword)].into_boxed_slice(); 50];
        assert!(colors.take(&text, 10, lines, Some(70)));
        colors.asked(0, 10);
        colors.changed(30);
        assert_eq!((colors.line(29).len(), colors.line(30).len(), colors.line(10).len()), (1, 0, 1));
        // What followed the change is asked for again, also where
        // nothing could be known before.
        assert_eq!(colors.wanted(40, 20, 100), Some((20, 60)));
        colors.changed(5);
        assert_eq!(colors.line(10).len(), 0);
        assert_eq!(colors.wanted(0, 20, 100), Some((0, 40)));
    }

    #[test]
    fn a_text_changed_by_a_key_keeps_its_colors_in_view_and_asks_for_them_again() {
        // Lines long enough that a piece can end at the number of its line.
        let text = doc(&format!("{}\n", "x".repeat(99)).repeat(99));
        let known = |colors: &Colors, line: usize| colors.line(line).first().map(|(until, _)| *until);
        let taken = || {
            let mut colors = Colors::new(Some("Rust"));
            let lines = (10..60).map(|line| vec![(line, Kind::Keyword)].into_boxed_slice()).collect();
            assert!(colors.take(&text, 10, lines, Some(70)));
            colors
        };
        // A change inside a line: every line keeps what was known of it,
        // and from that line on it is asked for again.
        let mut colors = taken();
        colors.edited(Lines { first: 30, out: 0, put: 0 });
        assert_eq!((known(&colors, 30), known(&colors, 59)), (Some(30), Some(59)));
        assert_eq!(colors.wanted(20, 20, 99), Some((0, 60)));
        assert_eq!(colors.wanted(0, 10, 99), Some((0, 20)), "nothing was known up there before");
        // Two lines went and one came: what stood below is two up and
        // one down, and the new line has no pieces yet.
        let mut colors = taken();
        colors.edited(Lines { first: 30, out: 2, put: 1 });
        assert_eq!(
            [30, 31, 32, 58, 59].map(|line| known(&colors, line)),
            [Some(30), None, Some(33), Some(59), None]
        );
        // A change above what is known: the lines below it are others.
        let mut colors = taken();
        colors.edited(Lines { first: 5, out: 0, put: 1 });
        assert_eq!((known(&colors, 10), known(&colors, 30)), (None, None));
        // One below it: nothing moves, and nothing is asked for.
        let mut colors = taken();
        colors.edited(Lines { first: 80, out: 1, put: 0 });
        assert_eq!((known(&colors, 59), colors.wanted(20, 20, 99)), (Some(59), None));
        // More lines going than are known is no fault.
        let mut colors = taken();
        colors.edited(Lines { first: 58, out: 9, put: 0 });
        assert_eq!((known(&colors, 58), known(&colors, 59)), (Some(58), None));
    }

    #[test]
    fn a_file_without_a_grammar_too_large_or_given_up_has_none() {
        let text = doc("fn main() {}\n");
        let of = Edition { opening: 1, change: 1 };
        assert_eq!(Colors::new(None).grammar(&text), None);
        let mut colors = Colors::new(Some("Rust"));
        assert_eq!(colors.grammar(&text), Some("Rust"));
        assert!(colors.unsent(of));
        colors.sent(of);
        assert!(!colors.unsent(of) && colors.unsent(Edition { opening: 1, change: 2 }));
        let large = Document::new(vec![b'x'; FILE_MAX + 1], false).ok().unwrap();
        assert_eq!(colors.grammar(&large), None);
        assert!(colors.take(&text, 0, vec![vec![(2, Kind::Keyword)].into_boxed_slice()], None));
        colors.end(End::TookTooLong);
        assert_eq!(
            (colors.grammar(&text), colors.ended(), colors.line(0).len()),
            (None, Some(End::TookTooLong), 0)
        );
        assert!(!colors.unsent(of), "and nothing is sent for it again");
        // The text that is read after gets a second try, and no third:
        // the file then has one color for as long as it is open.
        colors.changed(0);
        assert_eq!((colors.grammar(&text), colors.ended()), (Some("Rust"), None));
        colors.end(End::Ended);
        colors.changed(0);
        assert_eq!((colors.grammar(&text), colors.ended()), (None, Some(End::Ended)));
        // Colors that came between the two times do not make the second
        // the first again: the top of a file can be cut and its end not.
        let mut colors = Colors::new(Some("Rust"));
        colors.end(End::TookTooLong);
        colors.changed(0);
        assert!(colors.take(&text, 0, vec![vec![(2, Kind::Keyword)].into_boxed_slice()], None));
        colors.end(End::TookTooLong);
        colors.changed(0);
        assert_eq!((colors.grammar(&text), colors.line(0).len()), (None, 0));
        // A text the process lost is given again, and its lines asked again.
        let mut colors = Colors::new(Some("Rust"));
        colors.sent(of);
        colors.asked(0, 80);
        assert_eq!(colors.wanted(0, 40, 100), None);
        colors.lost();
        assert!(colors.unsent(of));
        assert_eq!(colors.wanted(0, 40, 100), Some((0, 80)));
    }
}
