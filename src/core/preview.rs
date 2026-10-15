//! A file of Markdown as it is meant: what its text is made of, as the
//! process for the colors read it, and the rows that are made of that
//! for the width of a pane. Nothing of the renderer is in here: how wide
//! a character is, is asked of whoever calls.
//!
//! What the process says is a list of entries, each a kind and the bytes
//! of the text it spans. Nothing of it is believed before it was held
//! against the text (`Meant::new`), and the rows are made by a loop that
//! takes any list: one that ends what it never began, begins what it
//! never ends, or goes a hundred thousand levels deep.

use super::colors::FILE_MAX;
use super::files::Edition;
use super::text::{self, line_starts};

/// The grammar whose files can be shown as they are meant.
pub const GRAMMAR: &str = "Markdown";
/// Most entries a text is shown by. The largest file this was held
/// against has a twelfth of that, and a text made to harm has one for
/// every byte.
pub const ENTRIES_MAX: usize = 1 << 17;
/// Most bytes of characters an entry carries that stand nowhere in the
/// text: an address written elsewhere in the file, a sign named by `&`.
pub const WORD_MAX: usize = 4 << 10;
/// Most bytes of such characters in all the words of a text together.
/// What the crate hands over so is a sign named by `&` and code inside
/// a line that goes over a line end, never much of a text.
pub const SAID_MAX: usize = 1 << 20;
/// Most columns a table is said to have.
pub const COLUMNS_WIRE: usize = 255;
/// Most columns and cells a table is drawn with. One with more is shown
/// as it is written.
const COLUMNS: usize = 32;
const CELLS: usize = 20_000;
/// How many levels of lists and quotes are indented. Deeper ones stand
/// where the last of these stands.
const DEPTH: usize = 12;
/// Most rows that are made. Behind them a row says that there is more.
const ROWS: usize = 1 << 17;
/// How often the process may end over a file before it is not asked for
/// the structure of that file again, as with the colors.
const TRIES: u8 = 2;

/// A piece of text an entry names.
#[derive(Clone, Debug, PartialEq)]
pub enum Word {
    /// These bytes of the text.
    Here(u32, u32),
    /// Characters that stand nowhere in the text.
    Said(Box<str>),
}

/// How the cells of a column of a table are set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

impl Align {
    pub fn from_byte(byte: u8) -> Option<Self> {
        [Self::None, Self::Left, Self::Center, Self::Right].get(byte as usize).copied()
    }
}

/// What an entry is. Those down to `Other` begin something that an
/// `End` ends, the ones behind `End` stand alone.
#[derive(Clone, Debug, PartialEq)]
pub enum What {
    Paragraph,
    /// A heading of the level 1 to 6.
    Heading(u8),
    /// A quote, and which of five words stands over it: 0 for none.
    Quote(u8),
    /// A code block: the text entries in it are its lines.
    Code,
    /// HTML as a block, shown as it is written.
    Html,
    /// A list, and the number of its first entry if it has numbers.
    List(Option<u64>),
    Item,
    Table(Box<[Align]>),
    /// The row of a table that names its columns.
    Head,
    Row,
    Cell,
    /// The head of a file between two lines of dashes.
    Meta,
    Emphasis,
    Strong,
    Strike,
    /// A link, and where it leads.
    Link(Word),
    /// A picture, and where it lies. What is inside is its text.
    Image(Word),
    /// Something that is not drawn as anything of its own.
    Other,
    End,
    Text(Word),
    /// Code inside a line.
    CodeSpan(Word),
    /// HTML inside a line, shown as it is written.
    InlineHtml(Word),
    /// A line end inside a paragraph, which is a space.
    Soft,
    /// A line end that is meant.
    Hard,
    Rule,
    /// The box of a task, checked or not.
    Task(bool),
}

/// One thing the text is made of, and the bytes of the text it spans.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub what: What,
    pub from: u32,
    pub to: u32,
}

/// Why a file has no preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Why {
    /// It is larger than a text that is shown in colors.
    Long,
    /// Its structure has more to it than is taken.
    Large,
    /// Not all of it is UTF-8.
    NotText,
    /// Reading it took longer than is waited for.
    TookTooLong,
    /// The process ended over it, or could not be started.
    Ended,
    /// An answer did not fit the text or the question.
    Broken,
}

impl Why {
    /// Whether the process was ended over the file: that is counted, and
    /// not tried with every text.
    fn ends(self) -> bool {
        matches!(self, Self::TookTooLong | Self::Ended | Self::Broken)
    }
}

/// A text with what it is made of, held against each other: every entry
/// lies in the text, and every piece it names begins and ends between
/// two characters. There is no other way to make one.
pub struct Meant {
    text: String,
    starts: Vec<u32>,
    entries: Vec<Entry>,
}

impl Meant {
    /// Nothing if the bytes are no text or an entry does not fit them.
    pub fn new(text: Vec<u8>, entries: Vec<Entry>) -> Option<Self> {
        if text.len() > FILE_MAX || entries.len() > ENTRIES_MAX {
            return None;
        }
        let text = String::from_utf8(text).ok()?;
        let within = |from: u32, to: u32| from <= to && to as usize <= text.len();
        let word = |word: &Word| match word {
            Word::Here(from, to) => {
                within(*from, *to)
                    && text.is_char_boundary(*from as usize)
                    && text.is_char_boundary(*to as usize)
            }
            Word::Said(said) => said.len() <= WORD_MAX,
        };
        let fits = |entry: &Entry| {
            within(entry.from, entry.to)
                && match &entry.what {
                    What::Heading(level) => (1..=6).contains(level),
                    What::Quote(kind) => *kind <= 5,
                    What::Table(columns) => columns.len() <= COLUMNS_WIRE,
                    What::Link(to)
                    | What::Image(to)
                    | What::Text(to)
                    | What::CodeSpan(to)
                    | What::InlineHtml(to) => word(to),
                    _ => true,
                }
        };
        if !entries.iter().all(fits) {
            return None;
        }
        // And all of them together name no more than there is. What
        // the crate says names every byte once at the most: twice the
        // text is let through. Each alone lying in the text, a hundred
        // thousand entries can name all of it, and rows would be made
        // of a hundred thousand megabytes.
        let (mut named, mut said) = (0usize, 0usize);
        for entry in &entries {
            match &entry.what {
                What::Text(word) | What::CodeSpan(word) | What::InlineHtml(word) => match word {
                    Word::Here(from, to) => named = named.saturating_add((to - from) as usize),
                    Word::Said(word) => said = said.saturating_add(word.len()),
                },
                _ => {}
            }
        }
        if named > text.len().saturating_mul(2) || said > SAID_MAX {
            return None;
        }
        // Nor are the blocks that are read whole more than there is. A
        // block of HTML is read from end to end to see whether it is a
        // comment, and a table that is shown as it is written is the
        // lines it lies in, each read to its end. In what the crate
        // says no byte stands in two of them: with each alone lying in
        // the text, a hundred thousand of them are a hundred thousand
        // megabytes to read, on the thread of the window.
        let meant = Self { starts: line_starts(text.as_bytes()), text, entries };
        let mut read = 0usize;
        for entry in &meant.entries {
            read = read.saturating_add(match entry.what {
                What::Html => (entry.to - entry.from) as usize,
                What::Table(_) => {
                    let (first, last) = meant.lines_of(entry);
                    meant.start(last + 1) - meant.start(first)
                }
                _ => 0,
            });
        }
        (read <= meant.text.len().saturating_mul(2)).then_some(meant)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The line a byte of the text is in.
    pub fn line_of(&self, byte: u32) -> u32 {
        (self.starts.partition_point(|start| *start <= byte).max(1) - 1) as u32
    }

    /// Where a line begins, and the end of the text for one behind it.
    pub fn start(&self, line: u32) -> usize {
        self.starts.get(line as usize).map_or(self.text.len(), |start| *start as usize)
    }

    fn lines(&self) -> u32 {
        self.starts.len() as u32
    }

    /// The first and the last line an entry lies in.
    fn lines_of(&self, entry: &Entry) -> (u32, u32) {
        (self.line_of(entry.from), self.line_of(entry.to.saturating_sub(1).max(entry.from)))
    }
}

/// A size of type: how large its characters are, how high a row of it
/// is, and where in that row they stand.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Type {
    pub px: f32,
    pub h: f32,
    pub base: f32,
}

/// The sizes a preview is set in, in pixels of the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sizes {
    pub body: Type,
    pub heads: [Type; 6],
    /// A row of code, as a row of a text is, and how wide a cell of it is.
    pub code: Type,
    pub cell: f32,
}

impl Sizes {
    /// The type of a class of rows: 0 for the text, 1 for code, 2 to 7
    /// for the six levels of a heading.
    pub fn of(&self, class: u8) -> Type {
        match class {
            BODY => self.body,
            CODE => self.code,
            _ => self.heads[(class as usize - 2).min(5)],
        }
    }
}

pub const BODY: u8 = 0;
pub const CODE: u8 = 1;

/// How a run looks, as bits.
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
/// In the font of the terminal.
pub const MONO: u8 = 4;
pub const STRUCK: u8 = 8;
/// On a surface of its own, as code inside a line is.
pub const RAISED: u8 = 16;

/// The color of a run, by what it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    Text,
    /// What is quoted.
    Dim,
    /// What is shown as it is written because it is not drawn.
    Faint,
    Link,
    /// A line of a code block, in the colors its line has in the text.
    Code,
}

/// What a run draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drawn {
    /// These bytes of the text.
    Text(u32, u32),
    /// These bytes of what the layout wrote itself: a number, a space.
    Own(u32, u32),
    /// The box of a task.
    Task(bool),
    /// The sign that a picture stands here.
    Picture,
}

/// A piece of a row that is drawn in one way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    /// Where it begins, from where the rows begin, and how wide it is.
    pub x: f32,
    pub w: f32,
    pub what: Drawn,
    pub look: u8,
    pub class: u8,
    pub tone: Tone,
}

/// What lies under a row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ground {
    None,
    /// The surface of a code block.
    Code,
    /// The surface of the row that names the columns of a table.
    Head,
    /// A line across: the row is a rule.
    Rule,
}

/// A row as it is drawn. It is drawn whole or not at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    /// Room above it that belongs to it, and its own height.
    pub above: f32,
    pub h: f32,
    /// Where the characters stand, from its top.
    pub base: f32,
    /// Where its ground begins, from where the rows begin, and how wide
    /// it is: nothing for one that reaches to the right edge.
    pub x: f32,
    pub w: f32,
    /// The line of the text it comes from.
    pub line: u32,
    pub ground: Ground,
    /// A hairline under it: it ends a row of a table.
    pub under: bool,
    runs: (u32, u32),
}

/// The rows of a text for one width.
pub struct Laid {
    rows: Vec<Row>,
    runs: Vec<Run>,
    own: String,
}

impl Laid {
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn runs(&self, row: &Row) -> &[Run] {
        self.runs.get(row.runs.0 as usize..row.runs.1 as usize).unwrap_or_default()
    }

    /// What the layout wrote itself.
    pub fn own(&self) -> &str {
        &self.own
    }

    /// The first row that comes from this line of the text or from one
    /// behind it, or the last row there is.
    pub fn row_at(&self, line: u32) -> usize {
        self.rows.partition_point(|row| row.line < line).min(self.rows.len().saturating_sub(1))
    }

    /// The first byte of the text that a row draws, where it draws any.
    fn byte_of(&self, row: &Row) -> Option<u32> {
        self.runs(row).iter().find_map(|run| match run.what {
            Drawn::Text(from, _) => Some(from),
            _ => None,
        })
    }

    /// Where a row stands in the text, to find it again in other rows.
    fn place(&self, row: usize) -> Option<Place> {
        let row = self.rows.get(row)?;
        Some(Place { line: row.line, byte: self.byte_of(row) })
    }

    /// The row that stands at a place: of the rows of its line the last
    /// that begins at its byte or before it.
    fn row_of(&self, place: Place) -> usize {
        let first = self.row_at(place.line);
        let Some(byte) = place.byte else { return first };
        let mut found = first;
        for (at, row) in self.rows.iter().enumerate().skip(first) {
            if row.line != place.line {
                break;
            }
            match self.byte_of(row) {
                Some(from) if from > byte => break,
                Some(_) => found = at,
                None => {}
            }
        }
        found
    }

    /// The highest row from which all rows down to `top` are seen in a
    /// room that high: where a page up leads, so that the row at the top
    /// is the last one seen after it. The rows are of differing height,
    /// and as many rows up as are seen below can be more than fit.
    pub fn before(&self, top: usize, height: f32) -> usize {
        let mut left = height;
        let mut first = top.min(self.rows.len().saturating_sub(1));
        for (at, row) in self.rows.iter().enumerate().take(top.saturating_add(1)).rev() {
            left -= row.h;
            if left < 0.0 {
                break;
            }
            first = at;
            left -= row.above;
        }
        first
    }

    /// How many rows are seen from `top` in a room that high. The row at
    /// the top is drawn without the room above it.
    pub fn seen(&self, top: usize, height: f32) -> usize {
        let mut left = height;
        let fits = |(at, row): &(usize, &Row)| {
            left -= row.h + if *at == 0 { 0.0 } else { row.above };
            left >= 0.0
        };
        self.rows.get(top..).unwrap_or_default().iter().enumerate().take_while(fits).count()
    }

    /// The lowest row the view begins at in a room that high: the one
    /// from which the rows to the last just fit.
    pub fn lowest(&self, height: f32) -> usize {
        let mut left = height;
        let mut lowest = self.rows.len().saturating_sub(1);
        for (at, row) in self.rows.iter().enumerate().rev() {
            left -= row.h;
            if left < 0.0 {
                break;
            }
            lowest = at;
            left -= row.above;
        }
        lowest
    }
}

