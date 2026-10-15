//! Sessions: starting, showing, closing, what their terminals report, and
//! which conversations they are in.

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use winit::window::UserAttentionType;

use super::{App, Asked, FILESYSTEM, UserEvent};
use crate::action::Action;
use crate::core::config::Chosen;
use crate::core::history;
use crate::core::state::{Last, MAX_LAST};
use crate::core::workspace::clean_title;
use crate::core::{Doing, Exit, Pane, ProjectId, Reported, SessionId, StartGate, reported, spawn_bounded};
use crate::log::log;
use crate::platform::notify;
use crate::term::{self, Grid, Options, Terminal};
use crate::ui::sidebar;

impl App {
    /// Brings terminals in line with the panes: fits every visible grid to
    /// its pane and starts the processes of sessions that became visible.
    pub(super) fn fit(&mut self) {
        self.dirty = true;
        if self.ui.width == 0.0 {
            return;
        }
        let layouts = self.ui.panes(self.ws.panes().len());
        for (pane, layout) in layouts.iter().enumerate() {
            let Some(id) = self.ws.pane_session(pane) else { continue };
            let grid = self.ui.grid(layout);
            let focused = self.ui.focused;
            let Some(session) = self.ws.session_mut(id) else { continue };
            if focused {
                session.seen();
            }
            match &mut session.term {
                Some(term) => term.resize(grid),
                None if session.ended.is_none() => {
                    if self.quit {
                        continue;
                    }
                    // An agent that was not found may only be missing from
                    // the search path codn was started with, so the shell
                    // is asked for its own. Every session waits for that,
                    // also one whose agent was found: the answer is the
                    // search path the sessions get, and one started before
                    // it would run with another.
                    if self.asking_path {
                        continue;
                    }
                    // A session that cannot start stays in the list and says
                    // why. Removing it would lose its name and conversation.
                    if let Err(why) = self.start(id, grid)
                        && let Some(session) = self.ws.session_mut(id)
                    {
                        session.failed(why);
                    }
                }
                None => {}
            }
        }
        sidebar::clamp(&mut self.ui);
    }

    /// Starts the process on a bounded worker: stat, chdir and exec may all
    /// wait for a network filesystem. The window continues to accept input.
    fn start(&mut self, id: SessionId, grid: Grid) -> Result<(), String> {
        let Some(session) = self.ws.session(id) else { return Ok(()) };
        if session.starting {
            return Ok(());
        }
        if !history::valid_id(&session.uuid) {
            return Err("The conversation id is invalid.".into());
        }
        let agent = self
            .cfg
            .agent(&session.agent)
            .ok_or_else(|| format!("The agent \"{}\" is not in the configuration.", session.agent))?;
        let cmd = agent.cmd.clone();
        let (uuid, chosen) = (session.uuid.clone(), session.chosen.clone());
        let (resume, known) = (session.resume, session.known);
        let followed = agent.follows();
        let cwd = self.ws.project(session.project).map_or_else(PathBuf::new, |p| p.path.clone());
        let mut env: Vec<_> = self.cfg.path.iter().map(|path| ("PATH".to_owned(), path.clone())).collect();
        let (scrollback, shift_enter) = (self.cfg.scrollback, agent.shift_enter);
        let gate = StartGate::new();
        let ready = gate.notifier();
        let cancelled = gate.cancellation();
        let abort = gate.aborter();
        let session = self.ws.session_mut(id).unwrap();
        let generation = session.begin_start(gate, followed);
        // Where the program may report about itself. Named to every one:
        // codn does not know which will. A file of its own for this start,
        // and none where the path cannot be handed over as it is.
        let status = Some(reported::file(id, generation)).filter(|file| file.is_absolute());
        let status = status.filter(|file| file.to_str().is_some());
        let named = status.as_deref().and_then(|file| file.to_str());
        env.extend(named.map(|file| (reported::VARIABLE.to_owned(), file.to_owned())));
        // Also as an argument, for an agent whose reporter does not run in
        // the process started here and so has not its environment.
        let args = agent.arguments(&uuid, named.unwrap_or_default(), resume, known, &chosen);
        session.report_file = status.clone();
        let options = Options { scrollback, shift_enter, env, palette: Box::new(self.ui.palette) };
        let reports = self.proxy.clone();
        let notify = move |report| {
            if ready() {
                let _ = reports.send_event(UserEvent::Term(id, generation, report));
            }
        };
        let proxy = self.proxy.clone();
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        if !spawn_bounded(&STARTS, FILESYSTEM, move || {
            let result = if cancelled() {
                Err("Session startup was cancelled.".into())
            } else if !cwd.is_dir() {
                Err(format!("The folder {} does not exist.", cwd.display()))
            } else if cancelled() {
                Err("Session startup was cancelled.".into())
            } else {
                if let Some(file) = &status {
                    reported::prepare(file);
                }
                Terminal::spawn(&cmd, args, &cwd, grid, &options, notify)
                    .map_err(|e| format!("Could not start {cmd}: {e}."))
            };
            if proxy.send_event(UserEvent::Started(id, generation, result)).is_err() {
                abort();
            }
        }) {
            self.ws.session_mut(id).unwrap().cancel_start();
            return Err("Session startup is busy; try again shortly.".into());
        }
        Ok(())
    }

