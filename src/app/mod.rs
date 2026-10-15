//! The event loop: window, terminal and worker events become state changes,
//! and a redraw whenever something visible changed.
//!
//! `keyboard`, `pointer`, `paste`, `sessions`, `picker` and `files` hold
//! the parts of `App` that deal with those things, `timing` what
//! `CODN_TIMING` prints.

mod editing;
mod files;
mod keyboard;
mod paste;
mod picker;
mod pointer;
mod saving;
mod sessions;
mod timing;

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use raw_window_handle::HasDisplayHandle;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::platform::startup_notify::{self, EventLoopExtStartupNotify, WindowAttributesExtStartupNotify};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::WindowAttributesExtX11;
use winit::window::{UserAttentionType, Window, WindowId};

use crate::action::Action;
use crate::core::config::{self, Config};
use crate::core::discover::Repo;
use crate::core::history::Conversation;
use crate::core::picture::Again;
use crate::core::state::{Last, Saved, Saver};
use crate::core::{
    ProjectId, Reported, SessionId, Status, Workspace, files as disk, git, reported, save, shell,
    spawn_bounded,
};
use crate::log::log;
use crate::picture::client as pictures;
use crate::platform::clipboard::{self, Clipboard};
use crate::platform::window::Gfx;
use crate::render::Icon;
use crate::render::font::Fonts;
use crate::render::icon::STEPS;
use crate::syntax::client as colors;
use crate::term;
use crate::ui::{self, AgentTag, Ui, sidebar, theme};
pub use paste::PasteTarget;
use paste::Waiting;
use pointer::Pointer;
use timing::Timing;

/// What worker threads send to the event loop.
pub enum UserEvent {
    Term(SessionId, u64, term::Report),
    Git(git::Reply),
    /// A folder or a file was read, for the column with the files or the
    /// pane that shows one.
    Files(disk::Reply),
    /// Lines of the file that is shown were cut into pieces, or cannot be.
    Colors(colors::Reply),
    /// The answer for a picture lies ready to be taken.
    Picture,
    Repos(Vec<Repo>),
    /// The conversations found on disk for a project.
    Earlier(ProjectId, u64, Vec<Conversation>),
    /// The ids of the conversations a session's agent may be in.
    Conversation(SessionId, u64, Vec<String>),
    /// What a session's program reports about itself, nothing if it
    /// reports nothing, or `None` for a report that could not be read.
    Reported(SessionId, u64, Option<Option<Reported>>),
    /// A later start of codn handed over its folders.
    Open(Vec<PathBuf>),
    /// Fallback fonts can be looked up now.
    Fonts,
    /// The search path of the user's shell, or nothing if it did not say.
    Path(Option<String>, Vec<(String, bool)>),
    Pasted(PasteTarget, clipboard::Read),
    /// The thread that writes files has written one, or could not.
    Wrote(save::Reply),
    Browsed(u64, String, Vec<crate::core::discover::Row>),
    Started(SessionId, u64, Result<term::Terminal, String>),
    Projects(Vec<PathBuf>, Option<String>),
}

/// What was asked for and would have cost a file its changes: it waits
/// for the answer, and is done again when the changes are saved or let go.
pub(super) enum Asked {
    Do(Action, bool),
    /// A file of the column, by where it lies.
    File(PathBuf),
    /// Closing the pane of the file, whichever that is by then.
    CloseFile,
}

/// The fonts, resolved on a thread while the window is created.
pub type FontLoad = JoinHandle<Result<Fonts, String>>;

/// How often git is asked again while an agent is working.
const GIT_INTERVAL: Duration = Duration::from_secs(3);
const GIT_INTERVAL_AWAY: Duration = Duration::from_secs(15);
/// How long the mark of a session at work stands before it moves a step
/// on: five steps in a second, and across its field in four seconds.
const STEP: Duration = Duration::from_millis(200);
/// How long a change that is not urgent waits for others before it is saved.
const SAVE_DELAY: Duration = Duration::from_secs(1);
/// How long after the room of a picture last changed it is decoded for
/// the room it has then: not at every step of a window that is dragged.
const PICTURE_DELAY: Duration = Duration::from_millis(200);
/// What the workers are called that may wait for a filesystem.
const FILESYSTEM: &str = "codn filesystem";

