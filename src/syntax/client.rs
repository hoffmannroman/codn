//! The app's side of the colors: one thread that owns the process, gives
//! it the text, asks it for lines and keeps a watch on every answer. The
//! window's thread only sends and is sent to. The thread starts with the
//! first text, the process with it, so that a codn in which no file was
//! opened has neither.
//!
//! The same process says what a text of Markdown is made of, for the
//! preview of a file. That is asked apart from the colors and does not
//! need them: the question carries its text.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::wire::{self, ANSWER_MAX, Answer, Question};
use crate::core::colors::{End, Piece};
use crate::core::files::Edition;
use crate::core::preview::{Meant, Why};
use crate::core::text::line_starts;
use crate::core::wait_until;

/// How long one answer is waited for, not a file: an answer is a stretch
/// of lines at the most and one line at the least, and a text of many
/// lines that each stay under it is cut for as long as they last. A
/// line whose search gives up took 100 ms on a slow machine this was
/// measured on: a stretch of nothing but such lines fits, and the first
/// use of a language, a second there, as well.
const WATCH: Duration = Duration::from_secs(8);

pub enum Request {
    /// The text that is shown now, and the grammar for it. It takes the
    /// place of the text before, and lines asked of that one are void.
    Text { of: Edition, grammar: &'static str, bytes: Vec<u8> },
    /// The pieces of these lines of it: from, and how many.
    Lines { of: Edition, from: usize, count: usize },
    /// What this text of Markdown is made of. The text comes back with
    /// the answer, so that the two cannot be held against a wrong one.
    Outline { of: Edition, bytes: Vec<u8> },
    /// No file is shown any more: the process ends, and what it held
    /// goes back.
    Close,
}

pub enum Reply {
    /// The pieces of the lines from `from` on, and the line from which on
    /// nothing is known. Not held against the text yet.
    Pieces { of: Edition, from: usize, lines: Vec<Box<[Piece]>>, unknown: Option<usize> },
    /// The colors of this text are given up, and the process is ended.
    Ended { of: Edition, why: End },
    /// Lines were asked of a text the process does not hold: it has to
    /// be given again. The thread that was given it may be gone.
    Lost { of: Edition },
    /// A text of Markdown with what it is made of, held against each
    /// other.
    Outline { of: Edition, meant: Meant },
    /// That text has no structure that is said, and why.
    NoOutline { of: Edition, why: Why },
}

/// A process that is asked, or what stands in for one in a test.
struct Running {
    child: Option<Child>,
    to: File,
    from: File,
}

enum Failure {
    TookTooLong,
    Gone,
}

/// Waits until a pipe can be read or written, or the time is over.
fn ready(pipe: &File, events: libc::c_short, until: Instant) -> Result<(), Failure> {
    loop {
        let left = until.checked_duration_since(Instant::now()).ok_or(Failure::TookTooLong)?;
        let mut waited = libc::pollfd { fd: pipe.as_raw_fd(), events, revents: 0 };
        let millis = left.as_millis().min(i32::MAX as u128) as i32 + 1;
        match unsafe { libc::poll(&mut waited, 1, millis) } {
            0 => {}
            // Also where the other end is gone: reading or writing says so.
            1.. => return Ok(()),
            _ if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted => {}
            _ => return Err(Failure::Gone),
        }
    }
}

impl Running {
    /// Makes of a process, or of two pipes, something to ask. The pipes
    /// do not block from here on: a process that stops reading or
    /// answering is waited for with the watch and no longer.
    fn new(child: Option<Child>, to: OwnedFd, from: OwnedFd) -> Self {
        for pipe in [&to, &from] {
            unsafe {
                let flags = libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL);
                libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        Self { child, to: File::from(to), from: File::from(from) }
    }

    /// How the process is started, whatever program it is: under the
    /// name of codn, without the environment of the app, which holds
    /// what the user keeps there, and in the root folder, so that it
    /// holds no folder of a project. Apart from `start`, so that a test
    /// can see it of a program that is not codn.
    fn starting(program: &str, arguments: &[&str]) -> Command {
        let mut command = Command::new(program);
        command.arg0("codn").args(arguments).env_clear().current_dir("/");
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        command
    }

    /// codn itself, started as the process for the colors: the very
    /// build that asks, also after the file it was started from was
    /// replaced by a newer one.
    fn start() -> std::io::Result<Self> {
        let mut child = Self::starting("/proc/self/exe", &[super::ARGUMENT]).spawn()?;
        match (child.stdin.take(), child.stdout.take()) {
            (Some(to), Some(from)) => Ok(Self::new(Some(child), to.into(), from.into())),
            _ => Err(ErrorKind::BrokenPipe.into()),
        }
    }

    fn send(&mut self, mut bytes: &[u8], until: Instant) -> Result<(), Failure> {
        while !bytes.is_empty() {
            ready(&self.to, libc::POLLOUT, until)?;
            match self.to.write(bytes) {
                Ok(0) => return Err(Failure::Gone),
                Ok(written) => bytes = &bytes[written..],
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => return Err(Failure::Gone),
            }
        }
        Ok(())
    }

    fn receive(&mut self, mut bytes: &mut [u8], until: Instant) -> Result<(), Failure> {
        while !bytes.is_empty() {
            ready(&self.from, libc::POLLIN, until)?;
            match self.from.read(bytes) {
                Ok(0) => return Err(Failure::Gone),
                Ok(read) => bytes = &mut bytes[read..],
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => return Err(Failure::Gone),
            }
        }
        Ok(())
    }

    /// Asks, and waits for the answer no longer than the watch. Nothing
    /// for an answer that is none: too long, or not to be read.
    fn ask(&mut self, question: &Question, watch: Duration) -> Result<Option<Answer>, Failure> {
        let until = Instant::now() + watch;
        let mut frame = Vec::new();
        question.write(&mut frame);
        self.send(&frame, until)?;
        let mut head = [0; 4];
        self.receive(&mut head, until)?;
        let Some(length) = wire::length(head, ANSWER_MAX) else { return Ok(None) };
        let mut body = vec![0; length];
        self.receive(&mut body, until)?;
        Ok(Answer::read(&body))
    }

    /// Ends the process, whatever it is doing.
    fn stop(mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            wait_until(child, Instant::now() + Duration::from_secs(1));
        }
    }
}

/// How a process is started and how long an answer of it is waited for.
/// Tests put others in the place of both.
struct How {
    start: Box<dyn Fn() -> std::io::Result<Running> + Send>,
    watch: Duration,
}

/// A request as it goes to the thread: without its text, which waits in
/// `Texts`. The thread looks at what was sent only between two questions
/// to the process, which can be seconds, and every key in a file sends
/// its text anew: each on its own way would lie there whole, a megabyte
/// at a time, though only the last of them is wanted.
enum Told {
    Text(Edition),
    Lines { of: Edition, from: usize, count: usize },
    Outline(Edition),
    Close,
}

/// The texts that wait for the thread: the latest of each kind, and the
/// one before it gone the moment a newer one is sent.
#[derive(Default)]
struct Texts {
    colors: Option<(Edition, &'static str, Vec<u8>)>,
    outline: Option<(Edition, Vec<u8>)>,
}

impl Texts {
    /// Keeps the text of a request and says what the thread is told.
    fn keep(&mut self, request: Request) -> Told {
        match request {
            Request::Text { of, grammar, bytes } => {
                self.colors = Some((of, grammar, bytes));
                Told::Text(of)
            }
            Request::Lines { of, from, count } => Told::Lines { of, from, count },
            Request::Outline { of, bytes } => {
                self.outline = Some((of, bytes));
                Told::Outline(of)
            }
            // No file is shown: what waited for one is wanted no more.
            Request::Close => {
                *self = Self::default();
                Told::Close
            }
        }
    }
}

/// Where the questions about colors go.
pub struct Requests {
    commands: Option<Sender<Told>>,
    texts: Arc<Mutex<Texts>>,
    reply: Arc<dyn Fn(Reply) -> bool + Send + Sync>,
    how: fn() -> How,
}

impl Requests {
    /// `reply` is called on the thread that asks the process, and returns
    /// whether anyone is still listening.
    pub fn new(reply: impl Fn(Reply) -> bool + Send + Sync + 'static) -> Self {
        Self::with(reply, || How { start: Box::new(Running::start), watch: WATCH })
    }