    pub(super) fn started(&mut self, id: SessionId, generation: u64, result: Result<Terminal, String>) {
        let Some(session) = self.ws.session_mut(id) else { return };
        if session.generation != generation || !session.starting {
            return;
        }
        if self.quit {
            return session.cancel_start();
        }
        match result {
            Ok(term) => session.started(term),
            Err(why) => session.failed(why),
        }
        self.fit();
        self.save();
    }

    /// Starts an ended session again.
    pub(super) fn restart(&mut self, id: SessionId) {
        if let Some(session) = self.ws.session_mut(id) {
            session.restart();
            self.fit();
        }
    }

    pub(super) fn open(&mut self, id: SessionId, split: bool) {
        if self.ws.show(id, split).is_err() {
            return self.ask(Asked::Do(Action::Open(id), split));
        }
        self.fit();
        self.save();
    }

    /// Starts a session of an agent in a project, with what was set for it.
    pub(super) fn new_session(&mut self, project: ProjectId, agent: &str, chosen: Chosen, split: bool) {
        if self.cfg.agent(agent).is_none_or(|agent| !agent.installed) || self.ws.project(project).is_none() {
            return;
        }
        let id = self.ws.new_session(project, agent);
        if let Some(session) = self.ws.session_mut(id) {
            session.chosen = chosen.clone();
        }
        self.remember(project, agent, chosen);
        self.open(id, split);
    }

    /// Notes what an agent was started with in a project, as the latest.
    pub(super) fn remember(&mut self, project: ProjectId, agent: &str, chosen: Chosen) {
        let Some(path) = self.ws.project(project).map(|p| p.path.clone()) else { return };
        self.last.retain(|last| last.project != path || last.agent != agent);
        self.last.push(Last { project: path, agent: agent.to_owned(), chosen });
        // Projects come and go, what was set for them need not pile up.
        let over = self.last.len().saturating_sub(MAX_LAST);
        self.last.drain(..over);
    }

    /// The installed agents with what was last set for each in a project,
    /// the one used there last first.
    pub(super) fn usual(&self, project: ProjectId) -> Vec<(String, Chosen)> {
        let path = self.ws.project(project).map(|p| p.path.as_path());
        let here = |last: &&Last| Some(last.project.as_path()) == path;
        let set = |agent: &str| self.last.iter().rev().filter(here).find(|last| last.agent == agent);
        let installed = self.cfg.agents.iter().filter(|agent| agent.installed);
        let mut usual: Vec<(String, Chosen)> = installed
            .map(|agent| {
                (agent.name.clone(), set(&agent.name).map(|last| last.chosen.clone()).unwrap_or_default())
            })
            .collect();
        let latest = self.last.iter().rev().find(here).map(|last| last.agent.as_str());
        if let Some(at) = usual.iter().position(|(agent, _)| Some(agent.as_str()) == latest) {
            usual[..=at].rotate_right(1);
        }
        usual
    }