/// Most runs and signs of one row that are drawn. Signs that take no
/// room can be had by the hundred thousand for one row, marks that go
/// onto the sign before them, and each would be drawn in every frame.
pub const ROW_DRAWN: usize = 4096;

/// Where the view of a preview stands, to stand there in other rows:
/// the line of the text its top row comes from and, where that row
/// draws text, the first byte of it. A paragraph that is written in one
/// line is many rows of that line, and by the line alone the view
/// stood at the first of them after every reading and every width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    line: u32,
    byte: Option<u32>,
}

/// A character as a preview draws it: what ends a line or moves the pen
/// is a space, and what turns the writing round or is no sign is not
/// drawn.
pub fn shown(c: char) -> Option<char> {
    match c {
        '\t' | '\n' | '\r' => Some(' '),
        '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => None,
        c if c.is_control() => None,
        c => Some(c),
    }
}

/// A piece of a paragraph before it is set in rows.
#[derive(Clone, Copy)]
struct Span {
    what: Piece,
    look: u8,
    tone: Tone,
}

#[derive(Clone, Copy)]
enum Piece {
    Drawn(Drawn),
    /// The row ends here.
    Break,
}

/// A row of a paragraph: its runs from its left edge, and how far the
/// last sign of it reaches.
struct Line {
    runs: Vec<Run>,
    ink: f32,
}

/// What is open while the entries are gone through, innermost last.
enum Open {
    Paragraph,
    Heading(u8),
    Cell,
    /// A quote or an entry of a list: by how much it moved the left edge.
    Quote(f32),
    Item(f32),
    List,
    /// A block whose text entries are its lines: in this tone, on this
    /// ground.
    Written(Tone, Ground),
    /// What is left out: a comment.
    Skip,
    Table,
    Look(u8),
    Link,
    Other,
}

struct TableRow {
    head: bool,
    line: u32,
    cells: Vec<Vec<Span>>,
}

struct Table {
    columns: Vec<Align>,
    rows: Vec<TableRow>,
    cells: usize,
    /// The lines of the text it is written in.
    lines: (u32, u32),
}

struct Layout<'a> {
    meant: &'a Meant,
    sizes: Sizes,
    width: f32,
    measure: &'a mut dyn FnMut(char, u8, f32) -> f32,
    rows: Vec<Row>,
    runs: Vec<Run>,
    own: String,
    open: Vec<Open>,
    spans: Vec<Span>,
    /// The line of the text the spans began in.
    line: u32,
    indent: f32,
    /// How many quotes and lists are open that moved the left edge.
    depth: usize,
    quotes: usize,
    lists: Vec<Option<u64>>,
    looks: [u32; 3],
    links: u32,
    /// What stands before the first row of an entry of a list.
    marker: Option<Run>,
    /// Room that the next row has above it.
    gap: f32,
    table: Option<Table>,
    /// The last line of a block that is written went on without an end.
    midline: bool,
    /// No row of the block that is written was made yet, and how many
    /// rows there were when it began.
    fresh: bool,
    began: usize,
    full: bool,
}

/// Makes the rows of a text for a width. `measure` says how wide a
/// character is in a look and a size.
pub fn lay(meant: &Meant, sizes: &Sizes, width: f32, measure: &mut dyn FnMut(char, u8, f32) -> f32) -> Laid {
    let mut layout = Layout {
        meant,
        sizes: *sizes,
        width: width.max(1.0),
        measure,
        rows: Vec::new(),
        runs: Vec::new(),
        // A space, for the line end inside a paragraph.
        own: " ".to_owned(),
        open: Vec::new(),
        spans: Vec::new(),
        line: 0,
        indent: 0.0,
        depth: 0,
        quotes: 0,
        lists: Vec::new(),
        looks: [0; 3],
        links: 0,
        marker: None,
        gap: 0.0,
        table: None,
        midline: false,
        fresh: false,
        began: 0,
        full: false,
    };
    for entry in meant.entries() {
        if layout.full {
            break;
        }
        layout.take(entry);
    }
    layout.flush();
    layout.end()
}