    fn with(reply: impl Fn(Reply) -> bool + Send + Sync + 'static, how: fn() -> How) -> Self {
        Self { commands: None, texts: Arc::default(), reply: Arc::new(reply), how }
    }

    /// Says whether the command was handed over. Where there is no
    /// thread, or it is gone, one is started, and one that cannot be
    /// started hands nothing over: whoever sent has to send again.
    pub fn send(&mut self, command: Request) -> bool {
        let command = self.texts.lock().unwrap().keep(command);
        let command = match &self.commands {
            Some(commands) => match commands.send(command) {
                Ok(()) => return true,
                Err(unsent) => unsent.0,
            },
            None => command,
        };
        let (commands, waiting) = std::sync::mpsc::channel();
        let (reply, how, texts) = (self.reply.clone(), self.how, self.texts.clone());
        let work = move || Worker::new(how(), &*reply, &texts).run(&waiting);
        self.commands = None;
        if std::thread::Builder::new().name("codn colors".into()).spawn(work).is_err() {
            return false;
        }
        let handed = commands.send(command).is_ok();
        self.commands = Some(commands);
        handed
    }
}

/// What was sent and is not done yet: the latest of each kind.
#[derive(Default)]
struct Waiting {
    text: Option<(Edition, &'static str, Vec<u8>)>,
    lines: Option<(Edition, usize, usize)>,
    outline: Option<(Edition, Vec<u8>)>,
    close: bool,
}

impl Waiting {
    /// Takes in what the thread was told, with the text that waits for
    /// it. Where a newer text took the place of that one, this word is
    /// of a text that is gone: it still makes void what was asked of
    /// the text before, and the newer one comes with a word of its own.
    fn put(&mut self, told: Told, texts: &Mutex<Texts>) {
        match told {
            // Another text for the colors takes nothing from the question
            // for a structure, which carries a text of its own.
            Told::Text(of) => {
                let sent = texts.lock().unwrap().colors.take_if(|(waits, ..)| *waits == of);
                // Sent twice, the text of this edition was taken at the
                // first word of it.
                let text = sent.or_else(|| self.text.take().filter(|(held, ..)| *held == of));
                let outline = self.outline.take();
                *self = Self { text, lines: None, outline, close: false };
            }
            Told::Lines { of, from, count } => self.lines = Some((of, from, count)),
            Told::Outline(of) => {
                if let Some(sent) = texts.lock().unwrap().outline.take_if(|(waits, _)| *waits == of) {
                    self.outline = Some(sent);
                }
            }
            Told::Close => *self = Self { text: None, lines: None, outline: None, close: true },
        }
    }

    /// Takes in what was sent meanwhile. Says whether anything was, or
    /// nothing if nobody sends any more.
    fn more(&mut self, commands: &Receiver<Told>, texts: &Mutex<Texts>) -> Option<bool> {
        let mut any = false;
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    self.put(command, texts);
                    any = true;
                }
                Err(TryRecvError::Empty) => return Some(any),
                Err(TryRecvError::Disconnected) => return None,
            }
        }
    }
}

/// The text the process holds.
struct Held {
    of: Edition,
    /// The number it holds it under.
    id: u64,
    grammar: &'static str,
    bytes: Vec<u8>,
    /// Where every line of it begins, to hold the pieces of an answer
    /// against before they are kept.
    starts: Vec<u32>,
}

impl Held {
    fn new(of: Edition, id: u64, grammar: &'static str, bytes: Vec<u8>) -> Self {
        let starts = line_starts(&bytes);
        Self { of, id, grammar, bytes, starts }
    }

    /// Whether the pieces of some lines from `from` on, and the line
    /// from which on nothing is known, are of lines this text has, and
    /// no piece ends behind its line.
    fn fits(&self, from: usize, lines: &[Box<[Piece]>], unknown: Option<u32>) -> bool {
        let length = |line: usize| {
            let next = self.starts.get(line + 1).map_or(self.bytes.len(), |next| *next as usize);
            Some(next - *self.starts.get(line)? as usize)
        };
        let within = |(at, pieces): (usize, &[Piece])| {
            length(from + at)
                .is_some_and(|length| pieces.last().is_none_or(|(until, _)| *until as usize <= length))
        };
        unknown.is_none_or(|line| (line as usize) < self.starts.len())
            && lines.iter().map(|line| &**line).enumerate().all(within)
    }
}

struct Worker<'a> {
    how: How,
    reply: &'a (dyn Fn(Reply) -> bool + Send + Sync),
    /// The texts that wait, which the words that are sent stand for.
    texts: &'a Mutex<Texts>,
    running: Option<Running>,
    held: Option<Held>,
    /// How many texts the process was given, for the number of the next.
    given: u64,
}

