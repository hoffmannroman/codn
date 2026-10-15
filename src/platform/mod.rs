//! Everything that touches the desktop: window, clipboard, notifications,
//! the one instance per state folder, and what opens an address.

pub mod clipboard;
pub mod instance;
pub mod notify;
pub mod open;
mod start;
pub mod window;
