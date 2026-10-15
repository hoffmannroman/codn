//! The open projects and sessions and what the panes show.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::fileview::FileView;
use super::state::{Saved, SavedSession};
use super::tree::Tree;
use super::{Doing, Project, ProjectId, Session, SessionId, Status, folder_name, git, uuid};

pub struct DiffView {
    pub project: ProjectId,
    pub lines: Vec<git::Line>,
    /// First visible line.
    pub scroll: usize,
    pub loaded: bool,
    /// Where the folder of the project lies in its repository: nothing
    /// for its top, else the path from there with a slash behind it. The
    /// lines name every file from the top.
    prefix: String,
    /// Where to scroll to, once the lines are there.
    goto: Option<Goto>,
}

/// A file of the project and a line of it, counted from one, to scroll
/// the changes to.
struct Goto {
    file: String,
    line: u32,
    /// An answer has come that did not name the file: it may have been
    /// asked for before the file was.
    waited: bool,
}

impl DiffView {
    fn new(project: ProjectId) -> Self {
        Self { project, lines: Vec::new(), scroll: 0, loaded: false, prefix: String::new(), goto: None }
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(2)
    }

    /// A number that tells one name of a file from another, to know that
    /// a row still stands for the file it stood for when it was drawn.
    fn check(name: &str) -> u32 {
        name.bytes().fold(0x811c_9dc5, |hash, byte| (hash ^ byte as u32).wrapping_mul(0x0100_0193))
    }

    /// Whether a row leads to a file that can be shown: it names one that
    /// is there and lies in the folder of the project. The number that
    /// comes back goes into what a click on the row asks for.
    pub fn leads(&self, row: usize) -> Option<u32> {
        let line = self.lines.get(row).filter(|line| line.kind == git::Kind::File)?;
        // A name that was no text was made one with other characters in
        // it: it may be the name of another file, and is not this one's.
        // Nor does a name lead anywhere that steps out of the repository,
        // or out of the folder of the project once it is cut where that
        // folder ends: what is left is held against the same.
        let rest = line.text.strip_prefix(&self.prefix).filter(|_| !line.text.contains('\u{fffd}'))?;
        (git::inside(&line.text) && git::inside(rest)).then(|| Self::check(&line.text))
    }

    /// The file a row leads to, by its path in the folder of the project,
    /// if the row still names the file that `check` was made of.
    pub fn file_at(&self, row: usize, check: u32) -> Option<&str> {
        let name = &self.lines.get(row).filter(|_| self.leads(row) == Some(check))?.text;
        name.strip_prefix(&self.prefix)
    }

    /// The row to scroll to for a line of a file of the project, counted
    /// from one: a little above the first row of that file at or behind
    /// the line, so that what went right before it is seen, or above
    /// the last of its rows where lines went at its end. Nothing for a
    /// file that is not among the changes.
    fn row_of(&self, file: &str, line: u32) -> Option<usize> {
        let name = format!("{}{file}", self.prefix);
        let head = self.lines.iter().position(|l| l.names_file() && l.text == name)?;
        let rows = self.lines[head + 1..].iter().take_while(|l| !l.names_file()).count();
        let behind = self.lines[head + 1..head + 1 + rows].iter().position(|l| l.new >= line && l.new > 0);
        let at = head + 1 + behind.unwrap_or(rows.saturating_sub(1));
        // Near its top the row of the file itself is the one to begin at.
        Some(Some(at.saturating_sub(3)).filter(|row| rows > 0 && *row > head + 1).unwrap_or(head))
    }

    pub fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines).min(self.max_scroll());
        // Who scrolls is not taken elsewhere by an answer that is late.
        self.goto = None;
    }

    pub fn set_lines(&mut self, prefix: String, lines: Vec<git::Line>) {
        (self.prefix, self.lines, self.loaded) = (prefix, lines, true);
        if let Some(goto) = self.goto.take() {
            match self.row_of(&goto.file, goto.line) {
                Some(row) => self.scroll = row,
                // An answer that was on its way before the file changed
                // does not know it. The next one is the one asked for.
                None if !goto.waited => self.goto = Some(Goto { waited: true, ..goto }),
                None => {}
            }
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }
}

/// Why something was not done: it would have taken the place of a file
/// whose changes are not saved. Nothing was changed.
#[derive(Debug, PartialEq)]
pub struct Unsaved;

pub enum Pane {
    Empty,
    Session(SessionId),
    Diff(DiffView),
    /// Boxed: it is several times the size of the others.
    File(Box<FileView>),
}

impl Pane {
    /// Whether this is what is shown beside the sessions, a file or the
    /// changes. There is one of them at most.
    fn beside(&self) -> bool {
        matches!(self, Self::Diff(_) | Self::File(_))
    }

    /// Whether this is a file that must not lose its place, see
    /// `FileView::holds`.
    fn holds(&self) -> bool {
        matches!(self, Self::File(view) if view.holds())
    }
}

/// How many projects and how many sessions there are at the most. A
/// project is held against every other where one is added, and a
/// session against every other of its kind, on the window thread: a
/// state is another program's to write as well, and without a bound
/// one of half a megabyte kept the window away for most of a minute.
pub const MAX_PROJECTS: usize = 1000;
pub const MAX_SESSIONS: usize = 4000;

