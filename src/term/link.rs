//! Addresses in the text of the grid, found where the pointer is.
//!
//! Nothing is looked for until a cell is asked about, and then only the
//! text around that cell: the grid is not searched, and no cell keeps
//! anything. An address is `http://` or `https://` and what follows it up
//! to a blank, as it is read: with the marks over its letters, and
//! without text that is hidden or has the color of what it stands on. What a program marks as a link itself (OSC 8) is not kept
//! by the emulator, see `PATCHES.md` there.

use std::ops::Range;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};

use super::{CellAt, Terminal};

/// The longest address that is opened, in bytes, and how many cells are
/// read to each side of the pointer for one. The emulator is locked
/// while they are read.
const MAX: usize = 2048;

const SCHEMES: [&str; 2] = ["http://", "https://"];

/// The text around a cell without a blank in it, and where each letter
/// of it stands. Kept between frames by who asks every frame.
#[derive(Default)]
pub(super) struct Run {
    letters: Vec<char>,
    points: Vec<Point>,
}

impl Run {
    fn clear(&mut self) {
        self.letters.clear();
        self.points.clear();
    }

    fn push(&mut self, point: Point, letter: char) {
        self.letters.push(letter);
        self.points.push(point);
    }

    /// The cells a part of the text stands in, from the first to the last.
    pub(super) fn cells(&self, span: &Range<usize>) -> SelectionRange {
        SelectionRange::new(self.points[span.start], self.points[span.end - 1], false)
    }

    /// The address as it is handed on: the letters as they were
    /// printed, with their marks, and the scheme in small letters, which
    /// is what it means in any case.
    pub(super) fn text(&self, span: Range<usize>) -> String {
        let mut text: String = self.letters[span].iter().collect();
        if let Some(scheme) = text.find(':').and_then(|end| text.get_mut(..end)) {
            scheme.make_ascii_lowercase();
        }
        text
    }
}

impl Terminal {
    /// The address a click on a cell opens: the one the last frame
    /// underlined under it, and no other. The emulator is not asked
    /// anew: what a program printed since is another text than the one
    /// that is seen, and the emulator may be held by the reader for long.
    pub fn link(&self, at: CellAt) -> Option<String> {
        self.snapshot.borrow().address(at)
    }

    /// Whether the last frame drew an address as one that a click opens.
    pub fn linked(&self) -> bool {
        self.snapshot.borrow().linked()
    }
}

/// What a cell is to an address.
enum Part {
    Letter(char),
    /// The second half of a wide letter, or the room left for one that
    /// did not fit at the end of a row.
    Spacer,
    End,
}

fn part(cell: &Cell) -> Part {
    let c = cell.c;
    // Text that is not shown is not part of what a click opens: the
    // address would be another than the one that is read. Hidden, or
    // in the color it stands on, said the same way for both. Two ways
    // of saying one color are not held against each other here, and
    // colors that are near each other cannot be: the line under an
    // address is drawn in a color of codn's own for that (`paint`).
    // Asked before anything else of a cell: a hidden one is an end
    // also where it is the half of a wide letter.
    if cell.flags.contains(Flags::HIDDEN) || cell.fg == cell.bg {
        Part::End
    } else if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
        Part::Spacer
    } else if ends(c) {
        Part::End
    } else {
        Part::Letter(c)
    }
}

/// Whether a letter cannot stand in an address. Besides blanks and what
/// encloses an address in a text: the lines programs draw their frames
/// with, and the signs of a font for icons, which stand in prompts.
fn ends(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '<' | '>' | '"' | '`' | '{' | '|' | '}' | '^' | '\\' | '\u{27e8}' | '\u{27e9}')
        || ('\u{2500}'..='\u{259f}').contains(&c)
        || ('\u{e000}'..='\u{f8ff}').contains(&c)
        || ('\u{f0000}'..='\u{10fffd}').contains(&c)
}

