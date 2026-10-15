//! The interface, redrawn in full whenever something changed.
//!
//! Drawing a clickable thing also records its rectangle as a `Hit`. Input
//! handling looks up the hit under the pointer, so there is no widget tree.
//! Each element lives in one file with its drawing and its own input rules.

pub mod bar;
pub mod chrome;
pub mod diff;
pub mod field;
pub mod file;
pub mod pane;
pub mod picker;
pub mod preview;
pub mod sidebar;
pub mod theme;
pub mod tree;

use winit::window::ResizeDirection;

use crate::action::Action;
use crate::core::config::{self, Opt};
use crate::core::preview::Sizes;
use crate::core::{Session, SessionId, Workspace};
use crate::render::font::{Fonts, Metrics, UI};
use crate::render::{Canvas, Color, Icon, Rect};
use crate::term::{CellAt, Grid, Look, PALETTE};
use picker::Picker;

/// Sizes in logical pixels.
pub const SIDEBAR: f32 = 236.0;
/// Width of the strip the sidebar folds to: the mark of what a session
/// is doing, and as much room behind it as before it.
pub const RAIL: f32 = 44.0;
/// The least a pane keeps beside the column with the files: what each of
/// two has in the smallest window. Where less would be left, the column
/// gives way.
pub const PANE_MIN: f32 = 161.0;
pub const HEADER: f32 = 38.0;
pub const ROW: f32 = 28.0;
pub const PAD: f32 = 12.0;
/// Height of the bar along the bottom edge: that of a row, which is also
/// that of the field a project is searched in.
pub const BAR: f32 = ROW;
/// Where text begins in the sidebar, from its left edge, and how much room
/// is left at its right one.
pub const INSET: f32 = 14.0;
/// Size of interface text.
pub const TEXT_SIZE: f32 = 13.0;
/// The least of its header a pane keeps left of the icons at the top
/// right of the window: room for what closes it and for a little of its
/// title. A pane that would keep less has its header a row lower.
pub const HEAD_MIN: f32 = 80.0;
/// The scales of a window that are taken as they are named.
const MIN_SCALE: f32 = 0.25;
const MAX_SCALE: f32 = 16.0;
/// The most columns and rows a terminal is given. No screen shows as
/// many: a window that another program makes as large as the window
/// system allows would else have a grid of millions of cells, which the
/// emulator keeps twice and for every line that scrolled away.
pub const MAX_COLS: u16 = 2048;
pub const MAX_ROWS: u16 = 1024;

/// What a point of the window is, as recorded while drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    None,
    Resize(ResizeDirection),
    /// Empty header space: moves the window.
    Drag,
    /// The content of a pane.
    Pane(usize),
    /// An open text field or its list. A click here keeps it open.
    Field,
    /// Something to click.
    Do(Action),
}

pub type Hits = Vec<(Rect, Hit)>;

pub struct Ui {
    pub scale: f32,
    /// Window size in physical pixels.
    pub width: f32,
    pub height: f32,
    /// Neither maximized nor fullscreen: the window has edges to resize.
    pub floating: bool,
    /// The window fills the screen and can be given its own size back.
    pub maximized: bool,
    /// Whether to draw minimize, maximize and close.
    pub buttons: bool,
    /// Whether the window has the keyboard focus.
    pub focused: bool,
    /// Terminal font size in physical pixels and its cell.
    pub mono_px: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cell_base: f32,
    /// Interface font size in physical pixels and its metrics.
    pub text_px: f32,
    pub text: Metrics,
    /// The sizes a file of Markdown is set in where it is shown as it
    /// is meant.
    pub meant: Sizes,
    pub mouse: (f32, f32),
    pub hover: Hit,
    /// The session and the cell of its terminal under the pointer, while
    /// the key is held with which a click opens an address there.
    pub link: Option<(SessionId, CellAt)>,
    pub hits: Hits,
    /// The field and list for adding a project, while open.
    pub picker: Option<Picker>,
    /// The session being renamed and the text typed so far.
    pub renaming: Option<(SessionId, String)>,
    pub sidebar_scroll: f32,
    /// Height of the project list as last drawn.
    pub sidebar_height: f32,
    /// Bottom of the project list as last drawn: above the notice, and
    /// above the field of a list that is open.
    pub sidebar_bottom: f32,
    /// A problem the user should know about, shown in the sidebar.
    pub notice: Option<String>,
    /// The sidebar is folded to a strip, see `rail`.
    pub sidebar_folded: bool,
    /// A button went down while the folded sidebar lay over the panes,
    /// and is still down: it stays there until the button is let go.
    pub sidebar_held: bool,
    /// Whether the column with the files of a project is wanted. It is
    /// drawn where there is room for it, see `tree`.
    pub tree_open: bool,
    /// The configured agents, in order.
    pub agents: Vec<AgentTag>,
    /// Width of the column the marks stand in, so titles line up.
    pub mark_w: f32,
    /// Which of its `icon::STEPS` steps the mark of a session at work is
    /// at. The app moves it on while one works.
    pub step: u8,
    pub palette: [Color; PALETTE],
}

