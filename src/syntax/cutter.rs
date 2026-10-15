//! The process for the colors: it holds one text and the grammars, and
//! cuts the lines it is asked for into pieces. Everything in here runs in
//! a process of its own, started by `client`, because `syntect` can end
//! the process it runs in and a search of it cannot be stopped from
//! outside: what a file holds must not be able to end the app.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use super::wire::{self, Answer, LINE_MAX, QUESTION_MAX, Question, STRETCH};
use crate::core::colors::{FILE_MAX, Kind, Piece};
use crate::core::text::line_starts;

/// How far apart the places lie from which the cutting can go on, in
/// lines: a look at the end of a file then begins that near to it and
/// not at its first line, the second time.
const EVERY: usize = 64;
/// How many such places are kept. Measured on files as they are written,
/// one is 300 to 1,600 bytes on average and 4.7 KiB at the most, which
/// makes 20 MiB of them. A text can be made whose places are far larger:
/// nothing bounds them but the limit on the memory of this process,
/// which then ends it and nothing else. Behind the last of them the
/// cutting goes on from the last.
const PLACES: usize = 4096;
/// What the process may hold when it is given another text. Over it, it
/// ends, and a new one is started: the compiled patterns of every
/// language that was ever shown stay, 403 MiB for all of them.
const SOFT: usize = 256 << 20;
/// How long the lines of one answer are cut for, once one is cut: then
/// what there is, is answered. A stretch of lines can be made that takes
/// longer than the watch on an answer, and need not be made: in a code
/// block of Markdown the pattern for its end is compiled anew at every
/// word, and sixty-four lines of a hundred bytes took 1.6 s. The watch
/// then is on one line, not on sixty-four.
const SLICE: Duration = Duration::from_millis(250);
/// How many lines keep the pieces they were cut into, and how many pieces
/// those may be together: a few panes of lines, and 2 MiB where every
/// line is cut into thousands.
const KEPT: usize = 1024;
const PIECES: usize = 1 << 18;
/// What the process may ask of the system at all. A file that makes it
/// ask for more ends it, and nothing else.
const HARD: u64 = 1 << 30;

/// Which kind a name is, by how it begins. A piece has a stack of names,
/// from the language of the file inward.
struct Kinds {
    /// These hold for everything inside them.
    over: Vec<(Scope, Kind)>,
    /// Of these the innermost name that begins like one counts. Where two
    /// begin alike, the longer stands first.
    by: Vec<(Scope, Kind)>,
}

/// The key of JSON is a string by its name and a key by where it stands.
const OVER: [(&str, Kind); 2] = [("comment", Kind::Comment), ("meta.mapping.key.json", Kind::Name)];

const BY: [(&str, Kind); 33] = [
    ("string", Kind::String),
    ("markup.raw", Kind::String),
    ("markup.inserted", Kind::String),
    // What SQL calls a constant is the name of a table or of a column.
    ("constant.other.database-name", Kind::Plain),
    ("constant.other.table-name", Kind::Plain),
    ("constant", Kind::Constant),
    ("support.constant", Kind::Constant),
    ("markup.deleted", Kind::Constant),
    ("markup.quote", Kind::Comment),
    // The signs between two values are text, the words between them not.
    ("keyword.operator.word", Kind::Keyword),
    ("keyword.operator.logical.python", Kind::Keyword),
    ("keyword.operator", Kind::Plain),
    // The unit behind a number of CSS belongs to the number.
    ("keyword.other.unit", Kind::Constant),
    ("keyword", Kind::Keyword),
    ("storage", Kind::Keyword),
    ("variable.language", Kind::Keyword),
    ("markup.heading", Kind::Keyword),
    ("entity.name.section", Kind::Keyword),
    ("entity.name.function", Kind::Function),
    ("support.function", Kind::Function),
    ("variable.function", Kind::Function),
    ("support.macro", Kind::Function),
    ("markup.underline.link", Kind::Function),
    ("entity.name.tag", Kind::Name),
    ("entity.other.attribute-name", Kind::Name),
    ("support.type.property-name", Kind::Name),
    ("entity.name", Kind::Type),
    ("entity.other.inherited-class", Kind::Type),
    ("support.type", Kind::Type),
    ("support.class", Kind::Type),
    // Code inside a string, and a language inside another: what is not
    // named there is text, not the string or the block around it.
    ("meta.interpolation", Kind::Plain),
    ("meta.embedded", Kind::Plain),
    ("source", Kind::Plain),
];

impl Kinds {
    fn new() -> Self {
        let table = |names: &[(&str, Kind)]| {
            names.iter().filter_map(|(name, kind)| Some((Scope::new(name).ok()?, *kind))).collect()
        };
        Self { over: table(&OVER), by: table(&BY) }
    }

    fn of(&self, stack: &ScopeStack) -> Kind {
        let innermost = |table: &[(Scope, Kind)]| {
            let known = |name: &Scope| table.iter().find(|(begins, _)| begins.is_prefix_of(*name));
            stack.as_slice().iter().rev().find_map(known).map(|(_, kind)| *kind)
        };
        innermost(&self.over).or_else(|| innermost(&self.by)).unwrap_or(Kind::Plain)
    }
}

/// Where the cutting stands before a line: the state of the parser and
/// the names that are open.
type Place = (ParseState, ScopeStack);

/// The pieces of lines that were cut, for as long as the text above them
/// stays as it is. A pane that is moved by half its height asks for
/// three panes of lines, most of which it was given a moment ago, and a
/// key in a file asks for all of them again: cut anew each time, that was
/// 150 to 200 lines for 26 new ones.
#[derive(Default)]
struct Kept {
    lines: BTreeMap<usize, Box<[Piece]>>,
    /// How many pieces these are together.
    pieces: usize,
}

