//! The app's side of the pictures: one thread that has the process for
//! them. It holds one process at the most and one wish that waits, and a
//! new wish takes the place of the one that waits. It sleeps on what the
//! process writes and on a way to wake it, both at once, so that a click
//! on the next picture ends the process of the one before at once. The
//! window's thread only sends and is told that an answer lies ready, and
//! one answer lies ready at the most. The thread starts with the first
//! picture, so that a codn that showed none has neither.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::wire::{self, FILE_MAX, HEAD, Head, Wish};
use crate::core::files::{self, Stamp};
use crate::core::picture::{Decoded, Refused};
use crate::core::{open_unblocking, wait_until};

/// How long a picture is waited for by the clock. The process ends
/// itself by its own time, which counts only while it reckons: one that
/// waits, on a disk or on its output, is ended by this.
const WATCH: Duration = Duration::from_secs(20);

/// A picture that is wanted: of which file, and how. `id` is a number of
/// the asker's, to know the answer by.
pub struct Asked {
    pub id: u64,
    pub path: PathBuf,
    pub wish: Wish,
}

pub enum Request {
    Picture(Asked),
    /// No picture is wanted any more: the process ends, and what waits
    /// is given up.
    Close,
}

pub enum Outcome {
    /// The pixels, four bytes each, multiplied by their alpha.
    Picture { decoded: Decoded, pixels: Vec<u8> },
    /// The process said why there is none.
    Refused(Refused),
    /// The file could not be opened, and why.
    Unread(String),
    /// The process gave no answer that is one: it ended, it took too
    /// long, or it said what does not fit what it was asked.
    Failed,
}

pub struct Reply {
    pub id: u64,
    /// The state the file was in when it was opened, where it was.
    pub stamp: Option<Stamp>,
    /// Whether it was in the same state when the answer was there. Not,
    /// and a program wrote it meanwhile.
    pub steady: bool,
    pub outcome: Outcome,
}

/// A process that decodes, or what stands in for one in a test.
struct Running {
    child: Option<Child>,
    from: File,
}

enum Stopped {
    /// Something newer is wanted.
    Woken,
    TookTooLong,
    Gone,
}

impl Running {
    /// codn itself, started as the process for a picture, with the file
    /// as its input.
    fn start(file: File, wish: &Wish) -> std::io::Result<Self> {
        let mut command = Command::new("/proc/self/exe");
        command.arg0("codn").arg(super::ARGUMENT).args(wish.arguments());
        Self::of(command, file)
    }

    fn of(mut command: Command, file: File) -> std::io::Result<Self> {
        let mut child = command
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::from(file))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let from: Option<OwnedFd> = child.stdout.take().map(Into::into);
        // The pipe does not block from here on: a process that stops
        // writing is waited for with the watch and no longer.
        let unblocked = from.as_ref().is_some_and(|from| unsafe {
            let flags = libc::fcntl(from.as_raw_fd(), libc::F_GETFL);
            flags != -1 && libc::fcntl(from.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) != -1
        });
        match from {
            Some(from) if unblocked => Ok(Self { child: Some(child), from: File::from(from) }),
            _ => {
                let _ = child.kill();
                wait_until(child, Instant::now() + Duration::from_secs(1));
                Err(ErrorKind::BrokenPipe.into())
            }
        }
    }

    /// Fills `into` with what the process writes, until `until`, or
    /// until something newer is wanted.
    fn read(&mut self, mut into: &mut [u8], until: Instant, shared: &Shared) -> Result<(), Stopped> {
        while !into.is_empty() {
            let left = until.checked_duration_since(Instant::now()).ok_or(Stopped::TookTooLong)?;
            let mut waited = [
                libc::pollfd { fd: self.from.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: shared.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            ];
            let millis = left.as_millis().min(i32::MAX as u128 - 1) as i32 + 1;
            match unsafe { libc::poll(waited.as_mut_ptr(), 2, millis) } {
                0 => continue,
                1.. => {}
                _ if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted => continue,
                _ => return Err(Stopped::Gone),
            }
            // A call to wake says that something may be wanted, not
            // that something is: that is asked.
            if waited[1].revents != 0 && shared.newer() {
                return Err(Stopped::Woken);
            }
            match self.from.read(into) {
                Ok(0) => return Err(Stopped::Gone),
                Ok(read) => into = &mut into[read..],
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => return Err(Stopped::Gone),
            }
        }
        Ok(())
    }

    /// Ends the process, whatever it is doing: what it writes is read by
    /// nobody from here on, and it is not waited for longer than a
    /// second.
    fn stop(mut self) {
        let child = self.child.take();
        drop(self);
        if let Some(mut child) = child {
            let _ = child.kill();
            wait_until(child, Instant::now() + Duration::from_secs(1));
        }
    }
}