impl Layout<'_> {
    fn em(&self) -> f32 {
        self.sizes.body.px
    }

    /// The room between two blocks.
    fn block(&self) -> f32 {
        (self.sizes.body.h * 0.55).round()
    }

    fn look(&self) -> u8 {
        let bit = |count: u32, bit: u8| if count > 0 { bit } else { 0 };
        bit(self.looks[0], BOLD) | bit(self.looks[1], ITALIC) | bit(self.looks[2], STRUCK)
    }

    fn tone(&self) -> Tone {
        match (self.links > 0, self.quotes > 0) {
            (true, _) => Tone::Link,
            (false, true) => Tone::Dim,
            (false, false) => Tone::Text,
        }
    }

    /// Room above the next row, unless it is the first of all.
    fn space(&mut self, room: f32) {
        if !self.rows.is_empty() {
            self.gap = self.gap.max(room);
        }
    }

    /// Puts a row with these runs, each from the left edge that holds now.
    fn row(&mut self, runs: &[Run], kind: Type, line: u32, ground: Ground) {
        if self.rows.len() >= ROWS {
            self.full = true;
            return;
        }
        let from = self.runs.len() as u32;
        if let Some(marker) = self.marker.take() {
            self.runs.push(marker);
        }
        let indent = self.indent;
        self.runs.extend(runs.iter().map(|run| Run { x: run.x + indent, ..*run }));
        self.rows.push(Row {
            above: std::mem::take(&mut self.gap),
            h: kind.h,
            base: kind.base,
            x: indent,
            w: 0.0,
            line,
            ground,
            under: false,
            runs: (from, self.runs.len() as u32),
        });
    }

    /// How many rows can still be made, and one more: the one that says
    /// that the rows end there.
    fn left(&self) -> usize {
        ROWS.saturating_sub(self.rows.len()).saturating_add(1)
    }

    fn own(&mut self, text: &str) -> Drawn {
        let from = self.own.len() as u32;
        self.own.push_str(text);
        Drawn::Own(from, self.own.len() as u32)
    }

    fn drawn(&mut self, word: &Word) -> Drawn {
        match word {
            Word::Here(from, to) => Drawn::Text(*from, *to),
            Word::Said(said) => self.own(said),
        }
    }

    /// By how much a quote or an entry moves the left edge: by nothing
    /// where it is too deep, or too little room would be left.
    fn step(&self, by: f32) -> f32 {
        if self.depth >= DEPTH || self.width - self.indent - by < 8.0 * self.em() { 0.0 } else { by }
    }

    fn take(&mut self, entry: &Entry) {
        let line = self.meant.line_of(entry.from);
        match &entry.what {
            What::Paragraph => {
                self.flush();
                self.space(self.block());
                self.open.push(Open::Paragraph);
            }
            What::Heading(level) => {
                self.flush();
                self.space((self.sizes.body.h * 0.9).round());
                self.open.push(Open::Heading(*level));
            }
            What::Quote(kind) => {
                self.flush();
                self.space(self.block());
                let by = self.step((1.2 * self.em()).round());
                self.indent += by;
                self.depth += 1;
                self.quotes += 1;
                self.open.push(Open::Quote(by));
                let word = ["", "Note", "Tip", "Important", "Warning", "Caution"];
                if let Some(word) = word.get(*kind as usize).filter(|word| !word.is_empty()) {
                    let what = Piece::Drawn(self.own(word));
                    self.spans.push(Span { what, look: BOLD, tone: Tone::Dim });
                    self.line = line;
                    self.flush();
                }
            }
            What::Code => self.written(Tone::Code, Ground::Code),
            What::Meta => self.written(Tone::Faint, Ground::Code),
            What::Html => {
                let text = self.meant.text().get(entry.from as usize..entry.to as usize);
                if text.is_some_and(comment) {
                    self.flush();
                    self.open.push(Open::Skip);
                } else {
                    self.written(Tone::Faint, Ground::None);
                }
            }
            What::List(first) => {
                self.flush();
                // A list inside an entry follows the words of that entry.
                if self.lists.is_empty() {
                    self.space(self.block());
                }
                self.lists.push(*first);
                self.open.push(Open::List);
            }
            What::Item => self.item(),
            What::Table(columns) => {
                self.flush();
                self.space(self.block());
                let lines = self.meant.lines_of(entry);
                self.table = Some(Table { columns: columns.to_vec(), rows: Vec::new(), cells: 0, lines });
                self.open.push(Open::Table);
            }
            What::Head | What::Row => {
                self.spans.clear();
                let head = entry.what == What::Head;
                if let Some(table) = &mut self.table {
                    table.rows.push(TableRow { head, line, cells: Vec::new() });
                }
                self.open.push(Open::Other);
            }
            What::Cell => {
                self.spans.clear();
                self.open.push(Open::Cell);
            }
            What::Emphasis => self.looked(1),
            What::Strong => self.looked(0),
            What::Strike => self.looked(2),
            What::Link(_) => {
                self.links += 1;
                self.open.push(Open::Link);
            }
            What::Image(_) => {
                self.span(Drawn::Picture, 0, Tone::Link, line);
                self.links += 1;
                self.open.push(Open::Link);
            }
            What::Other => self.open.push(Open::Other),
            What::End => self.ended(),
            What::Text(word) => match self.open.last() {
                Some(Open::Written(tone, ground)) => {
                    let (tone, ground) = (*tone, *ground);
                    self.lines(word, line, tone, ground);
                }
                Some(Open::Skip) => {}
                _ => {
                    let (what, look, tone) = (self.drawn(word), self.look(), self.tone());
                    self.span(what, look, tone, line);
                }
            },
            What::CodeSpan(word) => {
                let (what, look, tone) = (self.drawn(word), self.look() | MONO | RAISED, self.tone());
                self.span(what, look, tone, line);
            }
            What::InlineHtml(word) => {
                let text = match word {
                    Word::Here(from, to) => self.meant.text().get(*from as usize..*to as usize),
                    Word::Said(said) => Some(&**said),
                };
                if !text.is_some_and(comment) {
                    let what = self.drawn(word);
                    self.span(what, MONO, Tone::Faint, line);
                }
            }
            What::Soft => {
                let (look, tone) = (self.look(), self.tone());
                self.span(Drawn::Own(0, 1), look, tone, line);
            }
            What::Hard => {
                if !self.spans.is_empty() {
                    self.spans.push(Span { what: Piece::Break, look: 0, tone: Tone::Text });
                }
            }
            What::Rule => {
                self.flush();
                self.space(self.block());
                self.row(&[], self.sizes.body, line, Ground::Rule);
            }
            What::Task(checked) => {
                // The box stands where the sign of the entry would, and
                // ends a little before its words.
                let (side, room) = ((0.85 * self.em()).round(), (0.5 * self.em()).round());
                let x = (self.indent - room - side).max(0.0);
                match &mut self.marker {
                    Some(marker) => (marker.what, marker.x, marker.w) = (Drawn::Task(*checked), x, side),
                    None => self.span(Drawn::Task(*checked), 0, Tone::Text, line),
                }
            }
        }
    }

    fn looked(&mut self, which: usize) {
        self.looks[which] += 1;
        self.open.push(Open::Look(which as u8));
    }

    fn span(&mut self, what: Drawn, look: u8, tone: Tone, line: u32) {
        if self.spans.is_empty() {
            self.line = line;
        }
        self.spans.push(Span { what: Piece::Drawn(what), look, tone });
    }

    /// Begins a block whose text entries are its lines.
    fn written(&mut self, tone: Tone, ground: Ground) {
        self.flush();
        self.space(self.block());
        (self.midline, self.fresh, self.began) = (false, true, self.rows.len());
        self.open.push(Open::Written(tone, ground));
    }

    /// Begins an entry of a list: its sign or its number waits for the
    /// first row of it.
    fn item(&mut self) {
        self.flush();
        let em = self.em();
        let number = self.lists.last_mut().and_then(|next| {
            let number = (*next)?;
            *next = Some(number.saturating_add(1));
            Some(number)
        });
        let text = number.map_or_else(|| "•".to_owned(), |number| format!("{number}."));
        let look = 0;
        let mut wide = |text: &str| text.chars().map(|c| (self.measure)(c, look, em)).sum::<f32>();
        let width = wide(&text);
        // The words of a list with numbers begin where two digits leave
        // them room, so that nine and ten stand under each other.
        let least = if number.is_some() { wide("00.") } else { 0.0 };
        let by = self.step((width.max(least) + 0.6 * em).max(1.5 * em).round());
        // A number ends where the words begin, less a little room.
        let x = self.indent + (by - 0.5 * em - width).max(0.0);
        let what = self.own(&text);
        self.marker = Some(Run { x, w: width, what, look, class: BODY, tone: Tone::Dim });
        self.indent += by;
        self.depth += 1;
        self.open.push(Open::Item(by));
    }

    fn ended(&mut self) {
        match self.open.pop() {
            Some(Open::Paragraph) => self.flush(),
            Some(Open::Heading(level)) => {
                let spans = std::mem::take(&mut self.spans);
                self.paragraph(&spans, level.clamp(1, 6) + 1, BOLD);
                self.spans = spans;
                self.spans.clear();
            }
            Some(Open::Cell) => {
                let spans = std::mem::take(&mut self.spans);
                if let Some(table) = &mut self.table
                    && let Some(row) = table.rows.last_mut()
                {
                    table.cells += 1;
                    row.cells.push(spans);
                }
            }
            Some(Open::Quote(by)) => {
                self.flush();
                self.indent = (self.indent - by).max(0.0);
                self.depth = self.depth.saturating_sub(1);
                self.quotes = self.quotes.saturating_sub(1);
            }
            Some(Open::Item(by)) => {
                self.flush();
                // An entry with nothing in it still shows its sign.
                if self.marker.is_some() {
                    let line = self.rows.last().map_or(0, |row| row.line);
                    self.indent = (self.indent - by).max(0.0);
                    self.row(&[], self.sizes.body, line, Ground::None);
                    self.indent += by;
                }
                self.indent = (self.indent - by).max(0.0);
                self.depth = self.depth.saturating_sub(1);
            }
            Some(Open::List) => {
                self.flush();
                self.lists.pop();
            }
            Some(Open::Written(_, ground)) => {
                // The surface of a code block reaches a little above its
                // first line and below its last.
                let by = (0.35 * self.sizes.code.h).round();
                if ground == Ground::Code
                    && self.rows.len() > self.began
                    && let Some(last) = self.rows.last_mut()
                {
                    last.h += by;
                }
                (self.midline, self.fresh) = (false, false);
            }
            Some(Open::Table) => {
                if let Some(table) = self.table.take() {
                    self.tabled(table);
                }
            }
            Some(Open::Look(which)) => {
                if let Some(count) = self.looks.get_mut(which as usize) {
                    *count = count.saturating_sub(1);
                }
            }
            Some(Open::Link) => self.links = self.links.saturating_sub(1),
            Some(Open::Skip | Open::Other) | None => {}
        }
    }

    /// The words that were gathered become a paragraph.
    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        self.paragraph(&spans, BODY, 0);
        self.spans = spans;
        self.spans.clear();
    }

    fn paragraph(&mut self, spans: &[Span], class: u8, look: u8) {
        let kind = self.sizes.of(class);
        let avail = (self.width - self.indent).max(1.0);
        let (text, most) = (self.meant.text(), self.left());
        let lines = wrap(text, &self.own, &mut *self.measure, spans, class, look, kind.px, avail, most);
        let mut block = self.line;
        for line in &lines {
            // The line of the text its first words stand in. A row that
            // draws nothing of the text is of the line of the row before
            // it, and no row of a line before that: a row is found by
            // its line, among rows that stand in the order of the lines.
            let from = line.runs.iter().find_map(|run| match run.what {
                Drawn::Text(from, _) => Some(self.meant.line_of(from)),
                _ => None,
            });
            block = from.map_or(block, |from| from.max(block));
            self.row(&line.runs, kind, block, Ground::None);
        }
    }

    /// The lines of a block that is shown as it is written: every line
    /// of the text a row, not wrapped.
    fn lines(&mut self, word: &Word, line: u32, tone: Tone, ground: Ground) {
        let (own, from, to) = match word {
            Word::Here(from, to) => (false, *from as usize, *to as usize),
            Word::Said(said) => {
                let Drawn::Own(from, to) = self.own(said) else { return };
                (true, from as usize, to as usize)
            }
        };
        let mut at = from;
        while at < to && !self.full {
            // What of the text this row is: how long, how many cells
            // wide, whether its line ends, and where the next begins.
            let (length, cols, ends, next) = {
                let text = if own { self.own.as_str() } else { self.meant.text() };
                let Some(rest) = text.get(at..to) else { return };
                let (piece, ends) = match rest.find('\n') {
                    Some(end) => (&rest[..end], true),
                    None => (rest, false),
                };
                let next = at + piece.len() + ends as usize;
                let piece = piece.strip_suffix('\r').unwrap_or(piece);
                let cols = text::cells(piece.as_bytes()).last().map_or(0, |cell| cell.col + cell.wide);
                (piece.len(), cols, ends, next)
            };
            let what = match own {
                true => Drawn::Own(at as u32, (at + length) as u32),
                false => Drawn::Text(at as u32, (at + length) as u32),
            };
            let line = if own { line } else { self.meant.line_of(at as u32) };
            let w = cols as f32 * self.sizes.cell;
            let mut run = Run { x: 0.0, w, what, look: MONO, class: CODE, tone };
            // A line that went on without an end goes on in its row.
            let last = self.rows.last().filter(|_| self.midline).map(|row| row.runs);
            match last {
                Some((first, end)) if end as usize == self.runs.len() => {
                    let before = self.runs.get(first as usize..end as usize).and_then(|runs| runs.last());
                    run.x = before.map_or(self.indent, |before| before.x + before.w);
                    if length > 0 {
                        self.runs.push(run);
                        if let Some(row) = self.rows.last_mut() {
                            row.runs.1 += 1;
                        }
                    }
                }
                _ => {
                    if ground == Ground::Code {
                        run.x = (self.sizes.cell * 0.9).round();
                    }
                    let runs = if length == 0 { &[][..] } else { std::slice::from_ref(&run) };
                    self.row(runs, self.sizes.code, line, ground);
                    // The surface begins a little above the first line.
                    if std::mem::take(&mut self.fresh)
                        && ground == Ground::Code
                        && let Some(row) = self.rows.last_mut()
                    {
                        let by = (0.35 * self.sizes.code.h).round();
                        (row.h, row.base) = (row.h + by, row.base + by);
                    }
                }
            }
            self.midline = !ends;
            at = next;
        }
    }

    /// Lines of the text as they are written, for what is not drawn as
    /// it is meant.
    fn as_written(&mut self, lines: (u32, u32)) {
        let last = lines.1.min(self.meant.lines().saturating_sub(1));
        for line in lines.0..=last {
            if self.full {
                break;
            }
            let (from, to) = (self.meant.start(line), self.meant.start(line + 1));
            let Some(text) = self.meant.text().get(from..to) else { break };
            let text = text.trim_end_matches(['\n', '\r']);
            let cols = text::cells(text.as_bytes()).last().map_or(0, |cell| cell.col + cell.wide);
            let what = Drawn::Text(from as u32, (from + text.len()) as u32);
            let w = cols as f32 * self.sizes.cell;
            let run = Run { x: 0.0, w, what, look: MONO, class: CODE, tone: Tone::Text };
            self.row(std::slice::from_ref(&run), self.sizes.code, line, Ground::None);
        }
    }

    /// Sets a table: every column as wide as its widest cell, and where
    /// that is more than the room, the widest columns share what the
    /// others leave and their cells are wrapped. A table for which even
    /// that is too little room, or that is larger than is laid out, is
    /// shown as it is written.
    fn tabled(&mut self, table: Table) {
        let (em, count) = (self.em(), table.columns.len());
        // Room between two columns, and before the first and behind the
        // last, so that no word stands at the edge of a surface.
        let (between, inset) = ((1.4 * em).round(), (0.6 * em).round());
        let avail = self.width - self.indent - between * count.saturating_sub(1) as f32 - 2.0 * inset;
        let ragged = table.rows.iter().any(|row| row.cells.len() > count);
        if count == 0 || count > COLUMNS || table.cells > CELLS || ragged || avail < em {
            return self.as_written(table.lines);
        }
        let px = self.sizes.body.px;
        let mut natural = vec![0f32; count];
        for row in &table.rows {
            let look = if row.head { BOLD } else { 0 };
            for (cell, widest) in row.cells.iter().zip(&mut natural) {
                let (text, most) = (self.meant.text(), self.left());
                let lines = wrap(text, &self.own, &mut *self.measure, cell, BODY, look, px, f32::MAX, most);
                *widest = lines.iter().map(|line| line.ink).fold(*widest, f32::max);
            }
        }
        // Columns stand at whole pixels, so the room is reckoned with
        // their widths made whole: with their parts it was a pixel too
        // little for every column.
        natural.iter_mut().for_each(|width| *width = width.ceil());
        let least: f32 = natural.iter().map(|width| width.min((6.0 * em).ceil())).sum();
        if least > avail {
            return self.as_written(table.lines);
        }
        let widths = shared(&natural, avail);
        let mut left = Vec::with_capacity(count);
        let mut x = inset;
        for width in &widths {
            left.push(x);
            x += width + between;
        }
        // As wide as its columns, with the room behind the last.
        let across = (x - between + inset).max(0.0);
        let kind = self.sizes.body;
        let mut runs = Vec::new();
        for row in &table.rows {
            let look = if row.head { BOLD } else { 0 };
            let cells: Vec<Vec<Line>> = row
                .cells
                .iter()
                .zip(&widths)
                .map(|(cell, width)| {
                    let (text, room, most) = (self.meant.text(), width.max(1.0), self.left());
                    wrap(text, &self.own, &mut *self.measure, cell, BODY, look, px, room, most)
                })
                .collect();
            let high = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
            for at in 0..high {
                runs.clear();
                for (column, lines) in cells.iter().enumerate() {
                    let Some(line) = lines.get(at) else { continue };
                    let spare = (widths[column] - line.ink).max(0.0);
                    let shift = match table.columns[column] {
                        Align::Center => (spare / 2.0).floor(),
                        Align::Right => spare.floor(),
                        _ => 0.0,
                    };
                    runs.extend(line.runs.iter().map(|run| Run { x: run.x + left[column] + shift, ..*run }));
                }
                let ground = if row.head { Ground::Head } else { Ground::None };
                self.row(&runs, kind, row.line, ground);
                if let Some(last) = self.rows.last_mut() {
                    (last.w, last.under) = (across, at + 1 == high);
                }
            }
        }
    }

    fn end(mut self) -> Laid {
        if self.full {
            let what = self.own("The preview ends here: the text has more rows than are laid out.");
            let run = Run { x: 0.0, w: 0.0, what, look: 0, class: BODY, tone: Tone::Faint };
            self.runs.push(run);
            let at = self.runs.len() as u32;
            if let Some(last) = self.rows.last_mut() {
                *last = Row {
                    above: last.above,
                    h: self.sizes.body.h,
                    base: self.sizes.body.base,
                    x: 0.0,
                    w: 0.0,
                    line: last.line,
                    ground: Ground::None,
                    under: false,
                    runs: (at - 1, at),
                };
            }
        }
        Laid { rows: self.rows, runs: self.runs, own: self.own }
    }
}

/// Whether HTML is a comment and nothing else: that is left out, as it
/// is meant to be.
fn comment(text: &str) -> bool {
    let text = text.trim();
    text.strip_prefix("<!--")
        .and_then(|rest| rest.strip_suffix("-->"))
        .is_some_and(|inside| !inside.contains("-->"))
}

/// The widths of columns that are too wide together for `room`: those
/// that are narrower than an even share keep their width, and the others
/// share what is left.
fn shared(natural: &[f32], room: f32) -> Vec<f32> {
    let mut widths = natural.to_vec();
    if natural.iter().sum::<f32>() <= room {
        return widths;
    }
    let mut open: Vec<usize> = (0..natural.len()).collect();
    let mut room = room;
    while !open.is_empty() {
        let share = room / open.len() as f32;
        let (fit, wide): (Vec<usize>, Vec<usize>) = open.iter().partition(|at| natural[**at] <= share);
        if fit.is_empty() {
            wide.iter().for_each(|at| widths[*at] = share.floor().max(1.0));
            break;
        }
        room -= fit.iter().map(|at| natural[*at]).sum::<f32>();
        open = wide;
    }
    widths
}

/// A stretch of a span that is all space or all something else.
#[derive(Clone, Copy)]
struct Frag {
    span: usize,
    from: u32,
    to: u32,
    w: f32,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Word,
    /// Space that is as wide as it is written, and may join the run
    /// before it: one space, or the spaces of code.
    Space,
    /// Space of several signs, which counts as one.
    Gap,
    /// A sign that is not text: the box of a task, the sign of a picture.
    Sign,
    Break,
}

/// A run goes on over a stretch that follows it.
fn extend(run: &mut Run, frag: &Frag) {
    if let Drawn::Text(_, to) | Drawn::Own(_, to) = &mut run.what {
        (*to, run.w) = (frag.to, run.w + frag.w);
    }
}

