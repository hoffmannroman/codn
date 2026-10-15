//! Adding a project: a text field with a list above it. The list filters
//! the known repositories, or browses folders once the text is a path.
//!
//! The same field and list start a session in a project: with one of the
//! agents, after setting its options if wanted, or by continuing one of
//! the conversations the agents have on disk.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use winit::keyboard::{Key, NamedKey};

use super::sidebar::field_row;
use super::{HEADER, Hit, Hits, INSET, ROW, Ui, chrome, field, theme};
use crate::action::Action;
use crate::core::ProjectId;
use crate::core::config::{self, Chosen, Opt};
use crate::core::discover::{self, Pick, Repo, Row};
use crate::core::history::{self, Conversation};
use crate::render::font::UI;
use crate::render::{Canvas, Icon, Rect};

/// The conversations listed below the agents before a row stands for the
/// rest: a project has dozens, and the one that is wanted is nearly always
/// among the last.
const RECENT: usize = 5;

#[derive(Default)]
pub struct Picker {
    pub text: String,
    /// The last Enter had nothing to choose.
    pub invalid: bool,
    pub rows: Vec<Row>,
    /// The highlighted row.
    pub selected: usize,
    /// The first row shown.
    pub scroll: usize,
    /// Set when the list starts a session, not adds a project.
    pub launch: Option<Launch>,
    /// Directory rows returned by the worker for this exact query.
    pub browsed: Option<(String, Vec<Row>)>,
    /// All directory workers are occupied; another edit retries the read.
    pub browse_busy: bool,
    /// Counts the lists that were built, and which of them the last
    /// frame drew: a click names a row by its place in the list it saw.
    pub made: u32,
    pub drawn: std::cell::Cell<u32>,
}

/// What a session in a project can be started with or continued from.
pub struct Launch {
    pub project: ProjectId,
    /// The installed agents, the one last used in the project first.
    pub starters: Vec<Starter>,
    /// The conversations on disk, newest first. Nothing while they are
    /// being looked for.
    pub found: Option<Vec<Conversation>>,
    /// The agent whose options are shown in place of the list.
    pub setting: Option<usize>,
    /// Every conversation is listed, not the most recent ones alone.
    pub all: bool,
}

/// An agent to start a session with, and what is set for it.
pub struct Starter {
    pub agent: String,
    pub options: Vec<Opt>,
    pub chosen: Chosen,
}

impl Launch {
    fn rows(&self, text: &str) -> Vec<Row> {
        if let Some(starter) = self.setting.and_then(|at| self.starters.get(at)) {
            let back = Row { pick: Pick::Back, label: starter.agent.clone(), detail: String::new() };
            let options = starter.options.iter().enumerate().map(|(at, option)| Row {
                pick: Pick::Option(at),
                label: option.name.clone(),
                detail: option.choices[option.chosen(&starter.chosen)].name.clone(),
            });
            let start = Row {
                pick: Pick::Start(self.setting.unwrap_or(0)),
                label: "Start".into(),
                detail: "Enter".into(),
            };
            return std::iter::once(back).chain(options).chain([start]).collect();
        }
        let query = text.to_lowercase();
        let fits = |starter: &&Starter| {
            let name = starter.agent.to_lowercase();
            query.split_whitespace().all(|word| name.contains(word))
        };
        let starters =
            self.starters.iter().enumerate().filter(|(_, starter)| fits(starter)).map(|(at, s)| Row {
                pick: Pick::Start(at),
                label: s.agent.clone(),
                detail: config::summary(&s.options, &s.chosen),
            });
        let found = self.found.as_deref().unwrap_or_default();
        let now = SystemTime::now();
        let conversation = |i: usize| Row {
            pick: Pick::Resume(i),
            label: found[i].title.clone(),
            detail: history::age(found[i].changed, now),
        };
        let matching = history::matching(found, text);
        // A search goes through all of them. One conversation more is
        // shown as itself: a row that stands for it would take its place.
        let searched = query.split_whitespace().next().is_some();
        let whole = self.all || searched || matching.len() <= RECENT + 1;
        let shown = if whole { matching.len() } else { RECENT };
        let more = (shown < matching.len()).then(|| Row {
            pick: Pick::More,
            label: format!("Show {} more", matching.len() - shown),
            detail: String::new(),
        });
        starters.chain(matching.into_iter().take(shown).map(conversation)).chain(more).collect()
    }

    /// The agent a row stands for, whose mark it starts with.
    fn agent(&self, pick: &Pick) -> Option<&str> {
        match pick {
            Pick::Start(at) if self.setting.is_none() => self.starters.get(*at).map(|s| s.agent.as_str()),
            Pick::Resume(at) => self.found.as_ref()?.get(*at).map(|c| c.agent.as_str()),
            _ => None,
        }
    }
}

