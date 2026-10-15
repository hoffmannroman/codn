//! The text of a file: its bytes as they were read, where its lines
//! begin, how a line lies in cells, and a caret and a selection in it.
//!
//! A place in the text is a line and a byte in that line, never a cell:
//! the cells are how a line looks, and they are reckoned again whenever a
//! line is drawn or clicked.

use unicode_width::UnicodeWidthChar;

use super::files::MAX_FILE;

/// A file with a zero byte this early is not text, which is how git
/// tells as well.
const SNIFF: usize = 8 << 10;
/// Most lines that are told apart. A file of line ends alone would
/// otherwise cost four times its size in places where lines begin.
const MAX_LINES: usize = 2_000_000;
/// How far apart tab stops are, in cells.
const TAB: usize = 4;
/// Most bytes of one line that are looked at. A line can be made that
/// has megabytes of marks which take no cell, and it never ends.
pub const ROW_BYTES: usize = 64 << 10;

/// The mark some programs put at the start of a file of UTF-8.
const MARK: &[u8] = b"\xef\xbb\xbf";

pub struct Document {
    bytes: Vec<u8>,
    /// Where every line begins. No file here is longer than 32 bits say.
    starts: Vec<u32>,
    /// There is more of the file than is held: bytes or lines.
    cut: bool,
    /// All of it is text: UTF-8, and no zero byte. One that is not is
    /// shown and not changed. What was written back of bytes that are
    /// no UTF-8 would be another file. And nothing that goes into a
    /// text may hold a zero byte, so one that a key took out could not
    /// be put back: a step would be refused, and every step before it.
    text: bool,
    /// The file begins with `MARK`, which is kept apart from the text:
    /// nothing is typed before it and nothing takes it out.
    marked: bool,
}

/// Why a text was not changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refused {
    /// The text is only shown: not all of the file is held, or not all
    /// of it is UTF-8.
    Shown,
    /// A place is not one between two characters of the text.
    Place,
    /// A line is longer than a line is looked at, or would be.
    Line,
    /// The text would be longer than what is read of a file.
    Limit,
    /// What was to go in is not text.
    NoText,
}

/// What a change did to a text, for taking it back.
#[derive(Debug, PartialEq)]
pub struct Change {
    /// Where in the bytes of the text it was made, and what went out.
    pub at: usize,
    pub out: Vec<u8>,
    /// The first line that is another now.
    pub line: usize,
    /// The place behind what went in.
    pub end: Pos,
}

/// A place in a text: before the byte `byte` of the line `line`.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
}

/// Where every line of a text begins: the first at its start, another
/// behind every line feed. For a text that is held whole, as the thread
/// and the process for the colors hold it: `Document` counts the same
/// way, with a limit on the lines.
pub fn line_starts(bytes: &[u8]) -> Vec<u32> {
    let ends = bytes.iter().enumerate().filter(|(_, byte)| **byte == b'\n');
    std::iter::once(0).chain(ends.map(|(at, _)| at as u32 + 1)).collect()
}

/// The length of `bytes` without a character that is cut in two at its
/// end. Cutting it is the doing of a limit, not a fault of the file.
fn whole(bytes: &[u8]) -> usize {
    let len = bytes.len();
    for back in 1..=len.min(3) {
        let byte = bytes[len - back];
        if byte & 0xc0 != 0x80 {
            let needs = match byte {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => 1,
            };
            return if needs > back { len - back } else { len };
        }
    }
    len
}

