//! The list that adds a project or starts a session, as far as the app has
//! a part in it: opening it, what its rows lead to, and the workers that
//! fill it. The list itself is `ui::picker`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use super::{App, FILESYSTEM, UserEvent};
use crate::core::config::{Chosen, expand};
use crate::core::discover::{self, Row};
use crate::core::history::{self, Conversation};
use crate::core::shell;
use crate::core::{ProjectId, spawn_bounded};
use crate::ui::picker::{Known, Launch, Outcome, Picker, Starter};

impl App {
    // ----- the open list -----

    /// Runs `f` on the open picker, with what it needs to know.
    pub(super) fn with_picker<R>(&mut self, f: impl FnOnce(&mut Picker, &Known) -> R) -> Option<R> {
        let projects = &self.ws.projects;
        let added = |path: &Path| projects.iter().any(|p| p.path == path);
        let known = Known { repos: &self.repos, added: &added };
        self.dirty = true;
        let result = self.ui.picker.as_mut().map(|picker| f(picker, &known));
        self.request_browse();
        result
    }

    /// Like `with_picker`, for things that can close the picker.
    pub(super) fn picker_outcome(&mut self, f: impl FnOnce(&mut Picker, &Known) -> Outcome) {
        match self.with_picker(f) {
            Some(Outcome::Close) => self.ui.picker = None,
            Some(Outcome::Add(path)) => {
                self.ui.picker = None;
                self.add_projects(vec![path]);
            }
            Some(Outcome::Resume(project, conversation)) => {
                self.ui.picker = None;
                self.resume_earlier(project, conversation);
            }
            Some(Outcome::Start(project, agent, chosen)) => {
                self.ui.picker = None;
                self.new_session(project, &agent, chosen, false);
            }
            Some(Outcome::Stay) | None => {}
        }
        self.request_browse();
    }

    /// The background scan came back.
    pub(super) fn rescanned(&mut self, repos: Vec<discover::Repo>) {
        self.repos = repos;
        self.with_picker(|picker, known| picker.rescanned(known));
    }

    pub(super) fn start_adding(&mut self) {
        self.ui.picker = Some(Picker::default());
        self.with_picker(|picker, known| picker.refresh(known));
        // Show what is known at once and look again in the background.
        let proxy = self.proxy.clone();
        let roots = self.cfg.roots.iter().map(|r| expand(r)).collect();
        discover::spawn(roots, move |repos| {
            let _ = proxy.send_event(UserEvent::Repos(repos));
        });
    }

    // ----- starting and continuing -----

    /// Opens the list that starts a session in a project: the agents, and
    /// below them the conversations they have on disk for it. Those are
    /// looked for on a thread and arrive as an event.
    pub(super) fn start_launch(&mut self, project: ProjectId) {
        let Some(folder) = self.ws.project(project).map(|p| p.path.clone()) else { return };
        let options =
            |agent: &str| self.cfg.agent(agent).map(|agent| agent.options.clone()).unwrap_or_default();
        let starter = |(agent, chosen): (String, Chosen)| Starter { options: options(&agent), agent, chosen };
        let starters: Vec<Starter> = self.usual(project).into_iter().map(starter).collect();
        if starters.is_empty() {
            return;
        }
        self.history_request = self.history_request.wrapping_add(1);
        let request = self.history_request;
        let installed = self.cfg.agents.iter().filter(|agent| agent.installed);
        // A history that is asked of the agent's program runs it as its
        // sessions are run: on the search path they get.
        let path = shell::search_path(self.cfg.path.as_deref());
        let agents: Vec<_> = installed
            .filter_map(|agent| {
                let mut history = agent.resumable()?.clone();
                history.program = Some((agent.cmd.clone().into(), path.clone()));
                Some((agent.name.clone(), history))
            })
            .collect();
        let launch = Launch { project, starters, found: None, setting: None, all: false };
        self.ui.picker = Some(Picker { launch: Some(launch), ..Picker::default() });
        self.with_picker(|picker, known| picker.refresh(known));
        if agents.is_empty() {
            return;
        }
        let proxy = self.proxy.clone();
        if !history::spawn(move || {
            let _ = proxy.send_event(UserEvent::Earlier(project, request, history::list(&agents, &folder)));
        }) {
            self.earlier_found(project, request, Vec::new());
            self.ui.notice = Some("Conversation history is busy; try again shortly.".into());
        }
    }

    pub(super) fn earlier_found(&mut self, project: ProjectId, request: u64, found: Vec<Conversation>) {
        if request != self.history_request {
            return;
        }
        let ws = &self.ws;
        let launch = self.ui.picker.as_mut().and_then(|picker| picker.launch.as_mut());
        let Some(launch) = launch.filter(|launch| launch.project == project) else { return };
        // What is open already has its row.
        launch.found = Some(found.into_iter().filter(|c| !ws.holds(&c.agent, &c.id)).collect());
        // While options are being set, the list waits behind them.
        if launch.setting.is_none() {
            self.with_picker(|picker, known| picker.rescanned(known));
        }
    }

    /// Continues a conversation from disk in a session of its own.
    fn resume_earlier(&mut self, project: ProjectId, conversation: Conversation) {
        let Conversation { agent, id: uuid, title, .. } = conversation;
        if self.ws.project(project).is_none() || self.cfg.agent(&agent).is_none_or(|a| !a.installed) {
            return;
        }
        // It may have been opened while the list stood there.
        let open = self.ws.sessions.iter().find(|s| s.agent == agent && s.uuid == uuid).map(|s| s.id);
        let id = open.unwrap_or_else(|| {
            // It goes on with what the agent was last started with here.
            let usual = self.usual(project).into_iter().find(|(name, _)| *name == agent);
            let chosen = usual.map(|(_, chosen)| chosen).unwrap_or_default();
            let id = self.ws.add_session(project, &agent, uuid, true);
            if let Some(session) = self.ws.session_mut(id) {
                session.known = true;
                session.title = Some(title);
                session.chosen = chosen.clone();
            }
            self.remember(project, &agent, chosen);
            id
        });
        self.open(id, false);
    }