/// What a key or click on the picker led to.
pub enum Outcome {
    Stay,
    Close,
    Add(PathBuf),
    /// Continue a conversation in a session of this project.
    Resume(ProjectId, Conversation),
    /// Start a session of this project with an agent and what was set.
    Start(ProjectId, String, Chosen),
}

/// The repositories known and the test for "is a project already".
pub struct Known<'a> {
    pub repos: &'a [Repo],
    pub added: &'a dyn Fn(&Path) -> bool,
}

impl Picker {
    /// Whether the list shows repositories, with the row that leads to
    /// browsing folders fixed below them.
    fn browsable(&self) -> bool {
        self.launch.is_none() && !discover::is_path(&self.text)
    }

    /// The path whose folders the list shows, while it does that: the text
    /// of a list that adds a project, once it reads as a path.
    pub fn browsing(&self) -> Option<&str> {
        (self.launch.is_none() && discover::is_path(&self.text)).then_some(self.text.as_str())
    }

    /// The row a click on row `row` of the frame means, if the list is
    /// still the one that frame drew. An answer of a worker may have
    /// built it anew since, and the same place then holds another
    /// folder or conversation: such a click chooses nothing, and the
    /// next frame shows what there is to click.
    pub fn clicked(&self, row: usize) -> Option<usize> {
        (self.drawn.get() == self.made).then_some(row)
    }

