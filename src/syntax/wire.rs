//! What codn and its process for the colors say to each other, over two
//! pipes: a question, then its answer, never two at a time. Each is four
//! bytes of length and then that many bytes, numbers with the low byte
//! first. Both sides are the same program, so there is one version of
//! this. Neither side trusts a number: a length is held against a limit
//! before anything is allocated, and what does not read to its last byte
//! is no message.

use crate::core::colors::{FILE_MAX, Kind, Piece};
use crate::core::preview::{Align, COLUMNS_WIRE, ENTRIES_MAX, Entry, WORD_MAX, What, Why, Word};

/// A line over this is not cut, and nothing is known from it on: its
/// time has no bound that fits a line.
pub const LINE_MAX: usize = 16 << 10;
/// How many lines are cut for one answer at the most. The watch on the
/// process is set for that much work.
pub const STRETCH: usize = 64;
/// Most bytes of a question: a text of the size that is shown in colors,
/// and what names it.
pub const QUESTION_MAX: usize = FILE_MAX + 512;
/// Most bytes of an answer: a stretch of lines of the longest kind, each
/// with a piece for every byte.
pub const ANSWER_MAX: usize = STRETCH * (4 + LINE_MAX * 5) + 64;
/// Most bytes of what a text of Markdown is made of. A text whose
/// structure is larger has none that is said, and is shown as its text.
pub const OUTLINE_MAX: usize = 4 << 20;
const _: () = assert!(OUTLINE_MAX <= ANSWER_MAX);

#[derive(Debug, PartialEq)]
pub enum Question<'a> {
    /// Here is a text, under a number, and the grammar to cut it with.
    /// It takes the place of the text before.
    Text { id: u64, grammar: &'a str, text: &'a [u8] },
    /// The text `of` with some of its lines replaced is the text `id`
    /// from now on: its first `keep` lines stay, the `gone` lines behind
    /// them go, and these bytes stand in their place. A key in a file
    /// sends this, and a file that was written to: not the text.
    Change { id: u64, of: u64, keep: u32, gone: u32, text: &'a [u8] },
    /// The pieces of these lines of the text with that number.
    Lines { id: u64, from: u32, count: u32 },
    /// What this text of Markdown is made of. It is not the text that is
    /// held, and is not kept: a file whose colors were given up has its
    /// structure all the same.
    Outline { id: u64, text: &'a [u8] },
}

#[derive(Debug, PartialEq)]
pub enum Answer {
    /// The text is taken.
    Taken { id: u64 },
    /// The pieces of the lines from `from` on, as many as were cut: fewer
    /// than asked for where the stretch ended, the text ended, or nothing
    /// is known from the line `unknown` on.
    Pieces { id: u64, from: u32, lines: Vec<Box<[Piece]>>, unknown: Option<u32> },
    /// Not there yet: cut as far as this line, ask again.
    Reached { id: u64, line: u32 },
    /// The process holds too much by now and ends. A new one is to be asked.
    Full,
    /// The question was none: no such grammar, no such text.
    Refused,
    /// What the text of that question is made of.
    Outline { id: u64, entries: Vec<Entry> },
    /// Why that is not said: the text is none, or has too much to it.
    NoOutline { id: u64, why: Why },
}

/// Reads numbers off the front of a message.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn short(&mut self) -> Option<u32> {
        self.take(4).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn long(&mut self) -> Option<u64> {
        self.take(8).map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
    }
}

fn put_word(out: &mut Vec<u8>, word: &Word) {
    match word {
        Word::Here(from, to) => {
            out.push(0);
            out.extend_from_slice(&from.to_le_bytes());
            out.extend_from_slice(&to.to_le_bytes());
        }
        Word::Said(said) => {
            out.push(1);
            out.extend_from_slice(&(said.len() as u32).to_le_bytes());
            out.extend_from_slice(said.as_bytes());
        }
    }
}

/// A piece of text an entry names. Characters it carries are held
/// against what an entry may carry before they are taken.
fn word(bytes: &mut Bytes) -> Option<Word> {
    Some(match bytes.byte()? {
        0 => Word::Here(bytes.short()?, bytes.short()?),
        1 => {
            let length = bytes.short()? as usize;
            if length > WORD_MAX {
                return None;
            }
            Word::Said(std::str::from_utf8(bytes.take(length)?).ok()?.into())
        }
        _ => return None,
    })
}

