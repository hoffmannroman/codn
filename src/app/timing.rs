//! What `CODN_TIMING` prints: how long the start took, what frames cost, and
//! the time from a key, a turn of the wheel or the window showing again to
//! the frame that answers it. Nothing of it exists without the variable.
//!
//! These lines go to the terminal codn was started from: whoever sets the
//! variable is reading there.

use std::fmt::Arguments;
use std::io::Write;
use std::time::{Duration, Instant};

use crate::core::SessionId;
use crate::log::console_takes;

/// Prints a line of the timing, if it can be printed now. Whoever read
/// these lines may be gone, with the terminal codn was started from, or
/// may have stopped reading: a line that cannot be written is left out.
/// `eprintln!` ends the app there, with every session, and a pipe that is
/// full would make the window wait.
fn say(line: Arguments) {
    if console_takes() {
        let _ = writeln!(std::io::stderr().lock(), "codn: {line}");
    }
}

pub(super) struct Timing {
    /// When the process started, until the first frame is out.
    started: Option<Instant>,
    /// How long the parts of the start took, for the timing of the first
    /// frame: to the event loop, the window and its context, the wait for
    /// the fonts, the renderer.
    stages: Option<[Duration; 4]>,
    frames: Frames,
    pub(super) echo: Echo,
}

impl Timing {
    /// The timing of a process that started at `started`, if it is asked for.
    pub(super) fn from_env(started: Instant) -> Option<Self> {
        std::env::var_os("CODN_TIMING").map(|_| Self {
            started: Some(started),
            stages: None,
            frames: Frames::default(),
            echo: Echo::default(),
        })
    }

    /// The window is there. It was begun at `entered`, the fonts were
    /// waited for from `asked` to `there`, and the renderer was `made`.
    pub(super) fn staged(&mut self, entered: Instant, asked: Instant, there: Instant, made: Instant) {
        if let Some(started) = self.started {
            self.stages = Some([entered - started, asked - entered, there - asked, made - there]);
        }
    }

    /// The rows of the preview of a file were made.
    pub(super) fn laid(&self, rows: usize, width: f32, took: Duration) {
        say(format_args!(
            "a preview of {rows} rows, {width} pixels wide, was laid out in {:.2} ms",
            took.as_secs_f64() * 1e3
        ));
    }

    /// A frame that began at `begun` and took `built` to build is on its
    /// way to the display.
    pub(super) fn frame(&mut self, begun: Instant, built: Duration, quads: usize, stale: bool) {
        self.frames.add(quads, built, begun.elapsed() - built, stale);
        self.echo.shown(begun);
        if let Some(started) = self.started.take() {
            let ms = |d: Duration| d.as_secs_f64() * 1e3;
            say(format_args!("first frame after {:.1} ms, {quads} quads", ms(started.elapsed())));
            if let Some([event_loop, window, fonts, renderer]) = self.stages.take() {
                say(format_args!(
                    "of that {:.1} ms to the event loop, window and context {:.1}, \
                     waiting for the fonts {:.1}, renderer {:.1}",
                    ms(event_loop),
                    ms(window),
                    ms(fonts),
                    ms(renderer),
                ));
            }
        }
    }
}

/// What frames cost, added up and printed about once a second while frames
/// are drawn.
#[derive(Default)]
struct Frames {
    since: Option<Instant>,
    count: u32,
    quads: usize,
    /// Time to build the quads and hand them to the driver.
    build: Duration,
    slowest: Duration,
    present: Duration,
    /// Frames that drew a terminal as it was before: its emulator was busy.
    stale: u32,
}

impl Frames {
    fn add(&mut self, quads: usize, build: Duration, present: Duration, stale: bool) {
        let since = *self.since.get_or_insert_with(Instant::now);
        self.count += 1;
        self.stale += stale as u32;
        self.quads += quads;
        self.build += build;
        self.present += present;
        self.slowest = self.slowest.max(build);
        if since.elapsed() >= Duration::from_secs(1) {
            let ms = |d: Duration| d.as_secs_f64() * 1e3;
            say(format_args!(
                "{} frames, build {:.2} ms each, slowest {:.2} ms, present {:.2} ms, {} quads, {} stale",
                self.count,
                ms(self.build) / self.count as f64,
                ms(self.slowest),
                ms(self.present) / self.count as f64,
                self.quads / self.count as usize,
                self.stale,
            ));
            *self = Self::default();
        }
    }
}

/// From a key press to the frame that shows what the program answered, one
/// line per key.
///
/// The answer is the first output of the session after the key. That is the
/// echo as long as the session was quiet when the key came.
#[derive(Default)]
pub(super) struct Echo {
    /// The oldest key that went to a session and has not been answered.
    pressed: Option<(SessionId, Instant)>,
    /// The times of a key and of its answer, which waits for its frame.
    answered: Option<(Instant, Instant)>,
    /// Since when a turn of the wheel waits for its frame, and how many
    /// turns came since the last one was shown.
    wheeled: Option<Instant>,
    turns: u32,
    /// Since when the window is to be seen again and waits for its frame.
    uncovered: Option<Instant>,
}

/// A key that is not answered within this time was not echoed at all.
const NO_ECHO: Duration = Duration::from_secs(1);

impl Echo {
    pub(super) fn key(&mut self, id: SessionId, now: Instant) {
        // The oldest key that waits counts: typing ahead of the echo must
        // not make the echo look fast.
        if self.pressed.is_none_or(|(_, at)| now - at > NO_ECHO) {
            self.pressed = Some((id, now));
        }
    }

    pub(super) fn output(&mut self, id: SessionId, now: Instant) {
        if let Some((_, key)) = self.pressed.take_if(|(session, _)| *session == id)
            && self.answered.is_none()
        {
            self.answered = Some((key, now));
        }
    }

    /// The wheel was turned, at `changed` if that moved what is on screen.
    pub(super) fn wheel(&mut self, changed: Option<Instant>) {
        self.turns += 1;
        self.wheeled = self.wheeled.or(changed);
    }

    /// The window was covered or minimized, or is to be seen again.
    pub(super) fn occluded(&mut self, hidden: bool) {
        say(format_args!("the window is {}", if hidden { "hidden" } else { "to be seen again" }));
        self.uncovered = (!hidden).then(Instant::now);
    }

    /// A frame that began at `begun` is on its way to the display.
    fn shown(&mut self, begun: Instant) {
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        if let Some(uncovered) = self.uncovered.take() {
            say(format_args!("seen again to frame {:.2} ms", ms(uncovered.elapsed())));
        }
        if let Some(turned) = self.wheeled.take() {
            say(format_args!(
                "wheel to frame {:.2} ms: until the frame {:.2}, frame {:.2}, {} turns",
                ms(turned.elapsed()),
                ms(begun.saturating_duration_since(turned)),
                ms(begun.elapsed()),
                std::mem::take(&mut self.turns),
            ));
        }
        let Some((key, answer)) = self.answered.take() else { return };
        say(format_args!(
            "key to echo {:.2} ms: answer {:.2}, until the frame {:.2}, frame {:.2}",
            ms(key.elapsed()),
            ms(answer - key),
            ms(begun.saturating_duration_since(answer)),
            ms(begun.elapsed()),
        ));
    }
}
