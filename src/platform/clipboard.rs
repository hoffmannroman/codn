//! Clipboard requests run off the window thread. The providers are dropped
//! before the display connection they borrow.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use copypasta::ClipboardProvider;
use copypasta::x11_clipboard::{self, Primary, X11ClipboardContext};
use raw_window_handle::RawDisplayHandle;

const MAX_TEXT: usize = 8 * 1024 * 1024;
/// Reads that may wait for their turn.
const MAX_READS: usize = 8;
/// How long a read may wait for its turn. An owner of the selection that
/// does not answer holds every read up for its whole deadline, and what
/// waited behind it would be pasted when nobody expects it any more.
const STALE: Duration = Duration::from_secs(2);

/// Why a read brought no text where there was one to be had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failed {
    /// Whoever holds the text did not hand it over in time.
    Late,
    /// More text than is taken.
    TooLarge,
}

/// What a read brings: the text, which is empty where there is none, or
/// why none came.
pub type Read = Result<String, Failed>;
type Reply = Box<dyn FnOnce(Read) + Send>;
type Provider = Box<dyn ClipboardProvider>;
type Error = Box<dyn std::error::Error + Send + Sync>;

/// Why a read failed, where that is worth a word to the user: a clipboard
/// with nothing in it, or with no text, is an error to some providers
/// too. The two crates that read are patched for codn and say these two
/// in words of their own, which is all that can be asked of an error
/// here: neither of their types is known to this crate.
fn failed(error: &Error) -> Option<Failed> {
    let said = error.to_string().to_ascii_lowercase();
    if said.contains("exceeds 8 mib") {
        Some(Failed::TooLarge)
    } else if said.contains("timed out") || said.contains("did not answer") {
        Some(Failed::Late)
    } else {
        None
    }
}

#[derive(Default)]
struct Queue {
    /// Which selection, since when it waits, and where the text goes.
    reads: VecDeque<(bool, Instant, Reply)>,
    /// What to put on the clipboard and on the selection: the latest of
    /// each. A copy neither waits behind the reads nor finds the queue full.
    sets: [Option<String>; 2],
}

enum Job {
    Get(bool, Reply),
    Set(bool, String),
}

/// Stands in where the display gives no clipboard.
struct Absent;
impl ClipboardProvider for Absent {
    fn get_contents(&mut self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Err("there is no clipboard".into())
    }
    fn set_contents(&mut self, _: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

pub struct Clipboard {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    closed: Arc<AtomicBool>,
}

impl Clipboard {
    pub fn new(display: RawDisplayHandle) -> Self {
        let (clipboard, primary): (Provider, Option<Provider>) =
            if let RawDisplayHandle::Wayland(handle) = display {
                // Safety: App drops Clipboard (and joins its worker) before
                // dropping Gfx and its display connection.
                let (primary, clipboard) = unsafe {
                    copypasta::wayland_clipboard::create_clipboards_from_external(handle.display.as_ptr())
                };
                (Box::new(clipboard), Some(Box::new(primary)))
            } else {
                let clipboard = X11ClipboardContext::<x11_clipboard::Clipboard>::new()
                    .map_or_else(|_| Box::new(Absent) as Provider, |c| Box::new(c) as _);
                let primary = X11ClipboardContext::<Primary>::new().ok().map(|c| Box::new(c) as _);
                (clipboard, primary)
            };
        Self::with_providers(clipboard, primary)
    }

    fn with_providers(mut clipboard: Provider, mut primary: Option<Provider>) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (waiting, stopping) = (queue.clone(), closed.clone());
        let worker = std::thread::spawn(move || {
            while let Some(job) = next_job(&waiting, &stopping) {
                let selection = matches!(job, Job::Get(true, _) | Job::Set(true, _));
                let provider = if selection { primary.as_mut() } else { Some(&mut clipboard) };
                match job {
                    Job::Get(_, reply) => reply(match provider.map(|p| p.get_contents()) {
                        Some(Ok(text)) if text.len() > MAX_TEXT => Err(Failed::TooLarge),
                        Some(Ok(text)) => Ok(text),
                        Some(Err(error)) => failed(&error).map_or(Ok(String::new()), Err),
                        None => Ok(String::new()),
                    }),
                    Job::Set(_, text) => {
                        if let Some(provider) = provider {
                            let _ = provider.set_contents(text);
                        }
                    }
                }
            }
        });
        Self { queue, worker: Some(worker), closed }
    }

    /// Asks for the text of the clipboard, or of the selection. `reply` is
    /// called once, on another thread or at once, with nothing if there is
    /// no text to be had. Why none came is said only by a read that was
    /// made: one that found no room or waited too long has its reason in
    /// the read before it, which says it.
    pub fn get(&self, primary: bool, reply: impl FnOnce(Read) + Send + 'static) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        if queue.reads.len() >= MAX_READS {
            drop(queue);
            return reply(Ok(String::new()));
        }
        queue.reads.push_back((primary, Instant::now(), Box::new(reply)));
        wake.notify_one();
    }

