use std::io::Result;
use std::sync::mpsc::Sender;

use sctk::reexports::calloop::channel::Channel;
use sctk::reexports::calloop::{EventLoop, channel};
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::Connection;
use sctk::reexports::client::globals::registry_queue_init;

use crate::state::{SelectionTarget, State};

/// Spawn a clipboard worker, which dispatches its own `EventQueue` and handles
/// clipboard requests.
pub fn spawn(
    name: String,
    display: Connection,
    rx_chan: Channel<Command>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            worker_impl(display, rx_chan);
        })
        .ok()
}

/// Clipboard worker thread command.
pub enum Command {
    /// Store data to a clipboard.
    Store(String),
    /// Store data to a primary selection.
    StorePrimary(String),
    /// Load data from a clipboard.
    Load(Sender<Result<String>>),
    /// Load primary selection.
    LoadPrimary(Sender<Result<String>>),
    /// Shutdown the worker.
    Exit,
}

/// Handle clipboard requests.
fn worker_impl(
    connection: Connection,
    rx_chan: Channel<Command>,
) {
    let (globals, event_queue) = match registry_queue_init(&connection) {
        Ok(data) => data,
        Err(_) => return,
    };

    let mut event_loop = EventLoop::<State>::try_new().unwrap();
    let loop_handle = event_loop.handle();

    let mut state = match State::new(&globals, &event_queue.handle(), loop_handle.clone())
    {
        Some(state) => state,
        None => return,
    };

    loop_handle
        .insert_source(rx_chan, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                match event {
                    Command::StorePrimary(contents) => {
                        state.store_selection(SelectionTarget::Primary, contents);
                    },
                    Command::Store(contents) => {
                        state.store_selection(SelectionTarget::Clipboard, contents);
                    },
                    Command::Load(reply) => {
                        if let Err(err) = state.load_selection(SelectionTarget::Clipboard, reply.clone()) {
                            let _ = reply.send(Err(err));
                        }
                    },
                    Command::LoadPrimary(reply) => {
                        if let Err(err) = state.load_selection(SelectionTarget::Primary, reply.clone()) {
                            let _ = reply.send(Err(err));
                        }
                    },
                    Command::Exit => state.exit = true,
                }
            }
        })
        .unwrap();

    WaylandSource::new(connection, event_queue).insert(loop_handle).unwrap();

    loop {
        if event_loop.dispatch(None, &mut state).is_err() || state.exit {
            break;
        }
    }
}