pub struct App {
    cfg: Config,
    proxy: EventLoopProxy<UserEvent>,
    fonts: Option<FontLoad>,
    // The clipboard holds a pointer into the window's display connection,
    // so it is declared, and therefore dropped, before the window.
    clipboard: Option<Clipboard>,
    gfx: Option<Gfx>,
    ws: Workspace,
    ui: Ui,
    /// Git repositories found on this machine, most recent first.
    repos: Vec<Repo>,
    git: git::Requests,
    git_due: Instant,
    /// Where the column with the files and the pane for a file ask for
    /// folders and files.
    files: disk::Requests,
    /// Where the pane for a file asks for the colors of its text, and
    /// whether the process that knows them may be running.
    colors: colors::Requests,
    coloring: bool,
    /// Where the pane for a file asks for the pixels of a picture, and
    /// whether the process that makes them may be running.
    pictures: pictures::Requests,
    picturing: bool,
    /// The number of the decoding that was asked for last.
    decodings: u64,
    /// When the picture that is shown is decoded again for a room it
    /// does not fit.
    again: Again<[u32; 8]>,
    /// The thread that writes the text of a file back, and how many
    /// texts it was handed that it has not answered.
    writer: save::Saver,
    writing: usize,
    /// What waits for the answer about changes that are not saved.
    asked: Option<Asked>,
    /// The window was asked to close and waits for a file to be written.
    leaving: bool,
    /// Where the window of an input method was last told the caret of a
    /// file stands.
    ime_at: Option<(i32, i32, u32, u32)>,
    /// The project the column with the files last followed the keyboard to.
    tree_followed: Option<ProjectId>,
    mods: ModifiersState,
    pointer: Pointer,
    /// The session whose program was last told that it has the focus.
    told_focused: Option<SessionId>,
    /// Terminal font size in logical pixels.
    font_size: f32,
    /// Size of the floating window in logical pixels.
    size: (f32, f32),
    /// Earliest time for the next frame, where the app paces itself.
    frame_due: Instant,
    frame: Duration,
    dirty: bool,
    /// What is to be drawn is what the user did, or the answer to a key:
    /// it is not held back until the compositor asks for the next frame.
    urgent: bool,
    /// A key went to this session, which has not answered yet.
    awaited: Option<(SessionId, Instant)>,
    quit: bool,
    quit_started: Option<Instant>,
    /// What `CODN_TIMING` asks to be measured, if it is set.
    timing: Option<Timing>,
    /// The window has a new size that the terminals have not been fitted to.
    refit: bool,
    /// The window is covered or minimized. What changes is drawn when it
    /// shows again.
    hidden: bool,
    saver: Saver,
    /// A change that is saved by this time at the latest, a title for one.
    save_due: Cell<Option<Instant>>,
    /// When the mark of a session at work moves a step on, and whether a
    /// frame with its latest step was asked for and not drawn.
    step_due: Instant,
    step_asked: bool,
    /// Tells later starts of codn that this one is closing and takes no
    /// more folders.
    closing: Arc<AtomicBool>,
    /// What was last chosen for an agent in a project, the latest last.
    last: Vec<Last>,
    /// The shell has not yet answered with its search path. Sessions whose
    /// agent was not found wait for it instead of failing to start.
    asking_path: bool,
    history_request: u64,
    input_epoch: u64,
    browse_request: u64,
    browse_pending: Option<(u64, String)>,
    /// Pastes the clipboard has not answered, and what was typed for a
    /// terminal behind them.
    waiting: Waiting,
}

/// What to say of an icon in the configuration by a name there is none
/// for: the mark is shown, and why.
fn icon_notice(cfg: &Config) -> Option<String> {
    let mut icons = cfg.agents.iter().filter_map(|agent| agent.icon.as_deref());
    icons.find(|name| Icon::named(name).is_none()).map(|name| {
        let names: Vec<&str> = crate::render::icon::NAMED.iter().map(|(name, _)| *name).collect();
        format!("config.toml: there is no icon \"{name}\". There are: {}.", names.join(", "))
    })
}

/// Asks the shell for its search path and which of the agents are found on
/// it, on a thread. The answer always comes, as `UserEvent::Path`: with
/// nothing, if the shell has not said within twelve seconds.
fn ask_path(agents: Vec<config::Agent>, proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let path = shell::shell_path();
            let search = shell::search_path(path.as_deref());
            let installed = agents
                .into_iter()
                .map(|agent| {
                    let found = shell::which(&agent.cmd, &search);
                    (agent.name, found)
                })
                .collect();
            let _ = tx.send((path, installed));
        });
        let (path, installed) = rx.recv_timeout(Duration::from_secs(12)).unwrap_or_default();
        let _ = proxy.send_event(UserEvent::Path(path, installed));
    });
}

fn agent_tags(cfg: &Config) -> Vec<AgentTag> {
    let tag = |a: &config::Agent| AgentTag {
        name: a.name.clone(),
        mark: a.mark.clone(),
        icon: a.icon.as_deref().and_then(Icon::named),
        installed: a.installed,
        options: a.options.clone(),
    };
    cfg.agents.iter().map(tag).collect()
}