    /// Puts a second session beside the one in a pane: one of the same
    /// project that is not shown, or else a new one with the same agent.
    pub(super) fn split(&mut self, pane: usize) {
        let Some(session) = self.ws.pane_session(pane).and_then(|id| self.ws.session(id)) else { return };
        let project = session.project;
        let (agent, chosen) = (session.agent.clone(), session.chosen.clone());
        let waiting = self.ws.sessions_of(project).find(|s| !self.ws.visible(s.id)).map(|s| s.id);
        if self.ws.split_beside(pane).is_err() {
            return self.ask(Asked::Do(Action::Split(pane), false));
        }
        match waiting {
            Some(id) => self.open(id, true),
            None => {
                self.new_session(project, &agent, chosen, true);
            }
        }
    }

    pub(super) fn close_session(&mut self, id: SessionId) {
        if self.ui.renaming.as_ref().is_some_and(|(renamed, _)| *renamed == id) {
            self.ui.renaming = None;
        }
        self.ws.remove_session(id);
        self.fit();
        self.save();
    }

    pub(super) fn cycle(&mut self, step: isize) {
        let ids = self.ws.ordered();
        if ids.is_empty() {
            return;
        }
        let current = self.ws.focused_session().and_then(|id| ids.iter().position(|&i| i == id));
        let next = current.map_or(0, |i| (i as isize + step).rem_euclid(ids.len() as isize) as usize);
        // A session that is shown without `split` costs no file its place.
        let _ = self.ws.show(ids[next], false);
        self.fit();
    }

    pub(super) fn toggle_diff(&mut self, project: ProjectId) {
        if self.ws.project(project).is_none() {
            return;
        }
        if self.ws.toggle_diff(project).is_err() {
            return self.ask(Asked::Do(Action::ToggleDiff(project), false));
        }
        self.refresh_git(project);
        self.fit();
    }

    pub(super) fn focused_term(&self) -> Option<&Terminal> {
        self.pane_term(self.ws.focus())
    }

    pub(super) fn pane_term(&self, pane: usize) -> Option<&Terminal> {
        self.ws.pane_session(pane).and_then(|id| self.ws.session(id)).and_then(|s| s.term.as_ref())
    }

    /// Whether a pane shows a terminal.
    pub(super) fn shows_terminal(&self, pane: usize) -> bool {
        matches!(self.ws.panes().get(pane), Some(Pane::Session(_))) && self.pane_term(pane).is_some()
    }

    /// Notifies the user of what a session came to while they were
    /// elsewhere: `news`, in the words of codn.
    pub(super) fn attention(&mut self, id: SessionId, news: &str) {
        let Some(session) = self.ws.session(id) else { return };
        let project = self.ws.project(session.project).map_or("", |p| p.name.as_str());
        notify::send(&format!("{project} · {}", self.ws.label(session)), news);
        if let Some(window) = self.window().filter(|_| !self.ui.focused) {
            window.request_user_attention(Some(UserAttentionType::Informational));
        }
    }

    /// A terminal reported something.
    pub(super) fn term_event(&mut self, id: SessionId, generation: u64, report: term::Report) {
        let seen = self.ws.seen(id, self.ui.focused);
        let visible = self.ws.visible(id);
        let Some(session) = self.ws.session_mut(id) else { return };
        if session.generation != generation {
            return;
        }
        let Some(event) = session.term.as_mut().and_then(|t| t.handle(report, &self.ui.palette)) else {
            return;
        };
        match event {
            term::Event::Output => {
                let now = Instant::now();
                let changed = session.output(now);
                self.dirty |= changed || visible;
                self.urgent |= visible && super::answers(&mut self.awaited, id, now);
                if let Some(timing) = &mut self.timing {
                    timing.echo.output(id, now);
                }
            }
            term::Event::Title(title) => {
                // Agents put a spinner in front of their title and change it
                // many times a second. Only what is shown counts.
                let same = session.title.as_deref().map(clean_title) == title.as_deref().map(clean_title);
                self.dirty |= session.name.is_none() && !same;
                session.title = title;
                if !same {
                    // Not at once: a program can change its title many
                    // times a second, and every save writes the whole state.
                    self.save_soon();
                }
            }
            term::Event::Bell => {
                let before = session.doing();
                // A program that says what it does itself has said, or
                // will say, what its bell is for.
                let tell = session.activity.bell(seen) && session.stopped();
                self.dirty |= session.doing() != before;
                if tell {
                    self.attention(id, STOPPED);
                }
            }
            term::Event::Program { state, anew, subs } => {
                let (before, project) = (session.doing(), session.project);
                self.dirty |= session.runs(subs);
                // Only what it runs beside itself is another: its own
                // state was not said again, and is not news a second time.
                let news = if anew { session.said(state, seen) } else { None };
                let now = session.doing();
                self.dirty |= now != before;
                if let Some(news) = news {
                    self.attention(id, news);
                }
                // What it worked at may be a file made or changed.
                if before == Doing::Working && now != Doing::Working {
                    self.refresh_git(project);
                    if self.shows_files_of(project) {
                        self.refresh_files();
                    }
                }
            }
            // Only what the user is looking at may replace the clipboard,
            // not a session printing in the background.
            term::Event::Clipboard(primary, text) if seen => {
                if let Some(clipboard) = &mut self.clipboard {
                    clipboard.set(primary, text);
                }
            }
            term::Event::Clipboard(..) => {}
            term::Event::InputRejected => {
                self.ui.notice = Some("Input was not sent: the terminal input queue is full.".into());
                self.dirty = true;
            }
            term::Event::SelectionRejected => {
                self.ui.notice = Some("The selection is too large to copy.".into());
                self.dirty = true;
            }
            term::Event::Exited(code) => self.ended(id, code),
        }
    }