pub struct Workspace {
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    /// One pane, or two side by side. Never empty. Only the methods here
    /// put something into a pane, take it out or move the keyboard: what
    /// a pane must not lose is kept in this one place.
    panes: Vec<Pane>,
    focus: usize,
    /// The files of one of the projects, for the column at the right.
    pub tree: Tree,
    next_id: u32,
    /// How many times a file was opened: the number of the next opening.
    openings: u64,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            projects: Vec::new(),
            sessions: Vec::new(),
            panes: vec![Pane::Empty],
            focus: 0,
            tree: Tree::default(),
            next_id: 0,
            openings: 0,
        }
    }

    pub fn restore(saved: &Saved) -> Self {
        let mut ws = Self::new();
        for path in saved.projects.iter().take(MAX_PROJECTS) {
            ws.add_project(path.clone());
        }
        // A row that is passed over costs what one that is kept costs,
        // and neither is held against all the others: a state can hold
        // hundreds of thousands of rows of which four thousand are kept.
        let mut folders: HashMap<&Path, ProjectId> = HashMap::new();
        for project in &ws.projects {
            folders.entry(&project.path).or_insert(project.id);
        }
        let folders: HashMap<PathBuf, ProjectId> =
            folders.into_iter().map(|(path, id)| (path.to_owned(), id)).collect();
        let mut held: HashSet<(&str, &str)> = HashSet::new();
        for s in &saved.sessions {
            if ws.sessions.len() >= MAX_SESSIONS {
                break;
            }
            if !super::history::valid_id(&s.id) || held.contains(&(s.agent.as_str(), s.id.as_str())) {
                continue;
            }
            if let Some(project) = folders.get(&s.project).copied() {
                held.insert((&s.agent, &s.id));
                let id = ws.add_session(project, &s.agent, s.id.clone(), true);
                let Some(session) = ws.sessions.last_mut() else { continue };
                session.name = s.name.clone();
                session.title = s.title.clone();
                session.known = s.known;
                session.chosen = s.chosen.clone();
                if saved.active.as_ref() == Some(&s.id)
                    && saved.active_agent.as_ref().is_none_or(|agent| *agent == s.agent)
                    && matches!(ws.panes[0], Pane::Empty)
                {
                    ws.panes[0] = Pane::Session(id);
                }
            }
        }
        ws
    }

    pub fn saved(&self, width: f32, height: f32) -> Saved {
        // The session with the keyboard, or beside a file or the changes
        // that have it the one in the other pane.
        let front =
            self.focused_session().or_else(|| (0..self.panes.len()).find_map(|p| self.pane_session(p)));
        let front = front.and_then(|id| self.session(id));
        let active = front.map(|s| s.uuid.clone());
        let active_agent = front.map(|s| s.agent.clone());
        let sessions = self.sessions.iter().filter_map(|s| {
            Some(SavedSession {
                project: self.project(s.project)?.path.clone(),
                agent: s.agent.clone(),
                id: s.uuid.clone(),
                name: s.name.clone(),
                title: s.title.clone(),
                known: s.known,
                chosen: s.chosen.clone(),
            })
        });
        Saved {
            width,
            height,
            active,
            active_agent,
            // TOML cannot hold a path that is not valid UTF-8.
            projects: self
                .projects
                .iter()
                .filter(|p| p.path.to_str().is_some())
                .map(|p| p.path.clone())
                .collect(),
            sessions: sessions.collect(),
            last: Vec::new(),
            tree: false,
            folded: false,
        }
    }

    fn next_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    // ----- lookups -----

    /// The panes, from the left.
    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    /// The pane that has the keyboard.
    pub fn focus(&self) -> usize {
        self.focus
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// The project that stands for a folder.
    pub fn project_at(&self, path: &Path) -> Option<&Project> {
        self.projects.iter().find(|p| p.path == path)
    }

    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn session_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// Whether a session of this agent is in the conversation `uuid`.
    pub fn holds(&self, agent: &str, uuid: &str) -> bool {
        self.sessions.iter().any(|s| s.agent == agent && s.uuid == uuid)
    }

    pub fn sessions_of(&self, project: ProjectId) -> impl Iterator<Item = &Session> {
        self.sessions.iter().filter(move |s| s.project == project)
    }

    /// The session shown in a pane.
    pub fn pane_session(&self, pane: usize) -> Option<SessionId> {
        match self.panes.get(pane) {
            Some(Pane::Session(id)) => Some(*id),
            _ => None,
        }
    }

    pub fn focused_session(&self) -> Option<SessionId> {
        self.pane_session(self.focus)
    }

    /// The pane showing a session.
    fn session_pane(&self, id: SessionId) -> Option<usize> {
        self.panes.iter().position(|p| matches!(p, Pane::Session(shown) if *shown == id))
    }

    /// Whether a session is shown in any pane.
    pub fn visible(&self, id: SessionId) -> bool {
        self.session_pane(id).is_some()
    }

    /// The pane showing the diff of a project.
    fn diff_pane(&self, project: ProjectId) -> Option<usize> {
        self.panes.iter().position(|p| matches!(p, Pane::Diff(d) if d.project == project))
    }

    pub fn diff_mut(&mut self, project: ProjectId) -> Option<&mut DiffView> {
        self.panes.iter_mut().find_map(|p| match p {
            Pane::Diff(view) if view.project == project => Some(view),
            _ => None,
        })
    }

    /// The changes a pane shows.
    pub fn pane_diff(&mut self, pane: usize) -> Option<&mut DiffView> {
        match self.panes.get_mut(pane) {
            Some(Pane::Diff(view)) => Some(view),
            _ => None,
        }
    }

    pub fn shows_diff(&self, project: ProjectId) -> bool {
        self.diff_pane(project).is_some()
    }

    /// The file that is shown, and the pane it is in.
    pub fn file(&self) -> Option<(usize, &FileView)> {
        self.panes.iter().enumerate().find_map(|(pane, shown)| match shown {
            Pane::File(view) => Some((pane, &**view)),
            _ => None,
        })
    }

    pub fn file_mut(&mut self) -> Option<&mut FileView> {
        self.panes.iter_mut().find_map(|pane| match pane {
            Pane::File(view) => Some(&mut **view),
            _ => None,
        })
    }

    /// The file a pane shows.
    pub fn pane_file(&mut self, pane: usize) -> Option<&mut FileView> {
        match self.panes.get_mut(pane) {
            Some(Pane::File(view)) => Some(view),
            _ => None,
        }
    }

    /// Where the file that is shown lies on this machine.
    pub fn file_path(&self) -> Option<PathBuf> {
        let (_, view) = self.file()?;
        Some(self.project(view.project)?.path.join(&view.path))
    }

    /// The project the user is working in: the one of the focused pane, or
    /// the only one there is.
    pub fn active_project(&self) -> Option<ProjectId> {
        match self.panes.get(self.focus) {
            Some(Pane::Session(id)) => self.session(*id).map(|s| s.project),
            Some(Pane::Diff(diff)) => Some(diff.project),
            Some(Pane::File(file)) => Some(file.project),
            _ if self.projects.len() == 1 => Some(self.projects[0].id),
            _ => None,
        }
    }

    // ----- names -----

    /// What a session is called before numbering: the name the user gave it,
    /// else the title its program set, else the agent.
    fn base_label<'a>(&'a self, session: &'a Session) -> &'a str {
        base_label_in(self.project(session.project).map_or("", |p| p.name.as_str()), session)
    }

    /// The name shown for a session. Sessions of a project that would read
    /// the same are numbered in the order they were opened.
    pub fn label(&self, session: &Session) -> String {
        let base = self.base_label(session);
        let earlier = self
            .sessions_of(session.project)
            .take_while(|s| s.id != session.id)
            .filter(|s| self.base_label(s) == base)
            .count();
        self.numbered(session, earlier)
    }

    /// The name shown for a session that `earlier` sessions of its project
    /// before it read like.
    pub fn numbered(&self, session: &Session, earlier: usize) -> String {
        let base = self.base_label(session);
        if earlier > 0 { format!("{base} {}", earlier + 1) } else { base.to_owned() }
    }

    /// The projects, each with its sessions in their order, found in one
    /// pass. `sessions_of` looks at every session there is for one
    /// project, and `label` at every session of a project for one name:
    /// whoever draws all of them, for every frame, asks here.
    pub fn grouped(&self) -> Vec<(&Project, Vec<&Session>)> {
        let places: HashMap<ProjectId, usize> =
            self.projects.iter().enumerate().map(|(place, project)| (project.id, place)).collect();
        let mut groups: Vec<_> = self.projects.iter().map(|project| (project, Vec::new())).collect();
        for session in &self.sessions {
            if let Some(place) = places.get(&session.project) {
                groups[*place].1.push(session);
            }
        }
        groups
    }

    /// For each of the sessions of a project, in their order, how many
    /// before it read like it: what `numbered` makes its name of.
    pub fn earlier<'a>(project: &Project, sessions: &[&'a Session]) -> Vec<usize> {
        let mut read: HashMap<&'a str, usize> = HashMap::new();
        let mut earlier = Vec::with_capacity(sessions.len());
        for session in sessions {
            let so_far = read.entry(base_label_in(&project.name, session)).or_default();
            earlier.push(*so_far);
            *so_far += 1;
        }
        earlier
    }

    // ----- changes -----

    /// Whether one more project has room. Asked by whoever adds a folder
    /// that is not a project yet.
    pub fn room(&self) -> bool {
        self.projects.len() < MAX_PROJECTS
    }

    /// Adds a project, or returns the one that has this path already.
    pub fn add_project(&mut self, path: PathBuf) -> ProjectId {
        if let Some(project) = self.project_at(&path) {
            return project.id;
        }
        let id = ProjectId(self.next_id());
        self.projects.push(Project { id, name: folder_name(&path), path, git: None });
        id
    }

    /// Whether a file is shown whose changes are not saved and were not
    /// let go.
    pub fn unsaved(&self) -> bool {
        self.panes.iter().any(Pane::holds)
    }

    /// Removes a project that has no sessions left.
    pub fn remove_project(&mut self, id: ProjectId) -> Result<(), Unsaved> {
        if self.sessions_of(id).next().is_some() {
            return Ok(());
        }
        if self
            .panes
            .iter()
            .any(|pane| matches!(pane, Pane::File(file) if file.project == id && file.holds()))
        {
            return Err(Unsaved);
        }
        if let Some(project) = self.project(id) {
            let root = project.path.clone();
            self.tree.forget(id, &root);
        }
        self.projects.retain(|p| p.id != id);
        self.panes.retain(|p| match p {
            Pane::Diff(diff) => diff.project != id,
            Pane::File(file) => file.project != id,
            _ => true,
        });
        self.tidy();
        Ok(())
    }

    pub fn add_session(&mut self, project: ProjectId, agent: &str, uuid: String, resume: bool) -> SessionId {
        let id = SessionId(self.next_id());
        self.sessions.push(Session::new(id, project, agent, uuid, resume));
        id
    }

    pub fn new_session(&mut self, project: ProjectId, agent: &str) -> SessionId {
        self.add_session(project, agent, uuid(), false)
    }

    pub fn remove_session(&mut self, id: SessionId) {
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else { return };
        let project = self.sessions.remove(index).project;
        // A running neighbour from the same project takes its place. One
        // that was never started stays where it is: closing a session must
        // not start another program.
        let next = self.sessions_of(project).find(|s| s.term.is_some() && !self.visible(s.id)).map(|s| s.id);
        for pane in &mut self.panes {
            if matches!(pane, Pane::Session(shown) if *shown == id) {
                *pane = next.map_or(Pane::Empty, Pane::Session);
            }
        }
        self.tidy();
    }

    /// Shows a session in the focused pane, or with `split` beside it.
    ///
    /// Like every method here that puts something into a pane or takes
    /// it out: where that would cost a file whose changes are not saved,
    /// nothing is done, and it says so. This is checked before anything
    /// is changed, here and not at the callers, of which there are many.
    pub fn show(&mut self, id: SessionId, split: bool) -> Result<(), Unsaved> {
        // What asked for it may have waited for an answer, and the
        // session ended meanwhile: there is nothing to show.
        if self.session(id).is_none() {
            return Ok(());
        }
        if let Some(pane) = self.session_pane(id) {
            self.focus = pane;
        } else if split {
            // The pane without the keyboard gives way.
            if self.panes.iter().enumerate().any(|(at, pane)| at != self.focus && pane.holds()) {
                return Err(Unsaved);
            }
            // Beside the pane that has the keyboard, on the side that is
            // free or that the other pane gives up.
            let right = self.focus == 1;
            let kept = self.panes.swap_remove(self.focus);
            self.panes = vec![kept];
            if matches!(self.panes[0], Pane::Empty) {
                self.panes[0] = Pane::Session(id);
                self.focus = 0;
            } else if right {
                self.panes.insert(0, Pane::Session(id));
                self.focus = 0;
            } else {
                self.panes.push(Pane::Session(id));
                self.focus = 1;
            }
        } else {
            // A file and the changes stay where they are, the session
            // goes beside them.
            if self.panes[self.focus].beside() {
                if self.panes.len() == 1 {
                    self.panes.insert(0, Pane::Empty);
                } else {
                    self.focus = 1 - self.focus;
                }
            }
            self.panes[self.focus] = Pane::Session(id);
        }
        Ok(())
    }

    /// Makes one of the panes the one that stays, with the keyboard, when
    /// the next session is shown with `split`: also the right one, whose
    /// header asked for it.
    pub fn split_beside(&mut self, pane: usize) -> Result<(), Unsaved> {
        // The other pane is the one that will give way.
        if self.panes.iter().enumerate().any(|(at, other)| at != pane && other.holds()) {
            return Err(Unsaved);
        }
        if pane == 1 && self.panes.len() > 1 {
            self.panes.swap(0, 1);
        }
        self.focus = 0;
        Ok(())
    }

    /// Opens the diff of a project beside the first pane, or closes it if open.
    pub fn toggle_diff(&mut self, project: ProjectId) -> Result<(), Unsaved> {
        match self.diff_pane(project) {
            Some(pane) => self.close_pane(pane),
            None => self.put_beside(Pane::Diff(DiffView::new(project))),
        }
    }

    /// Opens the changes of a project at a file and a line of it,
    /// counted from one, where the changes are shown.
    pub fn show_changes_at(&mut self, project: ProjectId, file: String, line: u32) -> Result<(), Unsaved> {
        let mut view = DiffView::new(project);
        view.goto = Some(Goto { file, line, waited: false });
        self.put_beside(Pane::Diff(view))
    }

    /// Shows a file of a project where the changes are shown: in the
    /// place of a file or of the changes, or else beside the first pane.
    /// `grammar` says by which grammar a file of that path is shown in
    /// colors, if by one, and is asked only for a file that is opened.
    /// Returns whether it was not shown already. A path that does not
    /// stay inside the folder of the project is not shown at all.
    pub fn open_file(
        &mut self,
        project: ProjectId,
        path: PathBuf,
        grammar: impl FnOnce(&Path) -> Option<&'static str>,
    ) -> Result<bool, Unsaved> {
        if self.file().is_some_and(|(_, shown)| shown.project == project && shown.path == path) {
            return Ok(false);
        }
        // A file of a project is named by a path inside its folder.
        // Every file that is shown passes here, and a path from the
        // root or with a step up in it, joined to the folder, is a file
        // anywhere on the disk, to be read, changed and saved.
        let within = path.components().all(|part| matches!(part, std::path::Component::Normal(_)));
        if !within || path.as_os_str().is_empty() {
            return Ok(false);
        }
        if self.unsaved() {
            return Err(Unsaved);
        }
        self.openings += 1;
        let grammar = grammar(&path);
        self.put_beside(Pane::File(Box::new(FileView::new(project, path, self.openings, grammar))))?;
        Ok(true)
    }

    /// Puts a file or the changes where the one of them that is shown
    /// stands, so that there is never a second. Else beside the first
    /// pane, in the place of whatever was the second.
    fn put_beside(&mut self, view: Pane) -> Result<(), Unsaved> {
        if let Some(at) = self.panes.iter().position(Pane::beside) {
            if self.panes[at].holds() {
                return Err(Unsaved);
            }
            self.panes[at] = view;
            return Ok(());
        }
        // What is opened leaves the keyboard where it was. Of two
        // sessions the one that has it stays, and the other gives way:
        // to a file and to the changes alike.
        if self.panes.len() == 2 && self.focus == 1 {
            self.panes.swap(0, 1);
            self.focus = 0;
        }
        self.panes.truncate(1);
        // Not beside a pane that shows nothing: it is then all there
        // is, and the keyboard is with it.
        if matches!(self.panes[0], Pane::Empty) {
            self.panes[0] = view;
        } else {
            self.panes.push(view);
        }
        self.focus = self.focus.min(self.panes.len() - 1);
        Ok(())
    }

    /// Closes one of two panes, or a file or a diff that is the only pane.
    pub fn close_pane(&mut self, pane: usize) -> Result<(), Unsaved> {
        if pane >= self.panes.len() {
            return Ok(());
        }
        if self.panes[pane].holds() {
            return Err(Unsaved);
        }
        if self.panes.len() > 1 {
            self.panes.remove(pane);
        } else if self.panes[pane].beside() {
            self.panes[pane] = Pane::Empty;
        }
        self.focus = 0;
        Ok(())
    }

    pub fn focus_pane(&mut self, pane: usize) {
        if pane < self.panes.len() {
            self.focus = pane;
        }
    }

    /// Keeps the pane list valid: no empty pane beside a used one.
    fn tidy(&mut self) {
        if self.panes.len() > 1 {
            self.panes.retain(|p| !matches!(p, Pane::Empty));
        }
        if self.panes.is_empty() {
            self.panes.push(Pane::Empty);
        }
        self.focus = self.focus.min(self.panes.len() - 1);
    }

    /// Sessions in sidebar order: by project, then by age.
    pub fn ordered(&self) -> Vec<SessionId> {
        self.projects.iter().flat_map(|p| self.sessions_of(p.id).map(|s| s.id)).collect()
    }

    // ----- status -----

    /// Whether the user is looking at a session: it is shown and the window
    /// has the focus.
    pub fn seen(&self, id: SessionId, focused: bool) -> bool {
        focused && self.visible(id)
    }

    /// Lets sessions whose output stopped go idle or ask for attention.
    /// Returns those that changed.
    pub fn settle(&mut self, now: Instant, focused: bool) -> Vec<(SessionId, ProjectId, Status)> {
        let shown: Vec<SessionId> = (0..self.panes.len()).filter_map(|p| self.pane_session(p)).collect();
        let mut changed = Vec::new();
        for session in &mut self.sessions {
            let seen = focused && shown.contains(&session.id);
            if session.activity.settle(now, seen) {
                changed.push((session.id, session.project, session.activity.status));
            }
        }
        changed
    }

    /// The sessions whose agents are to be asked now which conversation
    /// they are in.
    pub fn asks_due(&mut self, now: Instant) -> Vec<SessionId> {
        let due = self.sessions.iter_mut().filter(|s| s.ask_at.is_some_and(|at| at <= now));
        due.map(|s| (s.ask_at.take(), s.id).1).collect()
    }

    /// The sessions whose programs' reports about themselves are to be
    /// read now.
    pub fn reads_due(&mut self, now: Instant) -> Vec<SessionId> {
        self.sessions.iter_mut().filter_map(|s| s.read_due(now).then_some(s.id)).collect()
    }

    /// When `settle`, `asks_due` or `reads_due` has something to do next.
    pub fn next_wake(&self) -> Option<Instant> {
        self.sessions.iter().flat_map(|s| [s.activity.deadline(), s.ask_at, s.read_at]).flatten().min()
    }

    /// Whether a mark moves: that of a session shown as working, or
    /// of a row below one.
    pub fn any_working(&self) -> bool {
        self.sessions.iter().any(Session::moves)
    }

    /// Projects in which a session is working right now.
    pub fn working_projects(&self) -> Vec<ProjectId> {
        let mut projects: Vec<ProjectId> =
            self.sessions.iter().filter(|s| works(s)).map(|s| s.project).collect();
        projects.sort();
        projects.dedup();
        projects
    }
}