/// Sets spans in lines no wider than `avail`: broken at spaces, and a
/// word that is wider than a line inside the word. `look` is added to
/// that of every span. No more than `most` lines are made: those behind
/// them would be rows that are not kept, and a word of a megabyte in a
/// room one sign wide is a line for every sign of it.
#[allow(clippy::too_many_arguments)]
fn wrap(
    text: &str,
    own: &str,
    measure: &mut dyn FnMut(char, u8, f32) -> f32,
    spans: &[Span],
    class: u8,
    look: u8,
    px: f32,
    avail: f32,
    most: usize,
) -> Vec<Line> {
    let piece = |what: Drawn| match what {
        Drawn::Text(from, to) => text.get(from as usize..to as usize),
        Drawn::Own(from, to) => own.get(from as usize..to as usize),
        _ => None,
    };
    // Cut into words and spaces first.
    let mut frags: Vec<Frag> = Vec::new();
    for (at, span) in spans.iter().enumerate() {
        let face = (span.look | look) & (BOLD | ITALIC | MONO);
        let drawn = match span.what {
            Piece::Break => {
                frags.push(Frag { span: at, from: 0, to: 0, w: 0.0, kind: Kind::Break });
                continue;
            }
            Piece::Drawn(drawn) => drawn,
        };
        let (start, slice) = match drawn {
            Drawn::Text(from, _) | Drawn::Own(from, _) => (from, piece(drawn).unwrap_or("")),
            Drawn::Task(_) => {
                frags.push(Frag { span: at, from: 0, to: 0, w: (1.3 * px).round(), kind: Kind::Sign });
                continue;
            }
            Drawn::Picture => {
                frags.push(Frag { span: at, from: 0, to: 0, w: (1.4 * px).round(), kind: Kind::Sign });
                continue;
            }
        };
        let code = span.look & RAISED != 0;
        let space = measure(' ', face, px);
        let mut open: Option<(Frag, bool)> = None;
        for (offset, c) in slice.char_indices() {
            let is_space = c.is_whitespace();
            let (from, to) = (start + offset as u32, start + (offset + c.len_utf8()) as u32);
            let width = match shown(c) {
                _ if is_space => space,
                Some(c) => measure(c, face, px),
                None => 0.0,
            };
            match &mut open {
                Some((frag, spaces)) if *spaces == is_space => {
                    frag.to = to;
                    // The spaces of code are as wide as they are written.
                    // Elsewhere several signs of space count as one.
                    if !is_space || (code && c == ' ' && frag.kind == Kind::Space) {
                        frag.w += width;
                    } else {
                        (frag.kind, frag.w) = (Kind::Gap, space);
                    }
                }
                _ => {
                    frags.extend(open.take().map(|(frag, _)| frag));
                    let kind = match (is_space, c) {
                        (false, _) => Kind::Word,
                        (true, ' ') => Kind::Space,
                        (true, _) => Kind::Gap,
                    };
                    open = Some((Frag { span: at, from, to, w: width, kind }, is_space));
                }
            }
        }
        frags.extend(open.map(|(frag, _)| frag));
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut line = Line { runs: Vec::new(), ink: 0.0 };
    let mut pen = 0f32;
    let run_of = |frag: &Frag, x: f32, from: u32, to: u32, w: f32| {
        let span = &spans[frag.span];
        let what = match span.what {
            Piece::Drawn(Drawn::Text(..)) => Drawn::Text(from, to),
            Piece::Drawn(Drawn::Own(..)) => Drawn::Own(from, to),
            Piece::Drawn(sign) => sign,
            Piece::Break => Drawn::Own(0, 0),
        };
        Run { x, w, what, look: span.look | look, class, tone: span.tone }
    };
    // Whether a stretch goes on in the run before it: the same span,
    // and the bytes that follow.
    let joins = |last: &Run, frag: &Frag| {
        let span = &spans[frag.span];
        let follows = match (last.what, span.what) {
            (Drawn::Text(_, to), Piece::Drawn(Drawn::Text(..))) => to == frag.from,
            (Drawn::Own(_, to), Piece::Drawn(Drawn::Own(..))) => to == frag.from,
            _ => false,
        };
        follows && last.look == span.look | look && last.tone == span.tone
    };
    // The space the last run of the line went on over: how many bytes
    // and how wide. At the end of a row it is taken back, or the
    // surface of code and the line through what is struck reach over it.
    let mut trail: Option<(u32, f32)> = None;
    let ended = |mut line: Line, trail: &mut Option<(u32, f32)>| {
        if let (Some((bytes, width)), Some(last)) = (trail.take(), line.runs.last_mut())
            && let Drawn::Text(_, to) | Drawn::Own(_, to) = &mut last.what
        {
            (*to, last.w) = (to.saturating_sub(bytes), last.w - width);
        }
        line
    };
    let mut at = 0;
    while at < frags.len() && lines.len() < most {
        let frag = frags[at];
        match frag.kind {
            Kind::Break => {
                let done = std::mem::replace(&mut line, Line { runs: Vec::new(), ink: 0.0 });
                lines.push(ended(done, &mut trail));
                pen = 0.0;
                at += 1;
            }
            // Space at the start of a line is not there.
            Kind::Space | Kind::Gap if line.runs.is_empty() => at += 1,
            Kind::Space => {
                trail = None;
                if let Some(last) = line.runs.last_mut().filter(|last| joins(last, &frag)) {
                    extend(last, &frag);
                    trail = Some((frag.to - frag.from, frag.w));
                }
                pen += frag.w;
                at += 1;
            }
            Kind::Gap => {
                trail = None;
                pen += frag.w;
                at += 1;
            }
            Kind::Word | Kind::Sign => {
                // A word is what stands together without space, also
                // where its look changes inside it.
                let more = frags[at..].iter().take_while(|next| matches!(next.kind, Kind::Word | Kind::Sign));
                let (count, width) = more.fold((0, 0.0), |(count, width), next| (count + 1, width + next.w));
                if pen > 0.0 && pen + width > avail {
                    let done = std::mem::replace(&mut line, Line { runs: Vec::new(), ink: 0.0 });
                    lines.push(ended(done, &mut trail));
                    pen = 0.0;
                }
                trail = None;
                for frag in &frags[at..at + count] {
                    if lines.len() >= most {
                        return lines;
                    }
                    // A sign is not broken, and behind the right edge it
                    // is not drawn: one that has no room left in its
                    // row begins the next.
                    if frag.kind == Kind::Sign && pen > 0.0 && pen + frag.w > avail {
                        let done = std::mem::replace(&mut line, Line { runs: Vec::new(), ink: 0.0 });
                        lines.push(ended(done, &mut trail));
                        pen = 0.0;
                    }
                    if pen + frag.w <= avail || frag.kind == Kind::Sign {
                        match line.runs.last_mut().filter(|last| joins(last, frag)) {
                            Some(last) => extend(last, frag),
                            None => line.runs.push(run_of(frag, pen, frag.from, frag.to, frag.w)),
                        }
                        pen += frag.w;
                        line.ink = pen;
                        continue;
                    }
                    // Wider than what is left of the line, and it stood
                    // at its start or is wider than a whole one: it is
                    // broken inside, sign by sign.
                    let span = &spans[frag.span];
                    let face = (span.look | look) & (BOLD | ITALIC | MONO);
                    let Piece::Drawn(drawn) = span.what else { continue };
                    let (Drawn::Text(start, _) | Drawn::Own(start, _)) = drawn else { continue };
                    let whole = piece(drawn).unwrap_or("");
                    let inside = whole.get((frag.from - start) as usize..(frag.to - start) as usize);
                    let mut begun: Option<(u32, f32, f32)> = None;
                    for (offset, c) in inside.unwrap_or("").char_indices() {
                        let width = shown(c).map_or(0.0, |c| measure(c, face, px));
                        let here = frag.from + offset as u32;
                        if pen > 0.0 && pen + width > avail {
                            if let Some((from, x, w)) = begun.take() {
                                line.runs.push(run_of(frag, x, from, here, w));
                            }
                            line.ink = pen;
                            let done = std::mem::replace(&mut line, Line { runs: Vec::new(), ink: 0.0 });
                            lines.push(ended(done, &mut trail));
                            if lines.len() >= most {
                                return lines;
                            }
                            pen = 0.0;
                        }
                        let (_, _, w) = begun.get_or_insert((here, pen, 0.0));
                        *w += width;
                        pen += width;
                    }
                    if let Some((from, x, w)) = begun {
                        line.runs.push(run_of(frag, x, from, frag.to, w));
                    }
                    line.ink = pen;
                }
                at += count;
            }
        }
    }
    if lines.len() < most && (!line.runs.is_empty() || lines.is_empty()) {
        lines.push(ended(line, &mut trail));
    }
    lines
}

/// What the rows of a preview were made for: when one of these is
/// another, they are made anew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub width: f32,
    pub sizes: Sizes,
    /// Which fonts stand in for signs the fonts of codn lack: with
    /// others, a sign has another width.
    pub fonts: u32,
}

struct Made {
    of: Edition,
    meant: Meant,
    key: Key,
    laid: Laid,
}

/// The preview of the file a pane shows: whether it is wanted, what was
/// asked and answered for it, the rows that are drawn, and where the
/// view stands in them.
pub struct Preview {
    /// The file is one of Markdown.
    can: bool,
    /// It is to be shown as it is meant, where that can be done.
    on: bool,
    /// The text whose structure was asked for and not answered yet.
    asked: Option<Edition>,
    /// A text with its structure, for which no rows are made yet.
    next: Option<(Edition, Meant)>,
    made: Option<Made>,
    /// Why the file has no preview. It holds for every text of it until
    /// another is read of the file, or the switch asks anew: asked
    /// again with every key, a file that is changed in its text would
    /// be taken out of the hands of whoever types.
    failed: Option<Why>,
    /// It is asked for all the same, once: another text was read of the
    /// file, or the switch asked. The text stays shown until the answer
    /// says otherwise.
    retry: bool,
    /// How often the process ended over this file.
    given_up: u8,
    /// The first row that is shown.
    top: usize,
    /// Where in the text to stand once rows are made.
    anchor: Option<Place>,
}

impl Preview {
    pub fn new(can: bool) -> Self {
        Self {
            can,
            on: can,
            asked: None,
            next: None,
            made: None,
            failed: None,
            retry: false,
            given_up: 0,
            top: 0,
            anchor: None,
        }
    }

    /// Whether the file is one that can be shown as it is meant at all.
    pub fn can(&self) -> bool {
        self.can
    }

    /// Why a text of that many bytes is shown as its text though a
    /// preview of it is wanted. Nothing where none is wanted.
    pub fn refused(&self, bytes: usize) -> Option<Why> {
        if !(self.can && self.on) {
            return None;
        }
        if bytes > FILE_MAX { Some(Why::Long) } else { self.failed }
    }

    /// Whether a text of that many bytes is shown as it is meant.
    pub fn shows(&self, bytes: usize) -> bool {
        self.can && self.on && self.refused(bytes).is_none()
    }

    /// Whether a preview is asked for though the file is shown as its
    /// text: what stood against one may not stand against the text as
    /// it is now.
    pub fn tries(&self, bytes: usize) -> bool {
        self.can && self.on && self.retry && self.failed.is_some() && bytes <= FILE_MAX
    }

    /// Another text was read of the file, of that many bytes, or the
    /// switch asks for the file as it is meant: what stood against a
    /// preview may not stand against this, and it is asked once more.
    /// Unless the process ended over the file as often as that is
    /// tried. The text stays shown meanwhile: a file with too much to
    /// it that an agent writes would else say "Reading…" at every
    /// writing. A text that is too long is held as that from here on,
    /// like every other reason: reckoned anew with every key, one sign
    /// taken out of it gave the text to its preview, with what was
    /// typed in it.
    pub fn again(&mut self, bytes: usize) {
        if bytes > FILE_MAX {
            (self.failed, self.retry) = (Some(Why::Long), false);
            return;
        }
        if self.failed == Some(Why::Long) {
            self.failed = None;
        }
        self.retry = self.failed.is_some_and(|why| !(why.ends() && self.given_up >= TRIES));
    }

    /// Switches between the file as it is meant and its text. `line` is
    /// the line of the text the view stands at, to stand there after.
    pub fn switch(&mut self, line: u32, bytes: usize) {
        // A file that is wanted as it is meant and has no preview is
        // asked for anew by the switch, where that is still tried:
        // turned off, the wish looked as before, and only a second
        // press asked.
        if self.on && self.failed.is_some() {
            self.again(bytes);
            // Asked anew, or nothing stands against it any more: a text
            // that was too long and is not now.
            if self.retry || self.failed.is_none() {
                self.anchor = Some(Place { line, byte: None });
                return;
            }
        }
        self.on = !self.on;
        if !self.on {
            return;
        }
        self.again(bytes);
        // Where the preview stood when it was left, if the text still
        // stands at that line: the text is moved by lines, and a
        // paragraph of one line is many rows. Else the line it is told.
        let place = match self.anchor.take() {
            Some(kept) if kept.line == line => kept,
            _ => Place { line, byte: None },
        };
        // Rows that are still there are stood at now. Else the place
        // waits for them.
        match &self.made {
            Some(made) => self.top = made.laid.row_of(place),
            None => self.anchor = Some(place),
        }
    }

    /// Whether the structure of the text `of` is to be asked for.
    pub fn wants(&self, of: Edition) -> bool {
        let known = |at: Option<Edition>| at == Some(of);
        !(known(self.asked)
            || known(self.next.as_ref().map(|(at, _)| *at))
            || known(self.made.as_ref().map(|made| made.of)))
    }

    pub fn asked(&mut self, of: Edition) {
        self.asked = Some(of);
    }

    /// Takes a text with its structure, if it is the one that was asked
    /// for last. Says whether it did.
    pub fn take(&mut self, of: Edition, meant: Meant) -> bool {
        let wanted = self.asked == Some(of);
        if wanted {
            (self.asked, self.next, self.failed, self.retry) = (None, Some((of, meant)), None, false);
        }
        wanted
    }

    /// The text `of` has no preview. Says whether that was news.
    pub fn fail(&mut self, of: Edition, why: Why) -> bool {
        if self.asked != Some(of) {
            return false;
        }
        if why.ends() {
            self.given_up = self.given_up.saturating_add(1);
        }
        // Where the view stood is kept for the preview that comes
        // after.
        self.anchor = self.place().or(self.anchor);
        (self.asked, self.next, self.made, self.top) = (None, None, None, 0);
        (self.failed, self.retry) = (Some(why), false);
        true
    }

    /// Nothing of the file is shown as it is meant now: what was held
    /// for that goes back. The line the view stood at is kept.
    pub fn rest(&mut self) {
        if self.asked.is_none() && self.next.is_none() && self.made.is_none() {
            return;
        }
        self.anchor = self.anchor.or(self.place());
        (self.asked, self.next, self.made, self.top) = (None, None, None, 0);
    }

    /// Whether rows are to be made: for a text that came, or because
    /// what they were made for is another.
    pub fn unlaid(&self, key: &Key) -> bool {
        self.next.is_some() || self.made.as_ref().is_some_and(|made| made.key != *key)
    }

    /// Makes the rows, and stands where the view stood before.
    pub fn lay(&mut self, key: Key, measure: &mut dyn FnMut(char, u8, f32) -> f32) {
        let place = self.anchor.take().or(self.place());
        let (of, meant) = match (self.next.take(), self.made.take()) {
            (Some(next), _) => next,
            (None, Some(made)) => (made.of, made.meant),
            (None, None) => return,
        };
        let laid = lay(&meant, &key.sizes, key.width, measure);
        self.top = place.map_or(0, |place| laid.row_of(place));
        self.made = Some(Made { of, meant, key, laid });
    }