/// How many bytes an entry takes in a message.
pub fn weight(entry: &Entry) -> usize {
    let word = |word: &Word| match word {
        Word::Here(..) => 9,
        Word::Said(said) => 5 + said.len(),
    };
    9 + match &entry.what {
        What::Heading(_) | What::Quote(_) | What::Task(_) => 1,
        What::List(_) => 9,
        What::Table(columns) => 1 + columns.len(),
        What::Link(to) | What::Image(to) | What::Text(to) | What::CodeSpan(to) | What::InlineHtml(to) => {
            word(to)
        }
        _ => 0,
    }
}

fn put_entry(out: &mut Vec<u8>, entry: &Entry) {
    let kind = match &entry.what {
        What::End => 0,
        What::Paragraph => 1,
        What::Heading(_) => 2,
        What::Quote(_) => 3,
        What::Code => 4,
        What::Html => 5,
        What::List(_) => 6,
        What::Item => 7,
        What::Table(_) => 8,
        What::Head => 9,
        What::Row => 10,
        What::Cell => 11,
        What::Meta => 12,
        What::Emphasis => 13,
        What::Strong => 14,
        What::Strike => 15,
        What::Link(_) => 16,
        What::Image(_) => 17,
        What::Other => 18,
        What::Text(_) => 19,
        What::CodeSpan(_) => 20,
        What::InlineHtml(_) => 21,
        What::Soft => 22,
        What::Hard => 23,
        What::Rule => 24,
        What::Task(_) => 25,
    };
    out.push(kind);
    out.extend_from_slice(&entry.from.to_le_bytes());
    out.extend_from_slice(&entry.to.to_le_bytes());
    match &entry.what {
        What::Heading(byte) | What::Quote(byte) => out.push(*byte),
        What::Task(checked) => out.push(*checked as u8),
        What::List(first) => {
            out.push(first.is_some() as u8);
            out.extend_from_slice(&first.unwrap_or(0).to_le_bytes());
        }
        What::Table(columns) => {
            out.push(columns.len().min(COLUMNS_WIRE) as u8);
            out.extend(columns.iter().take(COLUMNS_WIRE).map(|column| *column as u8));
        }
        What::Link(to) | What::Image(to) | What::Text(to) | What::CodeSpan(to) | What::InlineHtml(to) => {
            put_word(out, to)
        }
        _ => {}
    }
}

fn entry(bytes: &mut Bytes) -> Option<Entry> {
    let (kind, from, to) = (bytes.byte()?, bytes.short()?, bytes.short()?);
    let what = match kind {
        0 => What::End,
        1 => What::Paragraph,
        2 => What::Heading(bytes.byte()?),
        3 => What::Quote(bytes.byte()?),
        4 => What::Code,
        5 => What::Html,
        6 => {
            let (numbered, first) = (bytes.byte()?, bytes.long()?);
            match numbered {
                0 => What::List(None),
                1 => What::List(Some(first)),
                _ => return None,
            }
        }
        7 => What::Item,
        8 => {
            let count = bytes.byte()? as usize;
            let columns = bytes.take(count)?.iter().map(|byte| Align::from_byte(*byte));
            What::Table(columns.collect::<Option<Box<[Align]>>>()?)
        }
        9 => What::Head,
        10 => What::Row,
        11 => What::Cell,
        12 => What::Meta,
        13 => What::Emphasis,
        14 => What::Strong,
        15 => What::Strike,
        16 => What::Link(word(bytes)?),
        17 => What::Image(word(bytes)?),
        18 => What::Other,
        19 => What::Text(word(bytes)?),
        20 => What::CodeSpan(word(bytes)?),
        21 => What::InlineHtml(word(bytes)?),
        22 => What::Soft,
        23 => What::Hard,
        24 => What::Rule,
        25 => match bytes.byte()? {
            0 => What::Task(false),
            1 => What::Task(true),
            _ => return None,
        },
        _ => return None,
    };
    Some(Entry { what, from, to })
}

