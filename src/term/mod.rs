//! A terminal: a child process on a PTY plus the emulator state it writes to.
//!
//! The emulator is `alacritty_terminal`, and this module is the only place
//! that knows it. Everything else talks to `Terminal` in terms of keys,
//! cells and events, so the emulator stays replaceable.

mod draw;
mod input;
mod keys;
mod link;
mod selection;

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as Raw, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSendError, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{ClipboardType, Config, Osc52, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Rgb;

use crate::core::program::{self, News, Records, Sub};
use crate::render::Color;
pub use draw::{DIMMED, Look};

/// Number of colors a terminal can address: 256 indexed plus the named ones.
pub const PALETTE: usize = 269;
/// Longest title a program may set. Titles are drawn, saved and passed to
/// the notification service, so they must not grow without bound.
const MAX_TITLE: usize = 200;

static READERS: AtomicUsize = AtomicUsize::new(0);

/// Kept by every clone of a reader's listener, including its terminal.
/// The event loop drops its PTY (and completes signal escalation) before
/// its terminal/listener fields, so the last release means cleanup ended.
struct ReaderLifetime;

impl ReaderLifetime {
    fn begin() -> Self {
        READERS.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for ReaderLifetime {
    fn drop(&mut self) {
        READERS.fetch_sub(1, Ordering::Release);
    }
}

/// Called after App and the event loop are dropped. Detached PTY readers
/// must finish escalation before main exits and destroys all threads.
pub fn finish_cleanup(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while READERS.load(Ordering::Acquire) != 0 {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// Something the emulator reported, not yet interpreted. Hand it to
/// `Terminal::handle` on the main thread.
pub struct Report(Raw);

/// What a report means for the rest of the app.
pub enum Event {
    /// The screen changed.
    Output,
    /// The program set a title, or took it back.
    Title(Option<String>),
    Bell,
    /// The program asked to put text on the clipboard, or with `true` on
    /// the selection that the middle button pastes.
    Clipboard(bool, String),
    InputRejected,
    SelectionRejected,
    /// What the program says of its own state is another now: the one
    /// state shown of it, where None is nothing that holds, and what it
    /// runs beside itself. `anew` says that the state is another or was
    /// said to be done again; without it only the rest changed, and the
    /// state is the one known already.
    Program {
        state: Option<program::State>,
        anew: bool,
        subs: Vec<Sub>,
    },
    /// The process ended, with its exit code if it has one.
    Exited(Option<i32>),
}

/// Passes emulator reports on. Called from the PTY reader thread.
type Notify = Arc<dyn Fn(Report) + Send + Sync>;

/// What a terminal shares with the listener of its emulator, which is
/// called on the PTY reader thread.
struct Shared {
    /// A report of new output is on its way to the main thread and has not
    /// been taken up there. Heavy output reports far more often than a frame
    /// is drawn, and one report says all there is to say.
    pending: AtomicBool,
    /// The title the program set last, until the main thread takes it. A
    /// program can set titles faster than anyone reads them, and each one
    /// on its own way would pile up there without a limit.
    title: Mutex<Option<Option<String>>>,
    /// A bell, or the notice that input found no room, is on its way to
    /// the main thread. A program can ring faster than anyone listens.
    bell: AtomicBool,
    rejected: AtomicBool,
    /// What the program said of its own state, kept here because a program
    /// can say it faster than anyone looks.
    program: Mutex<Said>,
    /// The way to the program, once it runs: what it asks the terminal is
    /// answered from the reader thread.
    input: OnceLock<EventLoopSender>,
    /// What a program is answered that asks for a color it did not set,
    /// and for the size of the window: kept here, because the reader
    /// answers and must not wait for the main thread.
    palette: Box<[Color; PALETTE]>,
    grid: Mutex<Grid>,
    /// The text a program put on the clipboard and on the selection last,
    /// until the main thread takes it. A program can put text there
    /// faster than anyone takes it, megabytes each time.
    clipboard: Mutex<[Option<String>; 2]>,
    _lifetime: ReaderLifetime,
}

/// What a program said of its own state, and what of it the main thread
/// was told is another and has not looked at yet.
#[derive(Default)]
struct Said {
    records: Records,
    /// The one state to show, or that something is done again.
    state: bool,
    /// What the program runs beside itself.
    subs: bool,
}

impl Said {
    /// What the main thread takes up when it looks: the one state, whether
    /// it was said anew, and what the program runs beside itself. Nothing
    /// is left to look at after, so the next change is told again.
    fn take(&mut self) -> Event {
        let anew = std::mem::take(&mut self.state);
        self.subs = false;
        Event::Program { state: self.records.state(), anew, subs: self.records.subs() }
    }
}

impl Shared {
    fn new(palette: Box<[Color; PALETTE]>, grid: Grid) -> Self {
        Self {
            pending: AtomicBool::new(false),
            title: Mutex::new(None),
            bell: AtomicBool::new(false),
            rejected: AtomicBool::new(false),
            program: Mutex::default(),
            input: OnceLock::new(),
            palette,
            grid: Mutex::new(grid),
            clipboard: Mutex::default(),
            _lifetime: ReaderLifetime::begin(),
        }
    }
}

#[derive(Clone)]
struct Listener {
    notify: Notify,
    shared: Arc<Shared>,
}

/// Where the text a program stored waits: for the clipboard, or for the
/// selection that the middle button pastes.
fn stored(kind: ClipboardType) -> usize {
    usize::from(kind == ClipboardType::Selection)
}

/// Queues bytes for the program. Says whether there was room for them.
fn queue(sender: &EventLoopSender, bytes: Cow<'static, [u8]>) -> bool {
    // The PTY writer never finishes an empty buffer: it keeps asking
    // whether it may write, and nothing queued behind it gets out.
    bytes.is_empty() || !matches!(sender.send(Msg::Input(bytes)), Err(EventLoopSendError::Full))
}

impl Listener {
    fn rejected(&self) {
        if !self.shared.rejected.swap(true, Ordering::AcqRel) {
            (self.notify)(Report(Raw::InputRejected));
        }
    }

    /// Writes the program what it asked for, from the reader thread.
    fn answer(&self, sender: &EventLoopSender, text: String) {
        if !queue(sender, text.into_bytes().into()) {
            self.rejected();
        }
    }

    fn title(&self, title: Option<String>) {
        let first = self.shared.title.lock().unwrap().replace(title).is_none();
        if first {
            (self.notify)(Report(Raw::ResetTitle));
        }
    }

    /// Changes what is known of the state the program says it is in. The
    /// main thread is told when the one state shown of it is another, or
    /// when `change` says there is news all the same, and once until it
    /// looked.
    fn program(&self, change: impl FnOnce(&mut Records) -> News) {
        let mut said = self.shared.program.lock().unwrap();
        let before = said.records.state();
        let news = change(&mut said.records);
        let state = news.done || said.records.state() != before;
        let first = (state || news.subs) && !(said.state || said.subs);
        said.state |= state;
        said.subs |= news.subs;
        drop(said);
        if first {
            (self.notify)(Report(Raw::ProgramStatus(None)));
        }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Raw) {
        match event {
            Raw::Wakeup if self.shared.pending.swap(true, Ordering::AcqRel) => {}
            Raw::Bell if self.shared.bell.swap(true, Ordering::AcqRel) => {}
            Raw::Title(title) => self.title(Some(title)),
            Raw::ResetTitle => self.title(None),
            Raw::ProgramStatus(Some(report)) => self.program(|records| records.report(&report)),
            Raw::ProgramStatus(None) => self.program(|records| News { done: false, subs: records.clear() }),
            Raw::PromptStart => self.program(|records| News { done: false, subs: records.expire() }),
            // Nothing is done with these. A program can cause thousands of
            // them with one chunk of output.
            Raw::ClipboardLoad(..) | Raw::CursorBlinkingChange | Raw::MouseCursorDirty => {}
            // A program that asks where the cursor is, or what the terminal
            // is, can ask thousands of times in one chunk as well.
            Raw::PtyWrite(text) => match self.shared.input.get() {
                Some(sender) => self.answer(sender, text),
                None => (self.notify)(Report(Raw::PtyWrite(text))),
            },
            // And so with a color and with the size of the window. Each
            // question on its own way to the main thread piled up there
            // faster than it was taken, without a limit: they are
            // answered here, from what is kept for it, into the queue to
            // the program, which has one. The emulator says itself in
            // the answer which color a program set.
            Raw::ColorRequest(index, format) => match self.shared.input.get() {
                Some(sender) => {
                    let [r, g, b, _] = self.shared.palette[index.min(PALETTE - 1)];
                    self.answer(sender, format(Rgb { r, g, b }));
                }
                None => (self.notify)(Report(Raw::ColorRequest(index, format))),
            },
            Raw::TextAreaSizeRequest(format) => match self.shared.input.get() {
                Some(sender) => {
                    let size = self.shared.grid.lock().unwrap().window_size();
                    self.answer(sender, format(size));
                }
                None => (self.notify)(Report(Raw::TextAreaSizeRequest(format))),
            },
            // Text for the clipboard waits as titles do: the latest of
            // each kind, and one report of it under way.
            Raw::ClipboardStore(kind, text) => {
                let first = self.shared.clipboard.lock().unwrap()[stored(kind)].replace(text).is_none();
                if first {
                    (self.notify)(Report(Raw::ClipboardStore(kind, String::new())));
                }
            }
            event => (self.notify)(Report(event)),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Dimensions for Grid {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

impl Grid {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_w,
            cell_height: self.cell_h,
        }
    }
}

/// How much a click selects.
#[derive(Clone, Copy)]
pub enum Select {
    Cells,
    Words,
    Lines,
}

/// A position on the visible grid: column, row, and whether the pointer is
/// in the right half of the cell.
#[derive(Clone, Copy, PartialEq)]
pub struct CellAt {
    pub col: usize,
    pub row: usize,
    pub right: bool,
}

/// How a terminal is set up, apart from what it runs.
pub struct Options {
    pub scrollback: usize,
    pub shift_enter: bool,
    /// Variables added to the environment of the program.
    pub env: Vec<(String, String)>,
    /// The colors a program is told when it asks for one it did not set.
    pub palette: Box<[Color; PALETTE]>,
}

impl Default for Options {
    fn default() -> Self {
        Self { scrollback: 0, shift_enter: false, env: Vec::new(), palette: Box::new([[0; 4]; PALETTE]) }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    /// The process id of the program.
    pub pid: u32,
    grid: Grid,
    exit_code: Option<i32>,
    /// Send Shift+Enter as Alt+Enter while the program uses plain key codes.
    shift_enter: bool,
    /// The modes as of the last draw, for decisions that are made on every
    /// pointer motion and must not wait for the emulator lock.
    mode: Cell<TermMode>,
    /// The listener of the emulator, with what it shares with the terminal:
    /// the reports that are under way, and the title that waits.
    listener: Listener,
    snapshot: RefCell<draw::Snapshot>,
    /// The last frame drew cells of an earlier one: the emulator was busy.
    stale: Cell<bool>,
}

impl Terminal {
    /// Starts `program` in `cwd`. `env` is added to the inherited
    /// environment, `notify` is called from the reader thread.
    pub fn spawn(
        program: &str,
        args: Vec<String>,
        cwd: &Path,
        grid: Grid,
        options: &Options,
        notify: impl Fn(Report) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let shared = Shared::new(options.palette.clone(), grid);
        let listener = Listener { notify: Arc::new(notify), shared: Arc::new(shared) };
        let config = Config {
            scrolling_history: options.scrollback,
            kitty_keyboard: true,
            // Programs may write the clipboard, never read it.
            osc52: Osc52::OnlyCopy,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &grid, listener.clone())));

        let mut env: HashMap<String, String> = options.env.iter().cloned().collect();
        for (key, value) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "codn")]
        {
            env.insert(key.to_owned(), value.to_owned());
        }
        // No shell stands between codn and the program to correct the folder
        // codn itself was started in.
        if let Some(cwd) = cwd.to_str() {
            env.insert("PWD".to_owned(), cwd.to_owned());
        }
        let tty_options = tty::Options {
            shell: Some(tty::Shell::new(program.to_owned(), args)),
            working_directory: Some(cwd.to_owned()),
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&tty_options, grid.window_size(), 0)?;
        let pid = pty.child().id();
        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, true, false)?;
        let sender = event_loop.channel();
        let _ = listener.shared.input.set(sender.clone());
        event_loop.spawn();
        Ok(Self {
            term,
            sender,
            pid,
            grid,
            exit_code: None,
            shift_enter: options.shift_enter,
            mode: Cell::new(TermMode::empty()),
            listener,
            snapshot: RefCell::default(),
            stale: Cell::new(false),
        })
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if !queue(&self.sender, bytes.into()) {
            self.listener.rejected();
        }
    }

    pub fn resize(&mut self, grid: Grid) {
        if grid != self.grid {
            self.grid = grid;
            *self.listener.shared.grid.lock().unwrap() = grid;
            self.term.lock().resize(grid);
            let _ = self.sender.send(Msg::Resize(grid.window_size()));
        }
    }

    /// Interprets a report from the emulator. Requests that only need an
    /// answer on the PTY are answered here.
    pub fn handle(&mut self, report: Report, palette: &[Color; PALETTE]) -> Option<Event> {
        match report.0 {
            Raw::Wakeup => {
                // Cleared before the screen is read: output from here on
                // reports again.
                self.listener.shared.pending.store(false, Ordering::Release);
                Some(Event::Output)
            }
            // Either says that a title waits, the latest of any number.
            Raw::Title(_) | Raw::ResetTitle => {
                let title = self.listener.shared.title.lock().unwrap().take()?;
                Some(Event::Title(title.map(|title| title.chars().take(MAX_TITLE).collect())))
            }
            // Both cleared before they are acted on, like output.
            Raw::Bell => {
                self.listener.shared.bell.store(false, Ordering::Release);
                Some(Event::Bell)
            }
            Raw::InputRejected => {
                self.listener.shared.rejected.store(false, Ordering::Release);
                Some(Event::InputRejected)
            }
            Raw::SelectionRejected => Some(Event::SelectionRejected),
            // Either says that what to show of the program is another,
            // not what. Cleared before it is read, like output.
            Raw::ProgramStatus(_) | Raw::PromptStart => {
                Some(self.listener.shared.program.lock().unwrap().take())
            }
            // Says that text of this kind waits, the latest of any number.
            Raw::ClipboardStore(kind, _) => {
                let text = self.listener.shared.clipboard.lock().unwrap()[stored(kind)].take()?;
                Some(Event::Clipboard(kind == ClipboardType::Selection, text))
            }
            Raw::ChildExit(status) => {
                self.exit_code = status.code().or(Some(1));
                None
            }
            Raw::Exit => Some(Event::Exited(self.exit_code)),
            Raw::PtyWrite(text) => {
                self.write(text.into_bytes());
                None
            }
            Raw::TextAreaSizeRequest(format) => {
                self.write(format(self.grid.window_size()).into_bytes());
                None
            }
            // Only where the reader had no way to the program yet. The
            // emulator is not asked, which the reader may hold for long.
            Raw::ColorRequest(index, format) => {
                let [r, g, b, _] = palette[index.min(PALETTE - 1)];
                self.write(format(Rgb { r, g, b }).into_bytes());
                None
            }
            Raw::ClipboardLoad(..) | Raw::CursorBlinkingChange | Raw::MouseCursorDirty => None,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn titles_wait_as_one_report_and_the_latest_is_taken() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let listener = Listener {
            notify: Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
            shared: Arc::new(Shared::new(
                Box::new([[0; 4]; PALETTE]),
                Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            )),
        };
        for n in 0..1000 {
            listener.send_event(Raw::Title(n.to_string()));
            // Reports nobody acts on do not leave the reader thread.
            listener.send_event(Raw::MouseCursorDirty);
            listener.send_event(Raw::CursorBlinkingChange);
        }
        assert_eq!(reports.load(Ordering::Relaxed), 1);
        assert_eq!(listener.shared.title.lock().unwrap().take(), Some(Some("999".to_owned())));
        // Once it was taken, the next title reports again, also its removal.
        listener.send_event(Raw::ResetTitle);
        assert_eq!(reports.load(Ordering::Relaxed), 2);
        assert_eq!(listener.shared.title.lock().unwrap().take(), Some(None));
        // A thousand bells are one report until it was taken up.
        for _ in 0..1000 {
            listener.send_event(Raw::Bell);
        }
        assert_eq!(reports.load(Ordering::Relaxed), 3);
        listener.shared.bell.store(false, Ordering::Release);
        listener.send_event(Raw::Bell);
        assert_eq!(reports.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn what_a_program_asks_of_the_terminal_is_answered_without_the_window_thread() {
        // A question for a color or for the size of the window was a
        // report of its own to the window thread, and a program asks
        // thousands of them in one chunk of output: they piled up there
        // faster than they were taken, three gigabytes in seconds.
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let mut palette = [[0; 4]; PALETTE];
        palette[11] = [1, 2, 3, 255];
        let options = Options { palette: Box::new(palette), ..Options::default() };
        let grid = Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
        let terminal =
            Terminal::spawn("/bin/sleep", vec!["30".into()], Path::new("/"), grid, &options, move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap();
        let colors = Arc::new(Mutex::new(Vec::new()));
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let before = reports.load(Ordering::Relaxed);
        for _ in 0..5000 {
            let (color, size) = (colors.clone(), sizes.clone());
            terminal.listener.send_event(Raw::ColorRequest(
                11,
                Arc::new(move |rgb| {
                    color.lock().unwrap().push((rgb.r, rgb.g, rgb.b));
                    String::new()
                }),
            ));
            terminal.listener.send_event(Raw::TextAreaSizeRequest(Arc::new(move |window| {
                size.lock().unwrap().push((window.num_cols, window.num_lines, window.cell_width));
                String::new()
            })));
        }
        assert_eq!(reports.load(Ordering::Relaxed) - before, 0, "nothing of it went to the window thread");
        // Each was answered, with the color codn draws and the size it has.
        let (colors, sizes) = (colors.lock().unwrap(), sizes.lock().unwrap());
        assert!(colors.len() == 5000 && colors.iter().all(|color| *color == (1, 2, 3)));
        assert!(sizes.len() == 5000 && sizes.iter().all(|size| *size == (80, 24, 8)));
    }

    #[test]
    fn a_program_that_asks_for_a_color_without_end_is_one_report_of_output() {
        // The same with a program and the reader that reads it. It never
        // reads the answers: the queue to it fills and says so once.
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let script = "q=$(printf '\\033]11;?\\007'); while :; do printf '%s%s%s%s%s%s%s%s' $q $q $q $q $q $q $q $q; done";
        let _terminal = Terminal::spawn(
            "/bin/sh",
            vec!["-c".into(), script.into()],
            Path::new("/"),
            Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            &Options::default(),
            move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(1));
        // Output, and at the most that there was no room for an answer.
        let reports = reports.load(Ordering::Relaxed);
        assert!((1..=2).contains(&reports), "{reports} reports in a second");
    }

    #[test]
    fn a_program_inherits_no_file_that_was_left_open() {
        // A library can leave a file open without saying that it closes
        // where a program is started: the drivers of graphics do, and
        // the program in a session then held their memory.
        // SAFETY: a path that ends in a zero, and no flag that closes it.
        let held = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
        assert!(held > 2);
        let seen = std::env::temp_dir().join(format!("codn-inherits-{}", std::process::id()));
        let script = format!("ls /proc/$$/fd > '{0}.part' && mv '{0}.part' '{0}'", seen.display());
        let _terminal = Terminal::spawn(
            "/bin/sh",
            vec!["-c".into(), script],
            Path::new("/"),
            Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            &Options::default(),
            |_| {},
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while !seen.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let open = std::fs::read_to_string(&seen).unwrap();
        let _ = std::fs::remove_file(&seen);
        // SAFETY: the descriptor is this test's own and used no more.
        unsafe { libc::close(held) };
        let open: Vec<i32> = open.lines().filter_map(|line| line.parse().ok()).collect();
        assert!(open.contains(&0) && !open.contains(&held), "{held} among {open:?}");
    }

    #[test]
    fn a_question_for_a_color_is_answered_with_the_one_a_program_set() {
        use alacritty_terminal::vte::ansi::Processor;
        // The emulator knows which color a program set, and says it in
        // the answer it makes: whoever answers need not ask the emulator,
        // which the reader has locked while it reads.
        let answers = Arc::new(Mutex::new(Vec::new()));
        let heard = answers.clone();
        let listener = Listener {
            notify: Arc::new(move |Report(raw)| {
                if let Raw::ColorRequest(_, format) = raw {
                    heard.lock().unwrap().push(format(Rgb { r: 9, g: 9, b: 9 }));
                }
            }),
            shared: Arc::new(Shared::new(
                Box::new([[0; 4]; PALETTE]),
                Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            )),
        };
        let grid = Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
        let mut term = Term::new(Config::default(), &grid, listener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b]11;?\x07\x1b]11;rgb:12/34/56\x07\x1b]11;?\x07");
        let answers = answers.lock().unwrap();
        assert_eq!(answers.len(), 2);
        assert!(answers[0].contains("rgb:0909/0909/0909"), "none set: the one it is given");
        assert!(answers[1].contains("rgb:1212/3434/5656"), "the one that was set: {}", answers[1]);
    }

    #[test]
    fn text_for_the_clipboard_waits_as_one_report_of_its_kind_and_the_latest_is_taken() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let terminal = Terminal::spawn(
            "/bin/sleep",
            vec!["30".into()],
            Path::new("/"),
            Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            &Options::default(),
            move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            },
        );
        let mut terminal = terminal.unwrap();
        let before = reports.load(Ordering::Relaxed);
        for n in 0..1000 {
            terminal.listener.send_event(Raw::ClipboardStore(ClipboardType::Clipboard, format!("c{n}")));
            terminal.listener.send_event(Raw::ClipboardStore(ClipboardType::Selection, format!("s{n}")));
        }
        assert_eq!(reports.load(Ordering::Relaxed) - before, 2, "one for each kind");
        let palette = [[0; 4]; PALETTE];
        let mut taken =
            |kind| match terminal.handle(Report(Raw::ClipboardStore(kind, String::new())), &palette) {
                Some(Event::Clipboard(selection, text)) => Some((selection, text)),
                _ => None,
            };
        assert_eq!(taken(ClipboardType::Clipboard), Some((false, "c999".to_owned())));
        assert_eq!(taken(ClipboardType::Selection), Some((true, "s999".to_owned())));
        // Taken once, and the next one reports again.
        assert_eq!(taken(ClipboardType::Clipboard), None);
        terminal.listener.send_event(Raw::ClipboardStore(ClipboardType::Clipboard, "again".into()));
        assert_eq!(reports.load(Ordering::Relaxed) - before, 3);
    }

    #[test]
    fn the_state_a_program_reports_is_told_when_it_is_another_and_once() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let listener = Listener {
            notify: Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
            shared: Arc::new(Shared::new(
                Box::new([[0; 4]; PALETTE]),
                Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            )),
        };
        let said = |report: &str| listener.send_event(Raw::ProgramStatus(Some(report.as_bytes().to_vec())));
        let state = || listener.shared.program.lock().unwrap().records.state();
        // The main thread takes a report up: it looks at what is known.
        let taken = || match listener.shared.program.lock().unwrap().take() {
            Event::Program { state, .. } => state,
            _ => unreachable!(),
        };
        // A program can report thousands of times in one chunk of output.
        for n in 0..1000 {
            said(if n % 2 == 0 { "state=working" } else { "state=idle" });
        }
        assert_eq!(reports.load(Ordering::Relaxed), 1);
        assert_eq!(taken(), Some(program::State::Idle));
        // The same state again, and a report that does not count, tell
        // nothing.
        said("state=idle");
        said("state=sleeping");
        said("state=blocked:title=a");
        assert_eq!(reports.load(Ordering::Relaxed), 1);
        said("state=blocked:id=sub");
        said("state=working:id=other");
        assert_eq!(reports.load(Ordering::Relaxed), 2);
        assert_eq!(taken(), Some(program::State::Blocked(None)));
        // A record that has an id is shown, so one more of them is told,
        // also where the one state stays. Said again as it is, it is not.
        said("state=idle:id=kept");
        assert_eq!((reports.load(Ordering::Relaxed), state()), (3, Some(program::State::Blocked(None))));
        assert_eq!(listener.shared.program.lock().unwrap().records.subs().len(), 3);
        // And it is not told as a state said anew: a session that was
        // looked at would count as not looked at for it.
        let news = |said: &Said| (said.state, said.subs);
        assert_eq!(news(&listener.shared.program.lock().unwrap()), (false, true));
        taken();
        for _ in 0..1000 {
            said("state=idle:id=kept");
            said("state=blocked:id=sub");
        }
        assert_eq!(reports.load(Ordering::Relaxed), 3);
        // A prompt of the shell ends what was being done, a terminal put
        // back as it was built ends everything.
        said("state=done:id=kept");
        assert_eq!((reports.load(Ordering::Relaxed), taken()), (4, Some(program::State::Blocked(None))));
        listener.send_event(Raw::PromptStart);
        assert_eq!((reports.load(Ordering::Relaxed), taken()), (5, Some(program::State::Done)));
        listener.send_event(Raw::PromptStart);
        assert_eq!(reports.load(Ordering::Relaxed), 5);
        listener.send_event(Raw::ProgramStatus(None));
        assert_eq!((reports.load(Ordering::Relaxed), taken()), (6, None));
        // What is done is told each time it is said, also when the state
        // shown stays the same: it was looked at, and is another result.
        said("state=done");
        assert_eq!((reports.load(Ordering::Relaxed), taken()), (7, Some(program::State::Done)));
        said("state=done");
        assert_eq!((reports.load(Ordering::Relaxed), taken()), (8, Some(program::State::Done)));
        // And said a thousand times before anyone looked, it is one report.
        for _ in 0..1000 {
            said("state=done");
        }
        assert_eq!(reports.load(Ordering::Relaxed), 9);
    }

    #[test]
    fn what_a_program_runs_beside_itself_is_told_again_once_it_was_looked_at() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let listener = Listener {
            notify: Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
            shared: Arc::new(Shared::new(
                Box::new([[0; 4]; PALETTE]),
                Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            )),
        };
        let said = |report: &str| listener.send_event(Raw::ProgramStatus(Some(report.as_bytes().to_vec())));
        // What the main thread takes up, as `Terminal::handle` does: how
        // many reports reached it, whether the state was said anew, and
        // how many rows there are.
        let taken = || match listener.shared.program.lock().unwrap().take() {
            Event::Program { anew, subs, .. } => (reports.load(Ordering::Relaxed), anew, subs.len()),
            _ => unreachable!(),
        };
        said("state=working");
        assert_eq!(taken(), (1, true, 0));
        // A row alone: told, and not as a state said anew.
        said("state=working:id=a");
        assert_eq!(taken(), (2, false, 1));
        // And the next one is told as well. Were the look not to clear
        // what it looked at, nothing of this terminal was ever told again.
        said("state=working:id=b");
        assert_eq!(taken(), (3, false, 2));
        said("state=blocked");
        assert_eq!(taken(), (4, true, 2));
        said("state=clear:id=a");
        assert_eq!(taken(), (5, false, 1));
    }

    #[test]
    fn a_program_that_asks_whether_its_state_is_taken_is_answered() {
        let (heard, hearing) = std::sync::mpsc::channel();
        // It asks, and tells in its title what came back. Then it reports.
        let script = "stty raw -echo; printf '\\033]7501;?\\033\\\\'; answer=$(dd bs=1 count=10 2>/dev/null | od -An -c | tr -d ' \\n'); \
            printf '\\033]0;%s\\007\\033]7501;state=blocked:kind=question\\007' \"$answer\"; sleep 5";
        let mut terminal = Terminal::spawn(
            "/bin/sh",
            vec!["-c".into(), script.into()],
            Path::new("/tmp"),
            Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            &Options::default(),
            move |report| drop(heard.send(report)),
        )
        .unwrap();
        let (mut title, mut state) = (None, None);
        let until = Instant::now() + Duration::from_secs(5);
        while (title.is_none() || state.is_none()) && Instant::now() < until {
            let Ok(report) = hearing.recv_timeout(Duration::from_millis(100)) else { continue };
            match terminal.handle(report, &[[0; 4]; PALETTE]) {
                Some(Event::Title(told)) => title = told,
                Some(Event::Program { state: told, .. }) => state = told,
                _ => {}
            }
        }
        assert_eq!(title.as_deref(), Some("033]7501;?033\\"));
        assert_eq!(state, Some(program::State::Blocked(Some(program::Kind::Question))));
    }

    #[test]
    fn final_shutdown_waits_for_an_owned_process_ignoring_hup_and_term() {
        let (ready, waiting) = std::sync::mpsc::channel();
        let terminal = Terminal::spawn(
            "/bin/sh",
            vec![
                "-c".into(),
                "trap '' HUP TERM; printf '\\033]0;ready\\007'; while :; do sleep 0.05; done".into(),
            ],
            Path::new("/tmp"),
            Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            &Options::default(),
            move |report| {
                // The report says that a title waits, not which.
                if matches!(report.0, Raw::ResetTitle) {
                    let _ = ready.send(());
                }
            },
        )
        .unwrap();
        waiting.recv_timeout(Duration::from_secs(3)).unwrap();
        let path = format!("/proc/{}/stat", terminal.pid);
        let birth = |text: &str| -> Option<String> {
            Some(text.rsplit_once(')')?.1.split_whitespace().nth(19)?.to_owned())
        };
        let started = birth(&std::fs::read_to_string(&path).unwrap());
        drop(terminal);
        assert!(finish_cleanup(Duration::from_secs(6)));
        let remaining = std::fs::read_to_string(path).ok().and_then(|text| birth(&text));
        assert_ne!(remaining, started, "the owned child survived final cleanup");
    }
}