/// Fills `run` with the text around a point of the grid and says which
/// part of it is the address the point is in. A row that the emulator
/// broke because it was full goes on in the next one; a row that a
/// program ended itself does not.
pub(super) fn find<T>(term: &Term<T>, point: Point, run: &mut Run) -> Option<Range<usize>> {
    run.clear();
    let grid = term.grid();
    // A pane can be laid out for more cells than its terminal has yet.
    if point.column.0 >= term.columns()
        || point.line < term.topmost_line()
        || point.line > term.bottommost_line()
    {
        return None;
    }
    let last = Column(term.columns() - 1);
    let wraps = |line: Line| grid[line][last].flags.contains(Flags::WRAPLINE);
    // What a cell is, where it stands. The half of a wide letter is
    // passed over only where it belongs to one: behind the letter, or
    // at the end of a row that was too short for it. A program can
    // leave one alone, by taking the letter away, and it is drawn as a
    // gap: the text on its two sides is two texts.
    let part = |at: Point| {
        let cell = &grid[at];
        let wide = |column: usize| grid[at.line][Column(column)].flags.contains(Flags::WIDE_CHAR);
        let alone = match part(cell) {
            Part::Spacer if cell.flags.contains(Flags::WIDE_CHAR_SPACER) => {
                at.column.0 == 0 || !wide(at.column.0 - 1)
            }
            Part::Spacer => at.column != last,
            _ => false,
        };
        if alone { Part::End } else { part(cell) }
    };

    // To the left, the point itself first. A cell is its letter and
    // the marks that stand over it, here the last of them first.
    let mut at = point;
    for step in 0.. {
        let cell = &grid[at];
        match part(at) {
            Part::Letter(c) => {
                for &mark in cell.zerowidth().unwrap_or_default().iter().rev() {
                    run.push(at, mark);
                }
                run.push(at, c);
            }
            Part::Spacer => {}
            Part::End => break,
        }
        // An address that begins further left is longer than any that
        // is opened. What was read is looked at all the same: a short
        // address can stand at the end of a long text without blanks.
        if step == MAX || run.letters.len() > MAX {
            break;
        }
        if at.column.0 > 0 {
            at.column = Column(at.column.0 - 1);
        } else if at.line > term.topmost_line() && wraps(Line(at.line.0 - 1)) {
            at = Point::new(Line(at.line.0 - 1), last);
        } else {
            break;
        }
    }
    run.letters.reverse();
    run.points.reverse();
    let under = run.letters.len().checked_sub(1)?;

    let mut at = point;
    for step in 0.. {
        if at.column < last {
            at.column = Column(at.column.0 + 1);
        } else if at.line < term.bottommost_line() && wraps(at.line) {
            at = Point::new(Line(at.line.0 + 1), Column(0));
        } else {
            break;
        }
        let cell = &grid[at];
        match part(at) {
            Part::Letter(c) => {
                run.push(at, c);
                for &mark in cell.zerowidth().unwrap_or_default() {
                    run.push(at, mark);
                }
            }
            Part::Spacer => {}
            Part::End => break,
        }
        // Longer than anything that is opened: not read to its end.
        if step + 1 == MAX || run.letters.len() - under > MAX {
            return None;
        }
    }
    let bytes = |span: &Range<usize>| run.letters[span.clone()].iter().map(|c| c.len_utf8()).sum::<usize>();
    span(&run.letters, under).filter(|span| bytes(span) <= MAX)
}