    // ----- projects -----

    pub(super) fn pick(&mut self, row: usize) {
        self.picker_outcome(|picker, known| picker.pick(row, known));
    }

    pub(super) fn browse(&mut self) {
        self.with_picker(|picker, known| picker.set_text("~/".into(), known));
    }

    /// Adds folders as projects: from the picker, or from a later start of
    /// codn that handed them over.
    pub(super) fn add_projects(&mut self, folders: Vec<PathBuf>) {
        if folders.is_empty() {
            return;
        }
        let proxy = self.proxy.clone();
        static PROJECTS: AtomicUsize = AtomicUsize::new(0);
        if !spawn_bounded(&PROJECTS, FILESYSTEM, move || {
            let (folders, notice) = normalize_projects(folders);
            let _ = proxy.send_event(UserEvent::Projects(folders, notice));
        }) {
            self.ui.notice = Some("Project folders are busy; try again shortly.".into());
            self.dirty = true;
        }
    }

    pub(super) fn projects_added(&mut self, folders: Vec<PathBuf>, notice: Option<String>) {
        if notice.is_some() {
            self.ui.notice = notice;
        }
        for path in folders {
            if self.ws.project_at(&path).is_none() && !self.ws.room() {
                self.ui.notice = Some("There is no room for another project: remove one first.".into());
                break;
            }
            let project = self.ws.add_project(path);
            self.refresh_git(project);
        }
        self.fit();
        self.save();
    }

    // ----- folders -----
    //
    // Directory reads stay on bounded workers; obsolete replies are ignored.

    /// Has the folders read that the open list is to show, if it browses a
    /// path and they are neither there nor on their way.
    fn request_browse(&mut self) {
        let query = self.ui.picker.as_ref().and_then(|picker| picker.browsing()).map(str::to_owned);
        let Some(text) = query else {
            self.browse_pending = None;
            return;
        };
        let ready = self
            .ui
            .picker
            .as_ref()
            .and_then(|picker| picker.browsed.as_ref())
            .is_some_and(|(query, _)| query == &text);
        if ready || self.browse_pending.as_ref().is_some_and(|(_, query)| query == &text) {
            return;
        }
        if let Some(picker) = &mut self.ui.picker {
            picker.browse_busy = false;
        }
        self.browse_request = self.browse_request.wrapping_add(1);
        let request = self.browse_request;
        let proxy = self.proxy.clone();
        let query = text.clone();
        self.browse_pending = Some((request, text));
        // Workers of their own: a folder that does not answer must not take
        // those that ask the agents which conversation they are in.
        static BROWSES: AtomicUsize = AtomicUsize::new(0);
        if !spawn_bounded(&BROWSES, FILESYSTEM, move || {
            let rows = discover::browse(&query);
            let _ = proxy.send_event(UserEvent::Browsed(request, query, rows));
        }) {
            self.browse_pending = None;
            if let Some(picker) = &mut self.ui.picker {
                picker.browse_busy = true;
            }
            self.dirty = true;
        }
    }

    pub(super) fn browsed(&mut self, request: u64, text: String, rows: Vec<Row>) {
        if self.browse_pending.as_ref() != Some(&(request, text.clone())) {
            return;
        }
        self.browse_pending = None;
        let Some(picker) = self.ui.picker.as_mut() else { return };
        let projects = &self.ws.projects;
        let added = |path: &Path| projects.iter().any(|project| project.path == path);
        picker.browsed(text, rows, &Known { repos: &self.repos, added: &added });
        self.dirty = true;
    }
}

fn normalize_projects(folders: Vec<PathBuf>) -> (Vec<PathBuf>, Option<String>) {
    let mut normalized = Vec::new();
    let mut notice = None;
    for path in folders {
        match std::fs::canonicalize(&path) {
            Ok(path) if path.is_dir() && path.to_str().is_some() => {
                if !normalized.contains(&path) {
                    normalized.push(path);
                }
            }
            // In codn's words first, and of the path its last name,
            // short and on one line: a later start hands a path over,
            // of any bytes and four thousand of them, and all of it
            // stood here before the words, in the color of a mistake.
            _ => {
                let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                let name: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
                notice = Some(format!("Not a usable project folder: {name}"));
            }
        }
    }
    (normalized, notice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_normalization_rejects_files_and_deduplicates_paths() {
        let root = std::env::temp_dir().join(format!("codn-project-normalize-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("file");
        std::fs::write(&file, "x").unwrap();
        let (paths, notice) = normalize_projects(vec![root.clone(), root.join("."), file]);
        assert_eq!(paths, [std::fs::canonicalize(&root).unwrap()]);
        assert_eq!(notice.as_deref(), Some("Not a usable project folder: file"));
        std::fs::remove_dir_all(root).unwrap();
        // What a later start hands over is any text: codn's words stand
        // first, and of the path a short name on one line.
        let handed = PathBuf::from(format!("/nowhere/{}\nAll is well.\u{1b}[31m", "x".repeat(4000)));
        let (_, notice) = normalize_projects(vec![handed]);
        let notice = notice.unwrap();
        assert!(notice.starts_with("Not a usable project folder: xxx") && notice.chars().count() <= 80);
        assert!(!notice.chars().any(char::is_control));
    }
}