    /// Builds the list for the current text again.
    pub fn refresh(&mut self, known: &Known) {
        self.made = self.made.wrapping_add(1);
        self.rows = match &self.launch {
            Some(launch) => launch.rows(&self.text),
            None if discover::is_path(&self.text) => self
                .browsed
                .as_ref()
                .filter(|(text, _)| text == &self.text)
                .map(|(_, rows)| {
                    rows.iter()
                        .filter(|row| !matches!(&row.pick, Pick::Add(path) if (known.added)(path)))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            None => discover::rows(known.repos, &self.text, known.added),
        };
        self.selected = 0;
        self.scroll = 0;
        self.invalid = false;
    }

    /// Accepts only a directory reply for the field that is still open.
    pub fn browsed(&mut self, text: String, rows: Vec<Row>, known: &Known) {
        if self.browsing() == Some(text.as_str()) {
            self.browsed = Some((text, rows));
            self.rescanned(known);
        }
    }

    /// Takes in a new list of repositories without moving the highlight
    /// away from what the user is on.
    pub fn rescanned(&mut self, known: &Known) {
        // By what the row does, not by how it reads: two repositories can
        // have the same name, and Enter would add the other one.
        let on = self.rows.get(self.selected).map(|row| row.pick.clone());
        let (selected, below) = (self.selected, self.selected.saturating_sub(self.scroll));
        self.refresh(known);
        let found = on.and_then(|pick| self.rows.iter().position(|row| row.pick == pick));
        self.selected = found.unwrap_or(selected).min(self.rows.len().saturating_sub(1));
        // As far below the top of the list as before, so it stays in view.
        self.scroll = self.selected.saturating_sub(below);
    }

    pub fn set_text(&mut self, text: String, known: &Known) {
        self.text = text;
        self.refresh(known);
    }

    pub fn type_text(&mut self, text: &str, known: &Known) {
        // The options are chosen, not searched.
        if self.setting().is_some() {
            return;
        }
        field::insert(&mut self.text, text);
        self.refresh(known);
    }

    /// What a click on row `k` does, as its hit names it.
    pub fn action(&self, k: usize) -> Action {
        match self.rows.get(k).map(|row| &row.pick) {
            Some(Pick::More) => Action::More(k),
            _ => Action::Pick(k),
        }
    }

    /// Chooses a row: adds the project or goes into the folder.
    pub fn pick(&mut self, row: usize, known: &Known) -> Outcome {
        match self.rows.get(row).map(|r| &r.pick) {
            Some(Pick::Add(path)) => Outcome::Add(path.clone()),
            Some(Pick::Open(text)) => {
                self.set_text(text.clone(), known);
                Outcome::Stay
            }
            Some(Pick::Resume(at)) => {
                let found = self.launch.as_ref().and_then(|l| Some((l.project, l.found.as_ref()?.get(*at)?)));
                found.map_or(Outcome::Stay, |(project, c)| Outcome::Resume(project, c.clone()))
            }
            Some(Pick::Start(at)) => {
                let starter = self.launch.as_ref().and_then(|l| Some((l.project, l.starters.get(*at)?)));
                starter.map_or(Outcome::Stay, |(project, s)| {
                    Outcome::Start(project, s.agent.clone(), s.chosen.clone())
                })
            }
            Some(Pick::Option(_)) => {
                self.choose(row, 1);
                Outcome::Stay
            }
            Some(Pick::Back) => {
                self.set(None, known);
                Outcome::Stay
            }
            Some(Pick::More) => {
                if let Some(launch) = &mut self.launch {
                    launch.all = true;
                }
                // The highlight goes to the first of those held back, where
                // the list goes on. The list grows up from the field, so
                // that row stands higher than the row for the rest did,
                // unless the list already filled its room.
                let scroll = self.scroll;
                self.refresh(known);
                (self.selected, self.scroll) = (row, scroll);
                Outcome::Stay
            }
            None => {
                self.invalid = true;
                Outcome::Stay
            }
        }
    }

    /// The agent whose options are shown, as its place among the starters.
    fn setting(&self) -> Option<usize> {
        self.launch.as_ref().and_then(|launch| launch.setting)
    }

    /// Shows the options of the agent a row stands for, if it has any.
    pub fn options(&mut self, row: usize, known: &Known) {
        let Some(Pick::Start(at)) = self.rows.get(row).map(|r| &r.pick) else { return };
        let has = |launch: &Launch| launch.starters.get(*at).is_some_and(|s| !s.options.is_empty());
        if self.setting().is_none() && self.launch.as_ref().is_some_and(has) {
            self.set(Some(*at), known);
        }
    }

    /// Goes to the options of an agent, or back to the list.
    fn set(&mut self, setting: Option<usize>, known: &Known) {
        let Some(launch) = &mut self.launch else { return };
        let from = launch.setting;
        launch.setting = setting;
        self.text.clear();
        self.refresh(known);
        // On the first option, and back on the agent that was left.
        let back = self.rows.iter().position(|row| matches!(row.pick, Pick::Start(at) if Some(at) == from));
        self.selected = if setting.is_some() { 1 } else { back.unwrap_or(0) };
    }

    /// Moves the option in a row on by `step` choices, around the ends.
    pub fn choose(&mut self, row: usize, step: isize) {
        let Some(Pick::Option(at)) = self.rows.get(row).map(|r| &r.pick) else { return };
        let Some(launch) = &mut self.launch else { return };
        let Some(starter) = launch.setting.and_then(|agent| launch.starters.get_mut(agent)) else { return };
        let Some(option) = starter.options.get(*at) else { return };
        let count = option.choices.len() as isize;
        let next = (option.chosen(&starter.chosen) as isize + step).rem_euclid(count) as usize;
        starter.chosen.insert(option.name.clone(), option.choices[next].name.clone());
        self.rows[row].detail = option.choices[next].name.clone();
        self.selected = row;
    }

    /// Moves the highlight and keeps it in view.
    fn select(&mut self, step: isize, capacity: usize) {
        let last = self.rows.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(step).min(last);
        self.scroll = self.scroll.clamp((self.selected + 1).saturating_sub(capacity), self.selected);
    }

    /// Scrolls the list by `lines`, positive is up.
    pub fn wheel(&mut self, lines: i32, capacity: usize) {
        let max = self.rows.len().saturating_sub(capacity);
        self.scroll = self.scroll.saturating_add_signed(-lines as isize).min(max);
    }

    /// A key press while the options of an agent are shown.
    fn option_key(&mut self, agent: usize, key: &Key, capacity: usize, known: &Known) -> Outcome {
        match key {
            Key::Named(NamedKey::Escape) => self.set(None, known),
            Key::Named(NamedKey::ArrowDown) => self.select(1, capacity),
            Key::Named(NamedKey::ArrowUp) => self.select(-1, capacity),
            Key::Named(NamedKey::ArrowRight) => self.choose(self.selected, 1),
            Key::Named(NamedKey::ArrowLeft) => self.choose(self.selected, -1),
            // Enter starts from every row but the way back.
            Key::Named(NamedKey::Enter) => {
                let back = matches!(self.rows.get(self.selected).map(|r| &r.pick), Some(Pick::Back));
                let start =
                    self.rows.iter().position(|row| matches!(row.pick, Pick::Start(at) if at == agent));
                return self.pick(if back { self.selected } else { start.unwrap_or(self.selected) }, known);
            }
            _ => {}
        }
        Outcome::Stay
    }

    /// A key press while the field is open. Pasting is the caller's job.
    pub fn key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        ctrl: bool,
        capacity: usize,
        known: &Known,
    ) -> Outcome {
        if let Some(agent) = self.setting() {
            return self.option_key(agent, key, capacity, known);
        }
        match key {
            Key::Named(NamedKey::Escape) => return Outcome::Close,
            Key::Named(NamedKey::ArrowRight) if self.launch.is_some() => self.options(self.selected, known),
            Key::Named(NamedKey::ArrowDown) => self.select(1, capacity),
            Key::Named(NamedKey::ArrowUp) => self.select(-1, capacity),
            Key::Named(NamedKey::PageDown) => self.select(capacity as isize, capacity),
            Key::Named(NamedKey::PageUp) => self.select(-(capacity as isize), capacity),
            Key::Named(NamedKey::Enter) => return self.pick(self.selected, known),
            // Tab goes into the folder that is being typed. It starts and
            // adds nothing, and with Control it is not meant for the list.
            Key::Named(NamedKey::Tab) => {
                if !ctrl && matches!(self.rows.get(self.selected).map(|row| &row.pick), Some(Pick::Open(_))) {
                    return self.pick(self.selected, known);
                }
            }
            Key::Named(NamedKey::Backspace) if ctrl => {
                // Delete back to the previous path separator.
                let end = self.text.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
                self.text.truncate(end);
                self.refresh(known);
            }
            Key::Named(NamedKey::Backspace) => {
                self.text.pop();
                self.refresh(known);
            }
            _ if !ctrl => {
                if let Some(text) = text {
                    self.type_text(text, known);
                }
            }
            _ => {}
        }
        Outcome::Stay
    }
}

/// Whether a click on `hit` leaves an open list as it is: one on the list
/// itself, or on what opens a list. Any other click closes it.
pub fn keeps_open(hit: Hit) -> bool {
    matches!(
        hit,
        Hit::Field
            | Hit::Do(
                Action::AddProject
                    | Action::Launch(_)
                    | Action::Options(_)
                    | Action::Previous(_)
                    | Action::Pick(_)
                    | Action::More(_)
                    | Action::Browse
            )
    )
}

/// Rows of the list that fit between the header and the field.
pub fn capacity(ui: &Ui) -> usize {
    let fixed = ui.picker.as_ref().is_some_and(|p| p.browsable()) as usize;
    let room = ui.sidebar().h - ui.px(HEADER) - ui.px(ROW) - ui.px(16.0);
    ((room / ui.px(ROW)).floor() as usize).saturating_sub(fixed).max(1)
}

/// The detail column keeps enough room for a readable label and controls.
fn value_width(ui: &Ui, x: f32, arrows: f32, desired: f32) -> f32 {
    desired.min((ui.sidebar().w - x - ui.px(INSET) - ui.px(64.0) - ui.px(12.0) - arrows).max(0.0))
}

pub fn draw(cv: &mut Canvas, ui: &Ui, picker: &Picker, hits: &mut Hits) {
    picker.drawn.set(picker.made);
    let area = ui.sidebar();
    let (row_h, px, inset) = (ui.px(ROW), ui.text_px, ui.px(INSET));
    let field = field_row(ui);
    let capacity = capacity(ui);
    let shown = picker.rows.len().clamp(1, capacity);
    let fixed = picker.browsable() as usize;
    let height = (shown + fixed) as f32 * row_h + ui.px(6.0);
    let list = Rect::new(0.0, field.y - height, area.w, height);
    cv.rect(list, theme::SIDEBAR);
    cv.rect(Rect::new(0.0, list.y, area.w, 1.0), theme::LINE);
    hits.push((list, Hit::Field));
    let top = list.y + ui.px(3.0);

    // Before the rows: where the two meet, a row of the list is on top.
    if picker.browsable() {
        browse_row(cv, ui, field, hits);
    }
    if picker.rows.is_empty() {
        let row = Rect::new(0.0, top, area.w, row_h);
        cv.text(nothing(picker), UI, px, inset, ui.baseline(row), theme::FAINT, area.w - 2.0 * inset);
    }
    // The widest value an option of the agent can take. The arrows around
    // a value stand where they would for that one, in every row, so they
    // do not move when a value changes.
    let setting = picker.launch.as_ref().and_then(|l| l.starters.get(l.setting?));
    let names = setting.iter().flat_map(|s| &s.options).flat_map(|option| &option.choices);
    let widest = names.map(|choice| cv.text_width(&choice.name, UI, px)).fold(0.0, f32::max);
    for k in (picker.scroll..picker.rows.len()).take(capacity) {
        let row = Rect::new(0.0, top + (k - picker.scroll) as f32 * row_h, area.w, row_h);
        entry(cv, ui, picker, k, row, widest, hits);
    }
    // More rows than fit: a thumb shows where in the list this is.
    if picker.rows.len() > capacity {
        thumb(cv, ui, picker, capacity, shown, top);
    }
    search_field(cv, ui, picker, field, hits);
}

/// The way to everything the search does not know: folders without git,
/// other disks, deeper trees. A row fixed below the list.
fn browse_row(cv: &mut Canvas, ui: &Ui, field: Rect, hits: &mut Hits) {
    let area = ui.sidebar();
    let (row_h, px, inset) = (ui.px(ROW), ui.text_px, ui.px(INSET));
    let row = Rect::new(0.0, field.y - row_h - ui.px(3.0), area.w, row_h);
    let color = ui.lit(Action::Browse, theme::DIM);
    let icon = ui.px(chrome::ICON);
    chrome::symbol(cv, ui, Icon::Folder, Rect::new(inset - ui.px(2.0), row.y, icon, row_h), color);
    let x = inset + icon + ui.px(4.0);
    cv.text("Browse folders…", UI, px, x, ui.baseline(row), color, area.w - x - inset);
    hits.push((row, Hit::Do(Action::Browse)));
}

/// What a list without rows says.
fn nothing(picker: &Picker) -> &'static str {
    match &picker.launch {
        Some(_) => "Nothing by that name",
        None if picker.browsable() => "No repository found",
        None if picker.browse_busy => "Browser busy; edit to retry",
        None if picker.browsed.as_ref().is_none_or(|(text, _)| text != &picker.text) => "Reading folders…",
        // The folder is there, and all it had to offer is added.
        None if picker.browsed.as_ref().is_some_and(|(_, rows)| !rows.is_empty()) => "Already a project",
        None => "No such folder",
    }
}