/// The address in a text without blanks that the letter at `under` is
/// part of: from the first `http://` or `https://` at or before that
/// letter to the end, without what closes a sentence or a bracket there.
fn span(letters: &[char], under: usize) -> Option<Range<usize>> {
    let begins = |at: usize| {
        let rest = &letters[at..];
        let is = |scheme: &str| {
            rest.len() >= scheme.len()
                && rest.iter().zip(scheme.chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b))
        };
        SCHEMES.into_iter().find(|scheme| is(scheme)).map(str::len)
    };
    let (start, scheme) = (0..=under).find_map(|at| begins(at).map(|len| (at, len)))?;
    // How many more brackets open than close, round and square.
    let mut open = [0i32; 2];
    for c in &letters[start..] {
        match c {
            '(' => open[0] += 1,
            ')' => open[0] -= 1,
            '[' => open[1] += 1,
            ']' => open[1] -= 1,
            _ => {}
        }
    }
    let mut end = letters.len();
    while end > start + scheme {
        match letters[end - 1] {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => {}
            // A bracket that was opened before the address closes after it.
            ')' if open[0] < 0 => open[0] += 1,
            ']' if open[1] < 0 => open[1] += 1,
            _ => break,
        }
        end -= 1;
    }
    // Something stands behind the two strokes, and the pointer is on
    // the address and not on what was cut off behind it.
    (end > start + scheme && under < end).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::term::Grid;

    /// A terminal of some columns and four rows that was sent `output`.
    fn term(cols: u16, output: &str) -> Term<VoidListener> {
        let grid = Grid { cols, rows: 4, cell_w: 8, cell_h: 16 };
        let mut term = Term::new(Config::default(), &grid, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, output.as_bytes());
        term
    }

    /// The address under a cell, and the cells it is drawn in.
    fn under(term: &Term<VoidListener>, line: i32, column: usize) -> Option<(String, SelectionRange)> {
        let mut run = Run::default();
        let span = find(term, Point::new(Line(line), Column(column)), &mut run)?;
        Some((run.text(span.clone()), run.cells(&span)))
    }

    fn address(text: &str, column: usize) -> Option<String> {
        under(&term(80, text), 0, column).map(|(address, _)| address)
    }

    #[test]
    fn an_address_is_found_under_any_of_its_letters_and_nowhere_else() {
        let text = "see https://example.org/a?b=c#d for more";
        for column in 4..31 {
            assert_eq!(address(text, column).as_deref(), Some("https://example.org/a?b=c#d"), "{column}");
        }
        for column in [0, 2, 3, 31, 32, 35, 60] {
            assert_eq!(address(text, column), None, "{column}");
        }
        // The cells it stands in are those that are underlined.
        let (_, cells) = under(&term(80, text), 0, 10).unwrap();
        assert_eq!((cells.start.column.0, cells.end.column.0), (4, 30));
        assert_eq!(address("http://localhost:3000/", 8).as_deref(), Some("http://localhost:3000/"));
        assert_eq!(address("HTTPS://EXAMPLE.ORG", 3).as_deref(), Some("https://EXAMPLE.ORG"));
    }

    #[test]
    fn only_http_and_https_are_addresses() {
        for text in [
            "file:///etc/passwd",
            "ftp://example.org/a",
            "javascript:alert(1)",
            "mailto:a@example.org",
            "ssh://host/path",
            "www.example.org",
            "src/app.rs:42",
            "https://",
            "http:/example.org",
        ] {
            assert_eq!(address(text, 2), None, "{text}");
        }
        // The address begins where its scheme does, whatever stands
        // before, and what stands before is none.
        assert_eq!(address("url=https://example.org", 6).as_deref(), Some("https://example.org"));
        assert_eq!(address("url=https://example.org", 1), None);
        assert_eq!(address("file://x/https://example.org", 12).as_deref(), Some("https://example.org"));
        assert_eq!(address("file://x/https://example.org", 3), None);
    }

    #[test]
    fn what_closes_a_sentence_or_a_bracket_is_not_part_of_the_address() {
        let found = |text: &str| address(text, text[..text.find("http").unwrap()].chars().count() + 3);
        assert_eq!(found("Read https://example.org/a.").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("(see https://example.org/a)").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("[docs](https://example.org/a).").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("**https://example.org/a**").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("'https://example.org/a',").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("<https://example.org/a>").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("\"https://example.org/a\"").as_deref(), Some("https://example.org/a"));
        // A bracket that opens within the address closes within it.
        let wiki = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
        assert_eq!(found(wiki).as_deref(), Some(wiki));
        assert_eq!(found(&format!("({wiki})")).as_deref(), Some(wiki));
        assert_eq!(found("http://[::1]:8080/").as_deref(), Some("http://[::1]:8080/"));
        // On what was cut off there is nothing to open.
        assert_eq!(address("https://example.org/a).", 21), None);
        assert_eq!(address("https://example.org/a).", 22), None);
        // The lines of a frame, and the signs of a prompt.
        assert_eq!(found("\u{2502}https://example.org/a\u{2502}").as_deref(), Some("https://example.org/a"));
        assert_eq!(found("\u{e0b0}https://example.org/a\u{e0b0}").as_deref(), Some("https://example.org/a"));
    }

    #[test]
    fn a_row_the_emulator_broke_goes_on_and_one_a_program_ended_does_not() {
        // Twenty columns: the address fills the first row and goes on.
        let broken = term(20, "https://example.org/a/long/path?q=1 x");
        let whole = "https://example.org/a/long/path?q=1";
        for (line, column) in [(0, 0), (0, 19), (1, 0), (1, 14)] {
            let (address, cells) = under(&broken, line, column).unwrap();
            assert_eq!(address, whole, "{line} {column}");
            assert_eq!(
                (cells.start, cells.end),
                (Point::new(Line(0), Column(0)), Point::new(Line(1), Column(14)))
            );
        }
        assert_eq!(under(&broken, 1, 15), None);
        // The same text with a new line in it is two texts.
        let ended = term(20, "https://example.org/\r\na/long/path?q=1 x");
        assert_eq!(under(&ended, 0, 3).unwrap().0, "https://example.org/");
        assert_eq!(under(&ended, 1, 3), None);
        // Across three rows, and from the history when the view is there.
        let mut scrolled = term(10, "see https://example.org/a/b/c/d/e\r\n1\r\n2\r\n3\r\n4");
        assert_eq!(under(&scrolled, -4, 9).unwrap().0, "https://example.org/a/b/c/d/e");
        scrolled.scroll_display(alacritty_terminal::grid::Scroll::Top);
        let at = Terminal::point(&scrolled, CellAt { col: 2, row: 1, right: false }).0;
        let mut run = Run::default();
        assert_eq!(
            find(&scrolled, at, &mut run).map(|span| run.text(span)).unwrap(),
            "https://example.org/a/b/c/d/e"
        );
    }

    #[test]
    fn a_wide_letter_and_a_hidden_one() {
        // A wide letter takes two cells, also where a row is broken.
        let wide = term(80, "https://example.org/\u{6f22}\u{5b57}/a");
        for column in [3, 20, 21, 22, 23, 25] {
            assert_eq!(
                under(&wide, 0, column).unwrap().0,
                "https://example.org/\u{6f22}\u{5b57}/a",
                "{column}"
            );
        }
        assert_eq!(under(&wide, 0, 3).unwrap().1.end.column.0, 25);
        let broken = term(21, "https://example.org/\u{6f22}/a");
        assert_eq!(under(&broken, 0, 3).unwrap().0, "https://example.org/\u{6f22}/a");
        assert_eq!(under(&broken, 1, 3).unwrap().0, "https://example.org/\u{6f22}/a");
        // What is not shown is not opened: the address that is read ends
        // before it.
        let hidden = term(80, "https://example.org\x1b[8m.evil.example\x1b[28m/a");
        assert_eq!(under(&hidden, 0, 3).unwrap().0, "https://example.org");
        assert_eq!(under(&hidden, 0, 22), None);
    }

    #[test]
    fn an_address_too_long_to_open_is_none_and_a_cell_outside_the_grid_has_none() {
        let path = |len: usize| format!("https://example.org/{}", "a".repeat(len));
        // The rows are broken by the emulator, two hundred columns each.
        let fits = term(200, &path(MAX - 20));
        assert_eq!(under(&fits, -3, 5).unwrap().0.len(), MAX);
        assert_eq!(under(&fits, fits.topmost_line().0, 0).unwrap().0.len(), MAX);
        assert_eq!(under(&fits, fits.bottommost_line().0, 0).unwrap().0.len(), MAX);
        for len in [MAX - 19, MAX + 1, 3 * MAX] {
            let long = term(200, &path(len));
            let top = long.topmost_line().0;
            assert_eq!(under(&long, top, 5), None, "{len}");
            assert_eq!(under(&long, long.bottommost_line().0, 0), None, "{len}");
        }
        // Nor is it read to its end, with the emulator locked.
        let long = term(200, &path(3 * MAX));
        let mut run = Run::default();
        assert_eq!(find(&long, Point::new(long.topmost_line(), Column(5)), &mut run), None);
        assert!(run.letters.len() <= MAX + 8, "{}", run.letters.len());
        // Letters of more than a byte count as their bytes.
        let bytes = term(200, &format!("https://example.org/{}", "\u{e4}".repeat(MAX / 2)));
        assert_eq!(under(&bytes, 0, 5), None);
        // A pane laid out for a larger grid than the terminal has.
        let small = term(20, "https://example.org");
        assert_eq!(under(&small, 0, 20), None);
        assert_eq!(under(&small, 4, 0), None);
        assert_eq!(under(&small, -1, 0), None);
    }

    #[test]
    fn a_mark_that_takes_no_cell_is_opened_as_it_is_read() {
        // U+0301 stands over the cell before it and is drawn there.
        // Read was "exámple", opened "example".
        let marked = term(80, "https://exa\u{301}mple.org/x and e\u{301}");
        for column in [3, 10, 11, 20] {
            assert_eq!(under(&marked, 0, column).unwrap().0, "https://exa\u{301}mple.org/x", "{column}");
        }
        assert_eq!(under(&marked, 0, 3).unwrap().1.end.column.0, 20);
        assert_eq!(under(&marked, 0, 26), None);
        // Marks count against the length that is opened, as their bytes.
        let heavy = format!("https://example.org/{}", "a\u{301}".repeat(MAX / 3));
        assert_eq!(under(&term(200, &heavy), 0, 5), None);
        // And against what is read with the emulator locked: a cell holds
        // up to 128 of them.
        let marks = "\u{301}".repeat(128);
        let dense = format!("{}https://example.org/a", format!("x{marks}").repeat(40));
        let dense = term(200, &dense);
        assert_eq!(under(&dense, 0, 45).unwrap().0, "https://example.org/a");
        let mut run = Run::default();
        find(&dense, Point::new(Line(0), Column(45)), &mut run);
        assert!(run.letters.len() <= 2 * MAX + 2 * 129, "{}", run.letters.len());
    }

    #[test]
    fn a_short_address_behind_a_long_text_without_blanks_is_found() {
        // The walk to the left gave the whole lookup up after 2048 cells.
        let long = term(200, &format!("{}https://example.org/a", "x".repeat(MAX + 1)));
        let last = long.bottommost_line().0;
        assert_eq!(under(&long, last, 55).unwrap().0, "https://example.org/a");
        // One that begins further left than is read is still none.
        let far = term(200, &format!("https://example.org/{}", "a".repeat(3 * MAX)));
        assert_eq!(under(&far, far.bottommost_line().0, 5), None);
    }

    #[test]
    fn text_in_the_color_of_its_ground_ends_an_address() {
        // Black on black, and one color said twice: nothing of the tail
        // is read, and it was opened.
        for hide in ["\x1b[30;40m", "\x1b[38;2;9;9;9;48;2;9;9;9m", "\x1b[7;31;41m", "\x1b[38;5;200;48;5;200m"]
        {
            let same = term(80, &format!("https://example.org{hide}.evil.example/x\x1b[0m"));
            assert_eq!(under(&same, 0, 3).unwrap().0, "https://example.org", "{hide:?}");
            assert_eq!(under(&same, 0, 22), None, "{hide:?}");
        }
        // A color on another ground is text.
        let shown = term(80, "https://example.org\x1b[31;40m/shown\x1b[0m");
        assert_eq!(under(&shown, 0, 3).unwrap().0, "https://example.org/shown");
    }

    #[test]
    fn the_scheme_is_handed_on_in_small_letters_and_an_icon_ends_an_address() {
        assert_eq!(
            address("HTTP://Example.org/A?HTTPS://B", 3).as_deref(),
            Some("http://Example.org/A?HTTPS://B")
        );
        // An icon of a font in the private use area behind the first
        // plane, where Nerd Fonts 3 has the Material icons.
        assert_eq!(address("https://example.org/a\u{f0001}", 3).as_deref(), Some("https://example.org/a"));
        assert_eq!(address("\u{100000}https://example.org/a", 3).as_deref(), Some("https://example.org/a"));
    }

    #[test]
    fn the_half_of_a_wide_letter_that_is_hidden_or_alone_ends_an_address() {
        // A wide letter, hidden, with its first cell taken away (DCH):
        // what is left is a hidden half, which was passed over.
        let hidden = term(80, "https://example.org\x1b[8m\u{6f22}\x1b[28m.evil.test\x1b[1;20H\x1b[P");
        let cell = &hidden.grid()[Line(0)][Column(19)];
        assert!(cell.flags.contains(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER));
        assert_eq!(under(&hidden, 0, 3).unwrap().0, "https://example.org");
        // The same without hiding: the half is drawn as a gap, and the
        // text behind the gap is none of the address.
        let alone = term(80, "https://example.org\u{6f22}.evil.test\x1b[1;20H\x1b[P");
        assert!(alone.grid()[Line(0)][Column(19)].flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(under(&alone, 0, 3).unwrap().0, "https://example.org");
        assert_eq!(under(&alone, 0, 22), None);
        // A half in the color of its ground, behind its letter.
        let same = term(80, "https://example.org/\x1b[30;40m\u{6f22}\x1b[0m/a");
        assert_eq!(under(&same, 0, 3).unwrap().0, "https://example.org/");
    }
}
