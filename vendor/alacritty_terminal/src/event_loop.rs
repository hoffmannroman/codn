//! The main event loop which performs I/O on the pseudoterminal.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Instant;

use log::error;
use polling::{Event as PollingEvent, Events, PollMode, Poller};

use crate::event::{self, Event, EventListener, WindowSize};
use crate::sync::FairMutex;
use crate::term::Term;
use crate::{thread, tty};
use vte::ansi;

/// Max bytes to read from the PTY before forced terminal synchronization.
pub(crate) const READ_BUFFER_SIZE: usize = 0x1_0000;

/// Maximum retained input payload across the channel and PTY write queue.
pub const MAX_QUEUED_INPUT: usize = 8 * 1024 * 1024;

/// Max bytes to read from the PTY while the terminal is locked.
const MAX_LOCKED_READ: usize = READ_BUFFER_SIZE;

/// Messages that may be sent to the `EventLoop`.
#[derive(Debug)]
pub enum Msg {
    /// Data that should be written to the PTY.
    Input(Cow<'static, [u8]>),

    /// Indicates that the `EventLoop` should shut down, as Alacritty is shutting down.
    Shutdown,

    /// Instruction to resize the PTY.
    Resize(WindowSize),
}

/// The main event loop.
///
/// Handles all the PTY I/O and runs the PTY parser which updates terminal
/// state.
pub struct EventLoop<T: tty::EventedPty, U: EventListener> {
    poll: Arc<Poller>,
    pty: T,
    rx: PeekableReceiver<Msg>,
    tx: Sender<Msg>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    drain_on_exit: bool,
    ref_test: bool,
    queued_input: Arc<AtomicUsize>,
}

impl<T, U> EventLoop<T, U>
where
    T: tty::EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Send + 'static,
{
    /// Create a new event loop.
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: T,
        drain_on_exit: bool,
        ref_test: bool,
    ) -> io::Result<EventLoop<T, U>> {
        let (tx, rx) = mpsc::channel();
        let poll = Poller::new()?.into();
        Ok(EventLoop {
            poll,
            pty,
            tx,
            rx: PeekableReceiver::new(rx),
            terminal,
            event_proxy,
            drain_on_exit,
            ref_test,
            queued_input: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn channel(&self) -> EventLoopSender {
        EventLoopSender {
            sender: self.tx.clone(),
            poller: self.poll.clone(),
            queued_input: self.queued_input.clone(),
        }
    }

    /// Drain the channel.
    ///
    /// Returns `false` when a shutdown message was received.
    fn drain_recv_channel(&mut self, state: &mut State) -> bool {
        while let Some(msg) = self.rx.recv() {
            match msg {
                Msg::Input(input) if !input.is_empty() => state.write_list.push_back(input),
                Msg::Input(_) => {}
                Msg::Resize(window_size) => self.pty.on_resize(window_size),
                Msg::Shutdown => return false,
            }
        }

        true
    }

    #[inline]
    fn pty_read<X>(&mut self, state: &mut State, buf: &mut [u8], mut writer: Option<&mut X>) -> io::Result<()>
    where
        X: Write,
    {
        let mut unprocessed = 0;
        let mut processed = 0;

        // Reserve the next terminal lock for PTY reading.
        let _terminal_lease = Some(self.terminal.lease());
        let mut terminal = None;

        loop {
            // Read from the PTY, no more than this lock may still parse:
            // a short read followed by a full one was two chunks under one lock.
            let room = buf.len().min(MAX_LOCKED_READ - processed).max(unprocessed);
            match self.pty.reader().read(&mut buf[unprocessed..room]) {
                // This is received on Windows/macOS when no more data is readable from the PTY.
                Ok(0) if unprocessed == 0 => break,
                Ok(got) => unprocessed += got,
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                        // Go back to mio if we're caught up on parsing and the PTY would block.
                        if unprocessed == 0 {
                            break;
                        }
                    }
                    _ => return Err(err),
                },
            }

            // Attempt to lock the terminal.
            let terminal = match &mut terminal {
                Some(terminal) => terminal,
                None => terminal.insert(match self.terminal.try_lock_unfair() {
                    // Force block if we are at the buffer size limit.
                    None if unprocessed >= READ_BUFFER_SIZE => self.terminal.lock_unfair(),
                    None => continue,
                    Some(terminal) => terminal,
                }),
            };

            // Write a copy of the bytes to the ref test file.
            if let Some(writer) = &mut writer {
                writer.write_all(&buf[..unprocessed]).unwrap();
            }

            // Parse the incoming bytes.
            state.parser.advance(&mut **terminal, &buf[..unprocessed]);

            processed += unprocessed;
            unprocessed = 0;

            // Assure we're not blocking the terminal too long unnecessarily.
            if processed >= MAX_LOCKED_READ {
                break;
            }
        }

        // Queue terminal redraw unless all processed bytes were synchronized.
        if state.parser.sync_bytes_count() < processed && processed > 0 {
            self.event_proxy.send_event(Event::Wakeup);
        }

        Ok(())
    }

    #[inline]
    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        state.ensure_next();

        'write_many: while let Some(mut current) = state.take_current() {
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        break 'write_many;
                    }
                    Ok(n) => {
                        current.advance(n);
                        if current.finished() {
                            self.queued_input.fetch_sub(current.source.len(), Ordering::AcqRel);
                            state.goto_next();
                            break 'write_one;
                        }
                    }
                    Err(err) => {
                        state.set_current(Some(current));
                        match err.kind() {
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => break 'write_many,
                            _ => return Err(err),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn spawn(mut self) -> JoinHandle<(Self, State)> {
        thread::spawn_named("PTY reader", move || {
            let mut state = State::default();
            let mut buf = [0u8; READ_BUFFER_SIZE];

            let poll_opts = PollMode::Level;
            let mut interest = PollingEvent::readable(0);

            // Register TTY through EventedRW interface.
            if let Err(err) = unsafe { self.pty.register(&self.poll, interest, poll_opts) } {
                error!("Event loop registration error: {err}");
                return (self, state);
            }

            let mut events = Events::with_capacity(NonZeroUsize::new(1024).unwrap());

            let mut pipe = if self.ref_test {
                Some(File::create("./alacritty.recording").expect("create alacritty recording"))
            } else {
                None
            };

            'event_loop: loop {
                // Wakeup the event loop when a synchronized update timeout was reached.
                let handler = state.parser.sync_timeout();
                let timeout = handler.sync_timeout().map(|st| st.saturating_duration_since(Instant::now()));

                events.clear();
                if let Err(err) = self.poll.wait(&mut events, timeout) {
                    match err.kind() {
                        ErrorKind::Interrupted => continue,
                        _ => {
                            error!("Event loop polling error: {err}");
                            break 'event_loop;
                        }
                    }
                }

                // Handle synchronized update timeout.
                if events.is_empty() && self.rx.peek().is_none() {
                    state.parser.stop_sync(&mut *self.terminal.lock());
                    self.event_proxy.send_event(Event::Wakeup);
                    continue;
                }

                // Handle channel events, if there are any.
                if !self.drain_recv_channel(&mut state) {
                    break;
                }

                for event in events.iter() {
                    match event.key {
                        tty::PTY_CHILD_EVENT_TOKEN => {
                            if let Some(tty::ChildEvent::Exited(status)) = self.pty.next_child_event() {
                                if let Some(status) = status {
                                    self.event_proxy.send_event(Event::ChildExit(status));
                                }
                                if self.drain_on_exit {
                                    let _ = self.pty_read(&mut state, &mut buf, pipe.as_mut());
                                }
                                self.terminal.lock().exit();
                                self.event_proxy.send_event(Event::Wakeup);
                                break 'event_loop;
                            }
                        }

                        tty::PTY_READ_WRITE_TOKEN => {
                            if event.is_interrupt() {
                                // Don't try to do I/O on a dead PTY.
                                continue;
                            }

                            if event.readable {
                                if let Err(err) = self.pty_read(&mut state, &mut buf, pipe.as_mut()) {
                                    // On Linux, a `read` on the master side of a PTY can fail
                                    // with `EIO` if the client side hangs up.  In that case,
                                    // just loop back round for the inevitable `Exited` event.
                                    // This sucks, but checking the process is either racy or
                                    // blocking.
                                    #[cfg(target_os = "linux")]
                                    if err.raw_os_error() == Some(libc::EIO) {
                                        continue;
                                    }

                                    error!("Error reading from PTY in event loop: {err}");
                                    break 'event_loop;
                                }
                            }

                            if event.writable {
                                if let Err(err) = self.pty_write(&mut state) {
                                    error!("Error writing to PTY in event loop: {err}");
                                    break 'event_loop;
                                }
                            }
                        }
                        _ => (),
                    }
                }

                // Register write interest if necessary.
                let needs_write = state.needs_write();
                if needs_write != interest.writable {
                    interest.writable = needs_write;

                    // Re-register with new interest.
                    self.pty.reregister(&self.poll, interest, poll_opts).unwrap();
                }
            }

            // The evented instances are not dropped here so deregister them explicitly.
            let _ = self.pty.deregister(&self.poll);

            (self, state)
        })
    }
}

/// Helper type which tracks how much of a buffer has been written.
struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

pub struct Notifier(pub EventLoopSender);

impl event::Notify for Notifier {
    fn notify<B>(&self, bytes: B)
    where
        B: Into<Cow<'static, [u8]>>,
    {
        let bytes = bytes.into();
        // Terminal hangs if we send 0 bytes through.
        if bytes.is_empty() {
            return;
        }

        let _ = self.0.send(Msg::Input(bytes));
    }
}

impl event::OnResize for Notifier {
    fn on_resize(&mut self, window_size: WindowSize) {
        let _ = self.0.send(Msg::Resize(window_size));
    }
}

#[derive(Debug)]
pub enum EventLoopSendError {
    /// Error polling the event loop.
    Io(io::Error),