impl Document {
    /// Takes the bytes of a file, or gives them back if they are not text.
    /// `cut` says that the file goes on behind them.
    pub fn new(mut bytes: Vec<u8>, mut cut: bool) -> Result<Self, Vec<u8>> {
        if bytes[..bytes.len().min(SNIFF)].contains(&0) {
            return Err(bytes);
        }
        if cut {
            bytes.truncate(whole(&bytes));
        }
        let marked = bytes.starts_with(MARK);
        if marked {
            bytes.drain(..MARK.len());
        }
        let mut starts = vec![0];
        let mut end = bytes.len();
        for (at, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                if starts.len() == MAX_LINES {
                    (end, cut) = (at, true);
                    break;
                }
                starts.push(at as u32 + 1);
            }
        }
        bytes.truncate(end);
        starts.shrink_to_fit();
        let text = !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok();
        Ok(Self { bytes, starts, cut, text, marked })
    }

    /// Whether the text can be changed and written back: all of the file
    /// is held, and all of it is text.
    pub fn editable(&self) -> bool {
        !self.cut && self.text
    }

    /// Whether the file begins with the mark that `bytes` leaves out.
    pub fn marked(&self) -> bool {
        self.marked
    }

    /// All of the file as it is to be written: the text, behind the
    /// mark the file began with. Nothing for a text that is only shown:
    /// written back, it would cut the file or make another of it.
    pub fn written(&self) -> Option<Vec<u8>> {
        let mark = if self.marked { MARK } else { &[] };
        self.editable().then(|| [mark, &self.bytes].concat())
    }

    /// Where a place is in the bytes of the text, if it is one between
    /// two characters of a line that is there.
    fn offset(&self, pos: Pos) -> Option<usize> {
        let start = *self.starts.get(pos.line)? as usize;
        let at = start + pos.byte;
        (pos.byte <= self.line(pos.line).len() && self.between(at)).then_some(at)
    }

    /// Whether a place in the bytes is not inside a character.
    fn between(&self, at: usize) -> bool {
        at == self.bytes.len() || self.bytes.get(at).is_some_and(|byte| byte & 0xc0 != 0x80)
    }

    /// The place a byte of the text is at.
    fn pos(&self, at: usize) -> Pos {
        let line = self.starts.partition_point(|start| *start as usize <= at) - 1;
        Pos { line, byte: (at - self.starts[line] as usize).min(self.line(line).len()) }
    }

    /// Puts `with` in the place of what stands between two places. See
    /// `splice`, which does it.
    pub fn replace(&mut self, from: Pos, to: Pos, with: &[u8]) -> Result<Change, Refused> {
        let (Some(at), Some(end)) = (self.offset(from), self.offset(to)) else { return Err(Refused::Place) };
        self.splice(at, end.checked_sub(at).ok_or(Refused::Place)?, with)
    }

    /// The one operation that changes a text: puts `with` in the place of
    /// `len` bytes at `at`. Everything is checked before anything is
    /// changed, also for a change that is taken back: the text is one
    /// that may be changed, the places are between characters, no line
    /// that is touched is or becomes longer than a line is looked at,
    /// what goes in is text without a zero byte, and the result is within
    /// what is read of a file. The places of the lines behind are moved,
    /// not found again.
    pub fn splice(&mut self, at: usize, len: usize, with: &[u8]) -> Result<Change, Refused> {
        if !self.editable() {
            return Err(Refused::Shown);
        }
        let end = at.checked_add(len).filter(|end| *end <= self.bytes.len()).ok_or(Refused::Place)?;
        if !self.between(at) || !self.between(end) {
            return Err(Refused::Place);
        }
        if with.contains(&0) || std::str::from_utf8(with).is_err() {
            return Err(Refused::NoText);
        }
        // The lines the change begins and ends in, and those between.
        let first = self.starts.partition_point(|start| *start as usize <= at) - 1;
        let behind = self.starts.partition_point(|start| *start as usize <= end);
        let last = behind - 1;
        if (first..=last).any(|line| self.line(line).len() > ROW_BYTES) {
            return Err(Refused::Line);
        }
        // What is left of the first line before the change and of the
        // last one behind it, and the lines these make with what goes in.
        // A line is as long as `line` will say: a carriage return before
        // the line feed that ends it is its end and not a byte of it.
        // Counted otherwise, a line that was there could not come back
        // when its cutting is taken back.
        let before = at - self.starts[first] as usize;
        let after = (self.starts[last] as usize + self.line(last).len()).saturating_sub(end);
        let ended = |length: usize, last: Option<&u8>| length - (last == Some(&b'\r')) as usize;
        let mut pieces = with.split(|byte| *byte == b'\n');
        let (head, tail) = (pieces.next().unwrap_or_default(), pieces.next_back());
        // The byte a line feed behind the first piece would end the line
        // with, and whether one follows the last piece in the text.
        let leads =
            head.last().or(at.checked_sub(1).filter(|_| before > 0).and_then(|at| self.bytes.get(at)));
        let follows = after == 0 && self.bytes.get(end) == Some(&b'\n');
        let fits = match tail {
            None if follows => ended(before + head.len(), leads) <= ROW_BYTES,
            None => before + head.len() + after <= ROW_BYTES,
            Some(tail) => {
                let last = if follows { ended(tail.len(), tail.last()) } else { tail.len() + after };
                ended(before + head.len(), leads) <= ROW_BYTES && last <= ROW_BYTES
            }
        };
        if !fits || pieces.any(|piece| ended(piece.len(), piece.last()) > ROW_BYTES) {
            return Err(Refused::Line);
        }
        let new = with.iter().filter(|byte| **byte == b'\n').count();
        // The mark at the start is a part of the file, if not of the text.
        let mark = if self.marked { MARK.len() } else { 0 };
        if self.bytes.len() - len + with.len() + mark > MAX_FILE
            || self.starts.len() - (last - first) + new > MAX_LINES
        {
            return Err(Refused::Limit);
        }

        let by = with.len() as isize - len as isize;
        for start in &mut self.starts[behind..] {
            *start = (*start as isize + by) as u32;
        }
        let begun =
            with.iter().enumerate().filter(|(_, byte)| **byte == b'\n').map(|(i, _)| (at + i + 1) as u32);
        self.starts.splice(first + 1..behind, begun);
        let out = self.bytes.splice(at..end, with.iter().copied()).collect();
        Ok(Change { at, out, line: first, end: self.pos(at + with.len()) })
    }

    /// How many lines there are. A text that ends with a line end has an
    /// empty line after it, as in an editor.
    pub fn lines(&self) -> usize {
        self.starts.len()
    }

    pub fn cut(&self) -> bool {
        self.cut
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// The text as it is held: every line with its end, one after another.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many lines at the start of this text another text has alike,
    /// each with its end: the line the first difference is in.
    pub fn same_lines(&self, other: &Self) -> usize {
        let alike = self.bytes.iter().zip(&other.bytes).take_while(|(one, two)| one == two).count();
        if alike == self.bytes.len() && alike == other.bytes.len() {
            return self.lines();
        }
        self.starts.partition_point(|start| *start as usize <= alike) - 1
    }

    /// A line with its end, as the file has it: a line feed, with a
    /// carriage return before it or without. The last line has none. All
    /// lines one after another are the text.
    pub fn ended(&self, line: usize) -> &[u8] {
        let Some(&start) = self.starts.get(line) else { return &[] };
        let next = self.starts.get(line + 1).map_or(self.bytes.len(), |next| *next as usize);
        self.bytes.get(start as usize..next).unwrap_or_default()
    }

    /// A line without its end.
    pub fn line(&self, line: usize) -> &[u8] {
        let bytes = self.ended(line);
        // The last line has no end. A carriage return there is not
        // half of one but a byte of the text.
        let Some(bytes) = bytes.strip_suffix(b"\n") else { return bytes };
        bytes.strip_suffix(b"\r").unwrap_or(bytes)
    }

    /// The last place in the text a caret can be sent to.
    pub fn end(&self) -> Pos {
        let line = self.lines() - 1;
        Pos { line, byte: stops_end(self.line(line)) }
    }

    /// Where the text ends, also behind what is looked at of a long last
    /// line: a selection reaches there, a caret that is moved does not.
    fn last(&self) -> Pos {
        let line = self.lines() - 1;
        Pos { line, byte: self.line(line).len() }
    }

    /// A place that is in the text: in a line there is, and in it.
    fn within(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.lines() - 1);
        Pos { line, byte: if line == pos.line { pos.byte.min(stops_end(self.line(line))) } else { 0 } }
    }

    /// The place a caret can stand at that is at or before a place: not
    /// inside a character and not before a mark that belongs to one. A
    /// place that was one in a text need not be one in the text read
    /// in its stead.
    fn at_stop(&self, pos: Pos) -> Pos {
        let pos = self.within(pos);
        let text = self.line(pos.line);
        if pos.byte >= stops_end(text) {
            return pos;
        }
        let stop = cells(text).take_while(|cell| cell.at <= pos.byte).filter(|cell| cell.wide > 0).last();
        Pos { line: pos.line, byte: stop.map_or(0, |cell| cell.at) }
    }

    /// The text between two places, as it is in the file.
    pub fn text(&self, from: Pos, to: Pos) -> String {
        let at =
            |pos: Pos| self.starts.get(pos.line).map_or(self.bytes.len(), |start| *start as usize + pos.byte);
        let (from, to) = (at(from).min(self.bytes.len()), at(to).min(self.bytes.len()));
        String::from_utf8_lossy(self.bytes.get(from..to).unwrap_or_default()).into_owned()
    }
}

/// One character of a line as it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    /// Where it begins in the line.
    pub at: usize,
    /// The first cell it takes, and how many: none for a mark that goes
    /// onto the character before it, up to the next stop for a tab.
    pub col: usize,
    pub wide: usize,
    /// The cell it is drawn in: its own, or for a mark that takes none
    /// the first cell of the character it belongs to.
    pub base: usize,
    /// What is drawn. A tab is drawn as nothing, a byte that is no text
    /// and a character that would move the pen as a replacement.
    pub c: char,
}

/// The cells of a line, as far as a line is looked at.
pub struct Cells<'a> {
    line: &'a [u8],
    at: usize,
    col: usize,
    /// The first cell of the last character that took one.
    base: usize,
}

pub fn cells(line: &[u8]) -> Cells<'_> {
    Cells { line: &line[..stops_end(line)], at: 0, col: 0, base: 0 }
}