/// Starts a process for a file and a wish.
type Start = Box<dyn Fn(File, &Wish) -> std::io::Result<Running> + Send>;

/// How a process is started and how long its answer is waited for. Tests
/// put others in the place of both.
struct How {
    start: Start,
    watch: Duration,
}

/// What the thread and the one that asks it share.
struct Shared {
    /// What is wanted next. A new wish takes the place of this one.
    wanted: Mutex<Option<Request>>,
    /// The answer that lies ready. A newer one takes its place.
    ready: Mutex<Option<Reply>>,
    /// Written to when something is wanted, to wake the thread wherever
    /// it waits.
    wake: OwnedFd,
    /// Nobody asks any more.
    over: AtomicBool,
}

impl Shared {
    fn wake(&self) {
        let one = 1u64.to_ne_bytes();
        unsafe { libc::write(self.wake.as_raw_fd(), one.as_ptr().cast(), one.len()) };
    }

    /// Sleeps until something is wanted.
    fn sleep(&self) {
        let mut waited = libc::pollfd { fd: self.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut waited, 1, -1) };
    }

    /// Takes back the calls to wake. Only while `wanted` is held: a wish
    /// and the call for it are put and taken together, or a call is
    /// left behind with nothing wanted, and ends the next decoding.
    fn quiet(&self) {
        let mut count = [0u8; 8];
        unsafe { libc::read(self.wake.as_raw_fd(), count.as_mut_ptr().cast(), count.len()) };
    }

    /// Takes what is wanted, and with it the call to wake.
    fn take(&self) -> Option<Request> {
        let mut wanted = self.wanted.lock().ok()?;
        self.quiet();
        wanted.take()
    }

    /// Whether something is wanted that the decoding at hand has to give
    /// way to. A call to wake with nothing behind it is taken back.
    fn newer(&self) -> bool {
        if self.over.load(Ordering::Acquire) {
            return true;
        }
        let Ok(wanted) = self.wanted.lock() else { return true };
        if wanted.is_none() {
            self.quiet();
        }
        wanted.is_some()
    }
}

/// Where the wishes for pictures go.
pub struct Requests {
    shared: Option<Arc<Shared>>,
    started: bool,
    tell: Arc<dyn Fn() -> bool + Send + Sync>,
    how: fn() -> How,
}

impl Requests {
    /// `tell` is called on the thread that has the process, when an
    /// answer lies ready to be taken, and returns whether anyone is
    /// still listening.
    pub fn new(tell: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self::with(tell, || How { start: Box::new(Running::start), watch: WATCH })
    }

    fn with(tell: impl Fn() -> bool + Send + Sync + 'static, how: fn() -> How) -> Self {
        let wake = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        let shared = (wake >= 0).then(|| {
            Arc::new(Shared {
                wanted: Mutex::new(None),
                ready: Mutex::new(None),
                wake: unsafe { OwnedFd::from_raw_fd(wake) },
                over: AtomicBool::new(false),
            })
        });
        Self { shared, started: false, tell: Arc::new(tell), how }
    }

    /// Says whether the wish was handed over. A wish that was not taken
    /// up yet gives way to it, and so does a picture that is being
    /// decoded.
    pub fn send(&mut self, request: Request) -> bool {
        let Some(shared) = &self.shared else { return false };
        // Nothing has to be ended where nothing was ever started.
        if !self.started && matches!(request, Request::Close) {
            return true;
        }
        if !self.started {
            let (shared, tell, how) = (shared.clone(), self.tell.clone(), self.how);
            let work = move || run(&shared, how(), &*tell);
            if std::thread::Builder::new().name("codn pictures".into()).spawn(work).is_err() {
                return false;
            }
            self.started = true;
        }
        let Ok(mut wanted) = shared.wanted.lock() else { return false };
        *wanted = Some(request);
        // While it is held: see `Shared::quiet`.
        shared.wake();
        true
    }

    /// The answer that lies ready, if one does.
    pub fn take(&self) -> Option<Reply> {
        self.shared.as_ref()?.ready.lock().ok()?.take()
    }
}