impl App {
    pub fn new(
        cfg: Config,
        saved: Saved,
        ws: Workspace,
        proxy: EventLoopProxy<UserEvent>,
        fonts: FontLoad,
        started: Instant,
        closing: Arc<AtomicBool>,
    ) -> Self {
        let size = restored_size(saved.width, saved.height);
        let mut ui = Ui::new(agent_tags(&cfg));
        ui.buttons = cfg.window.buttons;
        ui.tree_open = saved.tree;
        ui.sidebar_folded = saved.folded;
        ui.notice = cfg.error.clone().or_else(|| icon_notice(&cfg));

        let replies = proxy.clone();
        let git = git::spawn(move |reply| replies.send_event(UserEvent::Git(reply)).is_ok());
        let replies = proxy.clone();
        let files = disk::Requests::new(move |reply| replies.send_event(UserEvent::Files(reply)).is_ok());
        let replies = proxy.clone();
        let writer = save::Saver::new(move |reply| drop(replies.send_event(UserEvent::Wrote(reply))));
        let replies = proxy.clone();
        let colors = colors::Requests::new(move |reply| replies.send_event(UserEvent::Colors(reply)).is_ok());
        let replies = proxy.clone();
        let pictures = pictures::Requests::new(move || replies.send_event(UserEvent::Picture).is_ok());
        // An agent that is not found may only be missing from the search
        // path codn was started with. Ask the shell, off the main thread.
        let asking_path = cfg.agents.iter().any(|agent| !agent.installed);
        if asking_path {
            ask_path(cfg.agents.clone(), proxy.clone());
        }
        // What the programs of earlier runs reported is of no use to this
        // one. Not on this thread: it waits for the disk.
        static SWEEPS: AtomicUsize = AtomicUsize::new(0);
        spawn_bounded(&SWEEPS, FILESYSTEM, reported::sweep);
        let font_size = cfg.font.size;
        // In the order of the fields, which is the order they are dropped in.
        Self {
            cfg,
            proxy,
            fonts: Some(fonts),
            clipboard: None,
            gfx: None,
            ws,
            ui,
            repos: Vec::new(),
            git,
            git_due: Instant::now(),
            files,
            colors,
            coloring: false,
            pictures,
            picturing: false,
            decodings: 0,
            again: Again::default(),
            writer,
            writing: 0,
            asked: None,
            leaving: false,
            ime_at: None,
            tree_followed: None,
            mods: ModifiersState::empty(),
            pointer: Pointer::default(),
            told_focused: None,
            font_size,
            size,
            frame_due: Instant::now(),
            frame: Duration::from_millis(16),
            dirty: true,
            urgent: false,
            awaited: None,
            quit: false,
            quit_started: None,
            timing: Timing::from_env(started),
            refit: false,
            hidden: false,
            saver: Saver::start(),
            save_due: Cell::new(None),
            step_due: Instant::now(),
            step_asked: false,
            closing,
            last: saved.last,
            asking_path,
            history_request: 0,
            input_epoch: 0,
            browse_request: 0,
            browse_pending: None,
            waiting: Waiting::default(),
        }
    }

    fn window(&self) -> Option<&Window> {
        self.gfx.as_ref().map(|g| &g.window)
    }

    fn save(&self) {
        self.save_due.set(None);
        let last = self.last.clone();
        let (tree, folded) = (self.ui.tree_open, self.ui.sidebar_folded);
        self.saver.save(Saved { last, tree, folded, ..self.ws.saved(self.size.0, self.size.1) });
    }

    /// Saves within a second, together with what else changes until then.
    fn save_soon(&self) {
        if self.save_due.get().is_none() {
            self.save_due.set(Some(Instant::now() + SAVE_DELAY));
        }
    }

    fn refresh_git(&self, project: ProjectId) {
        let Some(p) = self.ws.project(project) else { return };
        self.git.send(git::Request::Status(p.path.clone()));
        if self.ws.shows_diff(project) {
            self.git.send(git::Request::Diff(p.path.clone()));
        }
        // A commit changes what is marked in a file without changing it.
        if self.ws.file().is_some_and(|(_, view)| view.project == project) {
            self.ask_marks();
        }
    }

    fn refresh_all_git(&self) {
        self.ws.projects.iter().for_each(|p| self.refresh_git(p.id));
    }