/// Puts a message into `out` with its length before it.
fn framed(out: &mut Vec<u8>, body: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    body(out);
    let length = (out.len() - at - 4) as u32;
    out[at..at + 4].copy_from_slice(&length.to_le_bytes());
}

impl<'a> Question<'a> {
    pub fn write(&self, out: &mut Vec<u8>) {
        framed(out, |out| match self {
            Self::Text { id, grammar, text } => {
                out.push(1);
                out.extend_from_slice(&id.to_le_bytes());
                out.push(grammar.len() as u8);
                out.extend_from_slice(grammar.as_bytes());
                out.extend_from_slice(text);
            }
            Self::Lines { id, from, count } => {
                out.push(2);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&from.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
            }
            Self::Change { id, of, keep, gone, text } => {
                out.push(3);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&of.to_le_bytes());
                out.extend_from_slice(&keep.to_le_bytes());
                out.extend_from_slice(&gone.to_le_bytes());
                out.extend_from_slice(text);
            }
            Self::Outline { id, text } => {
                out.push(4);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(text);
            }
        });
    }

    /// The question in the bytes of a message, if they are one.
    pub fn read(body: &'a [u8]) -> Option<Self> {
        let mut bytes = Bytes(body);
        let question = match bytes.byte()? {
            1 => {
                let id = bytes.long()?;
                let length = bytes.byte()? as usize;
                let grammar = std::str::from_utf8(bytes.take(length)?).ok()?;
                Self::Text { id, grammar, text: std::mem::take(&mut bytes.0) }
            }
            2 => Self::Lines { id: bytes.long()?, from: bytes.short()?, count: bytes.short()? },
            3 => Self::Change {
                id: bytes.long()?,
                of: bytes.long()?,
                keep: bytes.short()?,
                gone: bytes.short()?,
                text: std::mem::take(&mut bytes.0),
            },
            4 => Self::Outline { id: bytes.long()?, text: std::mem::take(&mut bytes.0) },
            _ => return None,
        };
        bytes.0.is_empty().then_some(question)
    }
}

impl Answer {
    pub fn write(&self, out: &mut Vec<u8>) {
        framed(out, |out| match self {
            Self::Taken { id } => {
                out.push(1);
                out.extend_from_slice(&id.to_le_bytes());
            }
            Self::Pieces { id, from, lines, unknown } => {
                out.push(2);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&from.to_le_bytes());
                // No text has a line of this number: it stands for "none".
                out.extend_from_slice(&unknown.unwrap_or(u32::MAX).to_le_bytes());
                out.extend_from_slice(&(lines.len() as u32).to_le_bytes());
                for line in lines {
                    out.extend_from_slice(&(line.len() as u32).to_le_bytes());
                    for (until, kind) in line.iter() {
                        out.extend_from_slice(&until.to_le_bytes());
                        out.push(*kind as u8);
                    }
                }
            }
            Self::Reached { id, line } => {
                out.push(3);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&line.to_le_bytes());
            }
            Self::Full => out.push(4),
            Self::Refused => out.push(5),
            Self::Outline { id, entries } => {
                out.push(6);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                entries.iter().for_each(|entry| put_entry(out, entry));
            }
            Self::NoOutline { id, why } => {
                out.push(7);
                out.extend_from_slice(&id.to_le_bytes());
                out.push(matches!(why, Why::NotText) as u8);
            }
        });
    }

    /// The answer in the bytes of a message, if they are one. A count in
    /// it is held against the bytes that are left before anything is
    /// made that long.
    pub fn read(body: &[u8]) -> Option<Self> {
        let mut bytes = Bytes(body);
        let answer = match bytes.byte()? {
            1 => Self::Taken { id: bytes.long()? },
            2 => {
                let (id, from, unknown) = (bytes.long()?, bytes.short()?, bytes.short()?);
                let count = bytes.short()? as usize;
                // A line is four bytes at the least, a piece five.
                if count > STRETCH || count > bytes.0.len() / 4 {
                    return None;
                }
                let mut lines = Vec::with_capacity(count);
                for _ in 0..count {
                    // A line that is cut has `LINE_MAX` bytes at the most,
                    // and every piece ends behind the one before: so
                    // many pieces it can have, and none ends further.
                    let pieces = bytes.short()? as usize;
                    if pieces > LINE_MAX || pieces > bytes.0.len() / 5 {
                        return None;
                    }
                    let (mut line, mut last) = (Vec::with_capacity(pieces), 0);
                    for _ in 0..pieces {
                        let (until, kind) = (bytes.short()?, Kind::from_byte(bytes.byte()?)?);
                        if until <= std::mem::replace(&mut last, until) || until as usize > LINE_MAX {
                            return None;
                        }
                        line.push((until, kind));
                    }
                    lines.push(line.into_boxed_slice());
                }
                Self::Pieces { id, from, lines, unknown: Some(unknown).filter(|line| *line != u32::MAX) }
            }
            3 => Self::Reached { id: bytes.long()?, line: bytes.short()? },
            4 => Self::Full,
            5 => Self::Refused,
            6 => {
                let (id, count) = (bytes.long()?, bytes.short()? as usize);
                // An entry is nine bytes at the least.
                if count > ENTRIES_MAX || count > bytes.0.len() / 9 {
                    return None;
                }
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    entries.push(entry(&mut bytes)?);
                }
                Self::Outline { id, entries }
            }
            7 => {
                let (id, why) = (bytes.long()?, bytes.byte()?);
                let why = match why {
                    0 => Why::Large,
                    1 => Why::NotText,
                    _ => return None,
                };
                Self::NoOutline { id, why }
            }
            _ => return None,
        };
        bytes.0.is_empty().then_some(answer)
    }
}