impl Drop for Requests {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.over.store(true, Ordering::Release);
            shared.wake();
        }
    }
}

fn run(shared: &Shared, how: How, tell: &(dyn Fn() -> bool + Send + Sync)) {
    while !shared.over.load(Ordering::Acquire) {
        let Some(request) = shared.take() else {
            shared.sleep();
            continue;
        };
        let Request::Picture(asked) = request else { continue };
        // Nothing, and something newer is wanted: this one is given up.
        let Some(reply) = decode(shared, &how, &asked) else { continue };
        // Held against what is wanted now, before it is handed on.
        let newer = shared.wanted.lock().is_ok_and(|wanted| wanted.is_some());
        if newer {
            continue;
        }
        if let Ok(mut ready) = shared.ready.lock() {
            *ready = Some(reply);
        }
        if !tell() {
            break;
        }
    }
}

/// Opens the file, has a process make pixels of it, and waits for those.
/// Nothing where something newer was wanted meanwhile.
fn decode(shared: &Shared, how: &How, asked: &Asked) -> Option<Reply> {
    let reply = |stamp, steady, outcome| Some(Reply { id: asked.id, stamp, steady, outcome });
    // The file is handed over open: the process is told no path.
    let file = match open_unblocking(&asked.path) {
        Ok(file) => file,
        Err(error) => return reply(None, true, Outcome::Unread(files::reason(&error))),
    };
    let stamp = match file.metadata() {
        Ok(meta) if meta.is_file() => files::stamp(&meta),
        Ok(meta) if meta.is_dir() => return reply(None, true, Outcome::Unread("It is a folder.".into())),
        Ok(_) => return reply(None, true, Outcome::Unread("It is not a regular file.".into())),
        Err(error) => return reply(None, true, Outcome::Unread(files::reason(&error))),
    };
    if stamp.size > FILE_MAX {
        return reply(Some(stamp), true, Outcome::Refused(Refused::Long));
    }
    // Asked again when the answer is there: whether a program wrote the
    // file meanwhile.
    let state = file.try_clone().ok();
    let Ok(mut running) = (how.start)(file, &asked.wish) else {
        return reply(Some(stamp), true, Outcome::Failed);
    };
    let until = Instant::now() + how.watch;
    let outcome = match answer(&mut running, &asked.wish, until, shared) {
        Ok(outcome) => outcome,
        Err(Stopped::Woken) => {
            running.stop();
            return None;
        }
        Err(Stopped::TookTooLong | Stopped::Gone) => Outcome::Failed,
    };
    running.stop();
    let after = state.and_then(|state| state.metadata().ok()).map(|meta| files::stamp(&meta));
    reply(Some(stamp), after == Some(stamp), outcome)
}