    /// Carries out an action. `split` asks to open beside the current pane.
    fn run(&mut self, action: Action, split: bool) {
        // Whoever does something else while the line at the bottom asks
        // about changes has answered "stay".
        if self.asked.is_some() {
            self.stay();
        }
        // What was typed into a file behind a paste that is still out is
        // typed before anything is done that could cost the file its
        // place: it is not yet a change, and would be lost unasked.
        if !matches!(action, Action::Copy | Action::Paste) {
            self.settle_file();
        }
        // Copying changes nothing on screen, and what a paste changes says
        // the terminal. A frame for the paste alone would come before its
        // echo, as with a key.
        self.dirty |= !matches!(action, Action::Copy | Action::Paste);
        match action {
            Action::Minimize => self.window().map_or((), |w| w.set_minimized(true)),
            Action::Maximize => self.window().map_or((), |w| w.set_maximized(!w.is_maximized())),
            Action::Quit => self.close(),
            Action::ToggleDiff(project) => self.toggle_diff(project),
            Action::RemoveProject(project) => {
                if self.ws.remove_project(project).is_err() {
                    return self.ask(Asked::Do(action, split));
                }
                self.fit();
                self.save();
            }
            Action::Open(id) | Action::OpenBelow(id) => self.open(id, split),
            Action::CloseSession(id) => self.close_session(id),
            // What opened the list of projects also closes it. Over the
            // list that starts a session it opens its own instead.
            Action::AddProject if self.ui.picker.as_ref().is_some_and(|open| open.launch.is_none()) => {
                self.ui.picker = None;
            }
            Action::AddProject => self.start_adding(),
            Action::Launch(project) => self.start_launch(project),
            Action::LaunchActive => self.ws.active_project().map_or((), |p| self.start_launch(p)),
            // A row is named by its place in the list the frame drew.
            Action::Options(row) | Action::Previous(row) | Action::Pick(row) | Action::More(row)
                if self.ui.picker.as_ref().is_some_and(|picker| picker.clicked(row).is_none()) =>
            {
                self.dirty = true;
            }
            Action::Options(row) => drop(self.with_picker(|picker, known| picker.options(row, known))),
            Action::Previous(row) => drop(self.with_picker(|picker, _| picker.choose(row, -1))),
            Action::Pick(row) | Action::More(row) => self.pick(row),
            Action::Browse => self.browse(),
            Action::ClosePane(pane) => {
                if self.ws.close_pane(pane).is_err() {
                    return self.ask(Asked::CloseFile);
                }
                self.fit();
            }
            Action::Split(pane) => self.split(pane),
            Action::CloseSplit => self.run(Action::ClosePane(self.ws.focus()), false),
            Action::FocusOther => self.ws.focus_pane(1 - self.ws.focus().min(1)),
            Action::DismissNotice => self.ui.notice = None,
            Action::Rename => {
                if let Some(session) = self.ws.focused_session().and_then(|id| self.ws.session(id)) {
                    self.ui.renaming = Some((session.id, self.ws.label(session)));
                    sidebar::reveal(&mut self.ui, &self.ws, session.id);
                }
            }
            Action::Copy => self.copy(false),
            Action::Paste => self.paste(false),
            Action::NewDefault => {
                if let Some(project) = self.ws.active_project()
                    && let Some((agent, chosen)) = self.usual(project).into_iter().next()
                {
                    self.new_session(project, &agent, chosen, false);
                }
            }
            Action::DiffActive => self.ws.active_project().map_or((), |p| self.toggle_diff(p)),
            Action::Cycle(step) => self.cycle(step),
            Action::Zoom(0) => self.zoom(self.cfg.font.size),
            Action::Zoom(step) => self.zoom(self.font_size + step as f32),
            Action::ToggleTree => self.toggle_tree(),
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::TreeRow(id) => self.tree_row(id),
            Action::TreeOf(project) => self.show_tree(project),
            Action::FileChanges(of, line) => self.file_changes(of, line),
            Action::TogglePreview => self.toggle_preview(),
            Action::DiffFile(row, check) => self.diff_file(row as usize, check),
        }
    }

    fn zoom(&mut self, size: f32) {
        let size = size.clamp(8.0, 40.0);
        if size == self.font_size {
            return;
        }
        self.font_size = size;
        // Glyphs are remembered by size, so those of the interface stay
        // good and those of the old size wait for a way back.
        self.measure();
        self.fit();
    }

    // ----- layout -----

    fn resized(&mut self, width: u32, height: u32) {
        let Some(gfx) = &mut self.gfx else { return };
        gfx.resize(width, height);
        self.ui.width = width as f32;
        self.ui.height = height as f32;
        // Maximized and fullscreen windows have no edges to drag, and their
        // size is not the one to come back to.
        self.ui.maximized = gfx.window.is_maximized();
        self.ui.floating = !self.ui.maximized && gfx.window.fullscreen().is_none();
        if self.ui.floating {
            self.size = (width as f32 / self.ui.scale, height as f32 / self.ui.scale);
        }
        // A drag of the window edge sends many sizes. The terminals take the
        // last one of a turn, each resize makes its program lay out again.
        self.refit = true;
        self.dirty = true;
    }