/// Row `k` of the list in `row`: what it chooses at the left, and at the
/// right what is set for it, with `widest` the room an option's value has.
fn entry(cv: &mut Canvas, ui: &Ui, picker: &Picker, k: usize, row: Rect, widest: f32, hits: &mut Hits) {
    let area = ui.sidebar();
    let (row_h, px, inset) = (ui.px(ROW), ui.text_px, ui.px(INSET));
    let item = &picker.rows[k];
    if k == picker.selected {
        cv.rect(row, theme::ACTIVE);
    } else if ui.hot(picker.action(k)) {
        cv.rect(row, theme::HOVER);
    }
    hits.push((row, Hit::Do(picker.action(k))));
    // The detail at the right gives way to a long label. Not so what
    // is set for an agent or the age of a conversation: titles are
    // long, and many read alike.
    let base = ui.baseline(row);
    let launch = picker.launch.as_ref();
    let agent = launch.and_then(|launch| launch.agent(&item.pick));
    // An agent with options says what is set, and an arrow leads to
    // them. An option stands between two arrows that change it.
    let opens = matches!(item.pick, Pick::Start(at) if agent.is_some()
        && launch.is_some_and(|l| l.starters.get(at).is_some_and(|s| !s.options.is_empty())));
    let option = matches!(item.pick, Pick::Option(_));
    let (icon, arrow) = (ui.px(chrome::ICON), ui.px(12.0));
    let arrows = if opens {
        arrow
    } else if option {
        2.0 * arrow + ui.px(4.0)
    } else {
        0.0
    };
    let detail_w = cv.text_width(&item.detail, UI, px);
    // The room for what stands at the right: an option's value has
    // that of the widest one and stands in the middle of it.
    let desired_value = if option { widest.max(detail_w) } else { detail_w };
    // Conversations stand below the agents, apart from them.
    let first =
        k > 0 && matches!(item.pick, Pick::Resume(_)) && matches!(picker.rows[k - 1].pick, Pick::Start(_));
    if first && k > picker.scroll {
        cv.rect(Rect::new(inset, row.y, area.w - 2.0 * inset, 1.0), theme::LINE);
    }
    // A session starts with the mark of its agent, as the header of
    // its pane will show it.
    let mut x = inset;
    if let Some(agent) = agent {
        chrome::agent(cv, ui, agent, x, row);
        x += ui.mark_w;
    }
    // The drawings of the arrows have air around them: they are placed
    // so that their lines begin and end where text would.
    let back = matches!(item.pick, Pick::Back);
    if back {
        let at = Rect::new(inset - ui.px(5.0), row.y, icon, row_h);
        chrome::symbol(cv, ui, Icon::ChevronLeft, at, theme::DIM);
        x += ui.px(10.0);
    }
    // The row for the rest reads as one of the conversations it stands
    // for, and begins where their titles do.
    let more = matches!(item.pick, Pick::More);
    if more {
        x += ui.mark_w;
    }
    let color = if back || more { theme::DIM } else { theme::TEXT };
    // Values must leave the label readable, and long values must never
    // take away the arrows that reach the options or change a choice.
    let value_w = value_width(ui, x, arrows, desired_value);
    let kept = if launch.is_some() { value_w + arrows + ui.px(12.0) } else { 0.0 };
    let label_w = cv.text(&item.label, UI, px, x, base, color, area.w - x - inset - kept);
    let room = area.w - x - inset - label_w - ui.px(12.0);
    if value_w + arrows <= room {
        let end = area.w - inset;
        let right = end - if arrows > 0.0 { ui.px(10.0) } else { 0.0 };
        let (options, previous) = (Action::Options(k), Action::Previous(k));
        let lit = |on: bool, idle| if on { theme::TEXT } else { idle };
        cv.text(
            &item.detail,
            UI,
            px,
            right - value_w + ((value_w - detail_w).max(0.0) / 2.0).round(),
            base,
            lit(opens && ui.hot(options), theme::FAINT),
            value_w,
        );
        if opens || option {
            let on = ui.hot(options) || (option && ui.hot(Action::Pick(k)));
            let at = Rect::new(end - arrow, row.y, icon, row_h);
            chrome::symbol(cv, ui, Icon::ChevronRight, at, lit(on, theme::DIM));
        }
        if opens {
            let from = right - value_w - ui.px(8.0);
            hits.push((Rect::new(from, row.y, area.w - from, row_h), Hit::Do(options)));
        }
        if option {
            let at = Rect::new(right - value_w - ui.px(14.0), row.y, icon, row_h);
            chrome::symbol(cv, ui, Icon::ChevronLeft, at, lit(ui.hot(previous), theme::DIM));
            hits.push((Rect::new(at.x - ui.px(2.0), row.y, icon + ui.px(2.0), row_h), Hit::Do(previous)));
        }
    }
}

