//! Projects, sessions and their state. Nothing in here draws or knows the
//! window system.

pub mod colors;
pub mod config;
pub mod discover;
pub mod edit;
pub mod files;
mod fileview;
pub mod git;
pub mod history;
pub mod picture;
pub mod preview;
pub mod program;
pub mod reported;
pub mod save;
mod session;
pub mod shell;
pub mod state;
pub mod status;
pub mod text;
pub mod tree;
pub mod workspace;

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use fileview::{FileView, Reading, Said, Shown, Taken};
pub use reported::Reported;
pub use session::{Doing, Exit, Session, StartGate};
pub use status::{Activity, Status};
pub use workspace::{DiffView, Pane, Workspace};

/// Programs that were killed for taking too long and have not ended yet.
/// One that is stuck in the kernel, on a dead network mount, stays so after
/// the signal, and whoever starts programs counts these before starting more.
static UNREAPED: AtomicUsize = AtomicUsize::new(0);

/// Waits for a program to end until `deadline` and returns how it ended.
/// After that it is killed and left to a thread of its own, so that nobody
/// waits for a program that does not die.
pub fn wait_until(mut child: Child, deadline: Instant) -> Option<ExitStatus> {
    let mut tries = 0;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {
                // Most programs end right after their last output.
                tries += 1;
                std::thread::sleep(Duration::from_millis(if tries < 20 { 1 } else { 10 }));
            }
            _ => break,
        }
    }
    let _ = child.kill();
    UNREAPED.fetch_add(1, Ordering::Relaxed);
    std::thread::spawn(move || {
        let _ = child.wait();
        UNREAPED.fetch_sub(1, Ordering::Relaxed);
    });
    None
}

/// How many workers of one kind run side by side.
const WORKERS: usize = 3;

/// Runs `task` on a thread of its own, called `name`, unless as many of its
/// kind as may run are running: `count` is their number. What waits for a
/// filesystem can wait without end, and such threads must not pile up.
/// Says whether the task was taken.
pub fn spawn_bounded(
    count: &'static AtomicUsize,
    name: &'static str,
    task: impl FnOnce() + Send + 'static,
) -> bool {
    let mut value = count.load(Ordering::Acquire);
    loop {
        if value >= WORKERS {
            return false;
        }
        match count.compare_exchange_weak(value, value + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(actual) => value = actual,
        }
    }
    let result = std::thread::Builder::new().name(name.into()).spawn(move || {
        struct Done(&'static AtomicUsize);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Release);
            }
        }
        let _done = Done(count);
        task();
    });
    if result.is_err() {
        count.fetch_sub(1, Ordering::Release);
        return false;
    }
    true
}

/// Opens a file to read it, without waiting for a writer: what a path names
/// may be a pipe. Whether it is a regular file is asked of what was opened.
/// It may also be a terminal, and one that is opened must not become that
/// of the app: an app a launcher started leads its session and has none,
/// so it would take the first one it opens, and be ended by a signal when
/// the program at the other end of it lets go.
pub(crate) fn open_unblocking(path: &Path) -> std::io::Result<File> {
    File::options().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(path)
}

/// Identifies a project for as long as the app runs. Never reused.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProjectId(pub u32);

/// Identifies a session for as long as the app runs. Never reused.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(pub u32);

pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub git: Option<git::Info>,
}

/// The last component of a path, as a name to show.
fn folder_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// A random version 4 UUID.
pub fn uuid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut b = [0u8; 16];
    let random = File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if random.is_err() {
        // Without a random source, at least never hand out the same id twice.
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let unique = (std::process::id() as u64) << 32 | COUNTER.fetch_add(1, Ordering::Relaxed);
        b[..8].copy_from_slice(&nanos.to_le_bytes());
        b[8..].copy_from_slice(&unique.wrapping_mul(0x9e37_79b9_7f4a_7c15).to_le_bytes());
    }
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_waited_for_only_until_its_deadline() {
        let run = |script: &str| std::process::Command::new("/bin/sh").args(["-c", script]).spawn().unwrap();
        let soon = || Instant::now() + Duration::from_millis(300);
        assert_eq!(wait_until(run("exit 3"), soon()).and_then(|status| status.code()), Some(3));
        let begun = Instant::now();
        assert!(wait_until(run("sleep 30"), soon()).is_none());
        assert!(begun.elapsed() < Duration::from_secs(5));
        // The killed program ends on its thread and is counted off again.
        while UNREAPED.load(Ordering::Relaxed) > 0 {
            assert!(begun.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn workers_are_bounded_and_release_slots() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        for _ in 0..3 {
            let barrier = barrier.clone();
            assert!(spawn_bounded(&COUNT, "codn test", move || {
                barrier.wait();
            }));
        }
        assert!(!spawn_bounded(&COUNT, "codn test", || panic!("fourth worker must not run")));
        barrier.wait();
        let deadline = Instant::now() + Duration::from_secs(3);
        while COUNT.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    /// The terminal that controls this process, 0 if there is none: a
    /// field of its `stat`, behind the name, which may hold spaces.
    fn terminal() -> i64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        stat.rsplit_once(')').unwrap().1.split_whitespace().nth(4).unwrap().parse().unwrap()
    }

    /// Run by the test below in a process of its own, which leads a
    /// session and has no terminal, as a codn does that a launcher
    /// started. Does nothing where it runs among the others.
    #[test]
    fn opens_a_terminal_while_it_leads_a_session() {
        if std::env::var_os("CODN_TEST_LEADS").is_none() {
            return;
        }
        // SAFETY: calls of libc with a buffer that lives through them.
        let (master, name) = unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0 && libc::grantpt(master) == 0 && libc::unlockpt(master) == 0);
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(libc::ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
            (master, std::ffi::CStr::from_ptr(name.as_ptr()).to_str().unwrap().to_owned())
        };
        // SAFETY: both ask the kernel for a number and change nothing.
        assert!(unsafe { libc::getsid(0) == libc::getpid() } && terminal() == 0);
        assert!(open_unblocking(Path::new(&name)).is_ok());
        assert_eq!(terminal(), 0, "the terminal that was opened is now that of the process");
        // SAFETY: the descriptor is this function's own and used no more.
        unsafe { libc::close(master) };
    }

    #[test]
    fn a_terminal_that_is_opened_as_a_file_does_not_become_that_of_the_app() {
        use std::os::unix::process::CommandExt;
        // What a path names may be a terminal, by a link a program made.
        // A process that leads its session and has no terminal takes the
        // first one it opens for its own, and is ended by a signal when
        // the other end of that one closes.
        let mut inner = std::process::Command::new(std::env::current_exe().unwrap());
        inner.args(["--exact", "core::tests::opens_a_terminal_while_it_leads_a_session"]);
        inner.env("CODN_TEST_LEADS", "1").stdin(std::process::Stdio::null());
        // SAFETY: `setsid` alone, which may be called between fork and exec.
        unsafe {
            inner.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) })
        };
        let output = inner.output().unwrap();
        let said = String::from_utf8_lossy(&output.stdout);
        // One test ran there: a name that finds none would pass as well.
        assert!(output.status.success() && said.contains("1 passed"), "{said}");
    }

    #[test]
    fn uuids_are_well_formed_and_differ() {
        let (a, b) = (uuid(), uuid());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
    }
}