impl Kept {
    /// Keeps the pieces of a line. Over the limits, what lies farthest
    /// from that line goes.
    fn put(&mut self, line: usize, pieces: Box<[Piece]>) {
        self.pieces += pieces.len();
        if let Some(before) = self.lines.insert(line, pieces) {
            self.pieces -= before.len();
        }
        while self.lines.len() > KEPT || self.pieces > PIECES {
            let ends = self.lines.keys().next().copied().zip(self.lines.keys().next_back().copied());
            let Some((first, last)) = ends.filter(|(first, last)| first != last) else { break };
            let far = if line - first > last - line { first } else { last };
            self.pieces -= self.lines.remove(&far).map_or(0, |gone| gone.len());
        }
    }

    /// Nothing is known of the lines from this one on any more.
    fn forget_from(&mut self, line: usize) {
        let gone = self.lines.split_off(&line);
        self.pieces -= gone.values().map(|pieces| pieces.len()).sum::<usize>();
    }
}

struct Text {
    id: u64,
    bytes: Vec<u8>,
    /// Where every line begins, as `Document` counts lines.
    starts: Vec<u32>,
    /// The places before every `EVERY`th line, as far as they were reached.
    places: Vec<Place>,
    /// The line the cutting stands before, and its place there.
    at: usize,
    place: Place,
    /// The line from which on nothing is known.
    unknown: Option<u32>,
    kept: Kept,
}

pub struct Cutter {
    set: SyntaxSet,
    kinds: Kinds,
    text: Option<Text>,
    /// `SOFT` and `SLICE`, which a test sets to see what they do.
    soft: usize,
    slice: Duration,
}

/// The mark some programs put at the start of a file to say that it is
/// UTF-8. It is no part of the first line for a grammar: a pattern that
/// begins at the start of a line would not find a heading behind it.
/// The app takes one such mark off a text before it sends it, so what
/// arrives here with one is a text that began with two.
const MARK: &[u8] = b"\xef\xbb\xbf";

/// The pieces of a line, and the place behind it. Nothing for a line
/// that is not cut: too long, no text, or one the crate fails at.
/// `first` says that it is the first line of the text.
fn cut(
    set: &SyntaxSet,
    kinds: &Kinds,
    (state, stack): &mut Place,
    line: &[u8],
    first: bool,
) -> Option<Box<[Piece]>> {
    if line.len() > LINE_MAX {
        return None;
    }
    // The mark is cut off for the grammar and counted in again behind it:
    // it has the kind of what follows it, and takes no cell.
    let mark = if first && line.starts_with(MARK) { MARK.len() } else { 0 };
    let (whole, line) = (line.len(), &line[mark..]);
    let changes = state.parse_line(std::str::from_utf8(line).ok()?, set).ok()?;
    let (mut pieces, mut at): (Vec<Piece>, usize) = (Vec::new(), 0);
    let mut put = |until: usize, kind: Kind| {
        if until > at {
            match pieces.last_mut() {
                Some(last) if last.1 == kind => last.0 = until as u32,
                _ => pieces.push((until as u32, kind)),
            }
            at = until;
        }
    };
    for (offset, change) in &changes {
        put((*offset).min(line.len()), kinds.of(stack));
        stack.apply(change).ok()?;
    }
    put(line.len(), kinds.of(stack));
    pieces.iter_mut().for_each(|piece| piece.0 += mark as u32);
    if pieces.is_empty() && whole > 0 {
        pieces.push((whole as u32, Kind::Plain));
    }
    Some(pieces.into_boxed_slice())
}

/// What the system counts as held by this process, in bytes. It counts
/// in pages, and how large one is it says itself: not 4 KiB everywhere.
fn resident() -> usize {
    let pages = std::fs::read_to_string("/proc/self/statm").ok();
    let pages = pages.as_deref().and_then(|line| line.split(' ').nth(1)?.parse::<usize>().ok());
    // SAFETY: asks the system for a number and changes nothing.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096);
    pages.unwrap_or(0).saturating_mul(page)
}

impl Cutter {
    /// Reads the grammars. Nothing if the file of them does not fit the
    /// crate, which a test of the build rules out.
    pub fn new() -> Option<Self> {
        // The tests run in one process, which holds what all of them
        // compiled: it is over the threshold by what others did.
        let soft = if cfg!(test) { usize::MAX } else { SOFT };
        Some(Self { set: super::grammars()?, kinds: Kinds::new(), text: None, soft, slice: SLICE })
    }

    pub fn answer(&mut self, question: Question) -> Answer {
        match question {
            Question::Text { id, grammar, text } => self.take(id, grammar, text),
            Question::Change { id, of, keep, gone, text } => {
                self.change(id, of, keep as usize, gone as usize, text)
            }
            Question::Lines { id, from, count } => self.lines(id, from as usize, count as usize),
            // Read and answered, and nothing of it kept: it is not the
            // text that is cut, and is asked for a file whose colors
            // were given up as well.
            Question::Outline { id, text } => match super::outline::read(text) {
                Ok(entries) => Answer::Outline { id, entries },
                Err(why) => Answer::NoOutline { id, why },
            },
        }
    }

    fn take(&mut self, id: u64, grammar: &str, text: &[u8]) -> Answer {
        if self.text.is_some() && resident() > self.soft {
            return Answer::Full;
        }
        let Some(syntax) = self.set.find_syntax_by_name(grammar) else { return Answer::Refused };
        let place = (ParseState::new(syntax), ScopeStack::new());
        self.text = Some(Text {
            id,
            bytes: text.to_vec(),
            starts: line_starts(text),
            places: vec![place.clone()],
            at: 0,
            place,
            unknown: None,
            kept: Kept::default(),
        });
        Answer::Taken { id }
    }