/// Shows where in a list of more rows than fit the shown ones are.
fn thumb(cv: &mut Canvas, ui: &Ui, picker: &Picker, capacity: usize, shown: usize, top: f32) {
    let (area, row_h) = (ui.sidebar(), ui.px(ROW));
    let track = shown as f32 * row_h;
    let thumb = (track * capacity as f32 / picker.rows.len() as f32).max(ui.px(18.0));
    let at = (track - thumb) * picker.scroll as f32 / (picker.rows.len() - capacity) as f32;
    cv.rounded(Rect::new(area.w - ui.px(5.0), top + at, ui.px(3.0), thumb), ui.px(2.0), theme::FAINT);
}

/// The field the list is searched from, with what it is for while empty.
fn search_field(cv: &mut Canvas, ui: &Ui, picker: &Picker, field: Rect, hits: &mut Hits) {
    cv.rounded(field, ui.px(6.0), theme::ACTIVE);
    hits.push((field, Hit::Field));
    let color = if picker.invalid { theme::DANGER } else { theme::TEXT };
    let hint = match &picker.launch {
        Some(launch) if launch.setting.is_some() => "Arrows change, Enter starts",
        Some(_) => "Start or continue",
        None => "Filter, or type a path",
    };
    let icon = ui.px(chrome::ICON);
    chrome::symbol(cv, ui, Icon::Search, Rect::new(field.x + ui.px(6.0), field.y, icon, field.h), theme::DIM);
    field::draw(cv, ui, field, field.x + icon + ui.px(11.0), &picker.text, hint, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Choice;

    fn options_picker() -> Picker {
        let option = |name: &str| Opt {
            name: name.into(),
            choices: ["default", "other"]
                .into_iter()
                .map(|name| Choice { name: name.into(), args: vec![] })
                .collect(),
        };
        let mut picker = Picker {
            launch: Some(Launch {
                project: ProjectId(0),
                starters: vec![Starter {
                    agent: "shell".into(),
                    options: vec![option("Model"), option("Effort"), option("Permissions")],
                    chosen: Chosen::new(),
                }],
                found: None,
                setting: Some(0),
                all: false,
            }),
            ..Picker::default()
        };
        picker.refresh(&Known { repos: &[], added: &|_| false });
        picker.selected = 1;
        picker
    }

    #[test]
    fn a_click_on_a_list_that_was_built_anew_since_it_was_drawn_chooses_nothing() {
        let known = Known { repos: &[], added: &|_| false };
        let folder = |name: &str| Row {
            label: name.into(),
            detail: String::new(),
            pick: Pick::Add(format!("/work/{name}").into()),
        };
        let mut picker = Picker::default();
        picker.set_text("/work/".into(), &known);
        picker.browsed("/work/".into(), vec![folder("b"), folder("c")], &known);
        // The frame that shows the two rows.
        picker.drawn.set(picker.made);
        assert_eq!(picker.clicked(0), Some(0));
        // The folder is read again before the click: another row is first.
        picker.browsed("/work/".into(), vec![folder("a"), folder("b"), folder("c")], &known);
        assert_eq!(picker.clicked(0), None);
        // What only the row itself changes leaves the list as it is.
        picker.drawn.set(picker.made);
        picker.wheel(1, 1);
        assert_eq!(picker.clicked(1), Some(1));
    }

    #[test]
    fn clicked_choice_becomes_the_keyboard_target_and_wraps_both_ways() {
        let mut picker = options_picker();
        picker.choose(3, -1);
        assert_eq!(picker.selected, 3);
        assert_eq!(picker.rows[3].detail, "other");
        picker.key(
            &Key::Named(NamedKey::ArrowRight),
            None,
            false,
            10,
            &Known { repos: &[], added: &|_| false },
        );
        assert_eq!(picker.rows[3].detail, "default");
        assert_eq!(picker.rows[1].detail, "default");
        assert_eq!(picker.rows[2].detail, "default");
        let Outcome::Start(_, _, chosen) = picker.key(
            &Key::Named(NamedKey::Enter),
            None,
            false,
            10,
            &Known { repos: &[], added: &|_| false },
        ) else {
            panic!("Enter must start")
        };
        assert_eq!(chosen.get("Permissions").map(String::as_str), Some("default"));
    }

    #[test]
    fn long_values_leave_room_for_labels_and_choice_controls_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = Ui::new(Vec::new());
            ui.scale = scale;
            for (x, arrows) in [(ui.px(INSET), ui.px(28.0)), (ui.px(37.0), ui.px(12.0))] {
                let available = ui.sidebar().w - x - ui.px(INSET);
                let value = value_width(&ui, x, arrows, 10_000.0);
                assert!(value > 0.0);
                assert!(value + arrows + ui.px(12.0) + ui.px(64.0) <= available);
                assert_eq!(value_width(&ui, x, arrows, ui.px(20.0)), ui.px(20.0));
            }
        }
    }

    #[test]
    fn directory_replies_are_query_specific_and_hide_already_added_projects() {
        let mut picker = Picker::default();
        let known = Known { repos: &[], added: &|path| path == Path::new("/already") };
        picker.set_text("/missing/path/".into(), &known);
        assert!(picker.rows.is_empty(), "refresh never reads the filesystem");
        let rows = || {
            vec![
                Row {
                    pick: Pick::Add("/already".into()),
                    label: "Add already".into(),
                    detail: String::new(),
                },
                Row {
                    pick: Pick::Open("/missing/path/child/".into()),
                    label: "child/".into(),
                    detail: String::new(),
                },
            ]
        };
        picker.browsed("/stale/".into(), rows(), &known);
        assert!(picker.rows.is_empty());
        picker.browsed("/missing/path/".into(), rows(), &known);
        assert_eq!(picker.rows.len(), 1);
        assert_eq!(picker.rows[0].label, "child/");
        picker.set_text("/another/".into(), &known);
        assert!(picker.rows.is_empty(), "previous query's rows must disappear");
        picker.browsed("/missing/path/".into(), rows(), &known);
        assert!(picker.rows.is_empty());
    }

    #[test]
    fn a_new_scan_keeps_the_highlight_on_its_row_and_in_view() {
        let repo = |parent: &str, name: &str| Repo {
            name: name.into(),
            parent: parent.into(),
            path: Path::new(parent).join(name),
        };
        // Two repositories of the same name, as a work and a private copy.
        let before = [repo("/p", "app"), repo("/work", "app")];
        let mut picker = Picker::default();
        picker.refresh(&Known { repos: &before, added: &|_| false });
        let down = |picker: &mut Picker, repos: &[Repo]| {
            picker.key(&Key::Named(NamedKey::ArrowDown), None, false, 3, &Known { repos, added: &|_| false });
        };
        down(&mut picker, &before);
        assert!(
            matches!(&picker.rows[picker.selected].pick, Pick::Add(path) if path == Path::new("/work/app"))
        );

        // The scan that was under way finds more, in front of both.
        let mut after: Vec<Repo> = (0..6).map(|n| repo("/new", &n.to_string())).collect();
        after.extend(before);
        picker.rescanned(&Known { repos: &after, added: &|_| false });
        assert!(
            matches!(&picker.rows[picker.selected].pick, Pick::Add(path) if path == Path::new("/work/app"))
        );
        assert_eq!((picker.selected, picker.scroll), (7, 6), "the row is where it was in the view");
    }

    /// The list that starts a session in a project with `count`
    /// conversations on disk, the newest first.
    fn earlier_picker(count: usize) -> Picker {
        let conversation = |at: usize| Conversation {
            agent: "shell".into(),
            id: format!("id-{at}"),
            title: format!("talk {at}"),
            changed: SystemTime::UNIX_EPOCH,
        };
        let starter = Starter { agent: "shell".into(), options: Vec::new(), chosen: Chosen::new() };
        let mut picker = Picker {
            launch: Some(Launch {
                project: ProjectId(0),
                starters: vec![starter],
                found: Some((0..count).map(conversation).collect()),
                setting: None,
                all: false,
            }),
            ..Picker::default()
        };
        picker.refresh(&Known { repos: &[], added: &|_| false });
        picker
    }

    fn resumes(picker: &Picker) -> Vec<usize> {
        let at = |row: &Row| if let Pick::Resume(at) = row.pick { Some(at) } else { None };
        picker.rows.iter().filter_map(at).collect()
    }

    fn rest(picker: &Picker) -> Option<&str> {
        picker.rows.iter().find(|row| row.pick == Pick::More).map(|row| row.label.as_str())
    }

    #[test]
    fn of_many_conversations_the_latest_are_listed_and_a_row_stands_for_the_rest() {
        let picker = earlier_picker(27);
        assert_eq!(resumes(&picker), [0, 1, 2, 3, 4]);
        assert_eq!(rest(&picker), Some("Show 22 more"));
        assert!(picker.rows.last().is_some_and(|row| row.pick == Pick::More), "the row is the last");
        assert_eq!(picker.rows.len(), 7);

        // One more than is listed is shown as itself, two are not.
        let picker = earlier_picker(RECENT + 1);
        assert_eq!((resumes(&picker).len(), rest(&picker)), (RECENT + 1, None));
        let picker = earlier_picker(RECENT + 2);
        assert_eq!((resumes(&picker).len(), rest(&picker)), (RECENT, Some("Show 2 more")));
    }

    #[test]
    fn the_row_for_the_rest_lists_all_and_highlights_the_first_held_back() {
        let known = Known { repos: &[], added: &|_| false };
        let mut picker = earlier_picker(27);
        for _ in 0..6 {
            picker.key(&Key::Named(NamedKey::ArrowDown), None, false, 12, &known);
        }
        assert!(picker.rows[picker.selected].pick == Pick::More);
        assert!(matches!(picker.key(&Key::Named(NamedKey::Enter), None, false, 12, &known), Outcome::Stay));
        assert_eq!(resumes(&picker), (0..27).collect::<Vec<_>>());
        assert_eq!(rest(&picker), None);
        // On the first of those that were held back, scrolled as it was.
        assert_eq!((picker.selected, picker.scroll), (6, 0));
        assert!(picker.rows[6].pick == Pick::Resume(5));

        // A row still continues the conversation it names.
        let Outcome::Resume(_, conversation) = picker.pick(27, &known) else { panic!("a row must continue") };
        assert_eq!(conversation.id, "id-26");
        // And the list stays whole after a search in it.
        picker.type_text("x", &known);
        picker.key(&Key::Named(NamedKey::Backspace), None, false, 12, &known);
        assert_eq!(resumes(&picker).len(), 27);
    }

    #[test]
    fn the_row_for_the_rest_is_another_element_than_the_row_that_takes_its_place() {
        // Where the list cannot grow, the first of those held back comes
        // to stand under the pointer. The second click of a double click
        // is only dropped when it hits another element than the first.
        let known = Known { repos: &[], added: &|_| false };
        let mut picker = earlier_picker(27);
        let row = picker.rows.len() - 1;
        let first = picker.action(row);
        assert!(matches!(picker.pick(row, &known), Outcome::Stay));
        assert!(picker.rows[row].pick == Pick::Resume(5));
        assert_ne!(picker.action(row), first);
    }

    #[test]
    fn a_search_goes_through_the_conversations_that_are_not_listed() {
        let known = Known { repos: &[], added: &|_| false };
        let mut picker = earlier_picker(27);
        picker.type_text("talk 2", &known);
        assert_eq!(resumes(&picker), [2, 12, 20, 21, 22, 23, 24, 25, 26]);
        assert_eq!(rest(&picker), None);
        let Outcome::Resume(_, conversation) = picker.pick(2, &known) else { panic!("a row must continue") };
        assert_eq!(conversation.id, "id-20");

        // Without the text the list is the short one again.
        for _ in 0.."talk 2".len() {
            picker.key(&Key::Named(NamedKey::Backspace), None, false, 12, &known);
        }
        assert_eq!((resumes(&picker).len(), rest(&picker)), (RECENT, Some("Show 22 more")));
        // Blanks alone search for nothing.
        picker.type_text("  ", &known);
        assert_eq!((resumes(&picker).len(), rest(&picker)), (RECENT, Some("Show 22 more")));
    }

    #[test]
    fn tab_goes_into_a_folder_and_does_nothing_else() {
        let known = Known { repos: &[], added: &|_| false };
        let tab =
            |picker: &mut Picker, ctrl| picker.key(&Key::Named(NamedKey::Tab), Some("\t"), ctrl, 10, &known);
        // In the list that starts sessions it starts none, with or
        // without the Control of "next session".
        let mut picker = options_picker();
        picker.key(&Key::Named(NamedKey::Escape), None, false, 10, &known);
        assert!(matches!(picker.rows[picker.selected].pick, Pick::Start(_)));
        assert!(matches!(tab(&mut picker, false), Outcome::Stay));
        assert!(matches!(tab(&mut picker, true), Outcome::Stay));
        assert!(picker.text.is_empty(), "the tab was typed into the field");

        let mut picker = Picker::default();
        picker.set_text("/p/".into(), &known);
        let row = Row { pick: Pick::Open("/p/child/".into()), label: "child/".into(), detail: String::new() };
        picker.browsed("/p/".into(), vec![row], &known);
        tab(&mut picker, false);
        assert_eq!(picker.text, "/p/child/");
    }
}