    /// What is drawn: the text, its rows, the first of them that is
    /// shown, and the text they are of.
    pub fn made(&self) -> Option<(&Meant, &Laid, usize, Edition)> {
        self.made.as_ref().map(|made| (&made.meant, &made.laid, self.top, made.of))
    }

    /// Where the view stands in the text.
    fn place(&self) -> Option<Place> {
        self.made.as_ref()?.laid.place(self.top)
    }

    /// The line of the text the view stands at.
    pub fn line(&self) -> Option<usize> {
        let made = self.made.as_ref()?;
        made.laid.rows.get(self.top).map(|row| row.line as usize)
    }

    /// The lines of the text that are seen in a room that high: the
    /// first, and how many.
    pub fn lines(&self, height: f32) -> Option<(usize, usize)> {
        let made = self.made.as_ref()?;
        let seen = made.laid.seen(self.top, height).max(1);
        let rows = made.laid.rows.get(self.top..)?;
        let rows = &rows[..seen.min(rows.len())];
        let first = rows.iter().map(|row| row.line).min()? as usize;
        let last = rows.iter().map(|row| row.line).max()? as usize;
        Some((first, last - first + 1))
    }

    /// Puts the view where it may stand in a room that high: no lower
    /// than where the last rows just fit. Rows that are made anew know
    /// nothing of the room, nor does the switch, and a room can grow.
    /// Says whether it moved.
    pub fn settle(&mut self, height: f32) -> bool {
        let Some(made) = &self.made else { return false };
        let was = self.top;
        self.top = was.min(made.laid.lowest(height));
        self.top != was
    }

    /// Moves the view by rows, down if positive, in a room that high.
    /// Says whether it moved.
    pub fn scroll_by(&mut self, rows: isize, height: f32) -> bool {
        let Some(made) = &self.made else { return false };
        let was = self.top;
        self.top = was.saturating_add_signed(rows).min(made.laid.lowest(height));
        self.top != was
    }