    /// The program of a session ended, with `code` if it has one.
    fn ended(&mut self, id: SessionId, code: Option<i32>) {
        let Some(session) = self.ws.session_mut(id) else { return };
        let (project, resumed) = (session.project, session.resume);
        match session.exited(code, Instant::now()) {
            Exit::Retry => {
                log(format_args!("resuming a {} session failed, starting it fresh", session.agent));
                // An agent that chooses its ids itself now begins a
                // conversation of its own. The row must not go on
                // standing for the one that could not be resumed.
                if self.cfg.agent(&session.agent).is_some_and(|agent| !agent.takes_id()) {
                    session.forget_conversation();
                }
                self.fit();
            }
            Exit::Keep => {
                log(format_args!("a {} session ended with {code:?} (resumed: {resumed})", session.agent));
                // Its row has the mark of a program that is not running,
                // which nobody sees who is elsewhere: he is told, as of
                // a program that says itself that it failed.
                let tell = session.crashed() && !self.ws.seen(id, self.ui.focused);
                if tell {
                    self.attention(id, FAILED);
                }
                self.dirty = true;
                self.save();
            }
            Exit::Close => self.close_session(id),
        }
        self.refresh_git(project);
    }

    /// Asks the agent's own files which conversation a session is in. It
    /// may have switched to another one, and an agent that chooses its ids
    /// itself has not said which one it started. The answer comes as an
    /// event.
    pub(super) fn follow(&mut self, id: SessionId) {
        let Some((session, history)) = self.followed(id) else { return };
        let Some(pid) = session.term.as_ref().map(|term| term.pid) else { return };
        let session = self.ws.session_mut(id).unwrap();
        let request = session.next_follow();
        let proxy = self.proxy.clone();
        let answer = move |uuids| drop(proxy.send_event(UserEvent::Conversation(id, request, uuids)));
        if !history::spawn(move || answer(history.current(pid).into_iter().collect())) {
            // Every worker is taken. The question is asked again, or what
            // the agent went on to would not be known after a restart.
            session.ask_refused(Instant::now());
        }
    }

    /// A running session and where its agent keeps its conversations.
    fn followed(&self, id: SessionId) -> Option<(&crate::core::Session, history::History)> {
        let running = |s: &&crate::core::Session| s.followed && s.term.is_some() && s.ended.is_none();
        let session = self.ws.session(id).filter(running)?;
        Some((session, self.cfg.agent(&session.agent)?.history.clone()?))
    }

    /// Takes over the id of the conversation a session turned out to be
    /// in. Of several candidates none is taken: which one is not known.
    pub(super) fn conversation(&mut self, id: SessionId, request: u64, uuids: Vec<String>) {
        let Some(session) = self.ws.session(id) else { return };
        if request != session.follow_request || session.ended.is_some() || session.term.is_none() {
            return;
        }
        let Some(uuid) = conversation_id(&self.ws, id, uuids) else { return };
        if session.uuid == uuid && session.known {
            return;
        }
        if let Some(session) = self.ws.session_mut(id) {
            session.uuid = uuid;
            session.known = true;
        }
        self.save();
    }

