//! Bounded logging on a worker: errors must not wait for a disk or pipe.

use std::fmt::{self, Display, Write as FmtWrite};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::config::state_dir;

const MAX_LINE: usize = 8192;
const MAX_FILE: u64 = 1024 * 1024;
static LOGGER: OnceLock<SyncSender<Record>> = OnceLock::new();
enum Record {
    Line(String),
    Flush(std::sync::mpsc::Sender<()>),
}

struct Line(String);
impl FmtWrite for Line {
    /// What is said is foreign in part: the name of a file, what a
    /// library says of a fault. A character that controls a terminal is
    /// written as its name, a line break among them, so that a line is
    /// one line and nothing in it reads as a record of its own.
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for c in text.chars() {
            let before = self.0.len();
            match c.is_control() {
                true => self.0.extend(c.escape_default()),
                false => self.0.push(c),
            }
            if self.0.len() > MAX_LINE {
                self.0.truncate(before);
                return Err(fmt::Error);
            }
        }
        Ok(())
    }
}

fn logger() -> &'static SyncSender<Record> {
    LOGGER.get_or_init(|| {
        let dir = state_dir();
        let (tx, rx) = sync_channel(64);
        std::thread::spawn(move || {
            let mut console: Option<Box<dyn Write>> = Some(Box::new(std::io::stderr()));
            while let Ok(record) = rx.recv() {
                match record {
                    // Without the console where it would make this
                    // thread wait: the line is in the file.
                    Record::Line(message) if console_takes() => write(&dir, &message, &mut console),
                    Record::Line(message) => write(&dir, &message, &mut None),
                    Record::Flush(reply) => {
                        let _ = reply.send(());
                    }
                }
            }
        });
        tx
    })
}

/// Writes a line to the log file, then to the console while there is one.
/// The terminal codn was started from may be closed long before codn is,
/// and `eprintln!` panics when the line cannot be written.
fn write(dir: &Path, message: &str, console: &mut Option<Box<dyn Write>>) {
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
    let path = dir.join("codn.log");
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() >= MAX_FILE) {
        let _ = std::fs::rename(&path, dir.join("codn.log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        && file.metadata().is_ok_and(|m| m.is_file())
    {
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "{seconds} {message}");
    }
    if console.as_mut().is_some_and(|console| writeln!(console, "codn: {message}").is_err()) {
        *console = None;
    }
}

/// Whether the place for faults takes a line now without making the
/// worker wait. A pipe that nobody reads is full at some time, and a
/// worker that waits there writes no line to the file either, nor the
/// one that says why the app ended.
pub(crate) fn console_takes() -> bool {
    let mut asked = libc::pollfd { fd: 2, events: libc::POLLOUT, revents: 0 };
    // SAFETY: one descriptor that lives through the call, and no wait.
    unsafe { libc::poll(&mut asked, 1, 0) == 1 && asked.revents & libc::POLLOUT != 0 }
}

pub fn log(message: impl Display) {
    // The tests of the app say what they do in nobody's log: the folder
    // of the state is the user's own, and his log is his to read.
    if cfg!(test) {
        return;
    }
    let mut line = Line(String::new());
    let _ = write!(line, "{message}");
    let _ = logger().try_send(Record::Line(line.0));
}

/// Wait briefly for pending diagnostics at shutdown, never for a dead disk.
pub fn flush() {
    let Some(logger) = LOGGER.get() else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    if logger.try_send(Record::Flush(tx)).is_ok() {
        let _ = rx.recv_timeout(Duration::from_millis(100));
    }
}

pub fn install_panic_hook() {
    // Do not start a thread here: main still removes inherited environment
    // variables before any other thread exists.
    std::panic::set_hook(Box::new(|info| {
        log(format_args!("panic: {info}"));
        flush();
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `println!` and its kin end the process when the line cannot be
    /// written, and the terminal codn was started from may be closed long
    /// before codn is. Only what answers a command line and ends may use
    /// them: `main` before the window, and the two processes codn starts
    /// of itself, which say that they are not for a command line.
    #[test]
    fn nothing_of_the_running_app_prints_to_the_terminal() {
        fn read(folder: &Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    read(&path, found);
                    continue;
                }
                let answers = ["main.rs", "syntax/cutter.rs", "picture/child.rs"];
                if answers.iter().any(|name| path.ends_with(name)) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                // The tests of a file are a module at its end. The mark
                // of one also stands before a function only tests use.
                let marks = text.match_indices("#[cfg(test)]").map(|(at, _)| at);
                let module = |at: &usize| {
                    let mut lines = text[*at..].lines().skip(1);
                    lines
                        .find(|line| !line.trim_start().starts_with("#["))
                        .is_some_and(|l| l.contains("mod "))
                };
                let app = &text[..marks.into_iter().find(module).unwrap_or(text.len())];
                for print in ["println!(", "print!(", "dbg!("] {
                    if app.contains(print) {
                        found.push(format!("{print} in {}", path.display()));
                    }
                }
            }
        }
        let mut found = Vec::new();
        read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn diagnostic_lines_have_a_unicode_safe_bound() {
        let mut line = Line(String::new());
        let _ = write!(line, "{}", "😀".repeat(MAX_LINE));
        assert_eq!(line.0.len(), MAX_LINE);
        assert!(line.0.is_char_boundary(line.0.len()));
    }

    #[test]
    fn a_line_of_the_log_is_one_line_whatever_is_said_in_it() {
        // The name of a file is said in the log, and a name holds any
        // byte but two: with a line break in it, what follows read as a
        // record of its own, with a time and words of its choosing.
        let mut line = Line(String::new());
        let said = "a.rs\n1700000000 all is well\r\u{1b}[2Kb\tc\u{7f}";
        let _ = write!(line, "{said}");
        assert_eq!(line.0, "a.rs\\n1700000000 all is well\\r\\u{1b}[2Kb\\tc\\u{7f}");
        // What stands for a character counts against the bound as it is written.
        let mut line = Line(String::new());
        let _ = write!(line, "{}", "\n".repeat(MAX_LINE));
        assert!(line.0.len() <= MAX_LINE && line.0.ends_with("\\n") && !line.0.contains('\n'));
    }

    /// The terminal codn was started from, closed since.
    struct Gone;
    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_console_that_is_gone_costs_no_line_and_is_not_tried_again() {
        let dir = std::env::temp_dir().join(format!("codn-log-{}", crate::core::uuid()));
        let mut console: Option<Box<dyn Write>> = Some(Box::new(Gone));
        write(&dir, "first", &mut console);
        assert!(console.is_none());
        write(&dir, "second", &mut console);
        let text = std::fs::read_to_string(dir.join("codn.log")).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(text.contains(" first\n") && text.contains(" second\n"), "{text}");
    }
}