    /// Replaces some lines of the text that is held. What was cut of the
    /// lines before them stays good, and so do the places up to there:
    /// what a line is depends on the lines above it and on nothing below.
    fn change(&mut self, id: u64, of: u64, keep: usize, gone: usize, new: &[u8]) -> Answer {
        // A change is another text as well: another file of the same
        // language comes as one. What holds too much ends here too.
        if resident() > self.soft {
            return Answer::Full;
        }
        let Some(text) = self.text.as_mut().filter(|text| text.id == of) else { return Answer::Refused };
        let lines = text.starts.len();
        let Some(behind) = keep.checked_add(gone).filter(|behind| keep < lines && *behind <= lines) else {
            return Answer::Refused;
        };
        let from = text.starts[keep] as usize;
        let to = text.starts.get(behind).map_or(text.bytes.len(), |start| *start as usize);
        // Lines that stay behind the new ones begin a line: what stands
        // in the place of whole lines is whole lines there.
        let whole = behind == lines || new.is_empty() || new.ends_with(b"\n");
        if !whole || text.bytes.len() - (to - from) + new.len() > FILE_MAX {
            return Answer::Refused;
        }
        text.bytes.splice(from..to, new.iter().copied());
        (text.id, text.starts) = (id, line_starts(&text.bytes));
        text.places.truncate(keep / EVERY + 1);
        text.kept.forget_from(keep);
        text.unknown = text.unknown.filter(|line| (*line as usize) < keep);
        if text.at > keep {
            let kept = text.places.len() - 1;
            (text.at, text.place) = (kept * EVERY, text.places[kept].clone());
        }
        Answer::Taken { id }
    }

    fn lines(&mut self, id: u64, from: usize, count: usize) -> Answer {
        let Some(text) = self.text.as_mut().filter(|text| text.id == id) else { return Answer::Refused };
        let pieces =
            |lines, unknown| Answer::Pieces { id, from: from.min(u32::MAX as usize) as u32, lines, unknown };
        // The lines that can be cut end with the text, or where nothing
        // is known any more. Behind that there is nothing to say but that.
        let end = text.unknown.map_or(text.starts.len(), |line| line as usize);
        let to = from.saturating_add(count).min(end);
        if from >= to {
            return pieces(Vec::new(), text.unknown);
        }
        let mut lines = Vec::new();
        let (begun, mut cut_so_far) = (Instant::now(), 0);
        while from + lines.len() < to && lines.len() < STRETCH {
            let wanted = from + lines.len();
            if let Some(kept) = text.kept.lines.get(&wanted) {
                lines.push(kept.clone());
                continue;
            }
            // It goes on where the last question left it, if that is
            // before this line and no place that is kept lies nearer to it.
            let near = (wanted / EVERY).min(text.places.len() - 1);
            if !(near * EVERY..=wanted).contains(&text.at) {
                (text.at, text.place) = (near * EVERY, text.places[near].clone());
            }
            // One line at the least, so that every answer is nearer.
            if cut_so_far == STRETCH || (cut_so_far > 0 && begun.elapsed() >= self.slice) {
                break;
            }
            cut_so_far += 1;
            let line = text.at;
            if line % EVERY == 0 && line / EVERY == text.places.len() && text.places.len() < PLACES {
                text.places.push(text.place.clone());
            }
            let (start, next) = (text.starts[line] as usize, text.starts.get(line + 1).copied());
            let bytes = &text.bytes[start..next.map_or(text.bytes.len(), |next| next as usize)];
            let Some(cut) = cut(&self.set, &self.kinds, &mut text.place, bytes, line == 0) else {
                // What follows depends on a line that was not cut: a color
                // that is wrong is worse than none. Where the crate failed
                // in the middle of a line the place is no place any more,
                // so the cutting stands at the last one that is kept.
                text.unknown = Some(line as u32);
                let kept = (line / EVERY).min(text.places.len() - 1);
                (text.at, text.place) = (kept * EVERY, text.places[kept].clone());
                return pieces(lines, text.unknown);
            };
            // A line on the way to the one that is wanted is kept as well.
            if line == wanted {
                lines.push(cut.clone());
            }
            text.kept.put(line, cut);
            text.at += 1;
        }
        // A stretch that ended before the lines asked for: nearer, not there.
        if lines.is_empty() {
            return Answer::Reached { id, line: text.at as u32 };
        }
        pieces(lines, None)
    }
}

/// Answers questions until there are no more. An error for a question
/// that is none of ours in its length, and for a pipe that broke.
pub fn serve(mut input: impl Read, mut output: impl Write) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let mut cutter = Cutter::new().ok_or_else(|| Error::from(ErrorKind::InvalidData))?;
    let (mut body, mut out) = (Vec::new(), Vec::new());
    loop {
        let mut head = [0; 4];
        match input.read_exact(&mut head) {
            // Nobody asks any more.
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            read => read?,
        }
        let length = wire::length(head, QUESTION_MAX).ok_or_else(|| Error::from(ErrorKind::InvalidData))?;
        body.resize(length, 0);
        input.read_exact(&mut body)?;
        let answer = Question::read(&body).map_or(Answer::Refused, |question| cutter.answer(question));
        out.clear();
        answer.write(&mut out);
        output.write_all(&out)?;
        output.flush()?;
        if answer == Answer::Full {
            return Ok(());
        }
    }
}