/// What the interface needs to know of an agent.
pub struct AgentTag {
    pub name: String,
    pub mark: String,
    /// Drawn in place of the mark.
    pub icon: Option<Icon>,
    /// Only installed agents are offered for new sessions.
    pub installed: bool,
    /// What can be set before a session of it starts.
    pub options: Vec<Opt>,
}

pub struct PaneLayout {
    pub area: Rect,
    /// The row with the title and with what closes the pane. At the top,
    /// or a row lower where the icons at the top right of the window
    /// would leave too little of it.
    pub header: Rect,
    /// Top left of the terminal grid.
    pub origin: (f32, f32),
    pub cols: u16,
    pub rows: u16,
}

impl Ui {
    pub fn new(agents: Vec<AgentTag>) -> Self {
        Self {
            scale: 1.0,
            width: 0.0,
            height: 0.0,
            floating: true,
            maximized: false,
            buttons: true,
            focused: true,
            mono_px: 0.0,
            cell_w: 1.0,
            cell_h: 1.0,
            cell_base: 0.0,
            text_px: 0.0,
            text: Metrics::default(),
            meant: Sizes::default(),
            mouse: (-1.0, -1.0),
            hover: Hit::None,
            link: None,
            hits: Vec::new(),
            picker: None,
            renaming: None,
            sidebar_scroll: 0.0,
            sidebar_height: 0.0,
            sidebar_bottom: 0.0,
            notice: None,
            sidebar_folded: false,
            sidebar_held: false,
            tree_open: false,
            agents,
            mark_w: 0.0,
            step: 0,
            palette: theme::palette(),
        }
    }

    /// Logical to physical pixels, snapped to the pixel grid.
    /// The scale of the window as the app takes it. The window system
    /// names it, from a setting of the desktop or a variable, and may
    /// name any number: sizes are reckoned from it everywhere, and
    /// outside these bounds nothing could be read.
    pub fn scale_of(factor: f64) -> f32 {
        if factor.is_finite() { factor.clamp(MIN_SCALE as f64, MAX_SCALE as f64) as f32 } else { 1.0 }
    }

    pub fn px(&self, logical: f32) -> f32 {
        (logical * self.scale).round()
    }

    /// Width of a hairline.
    pub fn line(&self) -> f32 {
        self.scale.max(1.0).round()
    }

    /// Recomputes font metrics after a change of scale or font size.
    pub fn measure(&mut self, fonts: &mut Fonts, font_size: f32) {
        self.mono_px = (font_size * self.scale).round();
        let m = fonts.metrics(0, self.mono_px);
        self.cell_w = m.advance.round().max(1.0);
        self.cell_h = (m.ascent + m.descent + m.leading).ceil().max(1.0);
        self.cell_base = ((self.cell_h - m.ascent - m.descent) / 2.0 + m.ascent).round();
        self.text_px = (TEXT_SIZE * self.scale).round();
        self.text = fonts.metrics(UI, self.text_px);
        self.meant = preview::sizes(fonts, self.mono_px, (self.cell_w, self.cell_h, self.cell_base));
        // Marks are set in the terminal font, two cells wide.
        let marked = self.agents.iter().any(|agent| !agent.mark.is_empty());
        let cell = fonts.metrics(0, self.text_px).advance;
        self.mark_w = if marked { (2.0 * cell).ceil() + self.px(chrome::MARK_GAP) } else { 0.0 };
        if self.agents.iter().any(|agent| agent.icon.is_some()) {
            self.mark_w = self.mark_w.max(self.px(chrome::ICON) + self.px(chrome::MARK_GAP));
        }
    }

