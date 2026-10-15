extern crate x11rb;
extern crate libc;

pub mod error;
mod run;

pub use x11rb::protocol::xproto::{Atom, Window};
pub use x11rb::rust_connection::RustConnection;

use std::thread;
use std::time::{ Duration, Instant };
use std::sync::{ Arc, RwLock };
use std::sync::mpsc::{ Sender, channel };
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME};
use x11rb::errors::ConnectError;
use x11rb::protocol::{Event, xfixes};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, EventMask, Property, WindowClass};
use error::Error;
use run::{create_pipe_drop_fd, PipeDropFds};

pub const INCR_CHUNK_SIZE: usize = 4000;
const POLL_DURATION: u64 = 50;
type SetMap = Arc<RwLock<HashMap<Atom, (Atom, Vec<u8>)>>>;

#[derive(Clone, Debug)]
pub struct Atoms {
    pub primary: Atom,
    pub clipboard: Atom,
    pub property: Atom,
    pub targets: Atom,
    pub string: Atom,
    pub utf8_string: Atom,
    pub incr: Atom,
}

impl Atoms {
    fn intern_all(conn: &RustConnection) -> Result<Atoms, Error> {
        let clipboard = conn.intern_atom(
            false,
            b"CLIPBOARD",
        )?;
        let property = conn.intern_atom(
            false,
            b"THIS_CLIPBOARD_OUT",
        )?;
        let targets = conn.intern_atom(
            false,
            b"TARGETS",
        )?;
        let utf8_string = conn.intern_atom(
            false,
            b"UTF8_STRING",
        )?;
        let incr = conn.intern_atom(
            false,
            b"INCR",
        )?;
        Ok(Atoms {
            primary: Atom::from(AtomEnum::PRIMARY),
            clipboard: clipboard.reply()?.atom,
            property: property.reply()?.atom,
            targets: targets.reply()?.atom,
            string: Atom::from(AtomEnum::STRING),
            utf8_string: utf8_string.reply()?.atom,
            incr: incr.reply()?.atom,
        })
    }
}

/// X11 Clipboard
pub struct Clipboard {
    pub getter: Context,
    pub setter: Arc<Context>,
    setmap: SetMap,
    send: Sender<Atom>,
    // Relying on the Drop in OwnedFd to close the fd
    _drop_fd: OwnedFd,
}

pub struct Context {
    pub connection: RustConnection,
    pub screen: usize,
    pub window: Window,
    pub atoms: Atoms
}

#[inline]
fn get_atom(connection: &RustConnection, name: &str) -> Result<Atom, Error> {
    let intern_atom = connection.intern_atom(
        false,
        name.as_bytes()
    )?;
    let reply = intern_atom.reply()
        .map_err(Error::XcbReply)?;
    Ok(reply.atom)
}

impl Context {
    pub fn new(displayname: Option<&str>) -> Result<Self, Error> {
        let (connection, screen) = RustConnection::connect(displayname)?;
        let window = connection.generate_id()?;

        {
            let screen = connection.setup().roots.get(screen)
                .ok_or(Error::XcbConnect(ConnectError::InvalidScreen))?;
            connection.create_window(
                COPY_DEPTH_FROM_PARENT,
                window,
                screen.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE)
            )?
                .check()?;
        }

        let atoms = Atoms::intern_all(&connection)?;

        Ok(Context { connection, screen, window, atoms })
    }

    pub fn get_atom(&self, name: &str) -> Result<Atom, Error> {
        get_atom(&self.connection, name)
    }
}


impl Clipboard {
    /// Create Clipboard.
    pub fn new() -> Result<Self, Error> {
        let getter = Context::new(None)?;
        let setter = Arc::new(Context::new(None)?);
        let setter2 = Arc::clone(&setter);
        let setmap = Arc::new(RwLock::new(HashMap::new()));
        let setmap2 = Arc::clone(&setmap);

        let PipeDropFds {
            read_pipe, write_pipe
        } = create_pipe_drop_fd()?;
        let (sender, receiver) = channel();
        let max_length = setter.connection.maximum_request_bytes();
        thread::spawn(move || run::run(setter2, setmap2, max_length, receiver, read_pipe));

        Ok(Clipboard { getter, setter, setmap, send: sender, _drop_fd: write_pipe })
    }