/// What a session of a project with this name is called before numbering:
/// the name the user gave it, else the title its program set, else the
/// agent.
fn base_label_in<'a>(project: &str, session: &'a Session) -> &'a str {
    if let Some(name) = &session.name {
        return name;
    }
    let title = session.title.as_deref().map_or("", clean_title);
    // A title that only repeats the project says nothing about the session.
    if title.is_empty() || title == project { &session.agent } else { title }
}

/// Drops a leading status symbol, like the spinner some agents put in front
/// of their title. The agent's mark stands in that place already.
pub fn clean_title(title: &str) -> &str {
    let mut chars = title.chars();
    match (chars.next(), chars.next()) {
        (Some(symbol), Some(' ')) if !symbol.is_ascii() && !symbol.is_alphanumeric() => chars.as_str().trim(),
        _ => title.trim(),
    }
}

/// Whether a session is at work, by what it is shown as doing: what its
/// program says, or else what its terminal lets guess. Not one whose
/// program is not running.
fn works(session: &Session) -> bool {
    session.ended.is_none() && session.doing() == Doing::Working
}

#[cfg(test)]
mod tests {
    use super::super::edit::Edit;
    use super::super::fileview::testing::{content, shown};
    use super::super::save::Outcome;
    use super::*;

    fn workspace(sessions: usize) -> (Workspace, ProjectId, Vec<SessionId>) {
        let mut ws = Workspace::new();
        let project = ws.add_project("/p/trail".into());
        let ids = (0..sessions).map(|_| ws.new_session(project, "codex")).collect();
        (ws, project, ids)
    }