/// Reads the answer of a process: its head, and after a head that says
/// so the pixels. What the head says is held against what was asked
/// before memory is asked for by it, and no byte more is read.
fn answer(running: &mut Running, wish: &Wish, until: Instant, shared: &Shared) -> Result<Outcome, Stopped> {
    let mut head = [0; HEAD];
    running.read(&mut head, until, shared)?;
    let decoded = match Head::read(&head) {
        Some(Head(Ok(decoded))) => decoded,
        Some(Head(Err(reason))) => return Ok(Outcome::Refused(reason)),
        None => return Ok(Outcome::Failed),
    };
    // Of the part that was asked for, and of no other.
    let asked = wish.takes(decoded.size) && decoded.part == wish.part_of(decoded.picture);
    let Some(length) = wire::bytes_of(decoded.size).filter(|_| asked) else {
        return Ok(Outcome::Failed);
    };
    let mut pixels = Vec::new();
    if pixels.try_reserve_exact(length).is_err() {
        return Ok(Outcome::Refused(Refused::Memory));
    }
    pixels.resize(length, 0);
    running.read(&mut pixels, until, shared)?;
    Ok(Outcome::Picture { decoded, pixels })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use super::*;
    use crate::core::picture::{Format, Notes};

    /// A head for a picture of one pixel, whole, and that pixel.
    const ONE: &str = r"printf 'cdnp\002\000\000\000\001\000\000\000\001\000\000\000\000\000\000\000\000\000\000\000\001\000\000\000\001\000\000\000\001\000\000\000\001\000\000\000\011\022\033\377'";

    /// What stands in for the process: a shell that says this.
    fn says(script: &str, file: File) -> std::io::Result<Running> {
        let mut shell = Command::new("/bin/sh");
        shell.arg("-c").arg(script);
        Running::of(shell, file)
    }

    /// By the room that is asked for: 1 never answers, 2 answers a
    /// picture that is larger than was asked for, whole and with all its
    /// pixels, 3 stops in the middle of its pixels, 4 says why there is
    /// no picture, 5 says nonsense, 6 answers another part than was asked
    /// for, and any other answers a picture.
    fn how() -> How {
        let start = |file: File, wish: &Wish| match wish.room.0 {
            1 => says("exec /bin/sleep 60", file),
            2 => {
                // A head that is one in itself: 104 pixels a side, of a
                // picture of that size, and every byte of them after it.
                let large =
                    [r"\150\000\000\000", r"\150\000\000\000", r"\000\000\000\000", r"\000\000\000\000"]
                        .concat()
                        + &r"\150\000\000\000".repeat(4);
                let head =
                    format!("printf 'cdnp\\002\\000\\000\\000{large}'; exec /bin/head -c 43264 /dev/zero");
                says(&head, file)
            }
            3 => says(&ONE.replace(r"\033\377", ""), file),
            4 => says(
                r"printf 'cdnp\002\005\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000'",
                file,
            ),
            5 => says("printf 'what is this, and where is the head of it'", file),
            // A head that is one in itself and fits the room: one pixel
            // from the middle of a picture of eight a side, where the
            // whole was asked.
            6 => says(
                r"printf 'cdnp\002\000\000\000\010\000\000\000\010\000\000\000\005\000\000\000\005\000\000\000\001\000\000\000\001\000\000\000\001\000\000\000\001\000\000\000\011\022\033\377'",
                file,
            ),
            _ => says(ONE, file),
        };
        How { start: Box::new(start), watch: Duration::from_millis(400) }
    }

    fn patient() -> How {
        How { watch: Duration::from_secs(60), ..how() }
    }

    fn never_started() -> How {
        How { start: Box::new(|_, _| Err(ErrorKind::NotFound.into())), watch: Duration::from_secs(1) }
    }

    fn requests(how: fn() -> How) -> (Requests, Receiver<()>) {
        let (tell, told) = channel();
        (Requests::with(move || tell.send(()).is_ok(), how), told)
    }

    fn file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("codn-picture-{name}-{}", std::process::id()));
        std::fs::write(&path, b"anything").unwrap();
        path
    }

    fn asked(id: u64, path: &std::path::Path, room: u32) -> Request {
        let wish = Wish { format: Format::Png, room: (room, room), side: 4096, whole: 0, part: [0; 4] };
        Request::Picture(Asked { id, path: path.to_owned(), wish })
    }

    fn answer(requests: &Requests, told: &Receiver<()>) -> Reply {
        told.recv_timeout(Duration::from_secs(10)).expect("an answer");
        requests.take().expect("it lies ready")
    }

    #[test]
    fn a_picture_comes_back_with_the_state_its_file_was_in() {
        let path = file("answer");
        let (mut requests, told) = requests(how);
        assert!(requests.take().is_none());
        assert!(requests.send(asked(7, &path, 100)));
        let reply = answer(&requests, &told);
        let Outcome::Picture { decoded, pixels } = reply.outcome else { panic!("a picture") };
        assert_eq!((reply.id, reply.steady), (7, true));
        assert_eq!(reply.stamp, files::stamp_of(&path));
        assert_eq!(decoded, Decoded::whole((1, 1), (1, 1), Notes(0)));
        assert_eq!(pixels, [9, 18, 27, 255]);
        // Taken once.
        assert!(requests.take().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_answer_that_is_none_is_no_picture() {
        let path = file("none");
        let (mut requests, told) = requests(how);
        let mut ask = |id, room| {
            assert!(requests.send(asked(id, &path, room)));
            let reply = answer(&requests, &told);
            assert_eq!(reply.id, id);
            reply.outcome
        };
        // More than was asked for, though every byte of it is there:
        // nothing is read of it. This was once asked with a head that was
        // none, which the test passed without the wish being held against
        // it.
        assert!(matches!(ask(1, 2), Outcome::Failed));
        // The pixels stop short, and the head is none.
        assert!(matches!(ask(2, 3), Outcome::Failed));
        assert!(matches!(ask(3, 5), Outcome::Failed));
        // Another part of the picture than was asked for.
        assert!(matches!(ask(5, 6), Outcome::Failed));
        // The process says why.
        assert!(matches!(ask(4, 4), Outcome::Refused(Refused::Apple)));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_process_that_never_answers_is_given_up_by_the_clock() {
        let path = file("silent");
        let (mut requests, told) = requests(how);
        let started = Instant::now();
        assert!(requests.send(asked(1, &path, 1)));
        assert!(matches!(answer(&requests, &told).outcome, Outcome::Failed));
        assert!(started.elapsed() >= Duration::from_millis(400), "not before the watch");
        // The thread goes on, and the next picture comes.
        assert!(requests.send(asked(2, &path, 100)));
        assert!(matches!(answer(&requests, &told).outcome, Outcome::Picture { .. }));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_newer_wish_ends_the_picture_before_at_once() {
        let path = file("newer");
        // The watch is a minute: what ends the first is the second.
        let (mut requests, told) = requests(patient);
        let started = Instant::now();
        assert!(requests.send(asked(1, &path, 1)));
        std::thread::sleep(Duration::from_millis(150));
        assert!(requests.send(asked(2, &path, 100)));
        let reply = answer(&requests, &told);
        assert!(reply.id == 2 && matches!(reply.outcome, Outcome::Picture { .. }));
        // Giving up says so too, and nothing comes back for it.
        assert!(requests.send(asked(3, &path, 1)));
        std::thread::sleep(Duration::from_millis(150));
        assert!(requests.send(Request::Close));
        assert!(told.recv_timeout(Duration::from_millis(400)).is_err() && requests.take().is_none());
        assert!(requests.send(asked(4, &path, 100)));
        assert_eq!(answer(&requests, &told).id, 4);
        assert!(started.elapsed() < Duration::from_secs(20));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn what_cannot_be_handed_to_a_process_starts_none() {
        let dir = std::env::temp_dir().join(format!("codn-picture-unread-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut requests, told) = requests(never_started);
        let mut ask = |path: &std::path::Path| {
            assert!(requests.send(asked(1, path, 100)));
            answer(&requests, &told)
        };
        assert!(
            matches!(ask(&dir.join("gone")).outcome, Outcome::Unread(why) if why == "It is not there any more.")
        );
        assert!(matches!(ask(&dir).outcome, Outcome::Unread(why) if why == "It is a folder."));
        // Longer than is read: said from its length alone.
        let long = dir.join("long");
        std::fs::File::create(&long).unwrap().set_len(FILE_MAX + 1).unwrap();
        let reply = ask(&long);
        assert!(matches!(reply.outcome, Outcome::Refused(Refused::Long)) && reply.stamp.is_some());
        // And one that is not: the process that cannot be started.
        std::fs::File::create(&long).unwrap().set_len(FILE_MAX).unwrap();
        assert!(matches!(ask(&long).outcome, Outcome::Failed));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_call_to_wake_with_nothing_behind_it_ends_no_decoding() {
        // A wish that is sent while the thread takes the one before it
        // leaves a call to wake behind, with nothing wanted. The decoding
        // that began then was ended by it, and its wish was gone: the pane
        // said "Reading…" for good.
        let path = file("stale");
        let (requests, _told) = requests(patient);
        let shared = requests.shared.as_ref().unwrap();
        shared.wake();
        assert!(shared.wanted.lock().unwrap().is_none());
        let Request::Picture(asked) = asked(1, &path, 100) else { unreachable!() };
        let reply = decode(shared, &patient(), &asked).expect("nothing newer is wanted");
        assert!(matches!(reply.outcome, Outcome::Picture { .. }));
        // With something behind it, it does end one: and a wish and the
        // call to wake for it are taken together, so that none is left.
        *shared.wanted.lock().unwrap() = Some(Request::Close);
        shared.wake();
        let Request::Picture(silent) = super::tests::asked(2, &path, 1) else { unreachable!() };
        assert!(decode(shared, &patient(), &silent).is_none());
        assert!(matches!(shared.take(), Some(Request::Close)));
        let mut waited = libc::pollfd { fd: shared.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut waited, 1, 0) }, 0, "no call to wake is left");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn giving_up_where_nothing_was_asked_starts_no_thread() {
        let (mut requests, _told) = requests(how);
        assert!(requests.send(Request::Close) && !requests.started);
    }
}
