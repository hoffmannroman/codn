//! Smithay Clipboard
//!
//! Provides access to the Wayland clipboard for gui applications. The user
//! should have surface around.

#![deny(clippy::all, clippy::if_not_else, clippy::enum_glob_use)]
use std::ffi::c_void;
use std::io::Result;
use std::sync::mpsc;
use std::time::Duration;

use sctk::reexports::calloop::channel::{self, Sender};
use sctk::reexports::client::Connection;
use sctk::reexports::client::backend::Backend;

mod mime;
mod state;
mod worker;

/// Access to a Wayland clipboard.
pub struct Clipboard {
    request_sender: Sender<worker::Command>,
    clipboard_thread: Option<std::thread::JoinHandle<()>>,
}

impl Clipboard {
    /// Creates new clipboard which will be running on its own thread with its
    /// own event queue to handle clipboard requests.
    ///
    /// # Safety
    ///
    /// `display` must be a valid `*mut wl_display` pointer, and it must remain
    /// valid for as long as `Clipboard` object is alive.
    pub unsafe fn new(display: *mut c_void) -> Self {
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);

        // Create channel to send data to clipboard thread.
        let (request_sender, rx_chan) = channel::channel();

        let name = String::from("smithay-clipboard");
        let clipboard_thread = worker::spawn(name, connection, rx_chan);

        Self { request_sender, clipboard_thread }
    }

    /// Load clipboard data.
    ///
    /// Loads content from a clipboard on a last observed seat.
    pub fn load(&self) -> Result<String> {
        self.request(false)
    }

    fn request(&self, primary: bool) -> Result<String> {
        let (tx, rx) = mpsc::channel();
        let command = if primary { worker::Command::LoadPrimary(tx) } else { worker::Command::Load(tx) };
        self.request_sender.send(command).map_err(|_| std::io::Error::other("clipboard is dead"))?;
        rx.recv_timeout(Duration::from_secs(3)).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "selection owner did not answer")
        })?
    }

    /// Store to a clipboard.
    ///
    /// Stores to a clipboard on a last observed seat.
    pub fn store<T: Into<String>>(&self, text: T) {
        let request = worker::Command::Store(text.into());
        let _ = self.request_sender.send(request);
    }

    /// Load primary clipboard data.
    ///
    /// Loads content from a  primary clipboard on a last observed seat.
    pub fn load_primary(&self) -> Result<String> {
        self.request(true)
    }

    /// Store to a primary clipboard.
    ///
    /// Stores to a primary clipboard on a last observed seat.
    pub fn store_primary<T: Into<String>>(&self, text: T) {
        let request = worker::Command::StorePrimary(text.into());
        let _ = self.request_sender.send(request);
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // Shutdown smithay-clipboard.
        let _ = self.request_sender.send(worker::Command::Exit);
        if let Some(clipboard_thread) = self.clipboard_thread.take() {
            let _ = clipboard_thread.join();
        }
    }
}