    /// What must hold after every operation.
    fn check(ws: &Workspace) {
        assert!(!ws.panes.is_empty() && ws.panes.len() <= 2);
        assert!(ws.focus < ws.panes.len());
        if ws.panes.len() > 1 {
            assert!(!ws.panes.iter().any(|p| matches!(p, Pane::Empty)));
        }
        assert!(ws.panes.iter().filter(|p| p.beside()).count() <= 1, "a file and the changes, or two of one");
        for pane in 0..ws.panes.len() {
            if let Some(id) = ws.pane_session(pane) {
                assert!(ws.session(id).is_some(), "a pane shows a removed session");
            }
        }
    }

    #[test]
    fn the_names_of_all_sessions_found_in_one_pass_are_those_found_one_by_one() {
        let mut ws = Workspace::restore(&Default::default());
        let (a, b) = (ws.add_project("/p/a".into()), ws.add_project("/p/b".into()));
        // Sessions of two projects in turn, with names that read alike
        // within a project and across the two.
        let names = [None, Some("fix"), None, Some("fix"), Some("a"), None, Some("fix")];
        for (n, name) in names.into_iter().cycle().take(40).enumerate() {
            let id =
                ws.new_session(if n % 3 == 0 { b } else { a }, if n % 5 == 0 { "codex" } else { "shell" });
            let session = ws.session_mut(id).unwrap();
            session.name = name.map(str::to_owned);
            session.title = (n % 4 == 1).then(|| "a".to_owned());
        }
        let groups = ws.grouped();
        assert_eq!(groups.iter().map(|(project, _)| project.id).collect::<Vec<_>>(), [a, b]);
        assert_eq!(groups.iter().map(|(_, sessions)| sessions.len()).sum::<usize>(), 40);
        let mut numbered = 0;
        for (project, sessions) in &groups {
            let one_by_one: Vec<SessionId> = ws.sessions_of(project.id).map(|s| s.id).collect();
            assert_eq!(sessions.iter().map(|s| s.id).collect::<Vec<_>>(), one_by_one);
            let earlier = Workspace::earlier(project, sessions);
            for (session, earlier) in sessions.iter().zip(earlier) {
                assert_eq!(ws.numbered(session, earlier), ws.label(session));
                numbered += usize::from(earlier > 0);
            }
        }
        assert!(numbered > 10, "{numbered} names with a number");
    }

    #[test]
    fn labels_prefer_name_then_title_then_agent_and_number_duplicates() {
        let (mut ws, _, ids) = workspace(4);
        let label = |ws: &Workspace, i: usize| ws.label(ws.session(ids[i]).unwrap());

        // Codex only reports the folder, which is no name for a session.
        ws.session_mut(ids[0]).unwrap().title = Some("trail".into());
        assert_eq!(
            (label(&ws, 0), label(&ws, 1), label(&ws, 2)),
            ("codex".into(), "codex 2".into(), "codex 3".into())
        );

        ws.session_mut(ids[1]).unwrap().title = Some("✳ Fix the route total".into());
        ws.session_mut(ids[2]).unwrap().title = Some("~/p/trail".into());
        ws.session_mut(ids[3]).unwrap().name = Some("review".into());
        assert_eq!(label(&ws, 1), "Fix the route total");
        assert_eq!(label(&ws, 2), "~/p/trail");
        assert_eq!(label(&ws, 3), "review");
    }

    #[test]
    fn panes_stay_valid_through_show_split_diff_and_removal() {
        let (mut ws, project, ids) = workspace(3);
        ws.show(ids[0], false).unwrap();
        check(&ws);
        ws.show(ids[1], true).unwrap();
        assert_eq!((ws.panes.len(), ws.focus), (2, 1));
        // Showing a session that is on screen only moves the focus.
        ws.show(ids[0], false).unwrap();
        assert_eq!((ws.panes.len(), ws.focus), (2, 0));

        // The diff takes the second pane, a session opened from it goes beside it.
        ws.toggle_diff(project).unwrap();
        assert!(ws.shows_diff(project) && !ws.visible(ids[1]));
        ws.focus_pane(1);
        ws.show(ids[2], false).unwrap();
        assert!(ws.shows_diff(project) && ws.pane_session(0) == Some(ids[2]));
        check(&ws);

        // A removed session leaves its pane empty rather than starting a
        // neighbour that was never run.
        ws.remove_session(ids[2]);
        assert!(ws.pane_session(0).is_none());
        check(&ws);
        assert!(ws.shows_diff(project) && ws.panes.len() == 1, "the empty pane gave way to the diff");
        ws.remove_session(ids[0]);
        ws.remove_session(ids[1]);
        check(&ws);

        // A diff that is the only pane can still be closed.
        ws.close_pane(0).unwrap();
        assert!(matches!(ws.panes[0], Pane::Empty));
        // Opened with nothing shown, it is the only pane again.
        ws.toggle_diff(project).unwrap();
        check(&ws);
        assert!(ws.shows_diff(project) && (ws.panes.len(), ws.focus) == (1, 0));
        ws.close_pane(0).unwrap();

        // A session opened from a diff that is alone goes beside it too.
        let more: Vec<SessionId> = (0..3).map(|_| ws.new_session(project, "codex")).collect();
        ws.toggle_diff(project).unwrap();
        ws.show(more[0], false).unwrap();
        check(&ws);
        assert!(ws.shows_diff(project) && ws.pane_session(0) == Some(more[0]) && ws.focus == 0);
        ws.toggle_diff(project).unwrap();

        // Beside the pane that has the keyboard, the other one gives way.
        ws.show(more[1], true).unwrap();
        assert_eq!((ws.pane_session(0), ws.pane_session(1), ws.focus), (Some(more[0]), Some(more[1]), 1));
        ws.show(more[2], true).unwrap();
        assert_eq!((ws.pane_session(0), ws.pane_session(1), ws.focus), (Some(more[2]), Some(more[1]), 0));
        ws.show(more[0], true).unwrap();
        assert_eq!((ws.pane_session(0), ws.pane_session(1), ws.focus), (Some(more[2]), Some(more[0]), 1));
        check(&ws);
        for id in more {
            ws.remove_session(id);
        }
        ws.toggle_diff(project).unwrap();
        ws.remove_project(project).unwrap();
        check(&ws);
        assert!(ws.projects.is_empty() && matches!(ws.panes[0], Pane::Empty));
    }