/// What codn does when it is started as this process, and with what it
/// ends. Called before anything else in `main`.
pub fn child() -> ExitCode {
    let limit = |what, most: u64| {
        let limit = libc::rlimit { rlim_cur: most, rlim_max: most };
        unsafe { libc::setrlimit(what, &limit) == 0 }
    };
    unsafe {
        // What the app had open is not this process's to hold: the PTY of
        // a session would stay open for as long as this runs. A kernel
        // that does not know the call is asked one by one.
        let all =
            libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, 0 as libc::c_uint);
        if all != 0 {
            (3..libc::sysconf(libc::_SC_OPEN_MAX).clamp(3, 65_536) as libc::c_int).for_each(|open| {
                libc::close(open);
            });
        }
        // It goes when the app goes, also when the app could not end it.
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
    }
    // Without the limit on its memory it does not run: the limit is what
    // lets a file end this process and nothing else.
    if !limit(libc::RLIMIT_AS, HARD) {
        return ExitCode::FAILURE;
    }
    limit(libc::RLIMIT_CORE, 0);
    if unsafe { libc::isatty(0) } == 1 {
        eprintln!(
            "codn: {} is how codn starts a process of its own, not for a command line",
            super::ARGUMENT
        );
        return ExitCode::FAILURE;
    }
    match serve(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: &str = "Bourne Again Shell (bash)";

    fn cutter(grammar: &str, text: &str) -> Cutter {
        let mut cutter = Cutter::new().unwrap();
        let taken = cutter.answer(Question::Text { id: 1, grammar, text: text.as_bytes() });
        assert_eq!(taken, Answer::Taken { id: 1 });
        cutter
    }

    /// The pieces of some lines of the text `id`, asked for until they
    /// are all there.
    fn pieces_of(cutter: &mut Cutter, id: u64, from: u32, count: u32) -> (Vec<Box<[Piece]>>, Option<u32>) {
        let mut got = Vec::new();
        loop {
            let at = from + got.len() as u32;
            match cutter.answer(Question::Lines { id, from: at, count: count - got.len() as u32 }) {
                Answer::Pieces { id: of, from, lines, unknown } => {
                    assert_eq!((of, from), (id, at));
                    let more = !lines.is_empty();
                    got.extend(lines);
                    if !more || unknown.is_some() || got.len() as u32 == count {
                        return (got, unknown);
                    }
                }
                Answer::Reached { id: of, line } => assert!(of == id && line <= at),
                other => panic!("{other:?}"),
            }
        }
    }

    fn pieces(cutter: &mut Cutter, from: u32, count: u32) -> (Vec<Box<[Piece]>>, Option<u32>) {
        pieces_of(cutter, 1, from, count)
    }

    /// All lines of the text `id`, and the line from which nothing is known.
    fn loop_pieces(cutter: &mut Cutter, id: u64) -> Vec<Box<[Piece]>> {
        pieces_of(cutter, id, 0, 9999).0
    }

    fn loop_unknown(cutter: &mut Cutter, id: u64) -> Option<u32> {
        pieces_of(cutter, id, 0, 9999).1
    }

    #[test]
    fn a_line_is_cut_into_pieces_that_follow_one_another_to_its_end() {
        let text = "// note\nfn main() {\n    let s = \"a\\n\"; // 1\n}\n";
        let mut cutter = cutter("Rust", text);
        let (lines, unknown) = pieces(&mut cutter, 0, 99);
        assert_eq!((lines.len(), unknown), (5, None), "the empty line at the end too");
        assert_eq!(&*lines[0], &[(8, Kind::Comment)]);
        assert_eq!(&*lines[4], &[]);
        for (line, pieces) in text.split_inclusive('\n').zip(&lines) {
            assert_eq!(pieces.last().map(|(until, _)| *until as usize), Some(line.len()), "{line:?}");
            assert!(pieces.windows(2).all(|pair| pair[0].0 < pair[1].0 && pair[0].1 != pair[1].1));
        }
        let kind_at = |line: usize, byte: u32| lines[line].iter().find(|(until, _)| *until > byte).unwrap().1;
        assert_eq!(kind_at(1, 0), Kind::Keyword, "fn");
        assert_eq!(kind_at(1, 3), Kind::Function, "main");
        assert_eq!(kind_at(1, 8), Kind::Plain, "the brace");
        assert_eq!((kind_at(2, 4), kind_at(2, 10)), (Kind::Keyword, Kind::Plain), "let, and =");
        assert_eq!(
            (kind_at(2, 12), kind_at(2, 14), kind_at(2, 16)),
            (Kind::String, Kind::Constant, Kind::String)
        );
        assert_eq!(kind_at(2, 22), Kind::Comment, "also the number inside it");
    }

    #[test]
    fn the_kinds_of_other_languages_are_told_by_the_same_names() {
        fn kinds<'a>(grammar: &str, text: &'a str) -> Vec<(&'a str, Kind)> {
            let (lines, _) = pieces(&mut cutter(grammar, text), 0, 99);
            let (mut from, mut found) = (0, Vec::new());
            for (until, kind) in lines[0].iter() {
                found.push((&text[from..*until as usize], *kind));
                from = *until as usize;
            }
            found
        }
        let has = |found: &[(&str, Kind)], piece: &str, kind: Kind| {
            assert!(
                found.iter().any(|(text, of)| text.contains(piece) && *of == kind),
                "{piece} in {found:?}"
            )
        };
        let json = kinds("JSON", "{\"key\": [1, true, \"s\"]}\n");
        has(&json, "key", Kind::Name);
        has(&json, "1", Kind::Constant);
        has(&json, "true", Kind::Constant);
        has(&json, "s", Kind::String);
        let html = kinds("HTML", "<a href=\"x\">t</a><!-- c -->\n");
        has(&html, "a", Kind::Name);
        has(&html, "href", Kind::Name);
        has(&html, "x", Kind::String);
        has(&html, "c", Kind::Comment);
        let python = kinds("Python", "def f(x): return not x and f\"a{x}b\"  # c\n");
        has(&python, "def", Kind::Keyword);
        has(&python, "f", Kind::Function);
        has(&python, "not", Kind::Keyword);
        has(&python, "and", Kind::Keyword);
        has(&python, "a", Kind::String);
        // Code inside a string is not the string.
        assert!(python.iter().any(|(text, kind)| text.contains('x') && *kind == Kind::Plain), "{python:?}");
        let ts = kinds("TypeScript", "class A extends B { n: number = 1 }\n");
        has(&ts, "class", Kind::Keyword);
        has(&ts, "A", Kind::Type);
        has(&ts, "number", Kind::Type);
        has(&ts, "=", Kind::Plain);
        let markdown = kinds("Markdown", "# Head\n");
        has(&markdown, "Head", Kind::Keyword);
        let yaml = kinds("YAML", "key: value # c\n");
        has(&yaml, "key", Kind::Name);
        // What SQL calls a constant is a name, and has no color of a number.
        let sql = kinds("SQL", "SELECT a.id, 10 FROM users a WHERE a.name = 'x';\n");
        has(&sql, "SELECT", Kind::Keyword);
        has(&sql, "10", Kind::Constant);
        has(&sql, "x", Kind::String);
        assert!(sql.iter().all(|(text, kind)| !text.contains("id") || *kind == Kind::Plain), "{sql:?}");
        // A key of a dictionary of Python is a string like any other.
        let python = kinds("Python", "d = {'key': 1}\n");
        has(&python, "key", Kind::String);
        let css = kinds("CSS", "a { margin: 16px; }\n");
        has(&css, "px", Kind::Constant);
    }

    #[test]
    fn lines_far_into_a_text_are_reached_in_stretches_and_are_the_same_by_any_way() {
        // A comment over many lines: what a line is depends on a line
        // far above it.
        let mut text = String::from("/*\n");
        for line in 0..1000 {
            text.push_str(&format!("let a{line} = {line};\n"));
        }
        text.push_str("*/\nlet z = 1;\n");
        let straight = pieces(&mut cutter("Rust", &text), 0, 2000).0;
        assert_eq!(straight.len(), 1004);
        assert_eq!(&*straight[500], &[(straight[500][0].0, Kind::Comment)]);
        assert_eq!(straight[1002][0], (3, Kind::Keyword));

        // Asked for the end first: not there in one answer.
        let mut far = cutter("Rust", &text);
        assert_eq!(
            far.answer(Question::Lines { id: 1, from: 1000, count: 4 }),
            Answer::Reached { id: 1, line: 64 }
        );
        assert_eq!(pieces(&mut far, 1000, 4).0, straight[1000..]);
        // Then lines before, from a place that is kept, and the end again.
        assert_eq!(pieces(&mut far, 130, 3).0, straight[130..133]);
        assert_eq!(pieces(&mut far, 1000, 4).0, straight[1000..]);
        let answer = far.answer(Question::Lines { id: 1, from: 1001, count: 2 });
        assert!(
            matches!(answer, Answer::Pieces { ref lines, .. } if lines[..] == straight[1001..1003]),
            "{answer:?}"
        );
        // No more than a stretch in one answer, and nothing behind the text.
        let mut whole = cutter("Rust", &text);
        let Answer::Pieces { lines, .. } = whole.answer(Question::Lines { id: 1, from: 0, count: 2000 })
        else {
            panic!("pieces")
        };
        assert_eq!(lines.len(), STRETCH);
        assert_eq!(pieces(&mut whole, 5000, 10), (Vec::new(), None));
        assert_eq!(pieces(&mut whole, 1004, 10), (Vec::new(), None));
    }

    #[test]
    fn nothing_is_known_from_a_line_that_is_too_long_or_no_text() {
        let long = format!("let a = 1;\nlet b = \"{}\";\nlet c = 3;\n", "x".repeat(LINE_MAX));
        let mut cutter_ = cutter("Rust", &long);
        let (lines, unknown) = pieces(&mut cutter_, 0, 10);
        assert_eq!((lines.len(), unknown), (1, Some(1)));
        // Asked for behind it, and again: nothing, and no work.
        assert_eq!(pieces(&mut cutter_, 2, 5), (Vec::new(), Some(1)));
        assert_eq!(pieces(&mut cutter_, 1, 5), (Vec::new(), Some(1)));
        assert_eq!(pieces(&mut cutter_, 0, 1).0.len(), 1, "what is before it stays known");

        let mut cutter_ = Cutter::new().unwrap();
        let text = b"let a = 1;\nlet b = \"\xff\";\n";
        assert_eq!(cutter_.answer(Question::Text { id: 1, grammar: "Rust", text }), Answer::Taken { id: 1 });
        assert_eq!(pieces(&mut cutter_, 0, 10).1, Some(1));
        // A line of just the length that is cut, is.
        let fits = format!("// {}\n", "x".repeat(LINE_MAX - 4));
        let (lines, unknown) = pieces(&mut cutter("Rust", &fits), 0, 1);
        assert_eq!((&*lines[0], unknown), (&[(LINE_MAX as u32, Kind::Comment)][..], None));
    }

    #[test]
    fn a_question_that_is_none_is_refused_and_a_new_text_takes_the_place_of_the_old() {
        let mut cutter_ = cutter("Rust", "fn a() {}\n");
        assert_eq!(cutter_.answer(Question::Lines { id: 2, from: 0, count: 1 }), Answer::Refused);
        assert_eq!(cutter_.answer(Question::Text { id: 2, grammar: "Klingon", text: b"" }), Answer::Refused);
        // The text before is still there after a text that was refused.
        assert_eq!(pieces(&mut cutter_, 0, 1).0.len(), 1);
        assert_eq!(
            cutter_.answer(Question::Text { id: 1, grammar: "Python", text: b"# a\n" }),
            Answer::Taken { id: 1 }
        );
        assert_eq!(pieces(&mut cutter_, 0, 9).0, vec![vec![(4, Kind::Comment)].into(), Vec::new().into()]);
        let mut none = Cutter::new().unwrap();
        assert_eq!(none.answer(Question::Lines { id: 1, from: 0, count: 1 }), Answer::Refused);
    }

    #[test]
    fn a_text_with_lines_replaced_is_cut_as_the_same_text_given_whole() {
        // A text long enough for places, with a comment opened in it, so
        // that what a line is depends on what was changed above it.
        let lines: Vec<String> = (0..300).map(|line| format!("let a{line} = {line};\n")).collect();
        let whole = |lines: &[String]| pieces(&mut cutter("Rust", &lines.concat()), 0, 999).0;
        let mut held = cutter("Rust", &lines.concat());
        assert_eq!(pieces(&mut held, 0, 999).0, whole(&lines));
        let change = |held: &mut Cutter, id, keep: u32, gone: u32, text: &str| {
            held.answer(Question::Change { id, of: id - 1, keep, gone, text: text.as_bytes() })
        };
        let all = |held: &mut Cutter, id| loop_pieces(held, id);

        // A line in the middle opens a comment: every line behind it is one.
        let mut now = lines.clone();
        now[150] = "/* open\n".into();
        assert_eq!(change(&mut held, 2, 150, 1, "/* open\n"), Answer::Taken { id: 2 });
        assert_eq!(all(&mut held, 2), whole(&now));
        assert_eq!(all(&mut held, 2)[299][0].1, Kind::Comment);
        // Two lines in the place of one, closing it again, then lines taken out.
        now.splice(200..201, ["*/\n".to_owned(), "fn f() {}\n".to_owned()]);
        assert_eq!(change(&mut held, 3, 200, 1, "*/\nfn f() {}\n"), Answer::Taken { id: 3 });
        assert_eq!(all(&mut held, 3), whole(&now));
        now.drain(10..20);
        assert_eq!(change(&mut held, 4, 10, 10, ""), Answer::Taken { id: 4 });
        assert_eq!(all(&mut held, 4), whole(&now));
        // The last line, which has no end, and lines put behind it.
        now.push("// end".into());
        assert_eq!(change(&mut held, 5, now.len() as u32 - 1, 1, "// end"), Answer::Taken { id: 5 });
        assert_eq!(all(&mut held, 5), whole(&now));
        // The whole of it at once.
        assert_eq!(change(&mut held, 6, 0, now.len() as u32, "fn g() {}\n"), Answer::Taken { id: 6 });
        assert_eq!(all(&mut held, 6), whole(&["fn g() {}\n".to_owned()]));

        // What is none: another text, lines there are not, half a line
        // before lines that stay, and more than is ever shown in colors.
        assert_eq!(
            held.answer(Question::Change { id: 9, of: 5, keep: 0, gone: 0, text: b"" }),
            Answer::Refused
        );
        assert_eq!(change(&mut held, 7, 2, 0, ""), Answer::Refused);
        assert_eq!(change(&mut held, 7, 0, 3, ""), Answer::Refused);
        assert_eq!(change(&mut held, 7, 0, u32::MAX, ""), Answer::Refused);
        assert_eq!(change(&mut held, 7, 0, 1, "half"), Answer::Refused);
        let much = "x".repeat(FILE_MAX);
        assert_eq!(change(&mut held, 7, 0, 0, &format!("{much}\n")), Answer::Refused);
        // And the text is as it was after all of them.
        assert_eq!(all(&mut held, 6), whole(&["fn g() {}\n".to_owned()]));
    }

    /// A pane that is moved asks for lines it was given a moment ago, and
    /// so does every key in a file. With no time at all an answer cuts
    /// one line and no more: what it holds beyond that was not cut for it.
    #[test]
    fn a_line_that_was_cut_is_not_cut_again_while_the_text_above_it_stays() {
        let lines: Vec<String> =
            (0..1500).map(|line| format!("let a{line} = \"{line}\"; // {line}\n")).collect();
        let straight = pieces(&mut cutter("Rust", &lines.concat()), 0, 9999).0;
        let mut held = cutter("Rust", &lines.concat());
        assert_eq!(pieces(&mut held, 1200, 150).0, straight[1200..1350]);
        held.slice = Duration::ZERO;
        let stretch =
            |held: &mut Cutter, id, from: u32| match held.answer(Question::Lines { id, from, count: 150 }) {
                Answer::Pieces { lines, .. } => lines,
                other => panic!("{other:?}"),
            };
        // Half a pane further: a stretch of what is known, at once.
        assert_eq!(stretch(&mut held, 1, 1226), straight[1226..1226 + STRETCH]);
        // Behind what is known the cutting goes on where it stood.
        assert_eq!(stretch(&mut held, 1, 1350), straight[1350..1351]);
        // Before what is kept, from the place before it, a line an answer.
        assert_eq!(
            held.answer(Question::Lines { id: 1, from: 300, count: 150 }),
            Answer::Reached { id: 1, line: 257 }
        );
        held.slice = SLICE;
        assert_eq!(pieces(&mut held, 300, 150).0, straight[300..450]);
        // The lines on the way there are known as well.
        held.slice = Duration::ZERO;
        assert_eq!(stretch(&mut held, 1, 256), straight[256..256 + STRETCH]);

        // A line that changes: what is above it stays known, and nothing
        // from it on, which is cut anew.
        let mut now = lines.clone();
        now[400] = "/* open\n".into();
        let change = Question::Change { id: 2, of: 1, keep: 400, gone: 1, text: b"/* open\n" };
        assert_eq!(held.answer(change), Answer::Taken { id: 2 });
        let anew = pieces(&mut cutter("Rust", &now.concat()), 0, 9999).0;
        assert_eq!(stretch(&mut held, 2, 350), anew[350..400]);
        held.slice = SLICE;
        assert_eq!(pieces_of(&mut held, 2, 350, 150).0, anew[350..500]);
        assert_eq!(anew[450][0].1, Kind::Comment);
    }

    #[test]
    fn no_more_of_what_was_cut_is_kept_than_the_limits_say() {
        let text: String = (0..3000).map(|line| format!("let a{line} = {line};\n")).collect();
        let mut held = cutter("Rust", &text);
        let straight = pieces(&mut held, 0, 9999).0;
        assert_eq!(straight.len(), 3001);
        let kept = &held.text.as_ref().unwrap().kept;
        // The lines nearest to the last that was cut.
        assert_eq!(kept.lines.len(), KEPT);
        assert_eq!(kept.lines.keys().next().copied(), Some(3001 - KEPT));
        assert_eq!(kept.pieces, kept.lines.values().map(|line| line.len()).sum::<usize>());
        // What is not kept any more is cut again, and is the same.
        assert_eq!(pieces(&mut held, 0, 100).0, straight[..100]);
        let kept = &held.text.as_ref().unwrap().kept;
        assert_eq!((kept.lines.len(), kept.lines.keys().next().copied()), (KEPT, Some(0)));

        // Lines of very many pieces: fewer of them are kept.
        let many = format!("let a = {}1;\n", "1+".repeat(7000));
        let mut held = cutter("Rust", &many.repeat(40));
        let straight = pieces(&mut held, 0, 99).0;
        assert!(straight[0].len() > 10_000, "{}", straight[0].len());
        let kept = &held.text.as_ref().unwrap().kept;
        assert!(kept.pieces <= PIECES && kept.lines.len() < 30, "{} {}", kept.pieces, kept.lines.len());
        assert_eq!(kept.pieces, kept.lines.values().map(|line| line.len()).sum::<usize>());
        assert_eq!(pieces(&mut held, 0, 99).0, straight);
    }

    #[test]
    fn nothing_known_is_known_again_when_the_line_it_came_from_goes() {
        let long = format!("let a = 1;\nlet b = \"{}\";\nlet c = 3;\n", "x".repeat(LINE_MAX));
        let mut held = cutter("Rust", &long);
        assert_eq!(pieces(&mut held, 0, 10).1, Some(1));
        // A change behind that line changes nothing of it, one at it does.
        assert_eq!(
            held.answer(Question::Change { id: 2, of: 1, keep: 2, gone: 1, text: b"let d = 4;\n" }),
            Answer::Taken { id: 2 }
        );
        assert_eq!(loop_unknown(&mut held, 2), Some(1));
        assert_eq!(
            held.answer(Question::Change { id: 3, of: 2, keep: 1, gone: 1, text: b"let b = 2;\n" }),
            Answer::Taken { id: 3 }
        );
        assert_eq!((loop_pieces(&mut held, 3).len(), loop_unknown(&mut held, 3)), (4, None));
    }

    /// A stretch of lines can take longer than an answer is waited for:
    /// in a code block of Markdown, sixty-four lines of a thousand bytes
    /// took twenty seconds. So an answer ends when its time is up.
    #[test]
    fn an_answer_ends_when_its_time_is_up_and_is_the_nearer_for_it() {
        let text: String = (0..200).map(|line| format!("let a{line} = \"{line}\"; // {line}\n")).collect();
        let straight = pieces(&mut cutter("Rust", &text), 0, 999).0;
        // With no time at all, every answer is one line and no more.
        let mut hurried = cutter("Rust", &text);
        hurried.slice = Duration::ZERO;
        let Answer::Pieces { lines, .. } = hurried.answer(Question::Lines { id: 1, from: 0, count: 50 })
        else {
            panic!("pieces")
        };
        assert_eq!(lines.len(), 1);
        assert_eq!(
            hurried.answer(Question::Lines { id: 1, from: 150, count: 5 }),
            Answer::Reached { id: 1, line: 2 }
        );
        assert_eq!(
            hurried.answer(Question::Lines { id: 1, from: 150, count: 5 }),
            Answer::Reached { id: 1, line: 3 }
        );
        // And the pieces are those of a cutter that had all the time.
        assert_eq!(pieces(&mut hurried, 0, 999).0, straight);
        assert_eq!(pieces(&mut hurried, 190, 5).0, straight[190..195]);
        // With time enough an answer is a stretch, as before.
        let Answer::Pieces { lines, .. } =
            cutter("Rust", &text).answer(Question::Lines { id: 1, from: 0, count: 99 })
        else {
            panic!("pieces")
        };
        assert_eq!(lines.len(), STRETCH);
    }

    #[test]
    fn a_process_that_holds_too_much_ends_at_the_next_text_however_that_comes() {
        let mut full = cutter("Rust", "fn a() {}\n");
        full.soft = 0;
        // As a change: another file of the same language comes so.
        let change = Question::Change { id: 2, of: 1, keep: 0, gone: 1, text: b"fn b() {}\n" };
        assert_eq!(full.answer(change), Answer::Full);
        assert_eq!(full.answer(Question::Text { id: 2, grammar: "Rust", text: b"" }), Answer::Full);
        // Lines of the text it has are still answered: it is the next
        // text that it does not take.
        assert_eq!(pieces(&mut full, 0, 1).0.len(), 1);
        // The first text of a process is always taken.
        let mut fresh = Cutter::new().unwrap();
        fresh.soft = 0;
        assert_eq!(
            fresh.answer(Question::Text { id: 1, grammar: "Rust", text: b"" }),
            Answer::Taken { id: 1 }
        );
    }

    #[test]
    fn a_mark_at_the_start_of_a_file_is_no_part_of_its_first_line() {
        let mut marked = Cutter::new().unwrap();
        let text = "\u{feff}# Head\n\u{feff}# no head\n";
        let taken = marked.answer(Question::Text { id: 1, grammar: "Markdown", text: text.as_bytes() });
        assert_eq!(taken, Answer::Taken { id: 1 });
        let lines = pieces(&mut marked, 0, 9).0;
        // The heading is one, and its pieces count the three bytes in.
        assert_eq!(lines[0].last(), Some(&(10, Kind::Keyword)), "{:?}", lines[0]);
        assert!(lines[0].iter().all(|(_, kind)| *kind == Kind::Keyword), "{:?}", lines[0]);
        // In a later line the same bytes are text like any other.
        assert!(lines[1].iter().all(|(_, kind)| *kind == Kind::Plain), "{:?}", lines[1]);
        // A file of nothing but the mark.
        let mut only = Cutter::new().unwrap();
        assert_eq!(
            only.answer(Question::Text { id: 1, grammar: "Rust", text: MARK }),
            Answer::Taken { id: 1 }
        );
        assert_eq!(pieces(&mut only, 0, 9).0, vec![vec![(3, Kind::Plain)].into()]);
    }

    /// A pattern can take a piece of the file in: the word that ends a
    /// heredoc, the fence of a code block, the hashes of a raw string.
    /// It is compiled with that piece when it is used, and one that did
    /// not compile would end the process. The pieces are escaped by the
    /// crate; here they are made of what a pattern would choke on.
    #[test]
    fn a_piece_of_the_file_that_goes_into_a_pattern_does_not_break_it() {
        let odd = ".*+?[](){}|^$\\#&-~";
        let long = "x".repeat(2000);
        let hashes = "#".repeat(200);
        let fence = "`".repeat(300);
        let texts: [(&str, String); 9] = [
            (SHELL, format!("cat <<'{odd}'\ntext\n{odd}\necho done\n")),
            (SHELL, format!("cat <<{long}\ntext\n{long}\necho done\n")),
            ("Rust", format!("let a = r{hashes}\"text\"{hashes};\nlet b = 1;\n")),
            ("C++", format!("auto a = R\"{}(text){}\";\nint b = 1;\n", &odd[..8], &odd[..8])),
            ("Markdown", format!("{fence}rust\nlet a = 1;\n{fence}\n# Head\n")),
            ("Markdown", format!("```{odd}\ntext\n```\n# Head\n")),
            ("Markdown", format!("~~~{long}\ntext\n~~~\n# Head\n")),
            ("YAML", format!("a: |{odd}\n  text\nb: >-\n  more\nc: 1\n")),
            ("Python", format!("a = r'''{odd}'''\nb = f\"{{{odd}!r:>{{width}}}}\"\n")),
        ];
        for (grammar, text) in &texts {
            let mut cutter = cutter(grammar, text);
            let (lines, unknown) = pieces(&mut cutter, 0, 99);
            assert_eq!(
                (lines.len(), unknown),
                (text.split_inclusive('\n').count() + 1, None),
                "{grammar}: {text:?}"
            );
        }
        // What follows such a piece is cut as what it is.
        let (lines, _) = pieces(&mut cutter("Markdown", &texts[4].1), 0, 99);
        assert!(lines[1].iter().any(|(_, kind)| *kind == Kind::Keyword), "{:?}", lines[1]);
        assert!(lines[3].iter().all(|(_, kind)| *kind == Kind::Keyword), "{:?}", lines[3]);
    }

    #[test]
    fn questions_are_answered_over_a_pipe_until_it_ends() {
        let mut input = Vec::new();
        Question::Text { id: 5, grammar: "Rust", text: b"// a\nfn b() {}\n" }.write(&mut input);
        Question::Lines { id: 5, from: 0, count: 2 }.write(&mut input);
        input.extend_from_slice(&3u32.to_le_bytes());
        input.extend_from_slice(b"odd");
        let mut output = Vec::new();
        serve(&input[..], &mut output).unwrap();
        let mut answers = Vec::new();
        let mut rest = &output[..];
        while let Some((head, behind)) = rest.split_first_chunk::<4>() {
            let length = wire::length(*head, wire::ANSWER_MAX).unwrap();
            answers.push(Answer::read(&behind[..length]).unwrap());
            rest = &behind[length..];
        }
        assert_eq!(answers.len(), 3);
        assert_eq!((&answers[0], &answers[2]), (&Answer::Taken { id: 5 }, &Answer::Refused));
        assert!(
            matches!(&answers[1], Answer::Pieces { id: 5, from: 0, lines, unknown: None } if lines.len() == 2)
        );
        // A question longer than any is not read at all, and one that
        // ends in its middle is a pipe that broke.
        let long = (QUESTION_MAX as u32 + 1).to_le_bytes();
        assert!(serve(&long[..], &mut Vec::new()).is_err());
        assert!(serve(&input[..9], &mut Vec::new()).is_err());
        assert!(serve(&[][..], &mut Vec::new()).is_ok());
    }
}