/// What makes `new` of `old` by replacing whole lines: how many lines at
/// the start stay, how many behind them go, and the bytes that stand in
/// their place. The lines that stay at the end are those left over. A
/// file that an agent writes to, or a key in it, changes few lines, and
/// what the process cut above them stays good.
fn replaced<'a>(old: &[u8], new: &'a [u8]) -> (u32, u32, &'a [u8]) {
    let lines = |bytes: &[u8]| bytes.iter().filter(|byte| **byte == b'\n').count();
    // The line the first difference is in begins the lines that go.
    let alike = old.iter().zip(new).take_while(|(one, two)| one == two).count();
    let from = old[..alike].iter().rposition(|byte| *byte == b'\n').map_or(0, |end| end + 1);
    // What is alike at the end stays as far as it is whole lines in
    // both: from behind the first line end in it.
    let (rest, now) = (&old[from..], &new[from..]);
    let alike = rest.iter().rev().zip(now.iter().rev()).take_while(|(one, two)| one == two).count();
    let tail = &rest[rest.len() - alike..];
    let whole = |bytes: &[u8]| alike == bytes.len() || bytes[bytes.len() - alike - 1] == b'\n';
    let tail = match tail.iter().position(|byte| *byte == b'\n') {
        _ if alike > 0 && whole(rest) && whole(now) => tail,
        Some(end) => &tail[end + 1..],
        None => &[],
    };
    let kept = if tail.is_empty() { 0 } else { lines(tail) + 1 };
    let gone = lines(rest) + 1 - kept;
    (lines(&old[..from]) as u32, gone as u32, &now[..now.len() - tail.len()])
}

impl<'a> Worker<'a> {
    fn new(how: How, reply: &'a (dyn Fn(Reply) -> bool + Send + Sync), texts: &'a Mutex<Texts>) -> Self {
        Self { how, reply, texts, running: None, held: None, given: 0 }
    }

    fn run(mut self, commands: &Receiver<Told>) {
        let mut waiting = Waiting::default();
        loop {
            // With nothing to do, it waits until something is sent.
            let nothing = waiting.text.is_none() && waiting.lines.is_none() && waiting.outline.is_none();
            if nothing && !waiting.close {
                let Ok(command) = commands.recv() else { break };
                waiting.put(command, self.texts);
            }
            if waiting.more(commands, self.texts).is_none() {
                break;
            }
            if std::mem::take(&mut waiting.close) {
                self.stop();
            }
            let mut listening = true;
            if let Some((of, grammar, bytes)) = waiting.text.take()
                && let Err(why) = self.give(of, grammar, bytes)
            {
                listening = (self.reply)(Reply::Ended { of, why });
            }
            // Before any line: a stretch of lines can take seconds, and
            // the preview of a file waits for this.
            if let Some((of, bytes)) = waiting.outline.take() {
                listening &= (self.reply)(self.outline(of, bytes));
            }
            if let Some((of, from, count)) = waiting.lines.take()
                && let Some(reply) = self.lines(of, from, count, &mut waiting, commands)
            {
                listening &= (self.reply)(reply);
            }
            if !listening {
                break;
            }
        }
        self.stop();
    }

    fn stop(&mut self) {
        self.held = None;
        if let Some(running) = self.running.take() {
            running.stop();
        }
    }

    /// Gives the process a text: as a change to the one it holds, where
    /// it holds one of the same language, else whole. A process that is
    /// gone or full is started anew, once.
    fn give(&mut self, of: Edition, grammar: &'static str, bytes: Vec<u8>) -> Result<(), End> {
        self.given += 1;
        let id = self.given;
        let before = self.held.take().filter(|held| held.grammar == grammar);
        if let (Some(before), Some(running)) = (before, self.running.as_mut()) {
            let (keep, gone, text) = replaced(&before.bytes, &bytes);
            let change = Question::Change { id, of: before.id, keep, gone, text };
            match running.ask(&change, self.how.watch) {
                Ok(Some(Answer::Taken { id: taken })) if taken == id => {
                    self.held = Some(Held::new(of, id, grammar, bytes));
                    return Ok(());
                }
                Err(Failure::TookTooLong) => {
                    self.stop();
                    return Err(End::TookTooLong);
                }
                // It holds too much and ends itself: a new one is given
                // the whole text below.
                Ok(Some(Answer::Full)) => self.stop(),
                // Refused, or gone: the whole text will do, as below.
                _ => {}
            }
        }
        for _ in 0..2 {
            let fresh = self.running.is_none();
            if fresh {
                self.running = Some((self.how.start)().map_err(|_| End::Ended)?);
            }
            let Some(running) = self.running.as_mut() else { return Err(End::Ended) };
            let why = match running.ask(&Question::Text { id, grammar, text: &bytes }, self.how.watch) {
                Ok(Some(Answer::Taken { id: taken })) if taken == id => {
                    self.held = Some(Held::new(of, id, grammar, bytes));
                    return Ok(());
                }
                Ok(Some(Answer::Full)) | Err(Failure::Gone) if !fresh => None,
                Ok(_) => Some(End::Broken),
                Err(Failure::TookTooLong) => Some(End::TookTooLong),
                Err(Failure::Gone) => Some(End::Ended),
            };
            self.stop();
            if let Some(why) = why {
                return Err(why);
            }
        }
        Err(End::Ended)
    }

    /// Asks what a text of Markdown is made of, of a process that need
    /// not hold any text: one is started where none runs. A process
    /// that was there before and is gone is no word about this text,
    /// and a new one is asked once.
    fn outline(&mut self, of: Edition, bytes: Vec<u8>) -> Reply {
        self.given += 1;
        let id = self.given;
        for _ in 0..2 {
            let fresh = self.running.is_none();
            if fresh {
                match (self.how.start)() {
                    Ok(running) => self.running = Some(running),
                    Err(_) => return Reply::NoOutline { of, why: Why::Ended },
                }
            }
            let Some(running) = self.running.as_mut() else { break };
            let why = match running.ask(&Question::Outline { id, text: &bytes }, self.how.watch) {
                // Held against the text before it is handed on.
                Ok(Some(Answer::Outline { id: answered, entries })) if answered == id => {
                    match Meant::new(bytes, entries) {
                        Some(meant) => return Reply::Outline { of, meant },
                        None => {
                            self.stop();
                            return Reply::NoOutline { of, why: Why::Broken };
                        }
                    }
                }
                // The process is sound, and the text has no structure.
                Ok(Some(Answer::NoOutline { id: answered, why })) if answered == id => {
                    return Reply::NoOutline { of, why };
                }
                Err(Failure::Gone) if !fresh => None,
                Ok(_) => Some(Why::Broken),
                Err(Failure::TookTooLong) => Some(Why::TookTooLong),
                Err(Failure::Gone) => Some(Why::Ended),
            };
            // The text it held for the colors is gone with it: lines
            // asked of it are answered with `Lost`, and it is given again.
            self.stop();
            if let Some(why) = why {
                return Reply::NoOutline { of, why };
            }
        }
        Reply::NoOutline { of, why: Why::Ended }
    }