    #[test]
    fn a_file_and_the_changes_share_one_place() {
        let (mut ws, project, ids) = workspace(3);
        let other = ws.add_project("/p/blog".into());
        let shows = |ws: &Workspace| ws.file().map(|(pane, file)| (pane, file.path.clone()));

        // With nothing shown, a file is all there is and has the keyboard.
        assert_eq!(ws.open_file(project, "a.rs".into(), |_| None), Ok(true));
        check(&ws);
        assert_eq!((ws.panes.len(), ws.focus, ws.active_project()), (1, 0, Some(project)));
        // A session goes beside it, and the file stays.
        ws.show(ids[0], false).unwrap();
        check(&ws);
        assert_eq!((ws.pane_session(0), shows(&ws)), (Some(ids[0]), Some((1, "a.rs".into()))));
        // Another file takes the place of the one shown, the keyboard stays.
        assert_eq!(
            (ws.open_file(project, "b.rs".into(), |_| None), ws.open_file(project, "b.rs".into(), |_| None)),
            (Ok(true), Ok(false))
        );
        assert_eq!((shows(&ws), ws.focus), (Some((1, "b.rs".into())), 0));
        // The changes take the place of the file, and a file theirs.
        ws.toggle_diff(project).unwrap();
        check(&ws);
        assert!(ws.shows_diff(project) && ws.file().is_none());
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        check(&ws);
        assert!(!ws.shows_diff(project) && ws.file().is_some());

        // A session shown while the file has the keyboard goes beside it too.
        ws.focus_pane(1);
        ws.show(ids[1], false).unwrap();
        check(&ws);
        assert_eq!((ws.pane_session(0), shows(&ws).map(|(pane, _)| pane)), (Some(ids[1]), Some(1)));
        // A second session takes the place of the file. A file then
        // takes the place of the session without the keyboard, which
        // keeps running.
        ws.show(ids[2], true).unwrap();
        assert!(ws.file().is_none() && ws.panes.len() == 2 && ws.focused_session() == Some(ids[2]));
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        check(&ws);
        assert!(ws.visible(ids[2]) && !ws.visible(ids[1]) && ws.session(ids[1]).is_some());

        // The session beside it ends: the file is left alone, and the
        // changes then take its place and do not open beside it.
        for id in ids {
            ws.remove_session(id);
        }
        check(&ws);
        assert_eq!((ws.panes.len(), shows(&ws).map(|(pane, _)| pane)), (1, Some(0)));
        ws.toggle_diff(project).unwrap();
        check(&ws);
        assert!(ws.panes.len() == 1 && ws.shows_diff(project));
        // The changes of a second project take the place of the first's.
        ws.toggle_diff(other).unwrap();
        check(&ws);
        assert!(ws.panes.len() == 1 && ws.shows_diff(other) && !ws.shows_diff(project));

        // A file that is alone can be closed, and goes with its project.
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        ws.close_pane(0).unwrap();
        assert!(matches!(ws.panes[0], Pane::Empty));
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        ws.remove_project(project).unwrap();
        check(&ws);
        assert!(ws.file().is_none());
    }

    #[test]
    fn the_session_beside_a_file_is_the_one_to_come_back_to() {
        let (mut ws, project, ids) = workspace(1);
        ws.show(ids[0], false).unwrap();
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        ws.focus_pane(1);
        assert!(ws.focused_session().is_none());
        let back = Workspace::restore(&ws.saved(800.0, 600.0));
        assert!(back.focused_session().is_some());
    }

    #[test]
    fn a_followed_session_is_asked_once_after_it_went_quiet() {
        let (mut ws, _, ids) = workspace(2);
        let now = Instant::now();
        let later = |ms: u64| now + std::time::Duration::from_millis(ms);
        ws.session_mut(ids[0]).unwrap().followed = true;
        for id in &ids {
            ws.session_mut(*id).unwrap().touched(now);
            ws.session_mut(*id).unwrap().touched(later(500));
        }
        // Every session may have a report to read, a second after
        // something happened.
        assert_eq!(ws.next_wake(), Some(later(1000)));
        assert!(ws.reads_due(later(990)).is_empty());
        assert_eq!(ws.reads_due(later(1000)), ids);
        assert!(ws.reads_due(later(9000)).is_empty());
        // After that only the one whose agent can be asked wakes the app.
        assert_eq!(ws.next_wake(), Some(later(2500)));
        assert!(ws.asks_due(later(2400)).is_empty());
        assert_eq!(ws.asks_due(later(2500)), [ids[0]]);
        assert!(ws.asks_due(later(9000)).is_empty());
        assert_eq!(ws.next_wake(), None);
    }

    #[test]
    fn ids_survive_the_removal_of_an_earlier_project() {
        let mut ws = Workspace::new();
        let (a, b) = (ws.add_project("/p/a".into()), ws.add_project("/p/b".into()));
        let session = ws.new_session(b, "shell");
        ws.remove_project(a).unwrap();
        assert_eq!(ws.project(b).unwrap().name, "b");
        assert_eq!(ws.session(session).unwrap().project, b);
        assert!(ws.project(a).is_none());
        // A project with sessions is not removed.
        ws.remove_project(b).unwrap();
        assert!(ws.project(b).is_some());
    }

    #[test]
    fn saved_state_round_trips() {
        let (mut ws, _, ids) = workspace(3);
        ws.session_mut(ids[1]).unwrap().name = Some("review".into());
        ws.session_mut(ids[1]).unwrap().known = true;
        ws.session_mut(ids[2]).unwrap().ended = Some("crashed".into());
        ws.show(ids[1], false).unwrap();

        let mut saved = ws.saved(800.0, 600.0);
        // A project that is gone takes its sessions with it.
        saved.sessions.push(SavedSession {
            project: "/p/gone".into(),
            agent: "claude".into(),
            id: "x".into(),
            name: None,
            title: None,
            known: false,
            chosen: Default::default(),
        });
        let back = Workspace::restore(&saved);
        assert_eq!(back.projects.len(), 1);
        assert_eq!(back.sessions.len(), 3, "a crashed session keeps its row, an orphaned one does not");
        let front = back.session(back.focused_session().unwrap()).unwrap();
        assert_eq!((front.name.as_deref(), front.resume, front.known), (Some("review"), true, true));
        assert!(back.holds("codex", &front.uuid) && !back.holds("other", &front.uuid));
    }

    #[test]
    fn restore_rejects_duplicate_and_unsafe_conversation_ids() {
        let project: PathBuf = "/p/a".into();
        let sessions = ["one", "one", "--unsafe", ""]
            .into_iter()
            .map(|id| SavedSession {
                project: project.clone(),
                agent: "a".into(),
                id: id.into(),
                name: None,
                title: None,
                known: true,
                chosen: Default::default(),
            })
            .collect();
        let saved = Saved { projects: vec![project], sessions, ..Default::default() };
        let ws = Workspace::restore(&saved);
        assert_eq!(ws.sessions.len(), 1);
        assert_eq!(ws.sessions[0].uuid, "one");
    }

    #[test]
    fn splitting_an_empty_workspace_has_no_empty_neighbor() {
        let (mut ws, _, ids) = workspace(1);
        ws.show(ids[0], true).unwrap();
        check(&ws);
        assert_eq!(ws.panes.len(), 1);
        assert_eq!(ws.focused_session(), Some(ids[0]));
    }