    pub fn set(&self, primary: bool, text: String) {
        if text.len() <= MAX_TEXT {
            let (lock, wake) = &*self.queue;
            lock.lock().unwrap().sets[primary as usize] = Some(text);
            wake.notify_one();
        }
    }
}

/// What the worker does next: the latest copy before any read, and the
/// reads in their order. Waits while there is nothing, and returns nothing
/// once the clipboard is closing.
fn next_job(queue: &(Mutex<Queue>, Condvar), stopping: &AtomicBool) -> Option<Job> {
    let (lock, wake) = queue;
    let mut queue = lock.lock().unwrap();
    loop {
        // Only the in-flight provider call may delay shutdown.
        // Do not spend another timeout on every queued paste.
        if stopping.load(Ordering::Acquire) {
            return None;
        }
        if let Some(at) = queue.sets.iter().position(Option::is_some) {
            return Some(Job::Set(at == 1, queue.sets[at].take().unwrap()));
        }
        match queue.reads.pop_front() {
            // Every read is answered, this one with nothing.
            Some((_, asked, reply)) if asked.elapsed() > STALE => reply(Ok(String::new())),
            Some((primary, _, reply)) => return Some(Job::Get(primary, reply)),
            None => queue = wake.wait(queue).unwrap(),
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        // With the lock: the worker is then either before its check of
        // `closed` or waiting, and not in between the two.
        drop(self.queue.0.lock().unwrap());
        self.queue.1.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn a_selection_that_does_not_answer_loses_no_copy_and_pastes_nothing_late() {
        /// Answers only once it is let go, as an owner that hangs.
        struct Hung {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
            copied: Arc<Mutex<Vec<String>>>,
        }
        impl ClipboardProvider for Hung {
            fn get_contents(&mut self) -> Result<String> {
                let _ = self.entered.send(());
                let _ = self.release.recv_timeout(Duration::from_secs(10));
                Ok("late".into())
            }
            fn set_contents(&mut self, text: String) -> Result<()> {
                self.copied.lock().unwrap().push(text);
                Ok(())
            }
        }
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let copied = Arc::new(Mutex::new(Vec::new()));
        let clipboard = Clipboard::with_providers(
            Box::new(Hung { entered, release: blocked, copied: copied.clone() }),
            None,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let read = |clipboard: &Clipboard| {
            let tx = tx.clone();
            clipboard.get(false, move |text| drop(tx.send(text.unwrap())));
        };
        read(&clipboard);
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        // The key repeats while the owner hangs, and more than can wait.
        for _ in 0..MAX_READS + 2 {
            read(&clipboard);
        }
        // Two copies meanwhile: the queue is full, and the later one counts.
        clipboard.set(false, "first".into());
        clipboard.set(false, "second".into());
        std::thread::sleep(STALE + Duration::from_millis(100));
        release.send(()).unwrap();
        // The read that was under way has its text. Every other one was
        // answered with nothing: at once, or because it had waited too long.
        let mut texts: Vec<String> =
            (0..MAX_READS + 3).map(|_| rx.recv_timeout(Duration::from_secs(3)).unwrap()).collect();
        texts.retain(|text| !text.is_empty());
        assert_eq!(texts, ["late"]);
        drop(clipboard);
        assert_eq!(*copied.lock().unwrap(), ["second"]);
    }

    struct Slow;
    impl ClipboardProvider for Slow {
        fn get_contents(&mut self) -> Result<String> {
            std::thread::sleep(Duration::from_millis(100));
            Ok("pasted".into())
        }
        fn set_contents(&mut self, _: String) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_slow_selection_never_blocks_the_caller() {
        let clipboard = Clipboard::with_providers(Box::new(Slow), None);
        let (tx, rx) = std::sync::mpsc::channel();
        let begun = Instant::now();
        clipboard.get(false, move |text| {
            let _ = tx.send(text);
        });
        assert!(begun.elapsed() < Duration::from_millis(50));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap().as_deref(), Ok("pasted"));
    }

    #[test]
    fn a_read_says_why_no_text_came_where_there_was_one() {
        /// Answers each read with the next of what it was given.
        struct Says(std::vec::IntoIter<Result<String>>);
        impl ClipboardProvider for Says {
            fn get_contents(&mut self) -> Result<String> {
                self.0.next().unwrap()
            }
            fn set_contents(&mut self, _: String) -> Result<()> {
                Ok(())
            }
        }
        let late = std::io::Error::new(std::io::ErrorKind::TimedOut, "selection owner did not answer");
        let answers: Vec<(Result<String>, Read)> = vec![
            (Ok("text".into()), Ok("text".into())),
            // The words of the two readers, on X11 and on Wayland.
            (Err("Selection timed out".into()), Err(Failed::Late)),
            (Err(late.into()), Err(Failed::Late)),
            (Err("Selection exceeds 8 MiB".into()), Err(Failed::TooLarge)),
            (Err(std::io::Error::other("selection exceeds 8 MiB").into()), Err(Failed::TooLarge)),
            // A reader that let more through than is taken.
            (Ok("x".repeat(MAX_TEXT + 1)), Err(Failed::TooLarge)),
            (Ok("x".repeat(MAX_TEXT)), Ok("x".repeat(MAX_TEXT))),
            // No clipboard, none with text in it, a text that is none:
            // nothing to paste, and nothing to say.
            (Err("there is no clipboard".into()), Ok(String::new())),
            (Err(std::io::Error::other("supported mime-type is not found").into()), Ok(String::new())),
            (Err("Unexpected Reply type: 31".into()), Ok(String::new())),
            (Ok(String::new()), Ok(String::new())),
        ];
        let (given, expected): (Vec<_>, Vec<_>) = answers.into_iter().unzip();
        let clipboard = Clipboard::with_providers(Box::new(Says(given.into_iter())), None);
        for expected in expected {
            let (tx, rx) = std::sync::mpsc::channel();
            clipboard.get(false, move |read| drop(tx.send(read)));
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), expected);
        }
        // A selection there is no provider for has nothing, and no fault.
        let (tx, rx) = std::sync::mpsc::channel();
        clipboard.get(true, move |read| drop(tx.send(read)));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(String::new()));
    }

    #[test]
    fn shutdown_discards_queued_reads() {
        struct Paused {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl ClipboardProvider for Paused {
            fn get_contents(&mut self) -> Result<String> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.entered.send(())?;
                self.release.recv_timeout(Duration::from_secs(2))?;
                Ok(String::new())
            }
            fn set_contents(&mut self, _: String) -> Result<()> {
                Ok(())
            }
        }
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let clipboard = Clipboard::with_providers(
            Box::new(Paused { entered, release: blocked, calls: calls.clone() }),
            None,
        );
        clipboard.get(false, |_| {});
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        for _ in 0..8 {
            clipboard.get(false, |_| {});
        }
        let closed = clipboard.closed.clone();
        let dropper = std::thread::spawn(move || drop(clipboard));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !closed.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        release.send(()).unwrap();
        dropper.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