    pub fn look(&self) -> Look<'_> {
        Look {
            px: self.mono_px,
            cell_w: self.cell_w,
            cell_h: self.cell_h,
            base: self.cell_base,
            line: self.line(),
            palette: &self.palette,
            selection: theme::SELECTION,
            cursor: theme::TEXT,
            idle_cursor: theme::DIM,
        }
    }

    /// The icon and the mark of an agent.
    pub fn mark(&self, agent: &str) -> (Option<Icon>, &str) {
        self.agents.iter().find(|tag| tag.name == agent).map_or((None, ""), |tag| (tag.icon, &tag.mark))
    }

    /// What was set for a session before it started, in a few words.
    pub fn set(&self, session: &Session) -> String {
        let agent = self.agents.iter().find(|tag| tag.name == session.agent);
        agent.map_or_else(String::new, |tag| config::summary(&tag.options, &session.chosen))
    }

    /// Whether a text field has the keyboard.
    pub fn editing(&self) -> bool {
        self.picker.is_some() || self.renaming.is_some()
    }

    /// Whether the pointer is over the element that stands for an action.
    pub fn hot(&self, action: Action) -> bool {
        self.hover == Hit::Do(action)
    }

    /// The color of an element that stands for an action: that of text
    /// while the pointer is over it, else `idle`.
    pub fn lit(&self, action: Action, idle: Color) -> Color {
        if self.hot(action) { theme::TEXT } else { idle }
    }

    /// Baseline that centers interface text in a row.
    pub fn baseline(&self, row: Rect) -> f32 {
        (row.y + (row.h - self.text.ascent - self.text.descent) / 2.0 + self.text.ascent).round()
    }

    pub fn grid(&self, pane: &PaneLayout) -> Grid {
        Grid { cols: pane.cols, rows: pane.rows, cell_w: self.cell_w as u16, cell_h: self.cell_h as u16 }
    }

    /// The bar along the bottom edge, as wide as the window.
    pub fn bar(&self) -> Rect {
        let height = self.px(BAR).min(self.height);
        Rect::new(0.0, self.height - height, self.width, height)
    }

    /// Whether the sidebar is drawn as a strip: it is folded, and nothing
    /// is in it that needs its width. A list, the field that renames a
    /// session and a notice are drawn in the whole sidebar, which lies
    /// over the panes for as long as they are there: the panes keep their
    /// place, and no terminal is given another size for a list.
    pub fn rail(&self) -> bool {
        let wide = self.picker.is_some() || self.renaming.is_some() || self.notice.is_some();
        self.sidebar_folded && !wide && !self.sidebar_held
    }

    /// The sidebar as it is drawn: whole, or the strip it is folded to.
    pub fn sidebar(&self) -> Rect {
        Rect::new(0.0, 0.0, self.px(if self.rail() { RAIL } else { SIDEBAR }), self.bar().y)
    }

    /// The width the sidebar takes off the panes. Not that of a sidebar
    /// which lies over them for a while, see `rail`.
    pub fn left(&self) -> f32 {
        self.px(if self.sidebar_folded { RAIL } else { SIDEBAR })
    }

    /// Whether there is room for the column with the files beside `count`
    /// panes: each of them keeps `PANE_MIN`.
    pub fn tree_fits(&self, count: usize) -> bool {
        let room = self.width - (self.left() + 1.0) - (self.px(SIDEBAR) + 1.0);
        // Every pane but the last gives a pixel to the line behind it.
        let line = (count > 1) as u8 as f32;
        (room / count.max(1) as f32).floor() - line >= self.px(PANE_MIN)
    }

    /// The column with the files at the right of `count` panes, if it is
    /// wanted and there is room for it.
    pub fn tree(&self, count: usize) -> Option<Rect> {
        let width = self.px(SIDEBAR);
        (self.tree_open && self.tree_fits(count))
            .then(|| Rect::new(self.width - width, 0.0, width, self.bar().y))
    }

    /// Where one of the icons at the top right stands, counted from the
    /// buttons of the window: the same place whatever is shown, see
    /// `chrome::controls`.
    pub fn control(&self, from_right: usize) -> Rect {
        let width = self.px(chrome::CONTROL);
        let x = self.width - chrome::buttons_width(self) - (from_right + 1) as f32 * width;
        Rect::new(x, 0.0, width, self.px(HEADER))
    }

    /// Where those icons begin. The header of a pane ends there.
    pub fn controls_left(&self) -> f32 {
        self.control(chrome::CONTROLS - 1).x
    }

    /// Splits the space between the sidebar and the column with the
    /// files evenly between the panes.
    pub fn panes(&self, count: usize) -> Vec<PaneLayout> {
        (0..count).map(|index| self.pane(index, count)).collect()
    }

    /// The place of one of `count` panes.
    pub fn pane(&self, index: usize, count: usize) -> PaneLayout {
        let left = self.left() + 1.0;
        let right = self.tree(count).map_or(self.width, |tree| tree.x - 1.0);
        let width = ((right - left) / count as f32).floor();
        let (header, pad) = (self.px(HEADER), self.px(PAD));
        let x = left + index as f32 * width;
        // The last pane takes the pixel that is left over, so that what
        // stands at the right of its header stands at the edge.
        let w = if index + 1 < count { width - 1.0 } else { right - x };
        let area = Rect::new(x, 0.0, w, self.bar().y);
        // Under the icons at the top right: with too little of the
        // header left of them, the pane has its title and what closes
        // it in a row of its own below, as the column with the files
        // has the name of its project.
        let from = self.controls_left();
        let low = area.right() > from && from - area.x < self.px(HEAD_MIN).min(area.w);
        let header = if low {
            Rect::new(area.x, header, area.w, self.px(ROW))
        } else {
            Rect::new(area.x, 0.0, area.w, header)
        };
        let top = header.bottom();
        PaneLayout {
            area,
            header,
            origin: (area.x + pad, top),
            cols: (((area.w - 2.0 * pad) / self.cell_w).floor().max(2.0) as u16).min(MAX_COLS),
            rows: (((area.h - top - pad) / self.cell_h).floor().max(1.0) as u16).min(MAX_ROWS),
        }
    }

    /// Where the header of a pane ends: where the icons at the top
    /// right of the window begin, if it stands at the top and reaches
    /// that far.
    pub fn head_right(&self, pane: &PaneLayout) -> f32 {
        if pane.header.y > 0.0 {
            return pane.header.right();
        }
        pane.header.right().min(self.controls_left()).max(pane.header.x)
    }

    /// What lies at a point. Later hits lie on top of earlier ones.
    pub fn hit(&self, x: f32, y: f32) -> Hit {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map_or(Hit::None, |(_, hit)| *hit)
    }

    /// The cell under a point in a pane, clamped to the grid.
    pub fn cell_at(&self, pane: &PaneLayout, x: f32, y: f32) -> CellAt {
        let fx = ((x - pane.origin.0) / self.cell_w).max(0.0);
        let col = (fx as usize).min(pane.cols as usize - 1);
        let row = (((y - pane.origin.1) / self.cell_h).max(0.0) as usize).min(pane.rows as usize - 1);
        CellAt { col, row, right: fx.fract() > 0.5 }
    }
}