    #[test]
    fn active_agent_disambiguates_conversation_namespaces() {
        let mut ws = Workspace::new();
        let project = ws.add_project("/p/a".into());
        let first = ws.add_session(project, "first", "shared-id".into(), false);
        ws.add_session(project, "second", "shared-id".into(), false);
        ws.show(first, false).unwrap();
        let back = Workspace::restore(&ws.saved(800.0, 600.0));
        assert_eq!(back.session(back.focused_session().unwrap()).unwrap().agent, "first");
    }

    #[test]
    fn a_file_leaves_the_keyboard_with_the_session_that_has_it() {
        let (mut ws, project, ids) = workspace(2);
        ws.show(ids[0], false).unwrap();
        ws.show(ids[1], true).unwrap();
        assert_eq!(ws.focused_session(), Some(ids[1]));
        // The session at the right has the keyboard: it stays, and the
        // other one gives way to the file.
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        check(&ws);
        assert_eq!(ws.focused_session(), Some(ids[1]));
        assert!(ws.file().is_some() && !ws.visible(ids[0]));
        // With the keyboard at the left nothing moves.
        ws.close_pane(1).unwrap();
        ws.show(ids[0], true).unwrap();
        ws.focus_pane(0);
        let left = ws.pane_session(0);
        ws.open_file(project, "b.rs".into(), |_| None).unwrap();
        assert_eq!((ws.pane_session(0), ws.focus), (left, 0));
    }

    #[test]
    fn the_changes_open_at_the_file_and_the_line_that_were_asked_for() {
        use git::{Kind, Line};
        let line = |kind, new: u32, text: &str| Line { kind, old: 0, new, text: text.into() };
        let lines = |prefix: &str| {
            let mut lines = vec![line(Kind::File, 0, &format!("{prefix}a.rs"))];
            lines.extend((1..=20).map(|n| line(Kind::Add, n, "x")));
            lines.push(line(Kind::File, 0, &format!("{prefix}src/b.rs")));
            lines.extend((1..=5).map(|n| line(Kind::Context, n, "y")));
            lines.push(line(Kind::Del, 0, "gone"));
            lines.extend((6..=30).map(|n| line(Kind::Context, n, "y")));
            lines.extend((0..4).map(|_| line(Kind::Del, 0, "gone at the end")));
            lines.push(line(Kind::Gone, 0, &format!("{prefix}old.rs")));
            lines.push(line(Kind::Del, 0, "all of it"));
            lines.push(line(Kind::File, 0, "elsewhere/c.rs"));
            lines
        };
        let (mut ws, project, _) = workspace(0);
        ws.open_file(project, "src/b.rs".into(), |_| None).unwrap();
        // The changes take the place of the file, and scroll when the
        // answer of git is there: three rows above the line, so that
        // what went before it is seen.
        ws.show_changes_at(project, "src/b.rs".into(), 6).unwrap();
        check(&ws);
        assert!(ws.file().is_none() && ws.shows_diff(project));
        let view = ws.diff_mut(project).unwrap();
        view.set_lines(String::new(), lines(""));
        // The row of the file is 21, its line 6 stands at row 28.
        assert_eq!(view.scroll, 25);
        // The answer after that leaves the view where it was scrolled to.
        view.scroll_by(-10);
        view.set_lines(String::new(), lines(""));
        assert_eq!(view.scroll, 15);

        let at = |prefix: &str, file: &str, line: u32| {
            let mut view = DiffView::new(project);
            view.goto = Some(Goto { file: file.into(), line, waited: false });
            view.set_lines(prefix.into(), lines(prefix));
            view.scroll
        };
        assert_eq!(at("", "src/b.rs", 1), 21, "never above the row of the file");
        assert_eq!(at("", "a.rs", 10), 7);
        assert_eq!(at("", "gone.rs", 3), 0, "a file that is not among them");
        // Lines that went at the end of a file are behind every line it
        // has: the view goes to the last of its rows, not to its first.
        assert_eq!(at("", "src/b.rs", 99), 53);
        // In a folder inside a repository the files are named from the
        // top of it, and asked for from the folder.
        assert_eq!(at("app/", "src/b.rs", 6), 25);
        assert_eq!(at("app/", "app/src/b.rs", 6), 0);

        // A row leads to a file only while it names the same file, only
        // to one that is there, and only to one in the folder.
        let mut view = DiffView::new(project);
        view.set_lines("app/".into(), lines("app/"));
        let tag = view.leads(21).unwrap();
        assert_eq!(view.file_at(21, tag), Some("src/b.rs"));
        assert_eq!(view.file_at(0, tag), None, "another file stands there");
        assert_eq!((view.leads(22), view.file_at(22, tag)), (None, None), "no row of a file");
        assert_eq!(view.leads(57), None, "a file that was deleted");
        assert_eq!(view.leads(59), None, "a file elsewhere in the repository");
        assert_eq!(view.file_at(999, tag), None);
    }

    #[test]
    fn an_answer_that_was_asked_for_before_the_click_does_not_use_up_the_place_to_go_to() {
        use git::{Kind, Line};
        let line = |kind, new: u32, text: &str| Line { kind, old: 0, new, text: text.into() };
        let with = |file: bool| {
            let mut lines: Vec<Line> = (1..=40).map(|n| line(Kind::Context, n, "other")).collect();
            lines.insert(0, line(Kind::File, 0, "other.rs"));
            if file {
                lines.push(line(Kind::File, 0, "late.rs"));
                lines.extend((1..=9).map(|n| line(Kind::Add, n, "x")));
            }
            lines
        };
        let (mut ws, project, _) = workspace(0);
        ws.show_changes_at(project, "late.rs".into(), 1).unwrap();
        let view = ws.diff_mut(project).unwrap();
        // The answer that was under way does not know the file yet.
        view.set_lines(String::new(), with(false));
        assert_eq!(view.scroll, 0);
        view.set_lines(String::new(), with(true));
        assert_eq!(view.scroll, 41);
        // It waits for one answer more and no longer, and not for
        // someone who has scrolled meanwhile.
        ws.show_changes_at(project, "never.rs".into(), 1).unwrap();
        let view = ws.diff_mut(project).unwrap();
        view.set_lines(String::new(), with(false));
        view.set_lines(String::new(), with(false));
        assert!(view.goto.is_none());
        ws.show_changes_at(project, "late.rs".into(), 1).unwrap();
        let view = ws.diff_mut(project).unwrap();
        view.set_lines(String::new(), with(false));
        view.scroll_by(5);
        view.set_lines(String::new(), with(true));
        assert_eq!(view.scroll, 5);
    }

    #[test]
    fn every_opening_of_a_file_has_a_number_of_its_own() {
        // The same file opened, left for another and opened again: a path
        // and a state do not tell the two openings apart, their numbers do.
        let (mut ws, project, _) = workspace(0);
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        let first = ws.file().unwrap().1.edition();
        assert_eq!(ws.open_file(project, "a.rs".into(), |_| None), Ok(false), "shown already");
        assert_eq!(ws.file().unwrap().1.edition(), first, "which is no opening");
        ws.open_file(project, "b.rs".into(), |_| None).unwrap();
        let between = ws.file().unwrap().1.edition();
        ws.open_file(project, "a.rs".into(), |_| None).unwrap();
        let second = ws.file().unwrap().1.edition();
        assert!(first.opening < between.opening && between.opening < second.opening);
    }

