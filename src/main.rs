//! codn: a minimal, fast workspace for terminal coding agents.

mod action;
mod app;
mod core;
mod log;
mod picture;
mod platform;
mod render;
mod syntax;
mod term;
mod ui;

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use winit::event_loop::EventLoop;

use crate::core::Workspace;
use crate::core::config::Config;
use crate::core::state::Saved;
use crate::platform::instance::{self, Instance};
use crate::render::font::Fonts;

const USAGE: &str = "Usage: codn [FOLDER]...

Opens the workspace. Each FOLDER is added as a project.

Configuration: ~/.config/codn/config.toml";

/// Variables by which programs recognize the terminal they run in. codn
/// may itself be started from such a terminal, and its sessions must not
/// take themselves for running inside it.
const FOREIGN_TERMINAL: [&str; 17] = [
    "TMUX",
    "TMUX_PANE",
    "STY",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "KITTY_LISTEN_ON",
    "VTE_VERSION",
    "KONSOLE_VERSION",
    "KONSOLE_DBUS_SESSION",
    "KONSOLE_DBUS_SERVICE",
    "KONSOLE_DBUS_WINDOW",
    "WEZTERM_PANE",
    "ALACRITTY_SOCKET",
    "TERM_PROGRAM_VERSION",
];

/// Variables Claude Code sets for the commands it runs. codn may be started
/// by such a command, and a session that inherits them takes itself for a
/// child of that conversation: it stops saving its transcript, so there is
/// nothing to resume after a restart.
const FOREIGN_AGENT: [&str; 11] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_DISABLE_BG_SHELL_PRESSURE_REAP",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

fn main() -> ExitCode {
    // Started by a codn to cut the lines of a file into pieces: that is
    // another program in the same file, and it begins before anything
    // of the app does.
    let inner = std::env::args_os().nth(1);
    if inner.as_ref().is_some_and(|argument| argument == syntax::ARGUMENT) {
        return syntax::child();
    }
    // Or to make pixels of one picture.
    if inner.is_some_and(|argument| argument == picture::ARGUMENT) {
        return picture::child();
    }
    let started = std::time::Instant::now();
    log::install_panic_hook();
    scrub_environment();
    let folders = match arguments() {
        Ok(folders) => folders,
        Err(code) => return code,
    };

    // A second start hands its folders to the first and is done.
    let owner = match instance::claim(&folders) {
        Ok(Instance::Handed) => return ExitCode::SUCCESS,
        Ok(Instance::First(owner)) => owner,
        Err(e) => {
            log::log(format_args!("cannot tell whether codn is running already: {e}"));
            log::flush();
            return ExitCode::FAILURE;
        }
    };

    let result = window_loop(&owner, folders, started);
    if !term::finish_cleanup(std::time::Duration::from_secs(5)) {
        log::log("PTY cleanup is still waiting for a stuck process.");
    }
    drop(owner);
    log::flush();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::log(&e);
            log::flush();
            ExitCode::FAILURE
        }
    }
}

/// Takes the marks of a foreign terminal or agent out of the environment
/// the sessions inherit. Called while no other thread exists yet, which is
/// what makes changing the environment safe.
fn scrub_environment() {
    FOREIGN_TERMINAL.iter().chain(&FOREIGN_AGENT).for_each(|name| unsafe { std::env::remove_var(name) });
    // A codn started in a session of another one was named that session's
    // file. Its own sessions get theirs, and one that gets none must not
    // write into the other's.
    unsafe { std::env::remove_var(core::reported::VARIABLE) };
    // The same commands get an editor that does nothing. In a session it
    // would make `git commit` fail for want of a message.
    if std::env::var_os("GIT_EDITOR").is_some_and(|editor| editor == "true") {
        unsafe { std::env::remove_var("GIT_EDITOR") };
    }
}

/// The folders named on the command line, or the code to end with: help
/// and version print and are done, what is not a folder is an error.
fn arguments() -> Result<Vec<PathBuf>, ExitCode> {
    let mut folders = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Err(ExitCode::SUCCESS);
            }
            Some("-V" | "--version") => {
                println!("codn {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            _ => match std::fs::canonicalize(&arg) {
                Ok(path) if path.is_dir() => folders.push(path),
                _ => {
                    eprintln!("codn: {} is not a folder", arg.to_string_lossy());
                    return Err(ExitCode::FAILURE);
                }
            },
        }
    }
    Ok(folders)
}

/// Runs the app until its window closes. The app and the event loop are
/// gone when this returns, which `main` relies on: the cleanup of the PTYs
/// is waited for after them, and the owner of the instance outlives both.
fn window_loop(
    owner: &instance::Owner,
    folders: Vec<PathBuf>,
    started: Instant,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<app::UserEvent>::with_user_event().build()?;
    let config = Config::load();

    // The fonts are resolved while the window is being created.
    let (mono, ui) = (config.font.family.clone(), config.font.ui_family.clone());
    let proxy = event_loop.create_proxy();
    let fonts = std::thread::spawn(move || {
        Fonts::load(&mono, &ui, move || {
            let _ = proxy.send_event(app::UserEvent::Fonts);
        })
    });
    let proxy = event_loop.create_proxy();
    instance::listen(owner, move |folders| proxy.send_event(app::UserEvent::Open(folders)).is_ok())?;

    let saved = Saved::load();
    let mut ws = Workspace::restore(&saved);
    for folder in folders {
        if folder.to_str().is_some() {
            if ws.project_at(&folder).is_some() || ws.room() {
                ws.add_project(folder);
            }
        } else {
            log::log(format_args!("{} is not added: its name is not text", folder.display()));
        }
    }
    let mut app =
        app::App::new(config, saved, ws, event_loop.create_proxy(), fonts, started, owner.closing());
    event_loop.run_app(&mut app)?;
    Ok(())
}