    fn process_event<T>(&self, buff: &mut Vec<u8>, selection: Atom, target: Atom, property: Atom, timeout: T, use_xfixes: bool, sequence_number: u64)
        -> Result<(), Error>
        where T: Into<Option<Duration>>
    {
        let mut is_incr = false;
        let timeout = timeout.into();
        let start_time =
            if timeout.is_some() { Some(Instant::now()) }
            else { None };

        loop {
            if timeout.into_iter()
                .zip(start_time)
                .next()
                .map(|(timeout, time)| (Instant::now() - time) >= timeout)
                .unwrap_or(false)
            {
                return Err(Error::Timeout);
            }

            let (event, seq) = match use_xfixes {
                true => self.getter.connection.wait_for_event_with_sequence()?,
                false => {
                    match self.getter.connection.poll_for_event_with_sequence()? {
                        Some(event) => event,
                        None => {
                            // codn: wait for the server, not for a fixed time. An owner
                            // that hands a text over in pieces answers each within a
                            // millisecond, and a sleep after every piece let 80 KB a
                            // second through, under the deadline of the whole read.
                            wait_for_server(&self.getter.connection, Duration::from_millis(POLL_DURATION));
                            continue
                        }
                    }
                }
            };

            if seq < sequence_number {
                continue;
            }

            match event {
                Event::XfixesSelectionNotify(event) if use_xfixes => {
                    self.getter.connection.convert_selection(
                        self.getter.window,
                        selection,
                        target,
                        property,
                        event.timestamp,
                    )?.check()?;
                }
                Event::SelectionNotify(event) => {
                    if event.selection != selection { continue };

                    // Note that setting the property argument to None indicates that the
                    // conversion requested could not be made.
                    if event.property == Atom::from(AtomEnum::NONE) {
                        break;
                    }

                    let reply = self.getter.connection.get_property(
                        false,
                        self.getter.window,
                        event.property,
                        AtomEnum::NONE,
                        buff.len() as u32,
                        (MAX_SELECTION / 4 + 1) as u32
                    )?.reply()?;

                    if reply.type_ == self.getter.atoms.incr {
                        if let Some(mut value) = reply.value32() {
                            if let Some(size) = value.next() {
                                selection_room(buff.len(), size as usize)?;
                                buff.reserve(size as usize);
                            }
                        }
                        self.getter.connection.delete_property(
                            self.getter.window,
                            property
                        )?.check()?;
                        is_incr = true;
                        continue
                    } else if reply.type_ != target {
                        return Err(Error::UnexpectedType(reply.type_));
                    }

                    if reply.bytes_after != 0 { return Err(Error::TooLarge); }
                    selection_room(buff.len(), reply.value.len())?;
                    buff.extend_from_slice(&reply.value);
                    break
                }

                Event::PropertyNotify(event) if is_incr => {
                    if event.state != Property::NEW_VALUE { continue };


                    let cookie = self.getter.connection.get_property(
                        false,
                        self.getter.window,
                        property,
                        AtomEnum::NONE,
                        0,
                        0
                    )?;

                    let length = cookie.reply()?.bytes_after;
                    selection_room(buff.len(), length as usize)?;

                    let cookie = self.getter.connection.get_property(
                        true,
                        self.getter.window,
                        property,
                        AtomEnum::NONE,
                        0, length.div_ceil(4)
                    )?;
                    let reply = cookie.reply()?;
                    if reply.type_ != target { continue };

                    if reply.bytes_after != 0 { return Err(Error::TooLarge); }
                    let value = reply.value;

                    if !value.is_empty() {
                        selection_room(buff.len(), value.len())?;
                        buff.extend_from_slice(&value);
                    } else {
                        break
                    }
                },
                _ => ()
            }
        }
        Ok(())
    }

    /// load value.
    pub fn load<T>(&self, selection: Atom, target: Atom, property: Atom, timeout: T)
        -> Result<Vec<u8>, Error>
        where T: Into<Option<Duration>>
    {
        let mut buff = Vec::new();
        let timeout = timeout.into();

        let cookie = self.getter.connection.convert_selection(
            self.getter.window,
            selection,
            target,
            property,
            CURRENT_TIME,
            // FIXME ^
            // Clients should not use CurrentTime for the time argument of a ConvertSelection request.
            // Instead, they should use the timestamp of the event that caused the request to be made.
        )?;

        let sequence_number = cookie.sequence_number();
        cookie.check()?;

        self.process_event(&mut buff, selection, target, property, timeout, false, sequence_number)?;

        self.getter.connection.delete_property(
            self.getter.window,
            property
        )?.check()?;

        Ok(buff)
    }