/// Most bytes for a column that a line is drawn with. Text needs four
/// at most, a character with some marks more. Marks that take no cell can
/// be had by the thousand for one column, and each would be drawn.
const DRAWN: usize = 32;

/// The cells of a line that lie before the column `end`, to draw them.
pub fn shown(line: &[u8], end: usize) -> impl Iterator<Item = Cell> + '_ {
    let limit = end.saturating_add(1).saturating_mul(DRAWN);
    cells(line).take_while(move |cell| cell.col < end && cell.at < limit)
}

impl Iterator for Cells<'_> {
    type Item = Cell;

    fn next(&mut self) -> Option<Cell> {
        let rest = self.line.get(self.at..)?;
        let first = *rest.first()?;
        let needs = match first {
            0..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 0,
        };
        let decoded = rest.get(..needs).and_then(|bytes| std::str::from_utf8(bytes).ok());
        let (c, len) = match decoded.and_then(|text| text.chars().next()) {
            Some(c) => (c, needs),
            None => ('\u{fffd}', 1),
        };
        let (c, wide) = match c {
            '\t' => (c, TAB - self.col % TAB),
            c if c.is_control() => ('\u{fffd}', 1),
            c => (c, c.width().unwrap_or(0).min(2)),
        };
        if wide > 0 {
            self.base = self.col;
        }
        let cell = Cell { at: self.at, col: self.col, wide, base: self.base, c };
        self.at += len;
        self.col += wide;
        Some(cell)
    }
}

/// The last place of a line a caret can stand at: its end, or as far as
/// a line is looked at.
fn stops_end(line: &[u8]) -> usize {
    if line.len() > ROW_BYTES { whole(&line[..ROW_BYTES]) } else { line.len() }
}

/// The column a place in a line is drawn at.
pub fn col_of(line: &[u8], byte: usize) -> usize {
    let mut col = 0;
    for cell in cells(line) {
        if cell.at >= byte {
            return cell.col;
        }
        col = cell.col + cell.wide;
    }
    col
}

/// The column a line ends at, looked for no further than the first
/// character that reaches behind `limit`: the column behind that one,
/// for a line that goes on. A line can be long, and who draws it wants
/// to know of its end only whether it is in what is shown.
pub fn col_until(line: &[u8], limit: usize) -> usize {
    let mut end = 0;
    for cell in cells(line) {
        end = cell.col + cell.wide;
        if end > limit {
            break;
        }
    }
    end
}

/// The place in a line that is nearest to `x`, counted in cells from the
/// start of the line: before a character or behind the last one.
pub fn byte_near(line: &[u8], x: f32) -> usize {
    for cell in cells(line).filter(|cell| cell.wide > 0) {
        if x < cell.col as f32 + cell.wide as f32 / 2.0 {
            return cell.at;
        }
    }
    stops_end(line)
}

/// The place before the character that lies under `x`, counted in cells
/// from the start of the line: the last one, for a place behind the line.
pub fn byte_under(line: &[u8], x: f32) -> usize {
    let mut under = 0;
    for cell in cells(line).filter(|cell| cell.wide > 0) {
        if (cell.col as f32) > x {
            break;
        }
        under = cell.at;
    }
    under
}

/// The place behind the character at `byte`: a mark that takes no cell
/// goes with the character before it.
pub(super) fn next_stop(line: &[u8], byte: usize) -> Option<usize> {
    if byte >= stops_end(line) {
        return None;
    }
    let next = cells(line).find(|cell| cell.at > byte && cell.wide > 0);
    Some(next.map_or(stops_end(line), |cell| cell.at))
}

pub(super) fn prev_stop(line: &[u8], byte: usize) -> Option<usize> {
    if byte == 0 {
        return None;
    }
    cells(line)
        .take_while(|cell| cell.at < byte)
        .filter(|cell| cell.wide > 0)
        .last()
        .map(|cell| cell.at)
        .or(Some(0))
}

/// What kind of character stands at a place: none at the end of the line.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Other
    }
}

/// The stops of a line with the class of the character behind each.
fn stops(line: &[u8]) -> Vec<(usize, Class)> {
    cells(line).filter(|cell| cell.wide > 0).map(|cell| (cell.at, class(cell.c))).collect()
}

/// The run of characters of one kind around the character at `byte`: a
/// word, the space between two, or signs. Nothing in an empty line.
fn run_at(line: &[u8], byte: usize) -> (usize, usize) {
    let stops = stops(line);
    let Some(last) = stops.len().checked_sub(1) else { return (0, 0) };
    let at = stops.iter().position(|(at, _)| *at >= byte).unwrap_or(last);
    let class = stops[at].1;
    let from = stops[..at].iter().rposition(|(_, other)| *other != class).map_or(0, |before| before + 1);
    let until = stops[at..].iter().position(|(_, other)| *other != class).map(|behind| at + behind);
    (stops[from].0, until.map_or(stops_end(line), |until| stops[until].0))
}

/// Where the word around or behind `byte` ends.
pub(super) fn word_end(line: &[u8], byte: usize) -> usize {
    let stops = stops(line);
    let mut rest = stops.iter().skip_while(|(at, _)| *at < byte).peekable();
    while rest.next_if(|(_, class)| *class == Class::Space).is_some() {}
    let Some((_, first)) = rest.peek().copied() else { return stops_end(line) };
    while rest.next_if(|(_, class)| *class == *first).is_some() {}
    rest.peek().map_or(stops_end(line), |(at, _)| *at)
}

/// Where the word around or before `byte` begins.
pub(super) fn word_start(line: &[u8], byte: usize) -> usize {
    let stops = stops(line);
    let mut rest = stops.iter().rev().skip_while(|(at, _)| *at >= byte).peekable();
    while rest.next_if(|(_, class)| *class == Class::Space).is_some() {}
    let Some((mut start, first)) = rest.peek().copied().copied() else { return 0 };
    while let Some((at, _)) = rest.next_if(|(_, class)| *class == first) {
        start = *at;
    }
    start
}

/// Where a caret can be sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// To the start and the end of the line.
    Home,
    End,
    /// To the start and the end of the text.
    Top,
    Bottom,
    /// Up and down by this many lines.
    PageUp(usize),
    PageDown(usize),
}

/// What of a text is seen, and the caret and the selection in it.
#[derive(Default)]
pub struct View {
    /// The first line and the first column that are shown.
    pub top: usize,
    pub left: usize,
    pub caret: Pos,
    /// Where a selection began. It reaches to the caret.
    pub anchor: Option<Pos>,
    /// The column the caret keeps to while it goes up and down through
    /// lines that are shorter.
    wanted: Option<usize>,
    /// What a selection by words or lines began with, while it is dragged.
    grain: Option<(Pos, Pos)>,
}

