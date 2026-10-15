//! Selecting text on the grid and taking it out.

use alacritty_terminal::event::Event as Raw;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::Term;

use super::{CellAt, Listener, Report, Select, Terminal};

/// A selection of more cells or bytes than this is not copied. The count
/// is made with the emulator locked, and the text is one allocation.
const MAX_CELLS: usize = 100_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;

impl Terminal {
    /// The point of the grid that a cell of the view shows, which is
    /// further up while the view is in the history.
    pub(super) fn point<T>(term: &Term<T>, at: CellAt) -> (Point, Side) {
        let offset = term.grid().display_offset() as i32;
        let side = if at.right { Side::Right } else { Side::Left };
        (Point::new(Line(at.row as i32 - offset), Column(at.col)), side)
    }

    pub fn select(&self, kind: Select, at: CellAt) {
        let kind = match kind {
            Select::Cells => SelectionType::Simple,
            Select::Words => SelectionType::Semantic,
            Select::Lines => SelectionType::Lines,
        };
        let mut term = self.term.lock();
        let (point, side) = Self::point(&term, at);
        term.selection = Some(Selection::new(kind, point, side));
    }

    pub fn select_to(&self, at: CellAt) {
        let mut term = self.term.lock();
        let (point, side) = Self::point(&term, at);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    /// Ends a drag. A selection without content was just a click and goes away.
    pub fn select_end(&self) {
        let mut term = self.term.lock();
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
    }

    /// The selected text, if there is any. One that is too large to copy is
    /// not taken, and the app is told.
    pub fn selection_text(&self) -> Option<String> {
        let term = self.term.lock();
        let range = term.selection.as_ref().and_then(|selection| selection.to_range(&*term))?;
        let text = (!too_large(&term, &range)).then(|| term.selection_to_string());
        drop(term);
        match text {
            Some(text) if text.as_ref().is_none_or(|text| text.len() <= MAX_BYTES) => {
                text.filter(|text| !text.is_empty())
            }
            _ => {
                (self.listener.notify)(Report(Raw::SelectionRejected));
                None
            }
        }
    }
}

/// Whether a selection is more than is copied. Bounds the work done while
/// the emulator is held, and rejects before a large String is allocated.
/// Combining marks count against the byte budget too.
fn too_large(term: &Term<Listener>, range: &SelectionRange) -> bool {
    let rows = (range.end.line.0 as i64 - range.start.line.0 as i64 + 1).max(0) as usize;
    let columns = term.columns();
    let cells = if range.is_block || rows == 1 {
        rows.saturating_mul(range.end.column.0.saturating_sub(range.start.column.0) + 1)
    } else {
        rows.saturating_sub(2).saturating_mul(columns)
            + columns.saturating_sub(range.start.column.0)
            + range.end.column.0
            + 1
    };
    if cells > MAX_CELLS {
        return true;
    }
    let mut bytes = rows + 16;
    for line in range.start.line.0..=range.end.line.0 {
        let start = if range.is_block || line == range.start.line.0 { range.start.column.0 } else { 0 };
        let end = if range.is_block || line == range.end.line.0 { range.end.column.0 } else { columns - 1 };
        for column in start..=end {
            let cell = &term.grid()[Line(line)][Column(column)];
            bytes += cell.c.len_utf8() + cell.zerowidth().map_or(0, |marks| marks.len() * 4);
            if bytes > MAX_BYTES {
                return true;
            }
        }
    }
    false
}