    /// Asks for lines, stretch after stretch, until they are all there,
    /// the text ends, or nothing is known further. Nothing if something
    /// newer was sent meanwhile: what this would answer is not wanted
    /// any more.
    fn lines(
        &mut self,
        of: Edition,
        from: usize,
        count: usize,
        waiting: &mut Waiting,
        commands: &Receiver<Told>,
    ) -> Option<Reply> {
        let Some(id) = self.held.as_ref().filter(|held| held.of == of).map(|held| held.id) else {
            return Some(Reply::Lost { of });
        };
        let (mut lines, mut unknown, mut reached) = (Vec::new(), None, None);
        let ended = loop {
            let (at, left) = (from + lines.len(), count - lines.len());
            if left == 0 {
                break None;
            }
            let (Ok(at), Some(running)) = (u32::try_from(at), self.running.as_mut()) else {
                break Some(End::Ended);
            };
            let question = Question::Lines { id, from: at, count: left.min(u32::MAX as usize) as u32 };
            match running.ask(&question, self.how.watch) {
                // Held against the text before it is kept: what is asked
                // of many answers must not add up to more than the text
                // allows, and what lies is not asked again.
                Ok(Some(Answer::Pieces { id: of_id, from: begins, lines: more, unknown: none }))
                    if of_id == id
                        && begins == at
                        && more.len() <= left
                        && self.held.as_ref().is_some_and(|held| held.fits(at as usize, &more, none)) =>
                {
                    let done = more.is_empty() || none.is_some();
                    lines.extend(more);
                    unknown = none.map(|line| line as usize);
                    if done {
                        break None;
                    }
                }
                // It has to come nearer with every answer, or this never ends.
                Ok(Some(Answer::Reached { id: of_id, line }))
                    if of_id == id && line <= at && reached.is_none_or(|before| line > before) =>
                {
                    reached = Some(line);
                }
                Ok(_) => break Some(End::Broken),
                Err(Failure::TookTooLong) => break Some(End::TookTooLong),
                Err(Failure::Gone) => break Some(End::Ended),
            }
            // Between two stretches: the lines of another place, or
            // another text, are what is wanted now.
            waiting.more(commands, self.texts)?;
            // A structure that is asked for meanwhile is answered here
            // and the lines go on, unless the process ended over it:
            // then it holds this text no more.
            if let Some((for_text, bytes)) = waiting.outline.take() {
                let reply = self.outline(for_text, bytes);
                if !(self.reply)(reply) {
                    return None;
                }
                if self.held.is_none() {
                    return Some(Reply::Lost { of });
                }
            }
            if waiting.text.is_some() || waiting.lines.is_some() || waiting.close {
                return None;
            }
        };
        match ended {
            Some(why) => {
                self.stop();
                Some(Reply::Ended { of, why })
            }
            None => Some(Reply::Pieces { of, from, lines, unknown }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::channel;

    use super::*;
    use crate::core::colors::Kind;

    /// The cutter on a thread, behind two pipes, in the place of a process.
    fn threaded() -> std::io::Result<Running> {
        let (questions, to) = std::io::pipe()?;
        let (from, answers) = std::io::pipe()?;
        std::thread::spawn(move || super::super::cutter::serve(questions, answers));
        Ok(Running::new(None, to.into(), from.into()))
    }

    /// A program in the place of the process.
    fn program(name: &str, args: &[&str]) -> std::io::Result<Running> {
        let mut child = Command::new(name)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (to, from) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        Ok(Running::new(Some(child), to.into(), from.into()))
    }

    fn asking(how: fn() -> How) -> (Requests, Receiver<Reply>) {
        let (replies, received) = channel();
        let replies = Mutex::new(replies);
        (Requests::with(move |reply| replies.lock().unwrap().send(reply).is_ok(), how), received)
    }

    const WAIT: Duration = Duration::from_secs(20);
    const ONE: Edition = Edition { opening: 1, change: 1 };
    const TWO: Edition = Edition { opening: 1, change: 2 };

    fn text(of: Edition, text: &str) -> Request {
        Request::Text { of, grammar: "Rust", bytes: text.as_bytes().to_vec() }
    }

    /// Sends and takes in at once, as where the thread looks after
    /// every request.
    fn put(waiting: &mut Waiting, texts: &Mutex<Texts>, request: Request) {
        let told = texts.lock().unwrap().keep(request);
        waiting.put(told, texts);
    }

    #[test]
    fn a_text_is_given_and_its_lines_come_back_whole_however_many_stretches_they_take() {
        let (mut requests, replies) = asking(|| How { start: Box::new(threaded), watch: WAIT });
        let mut long = String::from("/*\n");
        long.push_str(&"let a = 1;\n".repeat(500));
        long.push_str("*/\nfn z() {}\n");
        requests.send(text(ONE, &long));
        requests.send(Request::Lines { of: ONE, from: 400, count: 110 });
        let Reply::Pieces { of, from, lines, unknown } = replies.recv_timeout(WAIT).unwrap() else {
            panic!("pieces")
        };
        assert_eq!(
            (of, from, lines.len(), unknown),
            (ONE, 400, 104, None),
            "to the end of the text, its empty last line too"
        );
        assert_eq!(&*lines[0], &[(11, Kind::Comment)]);
        assert_eq!(lines[102][0], (2, Kind::Keyword));

        // Another text takes its place, and lines of the one before are void.
        requests.send(text(TWO, "// a\nfn b() {}\n"));
        requests.send(Request::Lines { of: ONE, from: 0, count: 2 });
        requests.send(Request::Lines { of: TWO, from: 0, count: 2 });
        let Reply::Pieces { of, lines, .. } = replies.recv_timeout(WAIT).unwrap() else { panic!("pieces") };
        assert_eq!((of, &*lines[0]), (TWO, &[(5, Kind::Comment)][..]));
        assert!(replies.try_recv().is_err(), "nothing for the lines of the text that is gone");

        // Closed and asked again: the text has to be given anew, and a
        // new process takes it. The answer is waited for before the text
        // is sent: a text that came first would make these lines void,
        // and which of the two it is would hang on the turn of the threads.
        requests.send(Request::Close);
        requests.send(Request::Lines { of: TWO, from: 0, count: 2 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Lost { of: TWO }));
        requests.send(text(ONE, "fn c() {}\n"));
        requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
        let Reply::Pieces { of, lines, .. } = replies.recv_timeout(WAIT).unwrap() else { panic!("pieces") };
        assert_eq!((of, lines[0][0]), (ONE, (2, Kind::Keyword)));
    }

    #[test]
    fn a_process_that_does_not_answer_is_ended_and_the_colors_are_given_up() {
        let silent =
            || How { start: Box::new(|| program("sleep", &["60"])), watch: Duration::from_millis(300) };
        let (mut requests, replies) = asking(silent);
        let begun = Instant::now();
        requests.send(text(ONE, "fn a() {}\n"));
        let Reply::Ended { of, why } = replies.recv_timeout(WAIT).unwrap() else { panic!("ended") };
        assert_eq!((of, why), (ONE, End::TookTooLong));
        assert!(begun.elapsed() < Duration::from_secs(5), "not waited for longer than the watch says");
        // Lines of a text that was never taken: it is not held.
        requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Lost { of: ONE }));
    }

    #[test]
    fn a_process_that_ends_or_answers_nonsense_gives_the_colors_up() {
        let gone = || How { start: Box::new(|| program("true", &[])), watch: WAIT };
        let (mut requests, replies) = asking(gone);
        requests.send(text(ONE, &"fn a() {}\n".repeat(20_000)));
        let Reply::Ended { why, .. } = replies.recv_timeout(WAIT).unwrap() else { panic!("ended") };
        assert_eq!(why, End::Ended);

        // `cat` answers a question with the question.
        let echo = || How { start: Box::new(|| program("cat", &[])), watch: WAIT };
        let (mut requests, replies) = asking(echo);
        requests.send(text(ONE, "fn a() {}\n"));
        let Reply::Ended { why, .. } = replies.recv_timeout(WAIT).unwrap() else { panic!("ended") };
        assert_eq!(why, End::Broken);

        let none = || How { start: Box::new(|| program("/nowhere/codn", &[])), watch: WAIT };
        let (mut requests, replies) = asking(none);
        requests.send(text(TWO, "fn a() {}\n"));
        let Reply::Ended { of, why } = replies.recv_timeout(WAIT).unwrap() else { panic!("ended") };
        assert_eq!((of, why), (TWO, End::Ended));
    }

    /// The lines of a text, each with its end, as the process counts them.
    fn split(text: &[u8]) -> Vec<&[u8]> {
        let mut lines: Vec<&[u8]> = text.split_inclusive(|byte| *byte == b'\n').collect();
        if text.is_empty() || text.ends_with(b"\n") {
            lines.push(b"");
        }
        lines
    }

    #[test]
    fn the_lines_that_were_replaced_make_the_new_text_of_the_old() {
        let check = |old: &str, new: &str| {
            let (keep, gone, text) = replaced(old.as_bytes(), new.as_bytes());
            let (keep, gone, lines) = (keep as usize, gone as usize, split(old.as_bytes()));
            assert!(keep < lines.len() && keep + gone <= lines.len(), "{old:?} to {new:?}: {keep} {gone}");
            // Lines that stay behind the new ones have to begin a line.
            assert!(
                keep + gone == lines.len() || text.is_empty() || text.ends_with(b"\n"),
                "{old:?} to {new:?}"
            );
            let made = [&lines[..keep].concat()[..], text, &lines[keep + gone..].concat()[..]].concat();
            assert_eq!(made, new.as_bytes(), "{old:?} to {new:?}: {keep} {gone} {text:?}");
            (keep, gone, text.len())
        };
        // One line of many: what is sent is that line.
        assert_eq!(check("a\nb\nc\nd\n", "a\nb\nX\nd\n"), (2, 1, 2));
        assert_eq!(check("a\nb\nc\nd\n", "a\nb\nd\n"), (2, 1, 0), "a line taken out");
        assert_eq!(check("a\nb\nd\n", "a\nb\nc\nd\n"), (2, 0, 2), "a line put in");
        assert_eq!(check("a\nb\n", "a\nb\nc\n"), (2, 1, 2), "a line put behind the last");
        assert_eq!(check("a\nbb\nc", "a\nbXb\nc").0, 1);
        assert_eq!(check("a\nb", "a\nb"), (1, 0, 0), "nothing");
        for (old, new) in
            [("", ""), ("", "a"), ("a", ""), ("a\n", "a"), ("a", "a\n"), ("\n\n", "\n"), ("x\ny", "y\nx")]
        {
            check(old, new);
        }
        // Lines at random, changed at random.
        let mut seed = 7u32;
        let mut random = |below: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % below
        };
        let words = ["a", "bb", "", "a", "c\rd", "\u{e4}"];
        for _ in 0..3000 {
            let text = |random: &mut dyn FnMut(u32) -> u32| {
                let mut text: String =
                    (0..random(6)).map(|_| format!("{}\n", words[random(6) as usize])).collect();
                if random(2) == 0 {
                    text.push_str(words[random(6) as usize]);
                }
                text
            };
            let (old, other) = (text(&mut random), text(&mut random));
            // Another text altogether, and the old one with a piece of it replaced.
            check(&old, &other);
            let (at, until) = (random(old.len() as u32 + 1) as usize, random(old.len() as u32 + 1) as usize);
            if at <= until && old.is_char_boundary(at) && old.is_char_boundary(until) {
                check(&old, &format!("{}{other}{}", &old[..at], &old[until..]));
            }
        }
    }