    /// Error sending a message to the event loop.
    Send(mpsc::SendError<Msg>),

    /// Retained input exceeded the bounded queue budget.
    Full,
}

impl Display for EventLoopSendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            EventLoopSendError::Io(err) => err.fmt(f),
            EventLoopSendError::Send(err) => err.fmt(f),
            EventLoopSendError::Full => f.write_str("terminal input queue is full"),
        }
    }
}

impl std::error::Error for EventLoopSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EventLoopSendError::Io(err) => err.source(),
            EventLoopSendError::Send(err) => err.source(),
            EventLoopSendError::Full => None,
        }
    }
}

#[derive(Clone)]
pub struct EventLoopSender {
    sender: Sender<Msg>,
    poller: Arc<Poller>,
    queued_input: Arc<AtomicUsize>,
}

impl EventLoopSender {
    pub fn send(&self, msg: Msg) -> Result<(), EventLoopSendError> {
        let bytes = match &msg {
            Msg::Input(bytes) => bytes.len(),
            _ => 0,
        };
        if bytes > 0 {
            let mut queued = self.queued_input.load(Ordering::Acquire);
            loop {
                let Some(next) = queued.checked_add(bytes).filter(|&next| next <= MAX_QUEUED_INPUT) else {
                    return Err(EventLoopSendError::Full);
                };
                match self.queued_input.compare_exchange_weak(
                    queued,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(actual) => queued = actual,
                }
            }
        }
        if let Err(error) = self.sender.send(msg) {
            if bytes > 0 {
                self.queued_input.fetch_sub(bytes, Ordering::AcqRel);
            }
            return Err(EventLoopSendError::Send(error));
        }
        self.poller.notify().map_err(EventLoopSendError::Io)
    }
}