    /// Fits the terminals to a window size that came in since the last time.
    fn fit_pending(&mut self) {
        if std::mem::take(&mut self.refit) {
            self.fit();
        }
    }

    /// Takes the font metrics again, after a change of scale or font size.
    fn measure(&mut self) {
        let Some(gfx) = &mut self.gfx else { return };
        self.ui.measure(&mut gfx.renderer.fonts, self.font_size);
        self.dirty = true;
    }

    /// Whether there is a window to draw into: one that is to be seen, and
    /// has a size.
    fn drawable(&self) -> bool {
        !self.hidden && self.ui.width >= 1.0 && self.ui.height >= 1.0
    }

    fn redraw(&mut self) {
        if !self.drawable() {
            return;
        }
        self.fit_pending();
        let Some(gfx) = &mut self.gfx else { return };
        self.dirty = false;
        self.urgent = false;
        if std::mem::take(&mut self.step_asked) {
            self.step_due = Instant::now() + STEP;
        }
        let (ui, ws) = (&mut self.ui, &self.ws);
        let begun = Instant::now();
        let quads =
            gfx.renderer.frame(ui.width as u32, ui.height as u32, theme::BG, |cv| ui::draw(cv, ui, ws));
        let built = begun.elapsed();
        gfx.present();
        // The window of an input method is told where the caret of a
        // file stands, when that is another place than before.
        let caret = ws
            .file()
            .filter(|(pane, _)| *pane == ws.focus())
            .and_then(|(pane, view)| ui::file::caret(ui, &ui.pane(pane, ws.panes().len()), view));
        let caret = caret.map(|at| (at.x as i32, at.y as i32, at.w as u32, at.h as u32));
        if let Some((x, y, w, h)) = caret.filter(|at| self.ime_at != Some(*at)) {
            gfx.window.set_ime_cursor_area(PhysicalPosition::new(x, y), PhysicalSize::new(w, h));
        }
        self.ime_at = caret;
        // A terminal that was busy is shown as it was, and drawn again
        // when the next frame is due.
        let mut stale = false;
        for pane in 0..self.ws.panes().len() {
            stale |= self.pane_term(pane).is_some_and(|term| term.take_stale());
        }
        self.dirty |= stale;
        if let Some(timing) = &mut self.timing {
            timing.frame(begun, built, quads, stale);
        }
        // From the start of the frame. A swap that waits for the display
        // has used the time up by now, and counting from here on would let
        // every other refresh pass.
        self.frame_due = begun + self.frame;
        // The layout may have moved under a resting pointer.
        self.hover();
    }

    /// Does what a key or a click asks for. What that changes on screen is
    /// drawn at once, see `about_to_wait`.
    fn at_once(&mut self, act: impl FnOnce(&mut Self)) {
        self.urgent |= self.changed_by(act);
    }

    /// Does `act` and says whether that changed what is on screen. What was
    /// to be drawn before it stays so.
    fn changed_by(&mut self, act: impl FnOnce(&mut Self)) -> bool {
        let was = std::mem::take(&mut self.dirty);
        act(self);
        let changed = self.dirty;
        self.dirty |= was;
        changed
    }

    /// Tells terminal programs which of them has the focus, whenever that
    /// changed: by a click, a switch of sessions, or the window itself.
    fn report_focus(&mut self) {
        let now = self.ws.focused_session().filter(|_| self.ui.focused && !self.ui.editing());
        if now == self.told_focused {
            return;
        }
        for (id, focused) in [(self.told_focused, false), (now, true)] {
            if let Some(term) = id.and_then(|id| self.ws.session(id)).and_then(|s| s.term.as_ref()) {
                term.focus(focused);
            }
        }
        self.told_focused = now;
    }

    // ----- what workers answer -----

    fn git_reply(&mut self, reply: git::Reply) {
        match reply {
            git::Reply::Status(path, info) => {
                if let Some(project) = self.ws.projects.iter_mut().find(|p| p.path == path)
                    && project.git != info
                {
                    project.git = info;
                    self.dirty = true;
                }
            }
            git::Reply::Diff(path, prefix, lines) => {
                let project = self.ws.project_at(&path).map(|p| p.id);
                if let Some(view) = project.and_then(|id| self.ws.diff_mut(id)) {
                    view.set_lines(prefix, lines);
                    self.dirty = true;
                }
            }
            git::Reply::Marks(of, stamp, marks) => self.marks_reply(of, stamp, marks),
        }
    }