    /// The cutter behind two pipes, and every kind of question it was
    /// asked written down.
    static ASKED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    fn recorded() -> std::io::Result<Running> {
        let (mut questions, to) = std::io::pipe()?;
        let (from, mut answers) = std::io::pipe()?;
        std::thread::spawn(move || {
            let mut cutter = super::super::cutter::Cutter::new().unwrap();
            loop {
                let mut head = [0; 4];
                if questions.read_exact(&mut head).is_err() {
                    return;
                }
                let mut body = vec![0; u32::from_le_bytes(head) as usize];
                questions.read_exact(&mut body).unwrap();
                ASKED.lock().unwrap().push(body[0]);
                let mut frame = Vec::new();
                cutter.answer(Question::read(&body).unwrap()).write(&mut frame);
                answers.write_all(&frame).unwrap();
            }
        });
        Ok(Running::new(None, to.into(), from.into()))
    }

    #[test]
    fn a_text_that_differs_in_some_lines_is_sent_as_those_lines_and_cut_as_if_whole() {
        let (mut requests, replies) = asking(|| How { start: Box::new(recorded), watch: WAIT });
        let mut lines: Vec<String> = (0..200).map(|line| format!("let a{line} = {line};\n")).collect();
        let pieces = |requests: &mut Requests, of| {
            requests.send(Request::Lines { of, from: 0, count: 300 });
            let Reply::Pieces { of: for_text, lines, .. } = replies.recv_timeout(WAIT).unwrap() else {
                panic!("pieces")
            };
            assert_eq!(for_text, of);
            lines
        };
        requests.send(text(ONE, &lines.concat()));
        let before = pieces(&mut requests, ONE);
        // A comment is opened in the middle: what follows is another thing.
        lines[100] = "/*\n".into();
        requests.send(text(TWO, &lines.concat()));
        let after = pieces(&mut requests, TWO);
        assert_eq!(after[..100], before[..100]);
        assert_eq!((after[100][0].1, after[150][0].1), (Kind::Comment, Kind::Comment));
        // The text was sent once whole and once as a change, and the
        // lines asked for in stretches: 201 lines are four of them.
        let asked = ASKED.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|kind| **kind == 1).count(), 1, "{asked:?}");
        assert_eq!(asked.iter().filter(|kind| **kind == 3).count(), 1, "{asked:?}");
        // Another language is another text, whole.
        requests.send(Request::Text { of: ONE, grammar: "Python", bytes: b"# a\n".to_vec() });
        assert_eq!(&*pieces(&mut requests, ONE)[0], &[(4, Kind::Comment)]);
        assert_eq!(ASKED.lock().unwrap().iter().filter(|kind| **kind == 1).count(), 2);
    }

    /// What stands in for a process and says what a test makes it say:
    /// the cutter behind two pipes, with one kind of answer replaced.
    fn scripted(
        lie: fn(&Question, Answer) -> Option<Answer>,
        ended: &'static AtomicBool,
    ) -> std::io::Result<Running> {
        let (mut questions, to) = std::io::pipe()?;
        let (from, mut answers) = std::io::pipe()?;
        std::thread::spawn(move || {
            let mut cutter = super::super::cutter::Cutter::new().unwrap();
            loop {
                let mut head = [0; 4];
                if questions.read_exact(&mut head).is_err() {
                    break;
                }
                let mut body = vec![0; u32::from_le_bytes(head) as usize];
                questions.read_exact(&mut body).unwrap();
                let question = Question::read(&body).unwrap();
                let honest = cutter.answer(Question::read(&body).unwrap());
                let Some(answer) = lie(&question, honest) else { break };
                let mut frame = Vec::new();
                answer.write(&mut frame);
                if answers.write_all(&frame).is_err() {
                    break;
                }
            }
            ended.store(true, Ordering::Release);
        });
        Ok(Running::new(None, to.into(), from.into()))
    }

    fn ended_soon(ended: &AtomicBool) -> bool {
        let until = Instant::now() + WAIT;
        while !ended.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        ended.load(Ordering::Acquire)
    }

    #[test]
    fn what_a_text_is_made_of_is_said_with_its_text_and_needs_no_colors() {
        let (mut requests, replies) = asking(|| How { start: Box::new(threaded), watch: WAIT });
        // Of a process that holds no text: a heading and a paragraph,
        // each begun, with its words, and ended.
        requests.send(Request::Outline { of: ONE, bytes: b"# a\n\nb\n".to_vec() });
        let Reply::Outline { of, meant } = replies.recv_timeout(WAIT).unwrap() else { panic!("an outline") };
        assert_eq!((of, meant.text(), meant.entries().len()), (ONE, "# a\n\nb\n", 6));
        // Asked with a text for the colors and with lines of it: the
        // structure is said before any line, and takes nothing from them.
        requests.send(text(TWO, "// a\n"));
        requests.send(Request::Outline { of: TWO, bytes: b"- c\n".to_vec() });
        requests.send(Request::Lines { of: TWO, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Outline { of: TWO, .. }));
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Pieces { of: TWO, .. }));
        // What is no text has none, and the process and the text it
        // holds for the colors are as they were.
        requests.send(Request::Outline { of: ONE, bytes: vec![b'a', 0xff] });
        let Reply::NoOutline { of, why } = replies.recv_timeout(WAIT).unwrap() else { panic!("none") };
        assert_eq!((of, why), (ONE, Why::NotText));
        requests.send(Request::Lines { of: TWO, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Pieces { of: TWO, .. }));
        // Of what was sent, the latest is done: a text for the colors
        // does not take the question for a structure with it, closing does.
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        put(&mut waiting, &texts, Request::Outline { of: ONE, bytes: Vec::new() });
        put(&mut waiting, &texts, Request::Outline { of: TWO, bytes: Vec::new() });
        put(&mut waiting, &texts, text(ONE, ""));
        assert!(matches!(waiting.outline, Some((TWO, _))) && waiting.text.is_some());
        put(&mut waiting, &texts, Request::Close);
        assert!(waiting.outline.is_none());
    }

    #[test]
    fn a_process_that_ends_over_a_structure_says_so_and_holds_no_text_after() {
        let outline = |of| Request::Outline { of, bytes: b"a\n".to_vec() };
        let reason = |how: fn() -> How| {
            let (mut requests, replies) = asking(how);
            requests.send(outline(ONE));
            let Reply::NoOutline { of: ONE, why } = replies.recv_timeout(WAIT).unwrap() else {
                panic!("none")
            };
            // The text it would have held for the colors is gone with it.
            requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
            assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Lost { of: ONE }));
            why
        };
        let silent =
            || How { start: Box::new(|| program("sleep", &["60"])), watch: Duration::from_millis(300) };
        assert_eq!(reason(silent), Why::TookTooLong);
        assert_eq!(reason(|| How { start: Box::new(|| program("true", &[])), watch: WAIT }), Why::Ended);
        // `cat` answers a question with the question.
        assert_eq!(reason(|| How { start: Box::new(|| program("cat", &[])), watch: WAIT }), Why::Broken);
        assert_eq!(
            reason(|| How { start: Box::new(|| program("/nowhere/codn", &[])), watch: WAIT }),
            Why::Ended
        );

        // An answer that does not fit the text is not handed on, and
        // ends the process: an entry that ends behind the text.
        static ENDED: AtomicBool = AtomicBool::new(false);
        fn behind(_: &Question, answer: Answer) -> Option<Answer> {
            Some(match answer {
                Answer::Outline { id, mut entries } => {
                    entries.iter_mut().for_each(|entry| entry.to += 99);
                    Answer::Outline { id, entries }
                }
                other => other,
            })
        }
        let (mut requests, replies) =
            asking(|| How { start: Box::new(|| scripted(behind, &ENDED)), watch: WAIT });
        requests.send(text(TWO, "// a\n"));
        requests.send(outline(TWO));
        let Reply::NoOutline { of, why } = replies.recv_timeout(WAIT).unwrap() else { panic!("none") };
        assert_eq!((of, why), (TWO, Why::Broken));
        assert!(ended_soon(&ENDED), "what answers so is not kept");
        // The colors of the text are not given up for it: the text is
        // said to be lost, and is given to a new process.
        requests.send(Request::Lines { of: TWO, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Lost { of: TWO }));

        // An answer for another question is none for this one.
        static OTHER: AtomicBool = AtomicBool::new(false);
        fn other(_: &Question, answer: Answer) -> Option<Answer> {
            Some(match answer {
                Answer::Outline { id, entries } => Answer::Outline { id: id + 1, entries },
                other => other,
            })
        }
        let (mut requests, replies) =
            asking(|| How { start: Box::new(|| scripted(other, &OTHER)), watch: WAIT });
        requests.send(outline(ONE));
        assert!(matches!(
            replies.recv_timeout(WAIT).unwrap(),
            Reply::NoOutline { of: ONE, why: Why::Broken }
        ));
    }

    #[test]
    fn a_process_that_was_gone_before_is_no_word_about_a_structure() {
        // The first process ends as soon as it is asked the second
        // time, the one after it is sound.
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        static ENDED: AtomicBool = AtomicBool::new(false);
        fn once(question: &Question, answer: Answer) -> Option<Answer> {
            (!matches!(question, Question::Outline { .. })).then_some(answer)
        }
        fn start() -> std::io::Result<Running> {
            if STARTS.fetch_add(1, Ordering::AcqRel) == 0 { scripted(once, &ENDED) } else { threaded() }
        }
        let (mut requests, replies) = asking(|| How { start: Box::new(start), watch: WAIT });
        requests.send(text(ONE, "// a\n"));
        requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Pieces { of: ONE, .. }));
        requests.send(Request::Outline { of: ONE, bytes: b"a\n".to_vec() });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Outline { of: ONE, .. }));
        assert_eq!(STARTS.load(Ordering::Acquire), 2);
    }

    #[test]
    fn pieces_that_do_not_fit_the_text_are_not_passed_on_and_end_the_process() {
        static ENDED: AtomicBool = AtomicBool::new(false);
        // The last piece of every line ends a byte behind the line.
        fn behind(_: &Question, answer: Answer) -> Option<Answer> {
            Some(match answer {
                Answer::Pieces { id, from, mut lines, unknown } => {
                    for line in &mut lines {
                        let mut pieces = line.to_vec();
                        if let Some(last) = pieces.last_mut() {
                            last.0 += 1;
                        }
                        *line = pieces.into_boxed_slice();
                    }
                    Answer::Pieces { id, from, lines, unknown }
                }
                other => other,
            })
        }
        let (mut requests, replies) =
            asking(|| How { start: Box::new(|| scripted(behind, &ENDED)), watch: WAIT });
        requests.send(text(ONE, "fn a() {}\nfn b() {}\n"));
        requests.send(Request::Lines { of: ONE, from: 0, count: 2 });
        let Reply::Ended { of, why } = replies.recv_timeout(WAIT).unwrap() else { panic!("ended") };
        assert_eq!((of, why), (ONE, End::Broken));
        assert!(ended_soon(&ENDED), "what answers so is not kept");

        // Lines the text does not have, and a line from which nothing is
        // known that it does not have either.
        static GONE: AtomicBool = AtomicBool::new(false);
        fn beyond(_: &Question, answer: Answer) -> Option<Answer> {
            Some(match answer {
                Answer::Pieces { id, from, lines, .. } => {
                    Answer::Pieces { id, from, lines, unknown: Some(99) }
                }
                other => other,
            })
        }
        let (mut requests, replies) =
            asking(|| How { start: Box::new(|| scripted(beyond, &GONE)), watch: WAIT });
        requests.send(text(ONE, "fn a() {}\n"));
        requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
        let Reply::Ended { why, .. } = replies.recv_timeout(WAIT).unwrap() else { panic!("ended") };
        assert_eq!(why, End::Broken);
    }

    #[test]
    fn a_process_that_is_full_at_a_change_is_ended_and_a_new_one_given_the_whole_text() {
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        static ENDED: AtomicBool = AtomicBool::new(false);
        // The first process is full when the text changes, and ends.
        fn full(question: &Question, answer: Answer) -> Option<Answer> {
            let first = STARTED.load(Ordering::Acquire) == 1;
            Some(if first && matches!(question, Question::Change { .. }) { Answer::Full } else { answer })
        }
        fn start() -> std::io::Result<Running> {
            STARTED.fetch_add(1, Ordering::AcqRel);
            scripted(full, &ENDED)
        }
        let (mut requests, replies) = asking(|| How { start: Box::new(start), watch: WAIT });
        requests.send(text(ONE, "fn a() {}\n"));
        requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Pieces { of: ONE, .. }));
        requests.send(text(TWO, "// b\nfn a() {}\n"));
        requests.send(Request::Lines { of: TWO, from: 0, count: 1 });
        let Reply::Pieces { of, lines, .. } = replies.recv_timeout(WAIT).unwrap() else { panic!("pieces") };
        assert_eq!((of, &*lines[0]), (TWO, &[(5, Kind::Comment)][..]));
        assert_eq!(STARTED.load(Ordering::Acquire), 2);
        assert!(ended_soon(&ENDED), "the full one is not kept beside the new");
    }

    #[test]
    fn lines_of_a_text_the_process_does_not_hold_are_said_to_be_lost() {
        let (mut requests, replies) = asking(|| How { start: Box::new(threaded), watch: WAIT });
        // Nothing was given yet: a thread that had to be started anew
        // after the text was sent is in this place.
        assert!(requests.send(Request::Lines { of: ONE, from: 0, count: 1 }));
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Lost { of: ONE }));
        requests.send(text(ONE, "fn a() {}\n"));
        requests.send(Request::Lines { of: TWO, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Lost { of: TWO }));
        requests.send(Request::Lines { of: ONE, from: 0, count: 1 });
        assert!(matches!(replies.recv_timeout(WAIT).unwrap(), Reply::Pieces { of: ONE, .. }));
    }

    #[test]
    fn the_process_is_started_without_the_environment_and_the_folder_of_the_app() {
        // A shell in the place of codn says what it was started with:
        // how many bytes of environment, in which folder, by which name.
        let script = "wc -c < /proc/$$/environ; pwd; tr '\\0' ' ' < /proc/$$/cmdline";
        let output = Running::starting("/bin/sh", &["-c", script]).output().unwrap();
        let said = String::from_utf8(output.stdout).unwrap();
        let mut lines = said.lines();
        assert_eq!(lines.next().map(str::trim), Some("0"), "no variable of the app: {said}");
        assert_eq!(lines.next(), Some("/"));
        assert!(lines.next().is_some_and(|line| line.starts_with("codn -c ")), "{said}");
    }

    #[test]
    fn of_what_was_sent_the_latest_of_each_kind_is_done() {
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        put(&mut waiting, &texts, text(ONE, "a"));
        put(&mut waiting, &texts, Request::Lines { of: ONE, from: 0, count: 5 });
        put(&mut waiting, &texts, Request::Lines { of: ONE, from: 9, count: 5 });
        assert_eq!(waiting.lines, Some((ONE, 9, 5)));
        put(&mut waiting, &texts, text(TWO, "b"));
        assert!(waiting.lines.is_none() && waiting.text.as_ref().is_some_and(|(of, ..)| *of == TWO));
        put(&mut waiting, &texts, Request::Close);
        assert!(waiting.close && waiting.text.is_none());
        put(&mut waiting, &texts, text(ONE, "c"));
        assert!(!waiting.close, "a text after it needs the process again, a new one or not");
    }

    #[test]
    fn of_the_texts_that_wait_one_is_held_and_a_word_of_an_older_one_is_void() {
        let held = |texts: &Mutex<Texts>| texts.lock().unwrap().colors.as_ref().map(|(of, ..)| *of);
        // Sent one after the other before the thread looks: one text
        // waits, the last, and the thread is told of each.
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        let keep = |request| texts.lock().unwrap().keep(request);
        let first = keep(text(ONE, "a"));
        let lines = keep(Request::Lines { of: ONE, from: 0, count: 5 });
        let second = keep(text(TWO, "b"));
        assert_eq!(held(&texts), Some(TWO));
        // The word of the first is of a text that is gone. The lines
        // asked of that one are void all the same, with the second.
        waiting.put(first, &texts);
        assert!(waiting.text.is_none() && held(&texts) == Some(TWO));
        waiting.put(lines, &texts);
        waiting.put(second, &texts);
        assert!(waiting.lines.is_none() && waiting.text.as_ref().is_some_and(|(of, ..)| *of == TWO));
        assert_eq!(held(&texts), None, "taken, and held once");
        // A text, the file closed, another text: the last is the one.
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        let keep = |request| texts.lock().unwrap().keep(request);
        let all = [keep(text(ONE, "a")), keep(Request::Close), keep(text(TWO, "b"))];
        all.into_iter().for_each(|told| waiting.put(told, &texts));
        assert!(!waiting.close && waiting.text.as_ref().is_some_and(|(of, ..)| *of == TWO));
        // Closed and nothing after it: no text waits anywhere.
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        let keep = |request| texts.lock().unwrap().keep(request);
        let all = [keep(text(ONE, "a")), keep(Request::Close)];
        assert_eq!(held(&texts), None);
        all.into_iter().for_each(|told| waiting.put(told, &texts));
        assert!(waiting.close && waiting.text.is_none());
        // One edition sent twice, as after it was lost: its text is kept
        // at the second word, which finds none waiting.
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        let keep = |request| texts.lock().unwrap().keep(request);
        let all = [keep(text(ONE, "a")), keep(text(ONE, "a"))];
        all.into_iter().for_each(|told| waiting.put(told, &texts));
        assert!(waiting.text.as_ref().is_some_and(|(of, ..)| *of == ONE));
        // The same for a structure: the latest waits, an older word is void.
        let (mut waiting, texts) = (Waiting::default(), Mutex::<Texts>::default());
        let keep = |request| texts.lock().unwrap().keep(request);
        let all = [
            keep(Request::Outline { of: ONE, bytes: vec![1] }),
            keep(Request::Outline { of: TWO, bytes: vec![2] }),
        ];
        all.into_iter().for_each(|told| waiting.put(told, &texts));
        assert_eq!(waiting.outline, Some((TWO, vec![2])));
    }

    /// What the system counts as held by this process, in MiB.
    fn resident() -> usize {
        let pages = std::fs::read_to_string("/proc/self/statm").unwrap();
        (pages.split(' ').nth(1).unwrap().parse::<usize>().unwrap() * 4096) >> 20
    }

    /// Every key in a file sends its text anew, and the thread looks at
    /// what was sent only between two questions to the process. Sent
    /// whole, 200 texts of 1 MiB lay there for as long as one answer was
    /// waited for: 205 MiB. It measures the memory of the whole process,
    /// so it runs alone: `cargo test texts_sent_while -- --ignored`.
    #[test]
    #[ignore = "measures the memory of the process, which other tests share"]
    fn texts_sent_while_an_answer_is_waited_for_do_not_pile_up() {
        let silent = || How { start: Box::new(|| program("sleep", &["30"])), watch: Duration::from_secs(4) };
        let (mut requests, _replies) = asking(silent);
        let large = vec![b'a'; 1 << 20];
        let before = resident();
        for change in 1..=200 {
            let of = Edition { opening: 1, change };
            assert!(requests.send(Request::Text { of, grammar: "Rust", bytes: large.clone() }));
            std::thread::sleep(Duration::from_millis(5));
        }
        let more = resident().saturating_sub(before);
        assert!(more < 16, "{more} MiB more are held for 200 texts of 1 MiB");
    }
}