/// All of the mutable state needed to run the event loop.
///
/// Contains list of items to write, current write state, etc. Anything that
/// would otherwise be mutated on the `EventLoop` goes here.
#[derive(Default)]
pub struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
    parser: ansi::Processor,
}

impl State {
    #[inline]
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    #[inline]
    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    #[inline]
    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    #[inline]
    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    #[inline]
    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }
}

impl Writing {
    #[inline]
    fn new(c: Cow<'static, [u8]>) -> Writing {
        Writing { source: c, written: 0 }
    }

    #[inline]
    fn advance(&mut self, n: usize) {
        self.written += n;
    }

    #[inline]
    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    #[inline]
    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}

struct PeekableReceiver<T> {
    rx: Receiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: Receiver<T>) -> Self {
        Self { rx, peeked: None }
    }

    fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }

        self.peeked.as_ref()
    }

    fn recv(&mut self) -> Option<T> {
        if self.peeked.is_some() {
            self.peeked.take()
        } else {
            match self.rx.try_recv() {
                Err(TryRecvError::Disconnected) => panic!("event loop channel closed"),
                res => res.ok(),
            }
        }
    }
}

#[cfg(test)]
mod input_budget_tests {
    use super::*;

    #[test]
    fn input_budget_spans_cloned_senders_and_keeps_control_messages_available() {
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let sender = EventLoopSender {
            sender: tx,
            poller: Arc::new(Poller::new().unwrap()),
            queued_input: queued.clone(),
        };
        sender.send(Msg::Input(vec![b'x'; MAX_QUEUED_INPUT].into())).unwrap();
        assert!(matches!(sender.clone().send(Msg::Input(vec![b'y'].into())), Err(EventLoopSendError::Full)));
        sender.send(Msg::Shutdown).unwrap();
        assert!(matches!(rx.recv().unwrap(), Msg::Input(_)));
        assert!(matches!(rx.recv().unwrap(), Msg::Shutdown));
        queued.store(0, Ordering::Release); // The payload was completely written.
        sender.send(Msg::Input(vec![b'z'].into())).unwrap();
        drop(rx);
        assert!(sender.send(Msg::Input(vec![b'z'].into())).is_err());
        assert_eq!(queued.load(Ordering::Acquire), 1, "failed send releases its reservation");
    }