/// The length the first four bytes of a message name, if it is within
/// `limit`.
pub fn length(head: [u8; 4], limit: usize) -> Option<usize> {
    Some(u32::from_le_bytes(head) as usize).filter(|length| *length <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(frame: &[u8]) -> &[u8] {
        assert_eq!(length(frame[..4].try_into().unwrap(), usize::MAX), Some(frame.len() - 4));
        &frame[4..]
    }

    #[test]
    fn what_is_written_is_what_is_read() {
        let questions = [
            Question::Text { id: u64::MAX - 1, grammar: "Rust", text: b"fn main() {}\n\xff" },
            Question::Text { id: 0, grammar: "", text: b"" },
            Question::Lines { id: 7, from: 3, count: u32::MAX },
            Question::Change { id: 8, of: 7, keep: 2, gone: 1, text: b"let a = 2;\n" },
            Question::Change { id: 9, of: 8, keep: 0, gone: u32::MAX, text: b"" },
        ];
        for question in questions {
            let mut frame = Vec::new();
            question.write(&mut frame);
            assert_eq!(Question::read(body(&frame)), Some(question));
        }
        let line = |pieces: &[Piece]| pieces.to_vec().into_boxed_slice();
        let answers = [
            Answer::Taken { id: 9 },
            Answer::Pieces {
                id: 9,
                from: 100,
                lines: vec![
                    line(&[(3, Kind::Keyword), (9, Kind::Name)]),
                    line(&[]),
                    line(&[(1, Kind::Plain)]),
                ],
                unknown: Some(102),
            },
            Answer::Pieces { id: 1, from: 0, lines: Vec::new(), unknown: None },
            Answer::Reached { id: 2, line: 640 },
            Answer::Full,
            Answer::Refused,
        ];
        for answer in answers {
            let mut frame = Vec::new();
            answer.write(&mut frame);
            assert_eq!(Answer::read(body(&frame)), Some(answer));
        }
    }

    #[test]
    fn what_is_cut_short_goes_on_too_long_or_counts_too_much_is_no_message() {
        let mut frame = Vec::new();
        let line = vec![(3, Kind::Keyword), (9, Kind::Name)].into_boxed_slice();
        Answer::Pieces { id: 9, from: 1, lines: vec![line.clone(), line], unknown: None }.write(&mut frame);
        let whole = body(&frame).to_vec();
        assert!(Answer::read(&whole).is_some());
        // Every shorter one, and one byte more.
        for cut in 0..whole.len() {
            assert_eq!(Answer::read(&whole[..cut]), None, "{cut} bytes");
        }
        assert_eq!(Answer::read(&[&whole[..], &[0]].concat()), None);
        // A kind there is none of.
        let mut odd = whole.clone();
        *odd.last_mut().unwrap() = Kind::ALL.len() as u8;
        assert_eq!(Answer::read(&odd), None);
        // Counts of lines and of pieces that the bytes cannot hold, which
        // must not be made room for: four thousand million of each.
        let mut many = whole.clone();
        many[17..21].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Answer::read(&many), None);
        let mut many = whole.clone();
        many[21..25].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Answer::read(&many), None);
        // Pieces that do not follow one another, one that ends behind the
        // longest line there is, and more of them than such a line has
        // bytes: each fits the bytes of a message, and many such
        // messages would be more than the app may be made to hold.
        let line = |pieces: Vec<Piece>| {
            let mut frame = Vec::new();
            Answer::Pieces { id: 9, from: 1, lines: vec![pieces.into_boxed_slice()], unknown: None }
                .write(&mut frame);
            Answer::read(body(&frame))
        };
        assert!(line(vec![(3, Kind::Keyword), (LINE_MAX as u32, Kind::Name)]).is_some());
        assert_eq!(line(vec![(3, Kind::Keyword), (3, Kind::Name)]), None);
        assert_eq!(line(vec![(3, Kind::Keyword), (2, Kind::Name)]), None);
        assert_eq!(line(vec![(0, Kind::Keyword)]), None);
        assert_eq!(line(vec![(LINE_MAX as u32 + 1, Kind::Name)]), None);
        assert_eq!(line(vec![(1, Kind::Plain); LINE_MAX + 1]), None);
        assert_eq!(line((1..=LINE_MAX as u32 + 1).map(|until| (until, Kind::Plain)).collect()), None);
        // More lines than a stretch has.
        let empty = vec![Vec::new().into_boxed_slice(); STRETCH + 1];
        let mut frame = Vec::new();
        Answer::Pieces { id: 9, from: 1, lines: empty, unknown: None }.write(&mut frame);
        assert_eq!(Answer::read(body(&frame)), None);

        assert_eq!(Question::read(&[]), None);
        assert_eq!(Question::read(&[9]), None);
        assert_eq!(Question::read(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]), None, "no count");
        assert_eq!(Question::read(&[3, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]), None, "a change of nothing");
        // A grammar whose name is longer than the message, or no text.
        assert_eq!(Question::read(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 200, b'R']), None);
        assert_eq!(Question::read(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xff]), None);
    }

    fn entries() -> Vec<Entry> {
        let here = Word::Here(3, 9);
        let said = || Word::Said("ä&".into());
        let whats = [
            What::Paragraph,
            What::Heading(6),
            What::Quote(5),
            What::Code,
            What::Html,
            What::List(None),
            What::List(Some(u64::MAX)),
            What::Item,
            What::Table(vec![Align::None, Align::Left, Align::Center, Align::Right].into()),
            What::Table(Box::default()),
            What::Head,
            What::Row,
            What::Cell,
            What::Meta,
            What::Emphasis,
            What::Strong,
            What::Strike,
            What::Link(here.clone()),
            What::Image(said()),
            What::Other,
            What::End,
            What::Text(here),
            What::CodeSpan(said()),
            What::InlineHtml(Word::Said("".into())),
            What::Soft,
            What::Hard,
            What::Rule,
            What::Task(true),
            What::Task(false),
        ];
        whats
            .into_iter()
            .enumerate()
            .map(|(at, what)| Entry { what, from: at as u32, to: u32::MAX })
            .collect()
    }

    #[test]
    fn what_a_text_is_made_of_is_written_and_read_entry_by_entry() {
        let mut frame = Vec::new();
        Question::Outline { id: 7, text: b"# a\n\xff" }.write(&mut frame);
        assert_eq!(Question::read(body(&frame)), Some(Question::Outline { id: 7, text: b"# a\n\xff" }));
        for answer in [
            Answer::Outline { id: 9, entries: entries() },
            Answer::Outline { id: 0, entries: Vec::new() },
            Answer::NoOutline { id: 3, why: Why::Large },
            Answer::NoOutline { id: 4, why: Why::NotText },
        ] {
            let mut frame = Vec::new();
            answer.write(&mut frame);
            assert_eq!(Answer::read(body(&frame)), Some(answer));
        }
        // An entry weighs what is written of it.
        for entry in entries() {
            let mut out = Vec::new();
            put_entry(&mut out, &entry);
            assert_eq!(out.len(), weight(&entry), "{entry:?}");
        }
    }

    #[test]
    fn a_structure_that_is_cut_short_or_says_what_there_is_none_of_is_no_message() {
        let mut frame = Vec::new();
        Answer::Outline { id: 9, entries: entries() }.write(&mut frame);
        let whole = body(&frame).to_vec();
        for cut in 0..whole.len() {
            assert_eq!(Answer::read(&whole[..cut]), None, "{cut} bytes");
        }
        assert_eq!(Answer::read(&[&whole[..], &[0]].concat()), None);
        // More entries than the bytes can hold, which must not be made
        // room for, and more than a text is shown by.
        let mut many = whole.clone();
        many[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Answer::read(&many), None);
        let mut frame = Vec::new();
        let rule = Entry { what: What::Rule, from: 0, to: 0 };
        Answer::Outline { id: 1, entries: vec![rule.clone(); ENTRIES_MAX] }.write(&mut frame);
        assert!(Answer::read(body(&frame)).is_some() && frame.len() <= ANSWER_MAX);
        let mut frame = Vec::new();
        Answer::Outline { id: 1, entries: vec![rule; ENTRIES_MAX + 1] }.write(&mut frame);
        assert_eq!(Answer::read(body(&frame)), None);
        // One entry, altered: a kind there is none of, a way to name
        // text there is none of, an alignment, a box that is neither
        // checked nor not, a list that neither has numbers nor not.
        let one = |what: What, at: usize, byte: u8| {
            let mut frame = Vec::new();
            Answer::Outline { id: 1, entries: vec![Entry { what, from: 1, to: 2 }] }.write(&mut frame);
            let mut bytes = body(&frame).to_vec();
            assert!(Answer::read(&bytes).is_some());
            bytes[13 + at] = byte;
            Answer::read(&bytes)
        };
        assert_eq!(one(What::Rule, 0, 26), None);
        assert_eq!(one(What::Text(Word::Here(0, 1)), 9, 2), None);
        assert_eq!(one(What::Table(vec![Align::Left].into()), 10, 4), None);
        assert_eq!(one(What::Task(true), 9, 2), None);
        assert_eq!(one(What::List(None), 9, 2), None);
        assert!(one(What::Heading(1), 9, 9).is_some(), "what a level may be is for the text to say");
        // Characters that are no text, and more of them than an entry
        // carries.
        assert_eq!(one(What::Link(Word::Said("ab".into())), 14, 0xff), None);
        let long = |length: usize| {
            let mut frame = Vec::new();
            let what = What::Link(Word::Said("a".repeat(length).into()));
            Answer::Outline { id: 1, entries: vec![Entry { what, from: 0, to: 0 }] }.write(&mut frame);
            Answer::read(body(&frame)).is_some()
        };
        assert!(long(WORD_MAX) && !long(WORD_MAX + 1));
        // A reason there is none of.
        assert_eq!(Answer::read(&[&[7][..], &1u64.to_le_bytes(), &[2]].concat()), None);
    }

    #[test]
    fn a_length_over_the_limit_is_none() {
        assert_eq!(length(5u32.to_le_bytes(), 5), Some(5));
        assert_eq!(length(6u32.to_le_bytes(), 5), None);
        assert_eq!(length(u32::MAX.to_le_bytes(), ANSWER_MAX), None);
        // The largest answer there can be fits the limit for answers.
        let full =
            vec![(1..=LINE_MAX as u32).map(|until| (until, Kind::Plain)).collect::<Box<[Piece]>>(); STRETCH];
        let mut frame = Vec::new();
        Answer::Pieces { id: 0, from: 0, lines: full, unknown: None }.write(&mut frame);
        assert!(frame.len() - 4 <= ANSWER_MAX);
    }
}