/// Draws the whole window and records the hits.
pub fn draw(cv: &mut Canvas, ui: &mut Ui, ws: &Workspace) {
    let mut hits = std::mem::take(&mut ui.hits);
    hits.clear();

    let panes = ui.panes(ws.panes().len());
    let column = ui.tree(panes.len());
    for (i, layout) in panes.iter().enumerate() {
        pane::draw(cv, ui, ws, i, layout, i + 1 == panes.len(), &mut hits);
    }
    if let Some(column) = column {
        tree::draw(cv, ui, ws, column, &mut hits);
    }
    // After the panes: folded, it lies over them while a list is open.
    sidebar::draw(cv, ui, ws, &mut hits);
    // Over whichever header is at the top right.
    chrome::controls(cv, ui, ws, &mut hits);
    bar::draw(cv, ui, ws, &mut hits);
    chrome::edges(cv, ui, &mut hits);
    ui.hits = hits;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(width: f32, height: f32) -> Ui {
        let mut ui = Ui::new(Vec::new());
        (ui.width, ui.height, ui.cell_w, ui.cell_h) = (width, height, 8.0, 17.0);
        ui
    }

    #[test]
    fn a_grid_has_a_bound_however_large_the_window() {
        // The smallest cell there is, in windows up to what no window
        // system gives.
        for side in [760.0, 32_000.0, 65_535.0, 1e9, f32::MAX, f32::INFINITY] {
            let mut ui = sized(side, side);
            (ui.cell_w, ui.cell_h) = (1.0, 1.0);
            for pane in ui.panes(1).into_iter().chain(ui.panes(3)) {
                assert!((2..=MAX_COLS).contains(&pane.cols), "{} columns at {side}", pane.cols);
                assert!((1..=MAX_ROWS).contains(&pane.rows), "{} rows at {side}", pane.rows);
            }
        }
        // A window as screens are keeps the grid that fits it.
        let pane = sized(3840.0, 2160.0).pane(0, 1);
        assert!(pane.cols > 400 && pane.cols < MAX_COLS && pane.rows > 100 && pane.rows < MAX_ROWS);
    }

    #[test]
    fn the_scale_of_a_window_is_taken_within_bounds() {
        for (named, taken) in [(1.0, 1.0), (1.25, 1.25), (2.0, 2.0), (0.01, MIN_SCALE), (4096.0, MAX_SCALE)] {
            assert_eq!(Ui::scale_of(named), taken);
        }
        assert_eq!(Ui::scale_of(f64::MAX), MAX_SCALE);
        assert_eq!(Ui::scale_of(-3.0), MIN_SCALE);
        for none in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(Ui::scale_of(none), 1.0);
        }
    }

    #[test]
    fn panes_share_the_width_and_never_get_an_empty_grid() {
        let ui = sized(1200.0, 760.0);
        let one = ui.panes(1);
        assert_eq!((one[0].area.x, one[0].area.right()), (237.0, 1200.0));
        assert_eq!((one[0].cols, one[0].rows), (117, 40));
        // Sidebar and panes end where the bar along the bottom begins.
        assert_eq!((ui.bar().y, ui.bar().w), (760.0 - ui.px(BAR), 1200.0));
        assert_eq!((one[0].area.bottom(), ui.sidebar().bottom()), (ui.bar().y, ui.bar().y));

        let two = ui.panes(2);
        assert_eq!(two[0].area.right() + 1.0, two[1].area.x, "one pixel for the divider");
        assert!(two[1].area.right() <= 1200.0);

        let tiny = sized(240.0, 30.0).panes(2);
        assert!(tiny.iter().all(|p| p.cols >= 2 && p.rows >= 1));

        // Of an odd width the last pane takes the pixel that is left over.
        let odd = sized(1201.0, 760.0).panes(2);
        assert_eq!((odd[0].area.w, odd[1].area.w, odd[1].area.right()), (481.0, 482.0, 1201.0));
    }

    #[test]
    fn the_column_with_the_files_takes_its_width_off_the_panes_and_gives_way() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = sized(1200.0 * scale, 760.0 * scale);
            ui.scale = scale;
            assert!(ui.tree(1).is_none(), "it is not wanted");
            let before = ui.control(0);
            ui.tree_open = true;
            let column = ui.tree(2).unwrap();
            assert_eq!((column.right(), column.w, column.bottom()), (ui.width, ui.px(SIDEBAR), ui.bar().y));
            let two = ui.panes(2);
            assert_eq!(two[1].area.right() + 1.0, column.x, "one pixel for the divider");
            assert!(two.iter().all(|pane| pane.area.w >= ui.px(PANE_MIN)));
            // Its icon stands where it stood while the column was away.
            assert_eq!(ui.control(0), before);
            assert!(before.right() <= ui.width - 3.0 * ui.px(44.0) + 0.5);

            // Where a pane would keep too little, the column is not
            // drawn, and comes back with the width. At the least width
            // it is drawn at, every pane has what it is to keep.
            let least = 2.0 * (ui.px(SIDEBAR) + 1.0) + 2.0 * (ui.px(PANE_MIN) + 1.0);
            ui.width = least - 1.0;
            assert!(ui.tree(2).is_none() && ui.tree_open && ui.tree(1).is_some());
            assert_eq!(ui.panes(2)[1].area.right(), ui.width);
            ui.width = least;
            assert!(ui.tree(2).is_some());
            assert!(ui.panes(2).iter().all(|pane| pane.area.w >= ui.px(PANE_MIN)));
            ui.width = 2.0 * (ui.px(SIDEBAR) + 1.0) + ui.px(PANE_MIN);
            assert!(ui.tree(1).is_some() && ui.panes(1)[0].area.w == ui.px(PANE_MIN));
            ui.width -= 1.0;
            assert!(ui.tree(1).is_none());
        }
        // In the smallest window it never is, with one pane or two.
        let mut small = sized(560.0, 320.0);
        small.tree_open = true;
        assert!(small.tree(1).is_none() && small.tree(2).is_none());
        // Without buttons of the window the icon stands at the edge.
        small.buttons = false;
        assert_eq!(small.control(0).right(), 560.0);
    }

    #[test]
    fn a_folded_sidebar_is_a_strip_and_what_needs_its_width_lies_over_the_panes() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = sized(1200.0 * scale, 760.0 * scale);
            ui.scale = scale;
            let button = ui.control(chrome::CONTROLS - 1);
            assert!(!ui.rail());
            assert_eq!((ui.sidebar().w, ui.panes(1)[0].area.x), (ui.px(SIDEBAR), ui.px(SIDEBAR) + 1.0));
            ui.sidebar_folded = true;
            assert!(ui.rail());
            assert_eq!((ui.sidebar().w, ui.left()), (ui.px(RAIL), ui.px(RAIL)));
            let one = ui.panes(1)[0].area;
            assert_eq!((one.x, one.right()), (ui.px(RAIL) + 1.0, ui.width));
            let two = ui.panes(2);
            assert_eq!((two[0].area.x, two[1].area.right()), (one.x, ui.width));
            // What folds it stands where it stood.
            assert_eq!(ui.control(chrome::CONTROLS - 1), button);
            // A notice, the field that renames and a list are drawn in
            // the whole sidebar, and the panes stay where they are.
            ui.notice = Some("something".into());
            assert!(!ui.rail());
            assert_eq!((ui.sidebar().w, ui.left(), ui.panes(1)[0].area), (ui.px(SIDEBAR), ui.px(RAIL), one));
            ui.notice = None;
            ui.renaming = Some((SessionId(1), String::new()));
            assert_eq!((ui.sidebar().w, ui.panes(1)[0].area), (ui.px(SIDEBAR), one));
            ui.renaming = None;
            ui.picker = Some(Picker::default());
            assert_eq!((ui.sidebar().w, ui.panes(1)[0].area), (ui.px(SIDEBAR), one));
            ui.picker = None;
            assert!(ui.rail());
            // A click that closes what lay over the panes finds the
            // sidebar where it was until the button is let go.
            ui.sidebar_held = true;
            assert_eq!((ui.rail(), ui.sidebar().w, ui.panes(1)[0].area), (false, ui.px(SIDEBAR), one));
            ui.sidebar_held = false;
            // The column with the files has room in a narrower window
            // than beside the whole sidebar.
            ui.tree_open = true;
            ui.width = (ui.px(RAIL) + 1.0) + (ui.px(SIDEBAR) + 1.0) + ui.px(PANE_MIN);
            assert!(ui.tree(1).is_some() && ui.panes(1)[0].area.w == ui.px(PANE_MIN));
            ui.sidebar_folded = false;
            assert!(ui.tree(1).is_none());
            ui.sidebar_folded = true;
            ui.width -= 1.0;
            assert!(ui.tree(1).is_none());
        }
    }

    #[test]
    fn the_icons_at_the_top_right_keep_their_places_whatever_is_shown() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = sized(1200.0 * scale, 760.0 * scale);
            ui.scale = scale;
            let places = |ui: &Ui| (0..chrome::CONTROLS).map(|n| ui.control(n)).collect::<Vec<_>>();
            let before = places(&ui);
            // Side by side, up to the buttons of the window.
            assert_eq!(before[0].right(), ui.width - chrome::buttons_width(&ui));
            assert!(before.windows(2).all(|pair| pair[1].right() == pair[0].x));
            assert_eq!(ui.controls_left(), before[chrome::CONTROLS - 1].x);
            (ui.tree_open, ui.sidebar_folded) = (true, true);
            assert_eq!(places(&ui), before);
            // With the buttons of the window they are as wide as the
            // column with the files: no drawing of them stands across
            // the line before it, and the header of the pane beside it
            // loses nothing to them.
            let column = ui.tree(1).unwrap();
            let first = before[chrome::CONTROLS - 1];
            let drawing = first.x + ((first.w - ui.px(chrome::ICON)) / 2.0).round();
            assert!(drawing >= column.x, "{drawing} before {}", column.x);
            assert!(ui.controls_left() >= column.x - 2.0);
            // Without buttons of the window they stand at its edge.
            ui.buttons = false;
            assert_eq!(ui.control(0).right(), ui.width);
        }
    }

    #[test]
    fn every_pane_keeps_room_for_what_closes_it_beside_the_icons_at_the_top_right() {
        let button = 28.0;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for (buttons, folded, open) in [
                (true, false, false),
                (true, true, false),
                (true, false, true),
                (true, true, true),
                (false, false, false),
            ] {
                for logical in (560..=2400).step_by(7) {
                    let mut ui = sized((logical as f32 * scale).round(), 400.0 * scale);
                    (ui.scale, ui.buttons, ui.sidebar_folded, ui.tree_open) = (scale, buttons, folded, open);
                    for count in [1, 2] {
                        for pane in ui.panes(count) {
                            let right = ui.head_right(&pane);
                            let said =
                                format!("{logical} at {scale}, {count} panes, {buttons} {folded} {open}");
                            assert!(right - ui.px(button) >= pane.area.x, "no room to close it: {said}");
                            // And nothing of its header lies under them.
                            let under = pane.header.y == 0.0 && right > ui.controls_left();
                            assert!(!under && right <= pane.area.right(), "{said}");
                            // A header a row lower takes that row off
                            // the rows of the pane, and nothing else.
                            assert_eq!(pane.origin.1, pane.header.bottom(), "{said}");
                        }
                    }
                }
            }
        }
        // In the window of 1200 a header stands at the top. In a
        // narrower one the pane at the right has it a row lower, down
        // to the smallest, where the one at the left still keeps its
        // own beside the icons.
        let ui = sized(1200.0, 760.0);
        assert!(ui.panes(2).iter().all(|pane| pane.header.y == 0.0));
        let narrow = sized(700.0, 400.0);
        let two = narrow.panes(2);
        assert_eq!((two[0].header.y, two[1].header.y), (0.0, narrow.px(HEADER)));
        assert_eq!((two[1].header.h, two[1].header.w), (narrow.px(ROW), two[1].area.w));
        let small = sized(560.0, 320.0);
        let two = small.panes(2);
        assert_eq!((two[0].header.y, two[1].header.y), (0.0, small.px(HEADER)));
        assert_eq!(small.panes(1)[0].header.y, 0.0);
    }

    #[test]
    fn cells_are_clamped_to_the_grid_and_the_last_hit_wins() {
        let mut ui = sized(1200.0, 760.0);
        let pane = &ui.panes(1)[0];
        let at = ui.cell_at(pane, pane.origin.0 + 8.0 * 3.0 + 6.0, pane.origin.1 + 17.0 * 2.0);
        assert!((at.col, at.row, at.right) == (3, 2, true));
        let outside = ui.cell_at(pane, -50.0, 99_999.0);
        assert!((outside.col, outside.row) == (0, pane.rows as usize - 1));

        ui.hits = vec![
            (Rect::new(0.0, 0.0, 100.0, 100.0), Hit::Drag),
            (Rect::new(10.0, 10.0, 10.0, 10.0), Hit::Pane(0)),
        ];
        assert_eq!(ui.hit(15.0, 15.0), Hit::Pane(0));
        assert_eq!(ui.hit(50.0, 50.0), Hit::Drag);
        assert_eq!(ui.hit(500.0, 50.0), Hit::None);
    }
}