    #[test]
    fn input_budget_is_released_only_after_the_entire_payload_is_written() {
        use crate::event::{OnResize, VoidListener};
        use std::io::Cursor;

        struct LimitedWriter {
            quota: usize,
        }
        impl Write for LimitedWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.quota == 0 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let count = self.quota.min(bytes.len());
                self.quota -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        struct TestPty {
            reader: Cursor<Vec<u8>>,
            writer: LimitedWriter,
        }
        impl tty::EventedReadWrite for TestPty {
            type Reader = Cursor<Vec<u8>>;
            type Writer = LimitedWriter;
            unsafe fn register(&mut self, _: &Arc<Poller>, _: PollingEvent, _: PollMode) -> io::Result<()> {
                Ok(())
            }
            fn reregister(&mut self, _: &Arc<Poller>, _: PollingEvent, _: PollMode) -> io::Result<()> {
                Ok(())
            }
            fn deregister(&mut self, _: &Arc<Poller>) -> io::Result<()> {
                Ok(())
            }
            fn reader(&mut self) -> &mut Self::Reader {
                &mut self.reader
            }
            fn writer(&mut self) -> &mut Self::Writer {
                &mut self.writer
            }
        }
        impl OnResize for TestPty {
            fn on_resize(&mut self, _: WindowSize) {}
        }
        impl tty::EventedPty for TestPty {
            fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
                None
            }
        }
        let size = crate::term::test::TermSize::new(80, 24);
        let term = Arc::new(FairMutex::new(Term::new(crate::term::Config::default(), &size, VoidListener)));
        let pty = TestPty { reader: Cursor::new(Vec::new()), writer: LimitedWriter { quota: 1 } };
        let mut event_loop = EventLoop::new(term, VoidListener, pty, true, false).unwrap();
        let sender = event_loop.channel();
        sender.send(Msg::Input(vec![b'x'; MAX_QUEUED_INPUT].into())).unwrap();
        let mut state = State::default();
        assert!(event_loop.drain_recv_channel(&mut state));
        event_loop.pty_write(&mut state).unwrap();
        assert_eq!(sender.queued_input.load(Ordering::Acquire), MAX_QUEUED_INPUT);
        assert!(matches!(sender.send(Msg::Input(vec![b'y'].into())), Err(EventLoopSendError::Full)));
        event_loop.pty.writer.quota = MAX_QUEUED_INPUT;
        event_loop.pty_write(&mut state).unwrap();
        assert_eq!(sender.queued_input.load(Ordering::Acquire), 0);
        sender.send(Msg::Input(vec![b'z'].into())).unwrap();
    }
}