    /// Reads what the program of a session reports about itself, on a
    /// worker. The answer comes as an event, also when the file could not
    /// be read: the session waits for it before it reads again.
    pub(super) fn read_reported(&mut self, id: SessionId) {
        let Some(session) = self.ws.session_mut(id) else { return };
        let Some((file, request)) = session.begin_read() else { return };
        let proxy = self.proxy.clone();
        static READS: AtomicUsize = AtomicUsize::new(0);
        let read =
            move || drop(proxy.send_event(UserEvent::Reported(id, request, reported::read(&file).ok())));
        if !spawn_bounded(&READS, FILESYSTEM, read) {
            session.read_refused();
        }
    }

    /// Takes over what a session's program reported about itself.
    pub(super) fn reported(&mut self, id: SessionId, request: u64, found: Option<Option<Reported>>) {
        let visible = self.ws.visible(id);
        let Some(session) = self.ws.session_mut(id) else { return };
        // Shown in the bar of the pane the session is in, and in the
        // rows of its agents in the sidebar, which are there for a
        // session in no pane as well.
        let changed = session.read_done(request, found, Instant::now());
        self.dirty |= changed && (visible || !session.subs.is_empty());
    }

    /// Gives up every start that is under way. What it still answers
    /// belongs to an earlier generation and is dropped.
    pub(super) fn cancel_starts(&mut self) {
        for session in &mut self.ws.sessions {
            if session.starting {
                session.cancel_start();
                session.generation = session.generation.wrapping_add(1);
            }
        }
    }

    /// Queue final metadata reads. The event loop keeps accepting answers
    /// during its bounded shutdown grace period; this never waits for disk.
    pub(super) fn follow_all(&mut self) {
        let targets: Vec<_> = self
            .ws
            .sessions
            .iter()
            .filter_map(|session| {
                let (session, history) = self.followed(session.id)?;
                if history.running.is_empty() {
                    return None;
                }
                Some((session.id, session.term.as_ref()?.pid, history))
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        let targets: Vec<_> = targets
            .into_iter()
            .map(|(id, pid, history)| {
                let request = self.ws.session_mut(id).unwrap().next_follow();
                (id, request, pid, history)
            })
            .collect();
        let proxy = self.proxy.clone();
        history::spawn(move || {
            for (id, request, pid, history) in targets {
                let _ = proxy.send_event(UserEvent::Conversation(
                    id,
                    request,
                    history.current(pid).into_iter().collect(),
                ));
            }
        });
    }
}

/// What the user is told of a session that stopped while nobody looked,
/// where its program does not say why.
pub(super) const STOPPED: &str = "Waiting for you";

/// What the user is told of a session whose program ended with an error
/// while nobody looked: the word a program is told by that says so itself.
const FAILED: &str = "Failed";

/// Ambiguity is decided before excluding identities held by other rows.
fn conversation_id(ws: &crate::core::Workspace, id: SessionId, mut uuids: Vec<String>) -> Option<String> {
    let session = ws.session(id)?;
    uuids.sort();
    uuids.dedup();
    if uuids.len() != 1 {
        return None;
    }
    let uuid = uuids.pop()?;
    if !history::valid_id(&uuid)
        || ws
            .sessions
            .iter()
            .any(|other| other.id != id && other.agent == session.agent && other.uuid == uuid)
    {
        return None;
    }
    Some(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_or_held_conversations_are_never_assigned() {
        let mut ws = crate::core::Workspace::new();
        let project = ws.add_project("/p/a".into());
        let other = ws.add_session(project, "a", "held".into(), false);
        let id = ws.add_session(project, "a", "own".into(), false);
        assert!(conversation_id(&ws, id, vec!["held".into(), "free".into()]).is_none());
        assert!(conversation_id(&ws, id, vec!["held".into()]).is_none());
        assert!(conversation_id(&ws, id, vec!["--flag".into()]).is_none());
        assert_eq!(conversation_id(&ws, id, vec!["own".into(), "own".into()]), Some("own".into()));
        assert_eq!(conversation_id(&ws, other, vec!["new".into()]), Some("new".into()));
    }
}