    /// Moves the view as a key asks: by a row, by what is seen less a
    /// row, or to an end. Says whether it moved.
    pub fn go(&mut self, to: text::Move, height: f32) -> bool {
        let Some(made) = &self.made else { return false };
        let page = made.laid.seen(self.top, height).saturating_sub(1).max(1) as isize;
        // Up by as many rows as fit above the one at the top, which is
        // the last that is seen then, as down by a page makes the last
        // row the first.
        let back = self.top.saturating_sub(made.laid.before(self.top, height)).max(1) as isize;
        let by = match to {
            text::Move::Up => -1,
            text::Move::Down => 1,
            text::Move::PageUp(_) => -back,
            text::Move::PageDown(_) => page,
            text::Move::Top | text::Move::Home => isize::MIN,
            text::Move::Bottom | text::Move::End => isize::MAX,
            _ => return false,
        };
        self.scroll_by(by, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(what: What, from: usize, to: usize) -> Entry {
        Entry { what, from: from as u32, to: to as u32 }
    }

    fn here(text: &str, piece: &str) -> Word {
        let at = text.find(piece).unwrap();
        Word::Here(at as u32, (at + piece.len()) as u32)
    }

    /// Every sign as wide as its size, so that widths can be told.
    fn wide(_: char, _: u8, px: f32) -> f32 {
        px
    }

    fn sizes() -> Sizes {
        let kind = |px: f32| Type { px, h: 2.0 * px, base: 1.5 * px };
        Sizes { body: kind(10.0), heads: [kind(20.0); 6], code: kind(10.0), cell: 10.0 }
    }

    fn laid(text: &str, entries: Vec<Entry>, width: f32) -> (Meant, Laid) {
        let meant = Meant::new(text.as_bytes().to_vec(), entries).unwrap();
        let laid = lay(&meant, &sizes(), width, &mut wide);
        (meant, laid)
    }

    /// What a row says, with a bar between its runs.
    fn said(meant: &Meant, laid: &Laid, row: &Row) -> String {
        let runs = laid.runs(row).iter().map(|run| match run.what {
            Drawn::Text(from, to) => meant.text()[from as usize..to as usize].to_owned(),
            Drawn::Own(from, to) => laid.own()[from as usize..to as usize].to_owned(),
            Drawn::Task(checked) => format!("[{}]", if checked { 'x' } else { ' ' }),
            Drawn::Picture => "[picture]".to_owned(),
        });
        runs.collect::<Vec<_>>().join("|")
    }

    fn all(meant: &Meant, laid: &Laid) -> Vec<String> {
        laid.rows().iter().map(|row| said(meant, laid, row)).collect()
    }

    #[test]
    fn an_entry_that_does_not_fit_the_text_is_no_structure() {
        let text = "aä\n";
        let good = |what: What| Meant::new(text.as_bytes().to_vec(), vec![entry(what, 0, 4)]).is_some();
        assert!(good(What::Text(Word::Here(0, 4))));
        assert!(good(What::Text(Word::Here(1, 3))), "the two bytes of one sign");
        assert!(!good(What::Text(Word::Here(1, 2))), "half a sign");
        assert!(!good(What::Text(Word::Here(2, 3))), "from the middle of a sign");
        assert!(!good(What::Text(Word::Here(3, 2))), "an end before its beginning");
        assert!(!good(What::Link(Word::Here(0, 5))), "behind the text");
        assert!(!good(What::Heading(0)) && !good(What::Heading(7)) && good(What::Heading(6)));
        assert!(!good(What::Quote(6)) && good(What::Quote(5)));
        assert!(good(What::Image(Word::Said("x".repeat(WORD_MAX).into()))));
        assert!(!good(What::Image(Word::Said("x".repeat(WORD_MAX + 1).into()))));
        assert!(!good(What::Table(vec![Align::None; COLUMNS_WIRE + 1].into())));
        let bytes = || text.as_bytes().to_vec();
        assert!(Meant::new(bytes(), vec![entry(What::Rule, 0, 5)]).is_none(), "an entry behind the text");
        assert!(Meant::new(bytes(), vec![entry(What::Rule, 3, 2)]).is_none());
        assert!(Meant::new(vec![b'a', 0xff], Vec::new()).is_none(), "no text");
        assert!(Meant::new(vec![b'a'; FILE_MAX + 1], Vec::new()).is_none());
        assert!(Meant::new(bytes(), vec![entry(What::Rule, 0, 0); ENTRIES_MAX + 1]).is_none());
    }

    /// Every entry lay in the text, and all of them together named it a
    /// hundred thousand times: 512 entries over 1 MiB held 6 GB while their
    /// rows were made, on the thread of the window.
    #[test]
    fn entries_that_name_more_than_the_text_holds_are_no_structure() {
        let text = "word ".repeat(200);
        let naming = |count: usize, what: fn(Word) -> What| {
            let mut entries = vec![entry(What::Paragraph, 0, text.len())];
            entries.extend((0..count).map(|_| entry(what(Word::Here(0, text.len() as u32)), 0, text.len())));
            entries.push(entry(What::End, 0, text.len()));
            Meant::new(text.as_bytes().to_vec(), entries).is_some()
        };
        // What the crate says names every byte once at the most. Twice
        // is let through, more is not, whatever kind of words it is.
        assert!(naming(1, What::Text) && naming(2, What::Text) && !naming(3, What::Text));
        assert!(!naming(3, What::CodeSpan) && !naming(3, What::InlineHtml));
        assert!(!naming(1000, What::Text));
        // Where a link leads is not laid out: the same address is named
        // by every link to it.
        assert!(naming(1000, What::Link) && naming(1000, What::Image));
        // And characters that stand nowhere in the text count as well.
        let said = |count: usize| {
            let entries = vec![entry(What::Text(Word::Said("x".repeat(WORD_MAX).into())), 0, 1); count];
            Meant::new(b"ab".to_vec(), entries).is_some()
        };
        assert!(said(SAID_MAX / WORD_MAX) && !said(SAID_MAX / WORD_MAX + 1));
    }

    /// A block of HTML is read from end to end to see whether it is a
    /// comment, and a table that is shown as it is written is the lines
    /// it lies in, each read to its end. Every one lay in the text, and
    /// 131,072 blocks that each spanned a megabyte held the window for
    /// 44 seconds, 16,384 such tables for 28.
    #[test]
    fn blocks_that_are_read_whole_more_often_than_the_text_holds_are_no_structure() {
        let text = "<p>\n".repeat(100);
        let spanning = |count: usize, what: fn() -> What, to: usize| {
            let block = [entry(what(), 0, to), entry(What::End, 0, to)];
            let entries = (0..count).flat_map(|_| block.clone()).collect();
            Meant::new(text.as_bytes().to_vec(), entries).is_some()
        };
        let html: fn() -> What = || What::Html;
        let table: fn() -> What = || What::Table(vec![Align::None].into());
        // What the crate says names no byte in two such blocks. Twice
        // is let through, more is not.
        assert!(spanning(2, html, text.len()) && !spanning(3, html, text.len()));
        assert!(spanning(2, table, text.len()) && !spanning(3, table, text.len()));
        // A table is its lines whole, also one that names a byte of them.
        let long = "x".repeat(1000) + "\n";
        let tables = |count: usize| {
            let entries = (0..count).flat_map(|_| [entry(table(), 500, 501), entry(What::End, 500, 501)]);
            Meant::new(long.as_bytes().to_vec(), entries.collect()).is_some()
        };
        assert!(tables(2) && !tables(3));
        // Blocks that stand one behind the other are what a text is made of.
        let behind = (0..100).flat_map(|at| [entry(What::Html, 4 * at, 4 * at + 4), entry(What::End, 0, 0)]);
        assert!(Meant::new(text.as_bytes().to_vec(), behind.collect()).is_some());
    }

    /// A run went on over the space behind it, and at the end of a row the
    /// surface of code and the line through what is struck reached over
    /// that space.
    #[test]
    fn a_run_does_not_reach_over_the_space_at_the_end_of_its_row() {
        let text = "`aa bb cc`\n";
        let entries = vec![
            entry(What::Paragraph, 0, 11),
            entry(What::CodeSpan(Word::Here(1, 9)), 0, 10),
            entry(What::End, 0, 11),
        ];
        // Six signs fit a row: "aa bb", and the space behind it.
        let (meant, laid) = laid(text, entries, 60.0);
        assert_eq!(all(&meant, &laid), ["aa bb", "cc"]);
        assert_eq!(laid.runs(&laid.rows()[0])[0].w, 50.0);
    }

    /// The columns were placed at whole pixels and the room reckoned with
    /// their parts, a pixel too little for each.
    #[test]
    fn a_table_of_columns_that_are_no_whole_pixels_wide_stays_in_its_room() {
        let columns = 32;
        let text = "a|".repeat(columns);
        let mut entries = vec![entry(What::Table(vec![Align::None; columns].into()), 0, text.len())];
        entries.push(entry(What::Row, 0, 1));
        for at in 0..columns {
            let from = at * 2;
            entries.push(entry(What::Cell, from, from + 1));
            entries.push(entry(What::Text(Word::Here(from as u32, from as u32 + 1)), from, from + 1));
            entries.push(entry(What::End, from, from + 1));
        }
        entries.push(entry(What::End, 0, 1));
        entries.push(entry(What::End, 0, text.len()));
        let meant = Meant::new(text.as_bytes().to_vec(), entries).unwrap();
        // Every sign 10.5 wide. Between two columns are 14, before the
        // first and behind the last 6.
        let mut half = |_: char, _: u8, px: f32| px * 1.05;
        for width in [32.0 * 10.5 + 31.0 * 14.0 + 12.0 + 4.0, 32.0 * 11.0 + 31.0 * 14.0 + 12.0, 900.0] {
            let laid = lay(&meant, &sizes(), width, &mut half);
            let row = &laid.rows()[0];
            let reach = laid.runs(row).iter().map(|run| run.x + run.w).fold(0.0, f32::max);
            assert!(reach <= width && row.w <= width, "it reaches {reach} in a room of {width}");
            assert!(laid.runs(row).iter().all(|run| run.x.fract() == 0.0), "columns at whole pixels");
        }
    }

    #[test]
    fn a_paragraph_is_wrapped_at_its_spaces_and_a_long_word_inside_it() {
        let text = "one two  three\nabcdefghijkl x\n";
        let entries = vec![
            entry(What::Paragraph, 0, 30),
            entry(What::Text(here(text, "one two  three")), 0, 14),
            entry(What::Soft, 14, 15),
            entry(What::Text(here(text, "abcdefghijkl x")), 15, 29),
            entry(What::End, 0, 30),
        ];
        // Eight signs fit a row.
        let (meant, laid) = laid(text, entries, 80.0);
        assert_eq!(all(&meant, &laid), ["one two", "three", "abcdefgh", "ijkl x"]);
        // Each row knows the line of the text its first words stand in.
        assert_eq!(laid.rows().iter().map(|row| row.line).collect::<Vec<_>>(), [0, 0, 1, 1]);
        // Several spaces are one, and a row begins with none.
        let runs = laid.runs(&laid.rows()[1]);
        assert_eq!((runs[0].x, runs[0].w), (0.0, 50.0));
        // Nothing is wider than the room.
        for row in laid.rows() {
            assert!(laid.runs(row).iter().all(|run| run.x + run.w <= 80.0 + 10.0), "a space at the end");
        }
    }

    #[test]
    fn looks_change_inside_a_word_and_a_line_end_that_is_meant_ends_the_row() {
        let text = "a**b**c  \nd `e f` ~~g~~\n";
        let entries = vec![
            entry(What::Paragraph, 0, 24),
            entry(What::Text(here(text, "a")), 0, 1),
            entry(What::Strong, 1, 6),
            entry(What::Text(here(text, "b")), 3, 4),
            entry(What::End, 1, 6),
            entry(What::Text(here(text, "c")), 6, 7),
            entry(What::Hard, 7, 10),
            entry(What::Text(here(text, "d ")), 10, 12),
            entry(What::CodeSpan(here(text, "e f")), 12, 17),
            entry(What::Text(Word::Here(17, 18)), 17, 18),
            entry(What::Strike, 18, 23),
            entry(What::Text(here(text, "g")), 20, 21),
            entry(What::End, 18, 23),
            entry(What::End, 0, 24),
        ];
        let (meant, laid) = laid(text, entries, 400.0);
        assert_eq!(all(&meant, &laid), ["a|b|c", "d |e f|g"]);
        let looks = |row: usize| laid.runs(&laid.rows()[row]).iter().map(|run| run.look).collect::<Vec<_>>();
        assert_eq!(looks(0), [0, BOLD, 0]);
        assert_eq!(looks(1), [0, MONO | RAISED, STRUCK]);
        // The space between two looks is room and no run of its own.
        let runs = laid.runs(&laid.rows()[1]);
        assert_eq!(runs[2].x, runs[1].x + runs[1].w + 10.0);
        // What stands together without space is not broken apart.
        let (meant, narrow) = {
            let meant = Meant::new(text.as_bytes().to_vec(), meant.entries().to_vec()).unwrap();
            let laid = lay(&meant, &sizes(), 40.0, &mut wide);
            (meant, laid)
        };
        assert_eq!(all(&meant, &narrow)[0], "a|b|c");
    }

    fn list(text: &str, first: Option<u64>, items: &[&str]) -> Vec<Entry> {
        let mut entries = vec![entry(What::List(first), 0, text.len())];
        for item in items {
            let at = text.find(item).unwrap();
            entries.push(entry(What::Item, at, at + item.len()));
            entries.push(entry(What::Text(here(text, item)), at, at + item.len()));
            entries.push(entry(What::End, at, at + item.len()));
        }
        entries.push(entry(What::End, 0, text.len()));
        entries
    }

    #[test]
    fn the_entries_of_a_list_have_their_sign_or_their_number_before_them() {
        let text = "- one\n- two\n";
        let (meant, plain) = laid(text, list(text, None, &["one", "two"]), 400.0);
        assert_eq!(all(&meant, &plain), ["•|one", "•|two"]);
        let runs = plain.runs(&plain.rows()[0]);
        assert!(runs[0].x + runs[0].w <= runs[1].x, "the sign stands before the words");
        assert_eq!(runs[1].x, plain.rows()[0].x);

        let text = "9. one\n10. two\n";
        let (meant, numbered) = laid(text, list(text, Some(9), &["one", "two"]), 400.0);
        assert_eq!(all(&meant, &numbered), ["9.|one", "10.|two"]);
        // No room between the entries of a list that is written tight.
        assert!(numbered.rows().iter().all(|row| row.above == 0.0));
    }

    #[test]
    fn a_task_has_its_box_and_an_entry_with_nothing_in_it_its_sign() {
        let text = "- [x] done\n-\n";
        let entries = vec![
            entry(What::List(None), 0, 13),
            entry(What::Item, 0, 11),
            entry(What::Task(true), 2, 5),
            entry(What::Text(here(text, "done")), 6, 10),
            entry(What::End, 0, 11),
            entry(What::Item, 11, 13),
            entry(What::End, 11, 13),
            entry(What::End, 0, 13),
        ];
        let (meant, laid) = laid(text, entries, 400.0);
        assert_eq!(all(&meant, &laid), ["[x]|done", "•"]);
    }

    #[test]
    fn a_code_block_is_its_lines_as_they_are_written() {
        let text = "```rust\nlet a = 1;\n\n\tb\n```\n";
        let entries = vec![
            entry(What::Code, 0, 26),
            entry(What::Text(here(text, "let a = 1;\n")), 8, 19),
            entry(What::Text(Word::Here(19, 20)), 19, 20),
            entry(What::Text(here(text, "\tb\n")), 20, 23),
            entry(What::End, 0, 26),
        ];
        // Far too narrow: its lines are not wrapped.
        let (meant, laid) = laid(text, entries, 30.0);
        assert_eq!(all(&meant, &laid), ["let a = 1;", "", "\tb"]);
        assert_eq!(laid.rows().iter().map(|row| row.line).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(laid.rows().iter().all(|row| row.ground == Ground::Code));
        // A tab reaches to its stop: four cells and one for the letter.
        assert_eq!(laid.runs(&laid.rows()[2])[0].w, 50.0);
        let run = laid.runs(&laid.rows()[0])[0];
        assert_eq!((run.tone, run.class, run.look), (Tone::Code, CODE, MONO));
        // The surface has room above its first line and below its last.
        let rows = laid.rows();
        assert!(rows[0].h > rows[1].h && rows[2].h > rows[1].h && rows[0].base > rows[1].base);
    }

    #[test]
    fn a_line_of_code_that_comes_in_two_pieces_is_one_row() {
        let text = "    a\tb\n    c\n";
        let entries = vec![
            entry(What::Code, 0, 14),
            entry(What::Text(Word::Here(4, 5)), 4, 5),
            entry(What::Text(Word::Said("    ".into())), 5, 6),
            entry(What::Text(Word::Here(6, 8)), 6, 8),
            entry(What::Text(Word::Here(12, 14)), 12, 14),
            entry(What::End, 0, 14),
        ];
        let (meant, laid) = laid(text, entries, 400.0);
        assert_eq!(all(&meant, &laid), ["a|    |b", "c"]);
        let runs = laid.runs(&laid.rows()[0]);
        assert_eq!((runs[1].x, runs[2].x), (runs[0].x + 10.0, runs[0].x + 50.0));
    }

    #[test]
    fn a_comment_is_left_out_and_other_html_is_shown_as_it_is_written() {
        let text = "<!-- a -->\n\n<div>\nb\n</div>\n\nc <!-- d --> <b>e\n";
        let entries = vec![
            entry(What::Html, 0, 11),
            entry(What::Text(Word::Here(0, 11)), 0, 11),
            entry(What::End, 0, 11),
            entry(What::Html, 12, 27),
            entry(What::Text(Word::Here(12, 18)), 12, 18),
            entry(What::Text(Word::Here(18, 20)), 18, 20),
            entry(What::Text(Word::Here(20, 27)), 20, 27),
            entry(What::End, 12, 27),
            entry(What::Paragraph, 28, 46),
            entry(What::Text(here(text, "c ")), 28, 30),
            entry(What::InlineHtml(here(text, "<!-- d -->")), 30, 40),
            entry(What::Text(Word::Here(40, 41)), 40, 41),
            entry(What::InlineHtml(here(text, "<b>")), 41, 44),
            entry(What::Text(here(text, "e")), 44, 45),
            entry(What::End, 28, 46),
        ];
        let (meant, laid) = laid(text, entries, 400.0);
        assert_eq!(all(&meant, &laid), ["<div>", "b", "</div>", "c |<b>|e"]);
        assert_eq!(laid.runs(&laid.rows()[0])[0].tone, Tone::Faint);
        assert!(comment(" <!-- a\nb -->\n") && !comment("<!-- a --> b") && !comment("<!-- a --> <!-- b -->"));
    }

    /// A table of these rows, the first of which names the columns.
    fn table(text: &str, columns: Vec<Align>, rows: &[&[&str]]) -> Vec<Entry> {
        let mut entries = vec![entry(What::Table(columns.into()), 0, text.len())];
        for (at, row) in rows.iter().enumerate() {
            let start = text.find(row[0]).unwrap();
            entries.push(entry(if at == 0 { What::Head } else { What::Row }, start, start + 1));
            for cell in *row {
                let start = text.find(cell).unwrap();
                entries.push(entry(What::Cell, start, start + cell.len()));
                entries.push(entry(What::Text(here(text, cell)), start, start + cell.len()));
                entries.push(entry(What::End, start, start + cell.len()));
            }
            entries.push(entry(What::End, start, start + 1));
        }
        entries.push(entry(What::End, 0, text.len()));
        entries
    }

    #[test]
    fn the_columns_of_a_table_are_as_wide_as_their_widest_cell() {
        let text = "|n|name|\n|-|-:|\n|1|alpha|\n|22|b|\n";
        let rows: [&[&str]; 3] = [&["n", "name"], &["1", "alpha"], &["22", "b"]];
        let (meant, laid) = laid(text, table(text, vec![Align::None, Align::Right], &rows), 400.0);
        assert_eq!(all(&meant, &laid), ["n|name", "1|alpha", "22|b"]);
        let x = |row: usize, run: usize| laid.runs(&laid.rows()[row])[run].x;
        // The second column begins behind the widest cell of the first,
        // and its cells end at one edge.
        assert!(x(0, 1) > 20.0 && x(1, 1) == x(0, 1) - 10.0 && x(2, 1) == x(1, 1) + 40.0);
        assert_eq!(laid.rows()[0].ground, Ground::Head);
        assert!(laid.runs(&laid.rows()[0]).iter().all(|run| run.look == BOLD));
        assert!(laid.rows().iter().all(|row| row.under));
        assert_eq!(laid.rows().iter().map(|row| row.line).collect::<Vec<_>>(), [0, 2, 3]);
    }

    #[test]
    fn a_table_wider_than_the_room_has_its_cells_wrapped() {
        let text = "|k|what it is|\n|-|-|\n|a|one two three four five six|\n";
        let rows: [&[&str]; 2] = [&["k", "what it is"], &["a", "one two three four five six"]];
        let (meant, laid) = laid(text, table(text, vec![Align::None; 2], &rows), 200.0);
        // The narrow column keeps its width, the wide one what is left.
        let rows = all(&meant, &laid);
        assert_eq!(rows[0], "k|what it is");
        assert_eq!(&rows[1..], ["a|one two three", "four five six"]);
        // Only the last row of a row of the table has the line under it.
        assert_eq!(laid.rows().iter().map(|row| row.under).collect::<Vec<_>>(), [true, false, true]);
        for row in laid.rows() {
            assert!(laid.runs(row).iter().all(|run| run.x + run.w <= 200.0 + 10.0));
        }
    }

    #[test]
    fn a_table_without_room_or_with_more_than_is_laid_out_is_shown_as_it_is_written() {
        let text = "|a|b|\n|-|-|\n|one two three four five six seven|eight nine ten eleven twelve|\n";
        let rows: [&[&str]; 2] =
            [&["a", "b"], &["one two three four five six seven", "eight nine ten eleven twelve"]];
        let written = |width: f32, entries: Vec<Entry>| {
            let (meant, laid) = laid(text, entries, width);
            all(&meant, &laid) == text.lines().collect::<Vec<_>>()
        };
        assert!(!written(400.0, table(text, vec![Align::None; 2], &rows)));
        // Two columns of six signs and what stands between them need
        // more than this.
        assert!(written(100.0, table(text, vec![Align::None; 2], &rows)));
        // A row with more cells than the table has columns, and a table
        // of more columns than are drawn.
        assert!(written(400.0, table(text, vec![Align::None; 1], &rows)));
        assert!(written(4000.0, table(text, vec![Align::None; COLUMNS + 1], &rows)));
        assert!(written(400.0, table(text, Vec::new(), &rows)));
        // And a table of more cells than are laid out: as many rows of
        // one cell as that, and one more.
        let cells = |count: usize| {
            let text = "|x|\n".repeat(count);
            let mut entries = vec![entry(What::Table(vec![Align::None].into()), 0, text.len())];
            for at in 0..count {
                let cell = at * 4 + 1;
                entries.push(entry(What::Row, at * 4, at * 4 + 4));
                entries.push(entry(What::Cell, cell, cell + 1));
                entries.push(entry(What::Text(Word::Here(cell as u32, cell as u32 + 1)), cell, cell + 1));
                entries.extend([entry(What::End, 0, 0), entry(What::End, 0, 0)]);
            }
            entries.push(entry(What::End, 0, text.len()));
            let (meant, laid) = laid(&text, entries, 400.0);
            said(&meant, &laid, &laid.rows()[0])
        };
        assert_eq!((cells(CELLS).as_str(), cells(CELLS + 1).as_str()), ("x", "|x|"));
    }

    #[test]
    fn what_turns_the_writing_round_or_is_no_sign_is_not_drawn() {
        // The signs that say from which side a text is written: one of
        // them in the name of a file turns the letters behind it round.
        for turns in ['\u{200e}', '\u{200f}', '\u{061c}', '\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}'] {
            assert_eq!(shown(turns), None, "{turns:?}");
        }
        assert_eq!((shown('\u{7}'), shown('\u{9b}')), (None, None));
        assert_eq!((shown('\t'), shown('\n'), shown('\r')), (Some(' '), Some(' '), Some(' ')));
        assert_eq!(
            (shown('a'), shown('\u{e4}'), shown('\u{301}')),
            (Some('a'), Some('\u{e4}'), Some('\u{301}'))
        );
    }

    #[test]
    fn columns_too_wide_together_share_what_the_narrow_ones_leave() {
        assert_eq!(shared(&[10.0, 20.0], 40.0), [10.0, 20.0]);
        assert_eq!(shared(&[10.0, 100.0, 100.0], 70.0), [10.0, 30.0, 30.0]);
        assert_eq!(shared(&[50.0, 100.0], 60.0), [30.0, 30.0]);
        assert_eq!(shared(&[20.0, 30.0, 100.0], 90.0), [20.0, 30.0, 40.0]);
        assert_eq!(shared(&[], 10.0), Vec::<f32>::new());
    }

    #[test]
    fn quotes_and_lists_move_the_left_edge_up_to_a_depth_and_while_room_is_left() {
        let text = "x\n";
        let deep = |levels: usize, width: f32| {
            let mut entries: Vec<Entry> = (0..levels).map(|_| entry(What::Quote(0), 0, 2)).collect();
            entries.push(entry(What::Text(Word::Here(0, 1)), 0, 1));
            entries.extend((0..levels).map(|_| entry(What::End, 0, 2)));
            let (_, laid) = laid(text, entries, width);
            (laid.rows()[0].x, laid.runs(&laid.rows()[0])[0].tone)
        };
        assert_eq!(deep(1, 4000.0), (12.0, Tone::Dim));
        assert_eq!(deep(3, 4000.0).0, 36.0);
        assert_eq!(deep(DEPTH + 50, 4000.0).0, DEPTH as f32 * 12.0);
        // Eight signs of room are always left.
        assert_eq!(deep(9, 110.0).0, 24.0);
        // Sixty thousand levels, as many as an answer has room to open
        // and to end, are gone through like any others.
        assert_eq!(deep(60_000, 4000.0).0, DEPTH as f32 * 12.0);
        // Lists and their entries move it as well, to the same depth:
        // every entry by its sign and the room behind it.
        let listed = |levels: usize| {
            let open = [entry(What::List(None), 0, 2), entry(What::Item, 0, 2)];
            let mut entries: Vec<Entry> = (0..levels).flat_map(|_| open.clone()).collect();
            entries.push(entry(What::Text(Word::Here(0, 1)), 0, 1));
            entries.extend((0..2 * levels).map(|_| entry(What::End, 0, 2)));
            let (_, laid) = laid(text, entries, 4000.0);
            laid.rows().iter().map(|row| row.x).fold(0.0, f32::max)
        };
        assert_eq!((listed(1), listed(3), listed(DEPTH + 50)), (16.0, 48.0, DEPTH as f32 * 16.0));
        assert_eq!(listed(30_000), DEPTH as f32 * 16.0);
    }

    #[test]
    fn a_list_of_entries_that_is_no_structure_makes_rows_all_the_same() {
        // Long enough that its words may be named a few times over.
        let text = "abc\n                ";
        let word = || What::Text(Word::Here(0, 3));
        let odd = vec![
            entry(What::End, 0, 0),
            entry(What::End, 0, 0),
            entry(What::Cell, 0, 1),
            entry(word(), 0, 3),
            entry(What::Row, 0, 1),
            entry(What::Table(vec![Align::Left].into()), 0, 4),
            entry(What::Task(true), 0, 1),
            entry(What::Item, 0, 1),
            entry(What::Code, 0, 4),
            entry(What::Paragraph, 0, 4),
            entry(word(), 0, 3),
            entry(What::End, 0, 0),
            entry(What::Head, 0, 1),
            entry(What::Hard, 0, 1),
            entry(What::Emphasis, 0, 1),
            entry(What::Link(Word::Said("x".into())), 0, 1),
            entry(What::Image(Word::Here(0, 1)), 0, 1),
            entry(word(), 0, 3),
        ];
        let (_, laid) = laid(text, odd.clone(), 100.0);
        assert!(!laid.rows().is_empty());
        // Every way to cut it short, and every entry alone.
        for cut in 0..odd.len() {
            laid_of(text, odd[..cut].to_vec());
            laid_of(text, odd[cut..].to_vec());
            laid_of(text, vec![odd[cut].clone(); 3]);
        }
        fn laid_of(text: &str, entries: Vec<Entry>) {
            let meant = Meant::new(text.as_bytes().to_vec(), entries).unwrap();
            for width in [0.0, 1.0, 35.0, 1e9] {
                let laid = lay(&meant, &sizes(), width, &mut wide);
                for row in laid.rows() {
                    assert!(laid.runs(row).len() <= laid.runs.len());
                }
            }
        }
    }

    #[test]
    fn no_more_rows_are_made_than_the_limit_and_the_last_says_so() {
        let text = "\n".repeat(ROWS + 100);
        let entries =
            vec![entry(What::Code, 0, text.len()), entry(What::Text(Word::Here(0, text.len() as u32)), 0, 1)];
        let (_, laid) = laid(&text, entries, 100.0);
        assert_eq!(laid.rows().len(), ROWS);
        let last = laid.runs(laid.rows().last().unwrap())[0];
        let Drawn::Own(from, to) = last.what else { panic!() };
        assert!(laid.own()[from as usize..to as usize].starts_with("The preview ends here"));
    }

    /// All the lines of a paragraph were made before the bound of rows
    /// was asked: one word of a megabyte in a pane one sign wide held
    /// 158 MiB while its rows were made.
    #[test]
    fn no_more_lines_of_a_paragraph_are_made_than_rows_are_left() {
        let text = "x".repeat(40) + " a b\nc d e f g h i j";
        let word = |from: u32, to: u32| Span {
            what: Piece::Drawn(Drawn::Text(from, to)),
            look: 0,
            tone: Tone::Text,
        };
        let lines =
            |spans: &[Span], most: usize| wrap(&text, "", &mut wide, spans, BODY, 0, 10.0, 10.0, most).len();
        // A word that is broken inside, words at their spaces, and line
        // ends that are meant.
        let long = [word(0, 40)];
        assert_eq!((lines(&long, usize::MAX), lines(&long, 7), lines(&long, 1)), (40, 7, 1));
        let words = [word(40, 60)];
        assert_eq!((lines(&words, usize::MAX), lines(&words, 3)), (10, 3));
        let breaks = Span { what: Piece::Break, look: 0, tone: Tone::Text };
        let ends = [word(41, 42), breaks, word(43, 44), breaks, word(45, 46)];
        assert_eq!((lines(&ends, usize::MAX), lines(&ends, 2)), (3, 2));
        // Line ends one behind the other, signs that each fill a row,
        // and no line at all where none is left.
        let empty = [word(41, 42), breaks, breaks, breaks, breaks];
        assert_eq!((lines(&empty, usize::MAX), lines(&empty, 2)), (4, 2));
        let signs = [Span { what: Piece::Drawn(Drawn::Picture), look: 0, tone: Tone::Text }; 5];
        assert_eq!((lines(&signs, usize::MAX), lines(&signs, 2)), (5, 2));
        assert_eq!((lines(&long, 0), lines(&ends, 0), lines(&[], 0), lines(&[], 1)), (0, 0, 0, 1));
        // The rows of a paragraph end where the rows end, and say so.
        let text = "x ".repeat(ROWS + 100);
        let entries = vec![
            entry(What::Paragraph, 0, text.len()),
            entry(What::Text(Word::Here(0, text.len() as u32)), 0, text.len()),
            entry(What::End, 0, text.len()),
        ];
        let (meant, narrow) = laid(&text, entries, 10.0);
        assert_eq!(narrow.rows().len(), ROWS);
        assert!(all(&meant, &narrow).last().unwrap().starts_with("The preview ends here"));
    }

    /// The sign of a picture and the box of a task were set into their
    /// row whether there was room or not, and behind the right edge
    /// they are not drawn: of forty pictures side by side in a room for
    /// fourteen, twenty-five were not seen.
    #[test]
    fn a_sign_that_has_no_room_in_its_row_begins_the_next() {
        let text = "!".repeat(40);
        let mut entries = vec![entry(What::Paragraph, 0, 40)];
        for at in 0..40 {
            entries.extend([
                entry(What::Image(Word::Here(at, at + 1)), at as usize, at as usize + 1),
                entry(What::End, 0, 0),
            ]);
        }
        entries.push(entry(What::End, 0, 40));
        // A sign is 14 wide here: fourteen of them fit a room of 200.
        let (_, roomy) = laid(&text, entries, 200.0);
        let signs: Vec<usize> = roomy.rows().iter().map(|row| roomy.runs(row).len()).collect();
        assert_eq!(signs, [14, 14, 12]);
        assert!(roomy.rows().iter().flat_map(|row| roomy.runs(row)).all(|run| run.x + run.w <= 200.0));
        // One that is wider than the room stands in a row of its own.
        let entries = vec![
            entry(What::Paragraph, 0, 3),
            entry(What::Text(Word::Here(0, 1)), 0, 1),
            entry(What::Image(Word::Here(1, 2)), 1, 2),
            entry(What::End, 0, 0),
            entry(What::Text(Word::Here(2, 3)), 2, 3),
            entry(What::End, 0, 3),
        ];
        let (meant, narrow) = laid("a!b", entries, 12.0);
        assert_eq!(all(&meant, &narrow), ["a", "[picture]", "b"]);
    }

    #[test]
    fn the_view_keeps_its_line_when_the_rows_are_made_anew() {
        let text = "one two three four\n\nfive six seven eight\n";
        let entries = vec![
            entry(What::Paragraph, 0, 19),
            entry(What::Text(here(text, "one two three four")), 0, 18),
            entry(What::End, 0, 19),
            entry(What::Paragraph, 20, 41),
            entry(What::Text(here(text, "five six seven eight")), 20, 40),
            entry(What::End, 20, 41),
        ];
        let of = Edition { opening: 1, change: 0 };
        let key = |width: f32| Key { width, sizes: sizes(), fonts: 0 };
        let mut preview = Preview::new(true);
        assert!(preview.wants(of) && preview.shows(text.len()));
        preview.asked(of);
        assert!(!preview.wants(of));
        // An answer for a text that was not asked for last is not taken.
        let meant = || Meant::new(text.as_bytes().to_vec(), entries.clone()).unwrap();
        assert!(!preview.take(Edition { opening: 1, change: 9 }, meant()));
        assert!(preview.take(of, meant()) && !preview.wants(of));
        assert!(preview.made().is_none() && preview.unlaid(&key(400.0)));
        preview.lay(key(400.0), &mut wide);
        assert!(!preview.unlaid(&key(400.0)) && preview.unlaid(&key(90.0)));
        assert_eq!((preview.made().unwrap().1.rows().len(), preview.line()), (2, Some(0)));
        // Down to the second paragraph, and then a narrower pane: the
        // view stands at the row that line begins now.
        assert!(preview.scroll_by(1, 20.0) && preview.line() == Some(2));
        preview.lay(key(90.0), &mut wide);
        let (_, laid, top, _) = preview.made().unwrap();
        assert_eq!((laid.rows().len(), top, preview.line()), (6, 3, Some(2)));
        // Where all of it fits, the view begins at the first row.
        assert!(preview.scroll_by(9, 1000.0) && preview.made().unwrap().2 == 0);
        assert!(!preview.scroll_by(-9, 1000.0));
        // It ends where the last rows just fit, and the keys move it.
        assert!(preview.go(text::Move::Bottom, 45.0) && preview.made().unwrap().2 == 4);
        assert!(preview.go(text::Move::Top, 45.0) && preview.go(text::Move::PageDown(0), 65.0));
        assert_eq!(preview.made().unwrap().2, 2);
        assert_eq!(preview.lines(45.0), Some((0, 1)));
        // Switched off and on again: it stands at the line it is told.
        preview.switch(0, 9);
        assert!(!preview.shows(text.len()));
        preview.rest();
        assert!(preview.made().is_none());
        preview.switch(2, 9);
        assert!(preview.shows(text.len()) && preview.wants(of));
        preview.asked(of);
        assert!(preview.take(of, meant()));
        preview.lay(key(90.0), &mut wide);
        assert_eq!(preview.line(), Some(2));
    }

    /// Two paragraphs: a word, and forty words written in one line.
    fn one_line() -> (String, Vec<Entry>) {
        let words: Vec<String> = (0..40).map(|n| format!("w{n:02}")).collect();
        let text = format!("first\n\n{}\n", words.join(" "));
        let end = text.len();
        let entries = vec![
            entry(What::Paragraph, 0, 6),
            entry(What::Text(Word::Here(0, 5)), 0, 5),
            entry(What::End, 0, 6),
            entry(What::Paragraph, 7, end),
            entry(What::Text(Word::Here(7, end as u32 - 1)), 7, end - 1),
            entry(What::End, 7, end),
        ];
        (text, entries)
    }

    /// A preview of that text, in rows for a width, and what the row at
    /// the top of its view says.
    fn previewed(text: &str, entries: &[Entry], width: f32) -> Preview {
        let mut preview = Preview::new(true);
        read_again(&mut preview, text, entries, 0, width);
        preview
    }

    fn read_again(preview: &mut Preview, text: &str, entries: &[Entry], change: u64, width: f32) {
        let of = Edition { opening: 1, change };
        preview.asked(of);
        assert!(preview.take(of, Meant::new(text.as_bytes().to_vec(), entries.to_vec()).unwrap()));
        preview.lay(Key { width, sizes: sizes(), fonts: 0 }, &mut wide);
    }

    fn at_the_top(preview: &Preview) -> String {
        let (meant, laid, top, _) = preview.made().unwrap();
        said(meant, laid, &laid.rows()[top])
    }

    /// The view was kept by the line of the text its top row comes from,
    /// and a paragraph that is written in one line is many rows of one
    /// line: read again, made wider or switched to its text and back, it
    /// stood at the first of them.
    #[test]
    fn the_view_stays_where_it_was_inside_a_paragraph_written_in_one_line() {
        let (text, entries) = one_line();
        let key = |width: f32| Key { width, sizes: sizes(), fonts: 0 };
        // Two words to a row, and the view at the eleventh of them.
        let mut preview = previewed(&text, &entries, 80.0);
        assert!(preview.scroll_by(11, 40.0));
        assert_eq!((at_the_top(&preview).as_str(), preview.line()), ("w20 w21", Some(2)));
        // The same text read again. Until its rows are made, what is
        // drawn is the text before it, whole.
        let (before, read) = (Edition { opening: 1, change: 0 }, Edition { opening: 1, change: 1 });
        preview.asked(read);
        assert!(preview.take(read, Meant::new(text.as_bytes().to_vec(), entries.clone()).unwrap()));
        assert_eq!((preview.made().unwrap().3, at_the_top(&preview).as_str()), (before, "w20 w21"));
        preview.lay(key(80.0), &mut wide);
        assert_eq!((preview.made().unwrap().3, at_the_top(&preview).as_str()), (read, "w20 w21"));
        // The pane made wider, three words to a row: the row that holds
        // what the view began with. And narrower again.
        preview.lay(key(120.0), &mut wide);
        assert_eq!(at_the_top(&preview), "w18 w19 w20");
        preview.lay(key(80.0), &mut wide);
        assert_eq!(at_the_top(&preview), "w18 w19");
        // Switched to its text and back, the text at the line it stood.
        preview.switch(2, 9);
        preview.rest();
        preview.switch(2, 9);
        read_again(&mut preview, &text, &entries, 1, 80.0);
        assert_eq!(at_the_top(&preview), "w18 w19");
        // And with the text moved to another line: that line.
        preview.switch(2, 9);
        preview.rest();
        preview.switch(0, 9);
        read_again(&mut preview, &text, &entries, 1, 80.0);
        assert_eq!(at_the_top(&preview), "first");
        // After a failure the view is kept for the preview that comes.
        assert!(preview.scroll_by(7, 40.0) && at_the_top(&preview) == "w12 w13");
        let of = Edition { opening: 1, change: 2 };
        preview.asked(of);
        assert!(preview.fail(of, Why::Ended) && preview.made().is_none());
        read_again(&mut preview, &text, &entries, 3, 80.0);
        assert_eq!(at_the_top(&preview), "w12 w13");
        // The rows stand in the order of the lines, also one that draws
        // nothing of the text: it is of the line of the row before it.
        let text = "a\nb\nc\n";
        let entries = vec![
            entry(What::Paragraph, 0, 6),
            entry(What::Text(Word::Here(0, 1)), 0, 1),
            entry(What::Hard, 1, 2),
            entry(What::Text(Word::Here(2, 3)), 2, 3),
            entry(What::Hard, 3, 4),
            entry(What::Text(Word::Said("&".into())), 4, 5),
            entry(What::End, 0, 6),
        ];
        let (meant, rows) = laid(text, entries, 100.0);
        assert_eq!(all(&meant, &rows), ["a", "b", "&"]);
        assert_eq!(rows.rows().iter().map(|row| row.line).collect::<Vec<_>>(), [0, 1, 1]);
    }

    /// Rows that were made anew knew nothing of the room: the view could
    /// stand below the row from which the last ones just fit, and a
    /// step down then moved it up.
    #[test]
    fn the_view_is_no_lower_than_where_the_last_rows_just_fit() {
        let (text, entries) = one_line();
        // Five rows are seen, of twenty-one: the view at the last five.
        let mut preview = previewed(&text, &entries, 80.0);
        assert!(preview.go(text::Move::Bottom, 100.0) && at_the_top(&preview) == "w30 w31");
        assert!(!preview.settle(100.0), "it stands where it may");
        // Six words to a row: eight rows, and the one that holds what
        // the view began with is the seventh.
        preview.lay(Key { width: 240.0, sizes: sizes(), fonts: 0 }, &mut wide);
        assert_eq!((preview.made().unwrap().1.rows().len(), preview.made().unwrap().2), (8, 6));
        assert!(preview.settle(100.0));
        let (_, laid, top, _) = preview.made().unwrap();
        assert_eq!((top, laid.lowest(100.0)), (3, 3));
        assert!(!preview.settle(100.0) && !preview.scroll_by(1, 100.0));
        // A room that grew does the same.
        assert!(preview.settle(1000.0) && preview.made().unwrap().2 == 0);
    }

    /// A page up went by as many rows as were seen below, and rows above
    /// that are higher were passed over without being shown.
    #[test]
    fn a_page_up_shows_every_row_on_the_way() {
        // Three headings, which are twice as high as a row of the text,
        // and eight paragraphs of a word.
        let text = "h\n\n".repeat(3) + &"p\n\n".repeat(8);
        let mut entries = Vec::new();
        for at in 0..11 {
            let what = if at < 3 { What::Heading(1) } else { What::Paragraph };
            entries.push(entry(what, at * 3, at * 3 + 2));
            entries.push(entry(What::Text(Word::Here(at as u32 * 3, at as u32 * 3 + 1)), at * 3, at * 3 + 1));
            entries.push(entry(What::End, at * 3, at * 3 + 2));
        }
        let mut preview = previewed(&text, &entries, 100.0);
        // From the first paragraph a page up leads to the heading
        // before it, below which that paragraph is the last row seen.
        assert!(preview.scroll_by(3, 100.0) && preview.go(text::Move::PageUp(0), 100.0));
        assert_eq!(preview.made().unwrap().2, 2);
        // From everywhere, in rooms of every height: the row that stood
        // at the top is still seen, or the view moved by one row.
        for height in [10.0, 30.0, 60.0, 100.0, 150.0, 400.0] {
            for from in 1..11 {
                preview.go(text::Move::Top, height);
                preview.scroll_by(from, height);
                let was = preview.made().unwrap().2;
                let moved = preview.go(text::Move::PageUp(0), height);
                let (_, laid, top, _) = preview.made().unwrap();
                assert_eq!(moved, was > 0);
                assert!(top + laid.seen(top, height).max(1) > was || top + 1 == was, "{height} {was} {top}");
                assert!(top < was || was == 0);
            }
        }
    }

    #[test]
    fn a_file_without_a_preview_stays_its_text_until_one_of_it_can_be_made() {
        let text = |change: u64| Edition { opening: 1, change };
        let mut preview = Preview::new(true);
        preview.asked(text(0));
        assert!(!preview.fail(text(5), Why::Ended), "not the one that was asked for");
        assert!(preview.fail(text(0), Why::Large));
        // It holds for the text as it is typed on: a key in it does not
        // take the file out of the hands of whoever types, and nothing
        // is asked for it.
        assert_eq!(
            (preview.refused(9), preview.shows(9), preview.tries(9)),
            (Some(Why::Large), false, false)
        );
        // Another text of the file is another try. The text stays shown
        // while that is asked, and does not give way to "Reading…" at every
        // writing of a file that is too large.
        preview.again(9);
        assert!(!preview.shows(9) && preview.tries(9) && preview.wants(text(1)));
        preview.asked(text(1));
        assert!(preview.fail(text(1), Why::NotText) && !preview.shows(9) && !preview.tries(9));
        // One press of the switch asks anew. It turned the preview off,
        // which looked like nothing, and only a second press asked.
        preview.switch(0, 9);
        assert!(preview.tries(9) && preview.refused(9) == Some(Why::NotText));
        preview.asked(text(2));
        let meant = Meant::new(b"a".to_vec(), Vec::new()).unwrap();
        assert!(preview.take(text(2), meant) && preview.shows(9) && !preview.tries(9));
        // The process ended over it: once more with the next text, and
        // after the second time never again, whatever asks. The switch
        // then turns the wish off, and the reason with it.
        preview.asked(text(3));
        assert!(preview.fail(text(3), Why::Ended));
        preview.again(9);
        assert!(preview.tries(9));
        preview.asked(text(4));
        assert!(preview.fail(text(4), Why::TookTooLong));
        preview.again(9);
        assert_eq!((preview.refused(9), preview.tries(9)), (Some(Why::TookTooLong), false));
        preview.switch(0, 9);
        assert!(preview.refused(9).is_none() && !preview.shows(9));
        preview.switch(0, 9);
        assert_eq!((preview.refused(9), preview.tries(9)), (Some(Why::TookTooLong), false));
        // A text over the limit is not asked for at all.
        assert_eq!(Preview::new(true).refused(FILE_MAX + 1), Some(Why::Long));
        assert!(Preview::new(true).refused(FILE_MAX).is_none());
        let mut long = Preview::new(true);
        long.again(FILE_MAX + 1);
        assert!(!long.tries(FILE_MAX + 1));
        // One that was read too long stays its text while it is typed
        // in. A key that took a sign out of it gave it to its preview,
        // with what was typed and not saved, and nothing is typed into
        // a file as it is meant.
        assert_eq!((long.refused(FILE_MAX + 1), long.shows(FILE_MAX + 1)), (Some(Why::Long), false));
        assert_eq!(
            (long.refused(FILE_MAX), long.shows(FILE_MAX), long.tries(FILE_MAX)),
            (Some(Why::Long), false, false)
        );
        // The switch asks for the text as it is now, and so does another
        // text that is read of the file.
        long.switch(0, FILE_MAX);
        assert!(long.shows(FILE_MAX) && long.refused(FILE_MAX).is_none());
        long.again(FILE_MAX + 1);
        long.again(FILE_MAX);
        assert!(long.shows(FILE_MAX));
        // Switched to its text, typed in until it is too long, and
        // switched back: held from then on like one that was read so.
        long.switch(0, FILE_MAX);
        long.switch(0, FILE_MAX + 1);
        assert_eq!((long.refused(FILE_MAX), long.shows(FILE_MAX)), (Some(Why::Long), false));
        // A file that is none of Markdown has no preview and no reason.
        let other = Preview::new(false);
        assert!(!other.shows(9) && other.refused(9).is_none() && !other.can());
    }

    /// What the rows of the largest texts there can be cost, to be
    /// measured: `cargo test --release what_the_rows_cost -- --ignored
    /// --nocapture`. Without the renderer: every sign is as wide as its
    /// size.
    #[test]
    #[ignore]
    fn what_the_rows_cost() {
        let timed = |name: &str, text: String, entries: Vec<Entry>| {
            let meant = Meant::new(text.into_bytes(), entries).unwrap();
            for width in [200.0, 700.0] {
                let begun = std::time::Instant::now();
                let rows = lay(&meant, &sizes(), width, &mut wide).rows().len();
                let took = begun.elapsed().as_secs_f64() * 1e3;
                println!("{name}, {width} wide: {rows} rows in {took:.1} ms");
            }
        };
        // A megabyte of words in one paragraph, in as many pieces as a
        // text may have entries.
        let word = "word ";
        let count = FILE_MAX / word.len();
        let text = word.repeat(count);
        let each = count / (ENTRIES_MAX - 2) + 1;
        let mut entries = vec![entry(What::Paragraph, 0, text.len())];
        for from in (0..count).step_by(each) {
            let (from, to) = (from * word.len(), ((from + each).min(count)) * word.len());
            entries.push(entry(What::Text(Word::Here(from as u32, to as u32)), from, to));
        }
        entries.push(entry(What::End, 0, text.len()));
        timed("a megabyte of words in one paragraph", text, entries);
        // A megabyte of lines of code.
        let text = "let a = 1;\n".repeat(FILE_MAX / 11);
        let entries = vec![
            entry(What::Code, 0, text.len()),
            entry(What::Text(Word::Here(0, text.len() as u32)), 0, text.len()),
            entry(What::End, 0, text.len()),
        ];
        timed("a megabyte of lines of code", text, entries);
        // A table of as many cells as one is drawn with, in four
        // columns: wrapped in the wider room, and without room in the
        // narrower, where it is shown as it is written.
        let text = "cell of a table ".repeat(CELLS);
        let mut entries = vec![entry(What::Table(vec![Align::None; 4].into()), 0, text.len())];
        for row in 0..CELLS / 4 {
            entries.push(entry(What::Row, row * 64, row * 64 + 1));
            for cell in 0..4 {
                let from = (row * 4 + cell) * 16;
                entries.push(entry(What::Cell, from, from + 15));
                entries.push(entry(What::Text(Word::Here(from as u32, from as u32 + 15)), from, from + 15));
                entries.push(entry(What::End, from, from + 15));
            }
            entries.push(entry(What::End, row * 64, row * 64 + 1));
        }
        entries.push(entry(What::End, 0, text.len()));
        timed("a table of 20,000 cells in 4 columns", text, entries);
        // An entry for every sign: as many paragraphs as fit.
        let text = "a\n\n".repeat(ENTRIES_MAX / 3);
        let entries = (0..ENTRIES_MAX / 3)
            .flat_map(|at| {
                let from = at * 3;
                [
                    entry(What::Paragraph, from, from + 2),
                    entry(What::Text(Word::Here(from as u32, from as u32 + 1)), from, from + 1),
                    entry(What::End, from, from + 2),
                ]
            })
            .collect();
        timed("as many paragraphs as a text has entries for", text, entries);
    }

    #[test]
    fn what_is_seen_and_the_lowest_row_are_reckoned_with_the_room_above_a_row() {
        let text = "a\n\nb\n\nc\n";
        let mut entries = Vec::new();
        for (at, word) in ["a", "b", "c"].iter().enumerate() {
            entries.push(entry(What::Paragraph, at * 3, at * 3 + 2));
            entries.push(entry(What::Text(here(text, word)), at * 3, at * 3 + 1));
            entries.push(entry(What::End, at * 3, at * 3 + 2));
        }
        let (_, laid) = laid(text, entries, 100.0);
        // Rows of 20, with 11 above the second and the third.
        assert_eq!(
            laid.rows().iter().map(|row| (row.above, row.h)).collect::<Vec<_>>(),
            [(0.0, 20.0), (11.0, 20.0), (11.0, 20.0)]
        );
        assert_eq!((laid.seen(0, 50.0), laid.seen(0, 51.0), laid.seen(0, 82.0)), (1, 2, 3));
        assert_eq!((laid.seen(1, 20.0), laid.seen(1, 19.0), laid.seen(9, 99.0)), (1, 0, 0));
        assert_eq!(
            (laid.lowest(20.0), laid.lowest(50.0), laid.lowest(51.0), laid.lowest(999.0)),
            (2, 2, 1, 0)
        );
        assert_eq!((laid.row_at(0), laid.row_at(1), laid.row_at(4), laid.row_at(99)), (0, 1, 2, 2));
    }
}