    /// A later start of codn handed over its folders.
    fn handed_over(&mut self, folders: Vec<PathBuf>) {
        self.add_projects(folders);
        if let Some(window) = self.window() {
            // Wayland lets no window bring itself to the front. It
            // asks for attention there, in the task bar.
            window.focus_window();
            if !self.ui.focused {
                window.request_user_attention(Some(UserAttentionType::Informational));
            }
        }
    }

    /// The shell answered `ask_path`: with its search path, if it said, and
    /// with which agents are found on it.
    fn path_answered(&mut self, path: Option<String>, installed: Vec<(String, bool)>) {
        self.asking_path = false;
        let own = std::env::var("PATH").ok();
        if let Some(path) = path.filter(|path| own.as_ref() != Some(path)) {
            self.cfg.path = Some(path);
        }
        for (name, found) in installed {
            if let Some(agent) = self.cfg.agents.iter_mut().find(|a| a.name == name) {
                agent.installed = found;
            }
        }
        self.ui.agents = agent_tags(&self.cfg);
        // Starts the sessions that waited for this answer.
        self.fit();
    }

    // ----- between events -----

    /// Closing: the sessions get a moment to say which conversations they
    /// are in, then the state is written and the loop ends.
    fn shut_down(&mut self, event_loop: &ActiveEventLoop) {
        let begun = if let Some(begun) = self.quit_started {
            begun
        } else {
            self.closing.store(true, Ordering::Release);
            self.cancel_starts();
            self.follow_all();
            let begun = Instant::now();
            self.quit_started = Some(begun);
            begun
        };
        let deadline = begun + Duration::from_millis(150);
        if Instant::now() >= deadline {
            self.save();
            self.saver.finish();
            return event_loop.exit();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
    }

    /// Does what is due at `now` without an event having asked for it, and
    /// returns when the next of it is due. Nothing while all is quiet: the
    /// loop then waits without a limit.
    fn due(&mut self, now: Instant) -> Option<Instant> {
        // Sessions whose output stopped go back to idle or ask for attention.
        for (id, project, status) in self.ws.settle(now, self.ui.focused) {
            self.dirty = true;
            self.refresh_git(project);
            // What the session did may be a file made or changed.
            if self.shows_files_of(project) {
                self.refresh_files();
            }
            // Why a session stopped is guessed only where its program
            // does not say what it does.
            let stopped = status == Status::Attention;
            if stopped && self.ws.session_mut(id).is_some_and(|session| session.stopped()) {
                self.attention(id, sessions::STOPPED);
            }
        }

        // An agent may have gone on to another conversation.
        for id in self.ws.asks_due(now) {
            self.follow(id);
        }

        // A program may have reported something new about itself.
        for id in self.ws.reads_due(now) {
            self.read_reported(id);
        }

        // While agents work, keep the git state of their projects current.
        let working = self.ws.working_projects();
        if !working.is_empty() && now >= self.git_due {
            working.iter().for_each(|&p| self.refresh_git(p));
            // The files an agent makes show while it works, at the pace
            // git is asked at. A folder is read only if it says it changed.
            if working.iter().any(|&p| self.shows_files_of(p)) {
                self.refresh_files();
            }
            // Nobody reads the sidebar of a window that is not in front,
            // and it is brought up to date when it comes to the front.
            self.git_due = now + if self.ui.focused { GIT_INTERVAL } else { GIT_INTERVAL_AWAY };
        }

        if self.save_due.get().is_some_and(|due| now >= due) {
            self.save();
        }
        // A paste that did not come back lets go of what was typed after it.
        if self.waiting.due().is_some_and(|due| now >= due) {
            self.release_all();
        }

        let mut wake = self.ws.next_wake();
        if !working.is_empty() {
            wake = sooner(wake, self.git_due);
        }
        // The mark of a session at work moves, the one thing in the
        // window that moves by itself. The next step waits for the frame
        // that shows this one: a window nobody is shown gets no frames
        // where the desktop paces them, and the loop then does not wake
        // for a mark nobody sees.
        if self.ws.any_working() && self.drawable() && !self.step_asked {
            if now >= self.step_due {
                self.ui.step = (self.ui.step + 1) % STEPS;
                self.step_asked = true;
                self.dirty = true;
            } else {
                wake = sooner(wake, self.step_due);
            }
        }
        for due in [self.save_due.get(), self.waiting.due(), self.again.due()].into_iter().flatten() {
            wake = sooner(wake, due);
        }
        wake
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(fonts) = self.fonts.take() else { return };
        // X11 shows this in the task bar. Wayland has no window icons: there
        // the desktop entry named like the app id carries it, assets/codn.desktop.
        let icon =
            winit::window::Icon::from_rgba(include_bytes!("../../assets/icon-64.rgba").to_vec(), 64, 64).ok();
        let attributes = Window::default_attributes()
            .with_title("codn")
            .with_window_icon(icon)
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(self.size.0, self.size.1))
            .with_min_inner_size(LogicalSize::new(560.0, 320.0));
        let attributes = WindowAttributesExtWayland::with_name(attributes, "codn", "codn");
        let mut attributes = WindowAttributesExtX11::with_name(attributes, "codn", "codn");
        // A launcher that announced the start waits to hear of the window,
        // and on Wayland the token is what lets the window come to the front.
        if let Some(token) = event_loop.read_token_from_env() {
            startup_notify::reset_activation_token_env();
            attributes = attributes.with_activation_token(token);
        }
        // For the timing of the start: when the window and its context were
        // there, and when the fonts were.
        let (entered, mut waited) = (Instant::now(), None);
        let fonts = || {
            let asked = Instant::now();
            let fonts = fonts.join().unwrap_or_else(|_| Err("the font loader crashed".into()));
            waited = Some((asked, Instant::now()));
            fonts.map_err(Into::into)
        };
        let mut gfx = match Gfx::new(event_loop, attributes, fonts) {
            Ok(gfx) => gfx,
            Err(e) => {
                log(e);
                return event_loop.exit();
            }
        };
        let made = Instant::now();
        gfx.window.set_ime_allowed(true);
        self.clipboard = gfx.window.display_handle().ok().map(|h| Clipboard::new(h.as_raw()));
        self.ui.scale = Ui::scale_of(gfx.window.scale_factor());
        self.ui.measure(&mut gfx.renderer.fonts, self.font_size);
        // Where the compositor does not pace frames, the monitor's rate does.
        let hertz = gfx.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).unwrap_or(60_000);
        self.frame = Duration::from_secs_f64(1000.0 / hertz.max(1000) as f64);
        let size = gfx.window.inner_size();
        self.gfx = Some(gfx);
        self.resized(size.width, size.height);
        self.refresh_all_git();
        if let (Some(timing), Some((asked, there))) = (&mut self.timing, waited) {
            timing.staged(entered, asked, there, made);
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Term(id, generation, report) => self.term_event(id, generation, report),
            UserEvent::Git(reply) => self.git_reply(reply),
            UserEvent::Files(reply) => self.files_reply(reply),
            UserEvent::Colors(reply) => self.colors_reply(reply),
            UserEvent::Picture => self.picture_reply(),
            UserEvent::Repos(repos) => self.rescanned(repos),
            UserEvent::Earlier(project, request, found) => self.earlier_found(project, request, found),
            UserEvent::Conversation(id, request, uuids) => self.conversation(id, request, uuids),
            UserEvent::Reported(id, request, found) => self.reported(id, request, found),
            UserEvent::Pasted(target, read) => self.finish_paste(target, read),
            UserEvent::Wrote(reply) => self.wrote(reply),
            UserEvent::Browsed(request, text, rows) => self.browsed(request, text, rows),
            UserEvent::Started(id, generation, result) => self.started(id, generation, result),
            UserEvent::Projects(paths, notice) => self.projects_added(paths, notice),
            UserEvent::Open(folders) => self.handed_over(folders),
            UserEvent::Fonts => {
                if self.gfx.as_mut().is_some_and(|gfx| gfx.renderer.fonts_ready()) {
                    self.dirty = true;
                }
            }
            UserEvent::Path(path, installed) => self.path_answered(path, installed),
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close(),
            WindowEvent::Resized(size) => self.resized(size.width, size.height),
            // The new size follows as its own event, the grids are fitted then.
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.ui.scale = Ui::scale_of(scale_factor);
                if let Some(gfx) = &mut self.gfx {
                    gfx.renderer.reset_glyphs();
                }
                self.measure();
            }
            // The window is gone, taken away by another program: nothing
            // is shown in it and nothing can be asked there, not even
            // about changes that are not saved. A codn without a window
            // would run on unseen, and every later start would hand its
            // folders to it.
            // The event names the window: on X11 it also comes for one
            // that the window system of the app made for a moment, to
            // answer the launcher that started it.
            WindowEvent::Destroyed if self.window().is_some_and(|window| window.id() == id) => {
                self.hidden = true;
                self.quit = true;
            }
            WindowEvent::Focused(focused) => {
                self.ui.focused = focused;
                if focused {
                    self.refresh_all_git();
                    self.refresh_files();
                }
                // Some window managers change the window state without a
                // resize event that would report it. The size is the one
                // that is known: on X11 the window answers the question
                // for it with a panic once it is gone, and a window that
                // is taken away loses the keyboard first.
                self.resized(self.ui.width as u32, self.ui.height as u32);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.mods = mods.state();
                // Control shows what a click would open under a pointer
                // that rests.
                self.hover();
            }
            // Synthetic presses are keys that were already down when the
            // window got the focus. They were meant for someone else.
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. }
                if event.state == ElementState::Pressed =>
            {
                self.at_once(|app| app.key(event))
            }
            // A question about changes that are not saved takes every
            // key, and with them the text an input method made of keys.
            WindowEvent::Ime(Ime::Commit(_)) if self.asked.is_some() => {}
            WindowEvent::Ime(Ime::Commit(text)) => self.at_once(|app| app.type_text(text)),
            WindowEvent::CursorMoved { position, .. } => {
                self.ui.mouse = (position.x as f32, position.y as f32);
                self.mouse_moved();
            }
            WindowEvent::CursorLeft { .. } => {
                self.ui.mouse = (-1.0, -1.0);
                self.hover();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.at_once(|app| app.mouse_button(button, state == ElementState::Pressed))
            }
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            WindowEvent::Occluded(hidden) => {
                self.hidden = hidden;
                self.dirty |= !hidden;
                if let Some(timing) = &mut self.timing {
                    timing.echo.occluded(hidden);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit {
            return self.shut_down(event_loop);
        }
        let now = Instant::now();
        self.fit_pending();
        self.report_focus();
        self.follow_tree();
        self.follow_preview();
        self.follow_colors();
        self.follow_picture(now);
        let mut wake = self.due(now);
        // A window without a size draws nothing: asking for a frame would
        // come straight back here, without end.
        if self.dirty
            && self.drawable()
            && let Some(paced) = self.gfx.as_ref().map(|gfx| gfx.paced)
        {
            // The compositor paces redraws on Wayland. Elsewhere a flood of
            // output would otherwise draw as fast as the CPU allows.
            if paced && self.urgent {
                // Not this one. While something prints, a frame is always
                // under way, and the next one is asked for up to a refresh
                // later. The compositor shows the latest frame it has by
                // then, so one drawn now is on screen a refresh earlier.
                // There is one such frame per key or click at most.
                self.redraw();
                // A terminal that was busy is drawn again in its turn.
                if self.dirty
                    && let Some(window) = self.window()
                {
                    window.request_redraw();
                }
            } else if paced || now >= self.frame_due {
                if let Some(window) = self.window() {
                    window.request_redraw();
                }
            } else {
                wake = sooner(wake, self.frame_due);
            }
        }
        event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

/// Output this soon after a key is taken for its answer.
const ANSWER: Duration = Duration::from_millis(300);

/// Whether output of a session at `now` answers the key that is waited
/// for. The key is done with then, also when the answer came too late.
fn answers(awaited: &mut Option<(SessionId, Instant)>, id: SessionId, now: Instant) -> bool {
    awaited.take_if(|(session, _)| *session == id).is_some_and(|(_, at)| now.duration_since(at) < ANSWER)
}

/// The earlier of a time to wake at and another.
fn sooner(wake: Option<Instant>, at: Instant) -> Option<Instant> {
    Some(wake.map_or(at, |wake| wake.min(at)))
}

/// The saved size in logical pixels. The limit leaves room for the scale:
/// X11 takes no window of 65536 physical pixels or more.
fn restored_size(width: f32, height: f32) -> (f32, f32) {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        (1200.0, 760.0)
    } else {
        (width.clamp(560.0, 8192.0), height.clamp(320.0, 8192.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_saved_window_sizes_cannot_reach_the_platform() {
        for (w, h) in [(f32::INFINITY, 600.0), (800.0, f32::NAN), (800.0, 0.0), (-1.0, 600.0)] {
            assert_eq!(restored_size(w, h), (1200.0, 760.0));
        }
        assert_eq!(restored_size(1.0, 1.0), (560.0, 320.0));
        assert_eq!(restored_size(f32::MAX, f32::MAX), (8192.0, 8192.0));
        assert_eq!(restored_size(800.0, 600.0), (800.0, 600.0));
    }

    #[test]
    fn output_answers_a_key_of_its_own_session_once_and_soon() {
        let at = Instant::now();
        let soon = at + Duration::from_millis(20);
        let (typed, other) = (SessionId(1), SessionId(2));
        // What another session prints leaves the key waiting.
        let mut awaited = Some((typed, at));
        assert!(!answers(&mut awaited, other, soon));
        assert!(answers(&mut awaited, typed, soon));
        // One answer per key: what follows is output like any other.
        assert!(!answers(&mut awaited, typed, soon));
        // A program that printed nothing for the key is not answering it
        // with what it prints much later.
        let mut awaited = Some((typed, at));
        assert!(!answers(&mut awaited, typed, at + ANSWER));
        assert_eq!(awaited, None);
    }
}