    #[test]
    fn a_file_opens_with_the_grammar_its_name_was_given() {
        let (mut ws, project, _) = workspace(0);
        // Asked by the path of the file, once for each that is opened.
        let asked = std::cell::RefCell::new(Vec::new());
        let named = |grammar: Option<&'static str>| {
            let asked = &asked;
            move |path: &Path| {
                asked.borrow_mut().push(path.to_owned());
                grammar
            }
        };
        ws.open_file(project, "src/a.rs".into(), named(Some("Rust"))).unwrap();
        let view = ws.file_mut().unwrap();
        view.read(Ok(content("grammar", "fn a() {}\n")), 20);
        assert_eq!(view.colors.grammar(view.text().unwrap()), Some("Rust"));
        assert_eq!(*asked.borrow(), [PathBuf::from("src/a.rs")]);
        let marks = git::Marks { changed: vec![(0, 1)], ..Default::default() };
        assert_eq!(
            view.take_marks(view.edition(), view.stamp(), Some(marks)),
            crate::core::Taken::Marked(true)
        );
        // Not for the file that is shown already, and not for one that
        // is not opened because the one shown has changes.
        assert_eq!(ws.open_file(project, "src/a.rs".into(), named(None)), Ok(false));
        assert!(ws.file_mut().unwrap().edit(Edit::Type("x"), Instant::now()));
        assert_eq!(ws.open_file(project, "b.txt".into(), named(None)), Err(Unsaved));
        assert_eq!(asked.borrow().len(), 1);
        // The next file has its own, or none, and nothing of the one
        // before: not its text, and not what was marked in it.
        let view = ws.file_mut().unwrap();
        view.let_go();
        ws.open_file(project, "b.txt".into(), named(None)).unwrap();
        assert_eq!(asked.borrow().len(), 2);
        let view = ws.file_mut().unwrap();
        assert!(view.text().is_none() && *view.marks() == git::Marks::default());
        view.read(Ok(content("grammar", "text\n")), 20);
        assert_eq!(view.colors.grammar(view.text().unwrap()), None);
    }

    #[test]
    fn a_state_is_restored_up_to_a_bound_on_its_projects_and_sessions() {
        use crate::core::state::{Saved, SavedSession};
        // A state is another program's to write as well, and a later
        // start of codn hands folders over without end. Every project is
        // held against every other where one is added: forty thousand of
        // them kept the window away for most of a minute, at every start.
        let projects: Vec<PathBuf> = (0..MAX_PROJECTS + 50).map(|n| format!("/p/{n}").into()).collect();
        let session = |n: usize| SavedSession {
            project: "/p/0".into(),
            agent: "shell".into(),
            id: format!("00000000-0000-4000-8000-{n:012}"),
            name: None,
            title: None,
            known: false,
            chosen: Default::default(),
        };
        let sessions = (0..MAX_SESSIONS + 50).map(session).collect();
        let ws = Workspace::restore(&Saved { projects, sessions, ..Saved::default() });
        assert_eq!((ws.projects.len(), ws.sessions.len()), (MAX_PROJECTS, MAX_SESSIONS));
        assert!(!ws.room() && ws.projects[0].path == Path::new("/p/0"), "the first of them, in their order");
    }

    #[test]
    fn of_two_sessions_the_one_with_the_keyboard_stays_beside_what_is_opened() {
        // A file took the place of the session without the keyboard, the
        // changes that of the one with it, which was gone with the
        // keyboard on what was opened.
        let opened: [fn(&mut Workspace, ProjectId); 3] = [
            |ws, project| assert_eq!(ws.open_file(project, "a.rs".into(), |_| None), Ok(true)),
            |ws, project| ws.toggle_diff(project).unwrap(),
            |ws, project| ws.show_changes_at(project, "a.rs".into(), 1).unwrap(),
        ];
        for open in opened {
            for keyboard in [0, 1] {
                let (mut ws, project, ids) = workspace(2);
                ws.show(ids[0], false).unwrap();
                ws.show(ids[1], true).unwrap();
                ws.focus_pane(keyboard);
                let stays = ws.focused_session().unwrap();
                open(&mut ws, project);
                check(&ws);
                assert_eq!((ws.panes.len(), ws.focus, ws.pane_session(0)), (2, 0, Some(stays)));
                assert!(ws.panes[1].beside() && ws.focused_session() == Some(stays));
            }
        }
    }

    #[test]
    fn sessions_whose_output_stopped_go_quiet_or_ask_to_be_looked_at() {
        let (mut ws, project, ids) = workspace(2);
        let (shown, other) = (ids[0], ids[1]);
        ws.show(shown, false).unwrap();
        let begun = Instant::now();
        let ms = |ms: u64| begun + std::time::Duration::from_millis(ms);
        // Both print for three seconds, which is work that counts.
        for id in [shown, other] {
            for tenth in 0..=30 {
                ws.session_mut(id).unwrap().output(ms(100 * tenth));
            }
        }
        assert!(ws.any_working());
        assert!(ws.settle(ms(4400), true).is_empty(), "not before they were quiet for long enough");
        // The one that is looked at goes quiet, the other asks.
        let settled = ws.settle(ms(4500), true);
        assert_eq!(settled, [(shown, project, Status::Idle), (other, project, Status::Attention)]);
        assert!(!ws.any_working());
        assert!(ws.settle(ms(9000), true).is_empty(), "once");
        // With the window not in front, also the one that is shown asks.
        for tenth in 0..=30 {
            ws.session_mut(shown).unwrap().output(ms(10_000 + 100 * tenth));
        }
        assert_eq!(ws.settle(ms(14_500), false), [(shown, project, Status::Attention)]);
    }

    #[test]
    fn the_rows_of_a_state_that_are_passed_over_cost_no_more_than_those_kept() {
        use crate::core::state::{Saved, SavedSession};
        // A row that is not kept was held against every session that
        // is, and against every project: a state of 15 MiB, which is
        // less than is read, kept the window away for ten seconds.
        let session = |project: &str, n: usize| SavedSession {
            project: project.into(),
            agent: "shell".into(),
            id: format!("00000000-0000-4000-8000-{n:012}"),
            name: None,
            title: None,
            known: false,
            chosen: Default::default(),
        };
        let projects: Vec<PathBuf> = (0..MAX_PROJECTS).map(|n| format!("/p/{n}").into()).collect();
        // The sessions that are kept first, then rows of a project that
        // is not there, rows of ids that are held, and rows of no id.
        let mut sessions: Vec<SavedSession> = (0..MAX_SESSIONS - 1).map(|n| session("/p/7", n)).collect();
        sessions.extend((0..100_000).map(|n| session("/gone", MAX_SESSIONS + n)));
        sessions.extend((0..400_000).map(|n| session("/p/7", MAX_SESSIONS - 2 - n % 1000)));
        sessions.extend((0..50_000).map(|_| SavedSession { id: "no id".into(), ..session("/p/7", 0) }));
        sessions.push(session("/p/999", MAX_SESSIONS + 200_000));
        let begun = Instant::now();
        let ws = Workspace::restore(&Saved { projects, sessions, ..Saved::default() });
        let took = begun.elapsed();
        assert_eq!((ws.projects.len(), ws.sessions.len()), (MAX_PROJECTS, MAX_SESSIONS));
        assert_eq!(ws.sessions.last().map(|s| s.project), ws.project_at(Path::new("/p/999")).map(|p| p.id));
        // It took 12.6 s where it is 0.4 s with the making of the
        // rows, in a build that is not optimized.
        assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn a_row_of_the_changes_with_a_name_that_was_no_text_leads_nowhere() {
        use git::{Kind, Line};
        let (mut ws, project, _) = workspace(0);
        ws.toggle_diff(project).unwrap();
        let view = ws.diff_mut(project).unwrap();
        let named = |text: &str| Line { kind: Kind::File, old: 0, new: 1, text: text.into() };
        view.set_lines(String::new(), vec![named("odd\u{fffd}.txt"), named("fine.txt")]);
        assert!(view.leads(0).is_none() && view.leads(1).is_some());
    }

    #[test]
    fn a_row_of_the_changes_that_names_a_file_outside_leads_nowhere() {
        use git::{Kind, Line};
        // An index can be made to name files that are not the
        // repository's. A click on such a row opened that file to be
        // changed: a key of a user, anywhere on the disk.
        let (mut ws, project, _) = workspace(0);
        ws.toggle_diff(project).unwrap();
        let view = ws.diff_mut(project).unwrap();
        let named = |text: &str| Line { kind: Kind::File, old: 0, new: 1, text: text.into() };
        let names = ["../outside.txt", "/etc/hosts", "a/../../b.txt", "./c.txt", "fine/d.txt"];
        view.set_lines(String::new(), names.into_iter().map(named).collect());
        let leads: Vec<bool> = (0..names.len()).map(|row| view.leads(row).is_some()).collect();
        assert_eq!(leads, [false, false, false, false, true]);
        // The project is a folder inside its repository, and the names
        // are cut where that folder ends: what is left is held against
        // the same, and so is a name with nothing between two slashes,
        // which is a path from the root once it is cut.
        let names = [
            "app//etc/hosts",
            "app/",
            "app",
            "app/../outside.txt",
            "app/./c.txt",
            "app/a//b.txt",
            "other/d.txt",
            "app/fine/d.txt",
        ];
        view.set_lines("app/".into(), names.into_iter().map(named).collect());
        let led: Vec<Option<&str>> =
            (0..names.len()).map(|row| view.leads(row).and_then(|check| view.file_at(row, check))).collect();
        assert_eq!(led, [None, None, None, None, None, None, None, Some("fine/d.txt")]);
        // Also where the folder is said without the slash behind it:
        // what is left would begin with one.
        view.set_lines("app".into(), vec![named("app/fine/d.txt")]);
        assert!(view.leads(0).is_none());
        // And no file is opened in a pane by a path that is not one of
        // names inside the folder of its project, whoever asks.
        for outside in ["/etc/hosts", "../outside.txt", "a/../../b.txt", "./c.txt", ""] {
            assert_eq!(ws.open_file(project, outside.into(), |_| None), Ok(false), "{outside}");
            assert!(ws.file().is_none());
        }
        assert_eq!(ws.open_file(project, "fine/d.txt".into(), |_| None), Ok(true));
    }

    #[test]
    fn no_row_of_operations_makes_a_file_with_changes_disappear() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut dice = move |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 16) as usize % sides
        };
        // The panes and the keyboard, to see that a refusal changed nothing.
        let shape = |ws: &Workspace| {
            let panes: Vec<String> = ws
                .panes
                .iter()
                .map(|pane| match pane {
                    Pane::Empty => "empty".to_owned(),
                    Pane::Session(id) => format!("session {}", id.0),
                    Pane::Diff(view) => format!("changes {}", view.project.0),
                    Pane::File(view) => format!("file {:?} {:?}", view.path, view.reading()),
                })
                .collect();
            (panes, ws.focus)
        };
        let now = Instant::now();
        for _ in 0..40 {
            let (mut ws, first, _) = workspace(3);
            let second = ws.add_project("/p/other".into());
            ws.new_session(second, "codex");
            let (mut refused, mut files) = (0, 0);
            for _ in 0..300 {
                // Now and then a file is opened and typed into, and now
                // and then its changes are saved or let go.
                match (dice(6), ws.file_mut()) {
                    (0, None) => {
                        let project = [first, second][dice(2)];
                        if ws.project(project).is_some()
                            && ws.open_file(project, "a.rs".into(), |_| None) == Ok(true)
                        {
                            let view = ws.file_mut().unwrap();
                            view.read(Ok(content("rows", "one\n")), 20);
                            assert!(view.edit(Edit::Type("typed "), now));
                            files += 1;
                        }
                    }
                    (1, Some(view)) if dice(4) == 0 => view.let_go(),
                    (2, Some(view)) if dice(4) == 0 => {
                        // A file that is still being read has nothing to save.
                        if let Some((of, _)) = view.job(Path::new("/p"), false) {
                            view.saved(of, Outcome::Saved { stamp: None, digest: 0, flushed: true });
                        }
                    }
                    (3, Some(view)) => drop(view.edit(Edit::Type("x"), now)),
                    _ => {}
                }
                let held = ws.file().filter(|(_, view)| view.holds()).map(|(_, view)| shown(view));
                let before = shape(&ws);
                let ids = ws.ordered();
                let id = (!ids.is_empty()).then(|| ids[dice(ids.len())]);
                let (pane, project) = (dice(2), [first, second][dice(2)]);
                let done = match (dice(11), id) {
                    (0, Some(id)) => ws.show(id, false),
                    (1, Some(id)) => ws.show(id, true),
                    (2, _) => ws.split_beside(pane),
                    (3, _) if ws.project(project).is_some() => ws.toggle_diff(project),
                    (4, _) if ws.project(project).is_some() => ws.show_changes_at(project, "a.rs".into(), 1),
                    (5, _) if ws.project(project).is_some() => {
                        ws.open_file(project, "b.rs".into(), |_| None).map(drop)
                    }
                    (6, _) => ws.close_pane(pane),
                    (7, Some(id)) => {
                        ws.remove_session(id);
                        Ok(())
                    }
                    (8, _) => ws.remove_project(project),
                    (9, _) => {
                        ws.focus_pane(pane);
                        Ok(())
                    }
                    (10, _) if ws.project(project).is_some() => {
                        ws.new_session(project, "codex");
                        Ok(())
                    }
                    _ => Ok(()),
                };
                check(&ws);
                // What was refused changed nothing, and nothing is ever
                // refused but for a file that holds changes.
                if done.is_err() {
                    refused += 1;
                    assert!(held.is_some());
                    assert_eq!(shape(&ws), before);
                }
                if let Some(text) = held {
                    let (_, view) = ws.file().expect("the file with changes is gone");
                    assert!(view.holds() && shown(view) == text);
                }
            }
            // The rows did hold files with changes, and were refused.
            assert!(refused > 0 && files > 0, "{refused} {files}");
        }
    }

    #[test]
    fn a_file_with_changes_keeps_its_place_until_they_are_saved_or_let_go() {
        let (mut ws, project, ids) = workspace(2);
        ws.show(ids[0], false).unwrap();
        assert_eq!(ws.open_file(project, "a.rs".into(), |_| None), Ok(true));
        let view = ws.file_mut().unwrap();
        view.read(Ok(content("place", "one\n")), 20);
        // Unchanged, it gives way to whatever comes.
        assert!(!view.holds() && !ws.unsaved());
        assert_eq!(ws.open_file(project, "b.rs".into(), |_| None), Ok(true));
        let view = ws.file_mut().unwrap();
        view.read(Ok(content("place", "one\n")), 20);
        assert!(view.edit(Edit::Type("x"), Instant::now()));
        assert!(ws.unsaved());
        // Another file, the changes, a session beside the other one, the
        // pane closed, the project removed: none of them is done.
        assert_eq!(ws.open_file(project, "c.rs".into(), |_| None), Err(Unsaved));
        assert_eq!(ws.toggle_diff(project), Err(Unsaved));
        assert_eq!(ws.show_changes_at(project, "b.rs".into(), 1), Err(Unsaved));
        assert_eq!((ws.show(ids[1], true), ws.split_beside(0)), (Err(Unsaved), Err(Unsaved)));
        assert_eq!(ws.close_pane(1), Err(Unsaved));
        // The file that is shown is asked for again without a refusal.
        assert_eq!(ws.open_file(project, "b.rs".into(), |_| None), Ok(false));
        // A session that is shown goes beside the file, and one that
        // ends leaves it alone.
        ws.show(ids[1], false).unwrap();
        assert_eq!((ws.pane_session(0), ws.file().map(|(pane, _)| pane)), (Some(ids[1]), Some(1)));
        ws.remove_session(ids[1]);
        ws.remove_session(ids[0]);
        assert!(ws.unsaved() && ws.panes.len() == 1);
        assert_eq!(ws.remove_project(project), Err(Unsaved));
        // Let go, for the text as it is: one more key and it holds again.
        let view = ws.file_mut().unwrap();
        view.let_go();
        assert!(!view.holds() && view.changed());
        assert!(view.edit(Edit::Type("y"), Instant::now()) && view.holds());
        view.let_go();
        assert_eq!(ws.close_pane(0), Ok(()));
        assert!(ws.file().is_none());
    }

    #[test]
    fn a_session_that_is_gone_is_not_shown() {
        let (mut ws, _, ids) = workspace(2);
        ws.show(ids[0], false).unwrap();
        ws.remove_session(ids[1]);
        assert_eq!(ws.show(ids[1], true), Ok(()));
        check(&ws);
        assert_eq!((ws.panes.len(), ws.pane_session(0)), (1, Some(ids[0])));
        assert_eq!(ws.show(ids[1], false), Ok(()));
        check(&ws);
        assert_eq!(ws.pane_session(0), Some(ids[0]));
    }
}