impl View {
    /// The two ends of the selection in their order, if anything is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor.filter(|anchor| *anchor != self.caret)?;
        Some((anchor.min(self.caret), anchor.max(self.caret)))
    }

    fn set(&mut self, caret: Pos, select: bool) {
        if !select {
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = Some(self.caret);
        }
        self.caret = caret;
        self.grain = None;
    }

    /// Moves the caret, with `select` from where a selection begins.
    pub fn go(&mut self, doc: &Document, to: Move, select: bool) {
        let Pos { line, byte } = self.caret;
        let text = doc.line(line);
        let last = doc.lines() - 1;
        // A selection that is let go of leaves the caret at its end.
        if !select && let Some((from, until)) = self.selection() {
            match to {
                Move::Left => return self.set(from, false),
                Move::Right => return self.set(until, false),
                _ => {}
            }
        }
        let vertical = |view: &mut Self, line: usize| {
            let col = *view.wanted.get_or_insert_with(|| col_of(text, byte));
            Pos { line, byte: byte_near(doc.line(line), col as f32) }
        };
        let caret = match to {
            Move::Left => match prev_stop(text, byte) {
                Some(byte) => Pos { line, byte },
                None if line > 0 => Pos { line: line - 1, byte: stops_end(doc.line(line - 1)) },
                None => self.caret,
            },
            Move::Right => match next_stop(text, byte) {
                Some(byte) => Pos { line, byte },
                None if line < last => Pos { line: line + 1, byte: 0 },
                None => self.caret,
            },
            Move::WordLeft if byte == 0 && line > 0 => {
                Pos { line: line - 1, byte: stops_end(doc.line(line - 1)) }
            }
            Move::WordLeft => Pos { line, byte: word_start(text, byte) },
            Move::WordRight if byte >= stops_end(text) && line < last => Pos { line: line + 1, byte: 0 },
            Move::WordRight => Pos { line, byte: word_end(text, byte) },
            Move::Home => {
                // First to where the text of the line begins, then to its start.
                let indent = cells(text).find(|cell| !cell.c.is_whitespace()).map_or(0, |cell| cell.at);
                Pos { line, byte: if byte == indent { 0 } else { indent } }
            }
            Move::End => Pos { line, byte: stops_end(text) },
            Move::Top => Pos::default(),
            Move::Bottom => doc.end(),
            Move::Up if line == 0 => Pos::default(),
            Move::Down if line == last => doc.end(),
            Move::Up => vertical(self, line - 1),
            Move::Down => vertical(self, line + 1),
            Move::PageUp(rows) => vertical(self, line.saturating_sub(rows.max(1))),
            Move::PageDown(rows) => vertical(self, (line + rows.max(1)).min(last)),
        };
        let wanted =
            self.wanted.filter(|_| matches!(to, Move::Up | Move::Down | Move::PageUp(_) | Move::PageDown(_)));
        self.set(caret, select);
        self.wanted = wanted;
    }

    /// Puts the caret where a click was, or with `select` the end of the
    /// selection.
    pub fn place(&mut self, doc: &Document, pos: Pos, select: bool) {
        self.set(doc.within(pos), select);
        self.wanted = None;
    }

    /// Selects the word whose character stands at a place, or the run
    /// of whatever else is there.
    pub fn select_word(&mut self, doc: &Document, pos: Pos) {
        let pos = doc.within(pos);
        let (from, to) = run_at(doc.line(pos.line), pos.byte);
        self.select(Pos { line: pos.line, byte: from }, Pos { line: pos.line, byte: to });
    }

    /// Selects a line with its end.
    pub fn select_line(&mut self, doc: &Document, pos: Pos) {
        let line = pos.line.min(doc.lines() - 1);
        let to = if line + 1 < doc.lines() { Pos { line: line + 1, byte: 0 } } else { doc.last() };
        self.select(Pos { line, byte: 0 }, to);
    }

    pub fn select_all(&mut self, doc: &Document) {
        self.select(Pos::default(), doc.last());
        self.grain = None;
    }

    fn select(&mut self, from: Pos, to: Pos) {
        (self.anchor, self.caret, self.wanted) = (Some(from), to, None);
        self.grain = Some((from, to));
    }

    /// Carries a selection on to where the pointer was dragged. One that
    /// began with a word or a line keeps that word or line.
    pub fn drag(&mut self, doc: &Document, pos: Pos) {
        let pos = doc.within(pos);
        match self.grain {
            Some((from, to)) if pos < from => (self.anchor, self.caret) = (Some(to), pos),
            Some((from, to)) => (self.anchor, self.caret) = (Some(from), pos.max(to)),
            None => {
                self.anchor.get_or_insert(self.caret);
                self.caret = pos;
            }
        }
        self.wanted = None;
    }

    /// The selected text.
    pub fn selected(&self, doc: &Document) -> Option<String> {
        self.selection().map(|(from, to)| doc.text(from, to))
    }

    /// How far down the view can be moved: until the last lines stand at
    /// its top, with a little of the room below them left.
    fn lowest(doc: &Document, rows: usize) -> usize {
        doc.lines().saturating_sub(rows.saturating_sub(2).max(1))
    }

    pub fn scroll_by(&mut self, doc: &Document, lines: isize, rows: usize) {
        self.top = self.top.saturating_add_signed(lines).min(Self::lowest(doc, rows));
    }

    /// Moves the view sideways by `by` columns in a view of `rows` lines and
    /// `cols` columns: to the right until the longest line that is shown
    /// ends in it, with the room of a caret behind it. Returns whether it
    /// moved.
    pub fn sideways(&mut self, doc: &Document, by: isize, rows: usize, cols: usize) -> bool {
        let was = self.left;
        if by <= 0 {
            self.left = was.saturating_sub(by.unsigned_abs());
            return self.left != was;
        }
        let want = was.saturating_add(by.unsigned_abs());
        // No line is looked at further than the view is asked to reach.
        let reach = want.saturating_add(cols);
        let widest = (self.top..doc.lines())
            .take(rows)
            .map(|line| col_until(doc.line(line), reach))
            .max()
            .unwrap_or(0);
        // A view that stands further right already, behind the caret or
        // from longer lines above, is not brought back by a turn that way.
        self.left = want.min((widest + 1).saturating_sub(cols)).max(was);
        self.left != was
    }

    /// Moves the view so far that the caret is in it, in a view of `rows`
    /// lines and `cols` columns.
    pub fn reveal(&mut self, doc: &Document, rows: usize, cols: usize) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let line = self.caret.line;
        if line < self.top {
            self.top = line;
        } else if line >= self.top + rows {
            self.top = line + 1 - rows;
        }
        let col = col_of(doc.line(line), self.caret.byte);
        if col < self.left {
            self.left = col;
        } else if col >= self.left + cols {
            self.left = col + 1 - cols;
        }
    }

    /// Brings the view into a text that was read anew and may be shorter.
    pub fn fit(&mut self, doc: &Document, rows: usize) {
        self.caret = doc.at_stop(self.caret);
        self.anchor = self.anchor.map(|anchor| doc.at_stop(anchor));
        self.top = self.top.min(Self::lowest(doc, rows));
        (self.wanted, self.grain) = (None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::new(text.as_bytes().to_vec(), false).ok().unwrap()
    }

    #[test]
    fn lines_are_found_without_their_ends() {
        let text = doc("one\r\ntwo\n\nlast");
        assert_eq!(text.lines(), 4);
        assert_eq!(
            [text.line(0), text.line(1), text.line(2), text.line(3)],
            [&b"one"[..], b"two", b"", b"last"]
        );
        assert_eq!(text.line(9), b"");
        assert_eq!(text.end(), Pos { line: 3, byte: 4 });
        // A text that ends with a line end has an empty line behind it.
        assert_eq!((doc("a\n").lines(), doc("").lines()), (2, 1));
        // What is copied is what is in the file, line ends and all.
        assert_eq!(text.text(Pos { line: 0, byte: 1 }, Pos { line: 1, byte: 2 }), "ne\r\ntw");
        assert_eq!(text.text(Pos { line: 3, byte: 2 }, Pos { line: 9, byte: 0 }), "st");
    }

    /// A number that follows from the one before, for rows at random
    /// that are the same in every run.
    fn dice(seed: &mut u64) -> usize {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 16) as usize
    }

    #[test]
    fn a_change_moves_the_lines_behind_it_and_is_taken_back_by_another() {
        let mut text = doc("one\r\ntwo\n\nlast");
        let change = text.replace(Pos { line: 0, byte: 1 }, Pos { line: 1, byte: 2 }, b"X\nY").unwrap();
        assert_eq!(text.bytes(), b"oX\nYo\n\nlast");
        assert_eq!((change.at, &change.out[..], change.line), (1, &b"ne\r\ntw"[..], 0));
        assert_eq!(change.end, Pos { line: 1, byte: 1 });
        assert_eq!(
            [text.line(0), text.line(1), text.line(2), text.line(3)],
            [&b"oX"[..], b"Yo", b"", b"last"]
        );
        // What went in goes out again, and what went out goes in.
        let back = text.splice(change.at, 3, &change.out).unwrap();
        assert_eq!((text.bytes(), &back.out[..]), (&b"one\r\ntwo\n\nlast"[..], &b"X\nY"[..]));
        // A place is between two characters of a line that is there: not
        // inside one, not inside the end of a line, not behind the text.
        let mut text = doc("a\u{e4}\r\nb");
        let at = |line, byte| Pos { line, byte };
        for (from, to) in
            [(at(0, 2), at(0, 3)), (at(0, 4), at(1, 0)), (at(1, 0), at(1, 2)), (at(2, 0), at(2, 0))]
        {
            assert_eq!(text.replace(from, to, b"x"), Err(Refused::Place), "{from:?} {to:?}");
        }
        assert_eq!(text.replace(at(1, 0), at(0, 0), b"x"), Err(Refused::Place), "backwards");
        // What goes in is text, and has no zero byte.
        assert_eq!(text.replace(at(0, 0), at(0, 0), b"\xff"), Err(Refused::NoText));
        assert_eq!(text.replace(at(0, 0), at(0, 0), b"a\0b"), Err(Refused::NoText));
        assert_eq!(text.bytes(), "a\u{e4}\r\nb".as_bytes(), "nothing was changed by what was refused");
        // The end of a line goes whole, and with it the line.
        text.replace(at(0, 3), at(1, 0), b"").unwrap();
        assert_eq!((text.bytes(), text.lines()), ("a\u{e4}b".as_bytes(), 1));
    }

    #[test]
    fn a_text_that_is_only_shown_is_not_changed() {
        let none = Pos::default();
        // Cut at a limit: what was written back would be a shorter file.
        let mut cut = Document::new(b"abc".to_vec(), true).ok().unwrap();
        assert_eq!((cut.editable(), cut.replace(none, none, b"x")), (false, Err(Refused::Shown)));
        // Not UTF-8 somewhere behind its start.
        let mut latin = Document::new(b"caf\xe9\n".to_vec(), false).ok().unwrap();
        assert_eq!((latin.editable(), latin.replace(none, none, b"x")), (false, Err(Refused::Shown)));
        // A zero byte behind where a file is looked at for one. Nothing
        // that goes into a text may hold one, so what a key took out
        // with it could not be put back by a step.
        let mut zero = Document::new([&[b'a'; SNIFF][..], b"\nb\0c\n"].concat(), false).ok().unwrap();
        assert_eq!((zero.editable(), zero.replace(none, none, b"x")), (false, Err(Refused::Shown)));
        // The mark at the start of a file is not a part of the text.
        let mut marked = Document::new(b"\xef\xbb\xbfa\n".to_vec(), false).ok().unwrap();
        assert!(marked.editable() && marked.bytes() == b"a\n");
        marked.replace(none, none, b"x").unwrap();
        assert_eq!(marked.bytes(), b"xa\n");
        // What is written back has the mark again, and a text that is
        // only shown gives nothing out to be written.
        assert_eq!(marked.written().as_deref(), Some(&b"\xef\xbb\xbfxa\n"[..]));
        assert_eq!(doc("a").written().as_deref(), Some(&b"a"[..]));
        assert!(cut.written().is_none() && latin.written().is_none() && zero.written().is_none());
    }

    #[test]
    fn no_change_makes_a_line_or_a_text_longer_than_is_looked_at() {
        let at = |line, byte| Pos { line, byte };
        // A line that is too long is not changed, and none becomes so.
        let mut text = doc(&format!("{}\nshort\n", "x".repeat(ROW_BYTES + 1)));
        assert_eq!(text.replace(at(0, 0), at(0, 1), b""), Err(Refused::Line));
        assert_eq!(text.replace(at(0, 0), at(1, 2), b""), Err(Refused::Line), "nor as a part of more");
        assert!(text.replace(at(1, 0), at(1, 0), b"y").is_ok());
        let long = vec![b'y'; ROW_BYTES];
        assert_eq!(text.replace(at(1, 0), at(1, 0), &long), Err(Refused::Line));
        assert_eq!(
            text.replace(at(2, 0), at(2, 0), &[&long[..], b"\n", &long[..], b"z\n"].concat()),
            Err(Refused::Line)
        );
        assert!(text.replace(at(2, 0), at(2, 0), &[&long[..], b"\n", &long[..]].concat()).is_ok());
        // Not more bytes than are read of a file, and not more lines.
        let mut full = doc(&"0123456789abcde\n".repeat(MAX_FILE / 16));
        assert_eq!(full.len(), MAX_FILE);
        assert_eq!(full.replace(at(0, 0), at(0, 0), b"x"), Err(Refused::Limit));
        assert!(full.replace(at(0, 0), at(0, 1), b"x").is_ok());
        let mut lines = Document::new(vec![b'\n'; MAX_LINES - 1], false).ok().unwrap();
        assert_eq!((lines.lines(), lines.cut()), (MAX_LINES, false));
        assert_eq!(lines.replace(at(0, 0), at(0, 0), b"\n"), Err(Refused::Limit));
        assert!(lines.replace(at(0, 0), at(1, 0), b"\n").is_ok());
    }

    #[test]
    fn the_end_of_a_line_does_not_count_into_its_length() {
        let at = |line, byte| Pos { line, byte };
        // A line as long as a line may be, with an end of two bytes, goes
        // out and comes back in: what is taken back was there before.
        let first = format!("{}\r\nb", "x".repeat(ROW_BYTES));
        let mut text = doc(&first);
        let cut = text.replace(at(0, 0), at(1, 0), b"").unwrap();
        assert_eq!(text.bytes(), b"b");
        assert!(text.splice(cut.at, 0, &cut.out).is_ok());
        assert_eq!(text.bytes(), first.as_bytes());
        // In the middle of what goes in, and where it meets the end of
        // the line it goes into.
        let long = "y".repeat(ROW_BYTES);
        let mut text = doc("a\nb");
        assert!(text.replace(at(1, 0), at(1, 0), format!("{long}\r\n{long}\r\n").as_bytes()).is_ok());
        let mut text = doc("\nb");
        assert!(text.replace(at(0, 0), at(0, 0), format!("{long}\r").as_bytes()).is_ok());
        assert_eq!((text.line(0).len(), text.lines()), (ROW_BYTES, 2));
        // A carriage return that ends no line is a byte of the line.
        let mut text = doc("");
        assert_eq!(text.replace(at(0, 0), at(0, 0), format!("{long}\r").as_bytes()), Err(Refused::Line));
        let mut text = doc("\nb");
        assert_eq!(text.replace(at(0, 0), at(0, 0), format!("{long}\rc").as_bytes()), Err(Refused::Line));
        // One that stood in the line ends it when a line feed comes behind.
        let mut text = doc(&format!("{}\r", "z".repeat(ROW_BYTES)));
        assert_eq!(text.replace(at(0, 0), at(0, 0), b"q"), Err(Refused::Line), "one byte too long as it is");
        let mut text = doc(&format!("{}\r", "z".repeat(ROW_BYTES - 1)));
        assert!(text.replace(at(0, ROW_BYTES), at(0, ROW_BYTES), b"\nq").is_ok());
        assert_eq!((text.line(0).len(), text.line(1)), (ROW_BYTES - 1, &b"q"[..]));
    }

    #[test]
    fn the_mark_at_the_start_counts_into_what_a_file_may_hold() {
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend("0123456789abcde\n".repeat(MAX_FILE / 16).bytes().take(MAX_FILE - 3));
        let mut text = Document::new(bytes, false).ok().unwrap();
        assert_eq!((text.len(), text.editable()), (MAX_FILE - 3, true));
        let end = text.end();
        assert_eq!(text.replace(end, end, b"x"), Err(Refused::Limit));
        assert_eq!(text.written().map(|bytes| bytes.len()), Some(MAX_FILE));
    }

    #[test]
    fn rows_of_changes_at_random_leave_the_lines_where_a_new_reading_finds_them() {
        let pieces: [&[u8]; 8] =
            [b"", b"x", b"\n", b"\r\n", "\u{e4}\u{6f22}".as_bytes(), b"ab\ncd", b"\n\n", b"\r"];
        for seed in 1..=40u64 {
            let mut seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let mut text = doc("fn main() {\r\n\tlet x = 1;\n}\n");
            for step in 0..200 {
                let len = text.len();
                let (at, with) = (dice(&mut seed) % (len + 1), pieces[dice(&mut seed) % pieces.len()]);
                let take = (dice(&mut seed) % 6).min(len - at);
                let before = text.bytes().to_vec();
                match text.splice(at, take, with) {
                    Ok(change) => {
                        let mut wanted = before.clone();
                        wanted.splice(at..at + take, with.iter().copied());
                        assert_eq!(text.bytes(), &wanted[..], "seed {seed} step {step}");
                        assert_eq!(change.out, &before[at..at + take]);
                    }
                    // Only a place inside a character is refused here.
                    Err(why) => assert_eq!((why, text.bytes()), (Refused::Place, &before[..])),
                }
                let anew = Document::new(text.bytes().to_vec(), false).ok().unwrap();
                assert_eq!(text.starts, anew.starts, "seed {seed} step {step}");
            }
        }
    }

    /// Prints what a key costs at the top of the largest texts: the bytes
    /// behind it are moved, and the places where lines begin.
    /// `cargo test --release what_a_key_costs -- --ignored --nocapture`
    #[test]
    #[ignore = "prints times to look at"]
    fn what_a_key_costs_at_the_top_of_the_largest_text() {
        let texts = [
            ("8 MiB in lines of 80", "x".repeat(79) + "\n", MAX_FILE / 80),
            ("two million lines of 4", "abc\n".to_owned(), MAX_LINES - 2),
        ];
        for (name, line, count) in texts {
            let mut text = doc(&line.repeat(count));
            let begun = std::time::Instant::now();
            for _ in 0..50 {
                text.splice(0, 0, b"y").unwrap();
                text.splice(0, 1, b"").unwrap();
            }
            let typed = begun.elapsed() / 100;
            let begun = std::time::Instant::now();
            for _ in 0..50 {
                text.splice(0, 0, b"\n").unwrap();
                text.splice(0, 1, b"").unwrap();
            }
            println!("{name}: a character {typed:?}, a line end {:?}", begun.elapsed() / 100);
        }
    }

    #[test]
    fn a_zero_byte_makes_a_file_binary_and_a_cut_character_is_dropped() {
        assert!(Document::new(b"ab\0cd".to_vec(), false).is_err());
        // Cut in the middle of the three bytes of a character.
        let cut = Document::new("aä€".as_bytes()[..5].to_vec(), true).ok().unwrap();
        assert_eq!((cut.line(0), cut.cut()), ("aä".as_bytes(), true));
        // The same bytes at the end of a whole file are the file's own.
        let whole_file = Document::new("aä€".as_bytes()[..5].to_vec(), false).ok().unwrap();
        assert_eq!(whole_file.line(0).len(), 5);
    }

    #[test]
    fn the_lines_of_a_file_have_a_limit() {
        let text = Document::new(vec![b'\n'; MAX_LINES + 50], false).ok().unwrap();
        assert_eq!((text.lines(), text.cut()), (MAX_LINES, true));
        assert_eq!(text.len(), MAX_LINES - 1);
    }

    #[test]
    fn cells_know_tabs_wide_characters_marks_and_what_is_no_text() {
        let line = "a\tb\t漢e\u{301}\u{1b}".as_bytes();
        let seen: Vec<(usize, usize, usize)> =
            cells(line).map(|cell| (cell.at, cell.col, cell.wide)).collect();
        // a, a tab to column 4, b, a tab to column 8, a character two
        // cells wide, e with a mark that takes none, a replacement.
        assert_eq!(
            seen,
            [(0, 0, 1), (1, 1, 3), (2, 4, 1), (3, 5, 3), (4, 8, 2), (7, 10, 1), (8, 11, 0), (10, 11, 1)]
        );
        assert_eq!(cells(line).last().unwrap().c, '\u{fffd}');
        // A byte that is no text is one cell, and what follows is read on.
        let broken: Vec<(char, usize)> = cells(b"a\xffb\xe2\x82").map(|cell| (cell.c, cell.at)).collect();
        assert_eq!(broken, [('a', 0), ('\u{fffd}', 1), ('b', 2), ('\u{fffd}', 3), ('\u{fffd}', 4)]);
        assert_eq!((col_of(line, 4), col_of(line, 7), col_of(line, 99)), (8, 10, 12));
    }

    #[test]
    fn a_click_lands_between_characters_and_never_inside_one() {
        let line = "a\t漢e\u{301}x".as_bytes();
        // Left and right half of a, of the tab, of the wide character.
        assert_eq!([0.2, 0.6, 2.0, 3.0, 4.9, 5.1].map(|x| byte_near(line, x)), [0, 1, 1, 2, 2, 5]);
        // Behind the e the caret stands behind its mark.
        assert_eq!((byte_near(line, 6.6), byte_near(line, 99.0)), (8, 9));
        assert_eq!((next_stop(line, 5), prev_stop(line, 8)), (Some(8), Some(5)));
        assert_eq!((next_stop(line, 9), prev_stop(line, 0)), (None, None));
        // The character under the pointer, whichever half of it.
        assert_eq!([0.9, 1.0, 4.0, 5.9, 6.9, 7.2, 99.0].map(|x| byte_under(line, x)), [0, 1, 2, 2, 5, 8, 8]);
    }

    #[test]
    fn a_line_is_looked_at_only_so_far() {
        let line = vec![b'x'; ROW_BYTES + 100];
        assert_eq!(cells(&line).count(), ROW_BYTES);
        assert_eq!(byte_near(&line, 1e9), ROW_BYTES);
        // Megabytes of marks that take no cell end as well.
        let marks = "\u{301}".repeat(ROW_BYTES);
        assert_eq!(cells(marks.as_bytes()).count(), ROW_BYTES / 2);
    }

    #[test]
    fn the_caret_moves_by_characters_words_and_lines() {
        let text = doc("fn main() {\n\tlet x = 1;\n}");
        let mut view = View::default();
        let at = |view: &View| (view.caret.line, view.caret.byte);
        view.go(&text, Move::WordRight, false);
        assert_eq!(at(&view), (0, 2));
        view.go(&text, Move::WordRight, false);
        assert_eq!(at(&view), (0, 7));
        view.go(&text, Move::WordRight, false);
        assert_eq!(at(&view), (0, 9), "the brackets are a run of their own");
        view.go(&text, Move::WordLeft, false);
        assert_eq!(at(&view), (0, 7));
        view.go(&text, Move::End, false);
        view.go(&text, Move::Right, false);
        assert_eq!(at(&view), (1, 0), "from the end of a line to the next");
        view.go(&text, Move::Left, false);
        assert_eq!(at(&view), (0, 11));
        // Down through a shorter line and on, the column is kept.
        view.go(&text, Move::Down, false);
        assert_eq!(at(&view), (1, 8), "column 11, behind a tab of four cells");
        view.go(&text, Move::Down, false);
        assert_eq!(at(&view), (2, 1));
        view.go(&text, Move::Up, false);
        assert_eq!(at(&view), (1, 8));
        // Home goes to where the text begins, and from there to the start.
        view.go(&text, Move::Home, false);
        assert_eq!(at(&view), (1, 1));
        view.go(&text, Move::Home, false);
        assert_eq!(at(&view), (1, 0));
        view.go(&text, Move::Bottom, false);
        assert_eq!(at(&view), (2, 1));
        view.go(&text, Move::PageUp(40), false);
        assert_eq!(view.caret.line, 0);
    }

    #[test]
    fn a_selection_reaches_from_where_it_began_to_the_caret() {
        let text = doc("one two\nthree");
        let mut view = View::default();
        view.go(&text, Move::WordRight, true);
        view.go(&text, Move::Down, true);
        assert_eq!(view.selected(&text).as_deref(), Some("one two\nthr"));
        // An arrow without Shift lets go of it, at the end it points to.
        view.go(&text, Move::Left, false);
        assert_eq!((view.caret, view.selection()), (Pos::default(), None));

        view.select_word(&text, Pos { line: 0, byte: 5 });
        assert_eq!(view.selected(&text).as_deref(), Some("two"));
        view.select_word(&text, Pos { line: 0, byte: 3 });
        assert_eq!(view.selected(&text).as_deref(), Some(" "), "the room between two words");
        view.select_word(&text, Pos { line: 0, byte: 7 });
        assert_eq!(view.selected(&text).as_deref(), Some("two"), "behind the line is its last word");
        view.select_word(&doc(""), Pos::default());
        assert_eq!(view.selection(), None);
        // Dragged on, the word it began with stays in it, both ways.
        view.select_word(&text, Pos { line: 0, byte: 5 });
        view.drag(&text, Pos { line: 1, byte: 2 });
        assert_eq!(view.selected(&text).as_deref(), Some("two\nth"));
        view.drag(&text, Pos { line: 0, byte: 1 });
        assert_eq!(view.selected(&text).as_deref(), Some("ne two"));

        view.select_line(&text, Pos { line: 0, byte: 3 });
        assert_eq!(view.selected(&text).as_deref(), Some("one two\n"));
        view.select_line(&text, Pos { line: 1, byte: 0 });
        assert_eq!(view.selected(&text).as_deref(), Some("three"));
        view.select_all(&text);
        assert_eq!(view.selected(&text).as_deref(), Some("one two\nthree"));

        // A plain drag begins where the click put the caret.
        view.place(&text, Pos { line: 0, byte: 4 }, false);
        view.drag(&text, Pos { line: 0, byte: 99 });
        assert_eq!(view.selected(&text).as_deref(), Some("two"));
        // A place outside the text is brought into it.
        view.place(&text, Pos { line: 9, byte: 9 }, false);
        assert_eq!(view.caret, Pos { line: 1, byte: 0 });
    }

    #[test]
    fn the_view_follows_the_caret_and_fits_a_shorter_text() {
        let long = doc(&"x\n".repeat(100));
        let mut view = View::default();
        view.go(&long, Move::PageDown(30), false);
        view.reveal(&long, 20, 80);
        assert_eq!((view.caret.line, view.top), (30, 11));
        view.go(&long, Move::Top, false);
        view.reveal(&long, 20, 80);
        assert_eq!(view.top, 0);
        view.scroll_by(&long, 1000, 20);
        assert_eq!(view.top, 101 - 18);
        // Sideways in a line that is longer than the view is wide.
        let wide = doc(&"y".repeat(200));
        let mut view = View::default();
        view.go(&wide, Move::End, false);
        view.reveal(&wide, 20, 80);
        assert_eq!(view.left, 121);
        view.go(&wide, Move::Home, false);
        view.reveal(&wide, 20, 80);
        assert_eq!(view.left, 0);

        // And by the wheel, no further than the longest line shown ends,
        // with the room of a caret behind it.
        assert!(view.sideways(&wide, 50, 20, 80) && view.left == 50);
        assert!(view.sideways(&wide, 500, 20, 80) && view.left == 121);
        assert!(!view.sideways(&wide, 6, 20, 80));
        assert!(view.sideways(&wide, -100, 20, 80) && view.left == 21);
        assert!(view.sideways(&wide, -100, 20, 80) && view.left == 0);
        assert!(!view.sideways(&wide, -6, 20, 80));
        // Only the lines that are shown count, and a view that stands
        // behind them is not brought back by a turn to the right.
        let mixed = doc(&format!("{}\nshort\nshort", "y".repeat(200)));
        let mut view = View { top: 1, ..Default::default() };
        assert!(!view.sideways(&mixed, 6, 20, 80));
        view.left = 100;
        assert!(!view.sideways(&mixed, 6, 20, 80) && view.left == 100);
        assert!(view.sideways(&mixed, -6, 20, 80) && view.left == 94);

        let mut view = View { top: 90, caret: Pos { line: 95, byte: 1 }, ..Default::default() };
        view.fit(&doc("a\nb"), 20);
        assert_eq!((view.top, view.caret), (0, Pos { line: 1, byte: 0 }));
    }

    #[test]
    fn all_of_a_text_is_selected_also_where_a_line_is_longer_than_is_shown() {
        // One line, as a file that was made small for sending is.
        let long = doc(&"x".repeat(ROW_BYTES + 500));
        let mut view = View::default();
        view.select_all(&long);
        assert_eq!(view.selected(&long).map(|text| text.len()), Some(ROW_BYTES + 500));
        view.select_line(&long, Pos::default());
        assert_eq!(view.selected(&long).map(|text| text.len()), Some(ROW_BYTES + 500));
        // The caret stands behind what it can reach, and comes back from there.
        view.go(&long, Move::Left, true);
        assert!(view.caret.byte < ROW_BYTES);
        let two = doc(&format!("head\n{}", "y".repeat(ROW_BYTES + 9)));
        view.select_all(&two);
        assert_eq!(view.selected(&two).map(|text| text.len()), Some(5 + ROW_BYTES + 9));
    }

    #[test]
    fn the_end_of_a_line_is_looked_for_only_as_far_as_is_asked() {
        let line = "a\t漢".repeat(1000);
        assert_eq!(col_until(line.as_bytes(), 3), 4, "the column behind the tab that covers column 3");
        assert_eq!(col_until(b"abc", 80), 3);
        assert_eq!(col_until(b"", 80), 0);
    }

    #[test]
    fn a_carriage_return_is_the_end_of_a_line_only_before_a_line_feed() {
        let text = doc("a\r\nb\rc\r");
        assert_eq!([text.line(0), text.line(1)], [&b"a"[..], b"b\rc\r"]);
        // The last line of a text has no end: what stands there is its own.
        assert_eq!(doc("x\r").line(0), b"x\r");
    }

    #[test]
    fn the_lines_with_their_ends_are_the_text() {
        let bytes = "one\r\ntwo\n\nb\rc\r\nlast\r";
        let text = doc(bytes);
        let ended: Vec<&[u8]> = (0..text.lines()).map(|line| text.ended(line)).collect();
        assert_eq!(ended, [&b"one\r\n"[..], b"two\n", b"\n", b"b\rc\r\n", b"last\r"]);
        assert_eq!(ended.concat(), bytes.as_bytes());
        // A line is its text with the end taken off, and no line is none.
        for line in 0..text.lines() {
            assert!(text.ended(line).starts_with(text.line(line)));
        }
        assert_eq!((text.ended(9), doc("").ended(0), doc("a\n").ended(1)), (&b""[..], &b""[..], &b""[..]));
        // Where the lines were cut off, the last one held has no end.
        let cut = Document::new(vec![b'\n'; MAX_LINES + 50], false).ok().unwrap();
        assert_eq!((cut.ended(0), cut.ended(MAX_LINES - 1)), (&b"\n"[..], &b""[..]));
    }

    #[test]
    fn two_texts_are_alike_up_to_the_line_of_their_first_difference() {
        let one = doc("a\nbb\ncc\n");
        assert_eq!(one.same_lines(&doc("a\nbb\ncc\n")), 4, "all of them, the empty last one too");
        assert_eq!(one.same_lines(&doc("a\nbb\ncX\n")), 2);
        assert_eq!(one.same_lines(&doc("a\nbb\nXc\n")), 2, "at the start of a line");
        assert_eq!(one.same_lines(&doc("X\nbb\ncc\n")), 0);
        // A text that goes on, or ends early: the line where it does differs.
        assert_eq!(one.same_lines(&doc("a\nbb\ncc\ndd")), 3);
        assert_eq!(one.same_lines(&doc("a\nbb\ncc")), 2, "the line lost its end");
        assert_eq!(one.same_lines(&doc("a\nb")), 1);
        assert_eq!(
            (one.same_lines(&doc("")), doc("").same_lines(&one), doc("").same_lines(&doc(""))),
            (0, 0, 1)
        );
    }

    #[test]
    fn a_text_read_again_leaves_no_caret_inside_a_character() {
        let mut view = View {
            caret: Pos { line: 0, byte: 1 },
            anchor: Some(Pos { line: 0, byte: 3 }),
            ..Default::default()
        };
        // Where byte 1 was the second letter, it is now the middle of one.
        view.fit(&doc("\u{e4}\u{20ac}b"), 20);
        assert_eq!((view.caret.byte, view.anchor.map(|anchor| anchor.byte)), (0, Some(2)));
        // A mark that takes no cell is no place for a caret either.
        let mut view = View { caret: Pos { line: 0, byte: 1 }, ..Default::default() };
        view.fit(&doc("e\u{301}x"), 20);
        assert_eq!(view.caret.byte, 0);
        let mut view = View { caret: Pos { line: 0, byte: 3 }, ..Default::default() };
        view.fit(&doc("e\u{301}x"), 20);
        assert_eq!(view.caret.byte, 3);
    }

    #[test]
    fn what_is_drawn_of_a_line_has_an_end_also_where_its_marks_have_none() {
        // A mark goes onto the character before it, a wide one included.
        let seen: Vec<(char, usize)> =
            shown("a\u{6f22}\u{301}b".as_bytes(), 80).map(|cell| (cell.c, cell.base)).collect();
        assert_eq!(seen, [('a', 0), ('\u{6f22}', 1), ('\u{301}', 1), ('b', 3)]);
        // Only what lies before the last column that is shown.
        assert_eq!(shown(&[b'x'; 500], 80).count(), 80);
        // Thousands of marks on one character take no cell and would
        // each be drawn: a line is given up on where it needs more bytes
        // than text needs for the columns that are shown.
        let flood = format!("a{}b", "\u{301}".repeat(30_000));
        let drawn = shown(flood.as_bytes(), 80).count();
        assert!((80..=81 * 32).contains(&drawn), "{drawn}");
        // Scrolled to the right there is room for what lies before.
        let long = "\u{6f22}".repeat(2000);
        assert_eq!(shown(long.as_bytes(), 3000).count(), 1500);
    }
}