    /// wait for a new value and load it
    pub fn load_wait(&self, selection: Atom, target: Atom, property: Atom)
        -> Result<Vec<u8>, Error>
    {
        let mut buff = Vec::new();

        let screen = &self.getter.connection.setup().roots.get(self.getter.screen)
            .ok_or(Error::XcbConnect(ConnectError::InvalidScreen))?;

        xfixes::query_version(
            &self.getter.connection,
            5,
            0,
        )?;
        // Clear selection sources...
        xfixes::select_selection_input(
            &self.getter.connection,
            screen.root,
            self.getter.atoms.primary,
            xfixes::SelectionEventMask::default()
        )?;
        xfixes::select_selection_input(
            &self.getter.connection,
            screen.root,
            self.getter.atoms.clipboard,
            xfixes::SelectionEventMask::default()
        )?;
        // ...and set the one requested now
        let cookie = xfixes::select_selection_input(
            &self.getter.connection,
            screen.root,
            selection,
            xfixes::SelectionEventMask::SET_SELECTION_OWNER |
                xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE |
                xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
        )?;

        let sequence_number = cookie.sequence_number();
        cookie.check()?;

        self.process_event(&mut buff, selection, target, property, None, true, sequence_number)?;

        self.getter.connection.delete_property(self.getter.window, property)?.check()?;

        Ok(buff)
    }

    /// store value.
    pub fn store<T: Into<Vec<u8>>>(&self, selection: Atom, target: Atom, value: T)
        -> Result<(), Error>
    {
        self.send.send(selection)?;
        self.setmap
            .write()
            .map_err(|_| Error::Lock)?
            .insert(selection, (target, value.into()));

        self.setter.connection.set_selection_owner(
            self.setter.window,
            selection,
            CURRENT_TIME
        )?.check()?;

        if self.setter.connection.get_selection_owner(
            selection
        )?.reply()
            .map(|reply| reply.owner == self.setter.window)
            .unwrap_or(false) {
            Ok(())
        } else {
            Err(Error::Owner)
        }
    }
}

// codn: waits until the server has sent something, `limit` at the most. What it sent
// is read by the caller: a wait that ends early for any reason costs one more turn.
fn wait_for_server(connection: &RustConnection, limit: Duration) {
    wait_readable(connection.stream().as_raw_fd(), limit);
}

fn wait_readable(fd: std::os::fd::RawFd, limit: Duration) {
    let mut asked = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let limit = limit.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
    // Safety: one descriptor that lives through the call, and a wait with an end.
    unsafe { libc::poll(&mut asked, 1, limit) };
}

// codn: reject hostile selection sizes before reserve, property fetch or append.
const MAX_SELECTION: usize = 8 * 1024 * 1024;
fn selection_room(retained: usize, incoming: usize) -> Result<(), Error> {
    if incoming > MAX_SELECTION.saturating_sub(retained) { Err(Error::TooLarge) } else { Ok(()) }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    #[test]
    fn a_wait_for_the_server_ends_when_it_has_sent_something() {
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        // Nothing was sent: the wait lasts as long as it may.
        let begun = Instant::now();
        wait_readable(pipe[0], Duration::from_millis(60));
        assert!(begun.elapsed() >= Duration::from_millis(50));
        // Something is there: it does not wait at all.
        assert_eq!(unsafe { libc::write(pipe[1], [0u8].as_ptr().cast(), 1) }, 1);
        let begun = Instant::now();
        wait_readable(pipe[0], Duration::from_secs(5));
        assert!(begun.elapsed() < Duration::from_secs(1));
        unsafe { libc::close(pipe[0]); libc::close(pipe[1]); }
    }

    // codn tells its user why a paste brought nothing, and knows these two by their words.
    #[test]
    fn a_read_that_was_late_or_too_large_says_so_in_these_words() {
        assert_eq!(Error::Timeout.to_string(), "Selection timed out");
        assert_eq!(Error::TooLarge.to_string(), "Selection exceeds 8 MiB");
    }

    #[test]
    fn selection_size_is_checked_before_allocation() {
        assert!(selection_room(0, MAX_SELECTION).is_ok());
        assert!(selection_room(0, usize::MAX).is_err());
        assert!(selection_room(MAX_SELECTION, 1).is_err());
        assert!(selection_room(MAX_SELECTION - 1, 2).is_err());
    }
}
