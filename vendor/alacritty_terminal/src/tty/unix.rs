//! TTY related functionality.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result};
use std::mem::MaybeUninit;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{env, ptr};

use libc::{F_GETFL, F_SETFL, O_NONBLOCK, TIOCSCTTY, c_int, fcntl};
use log::error;
use polling::{Event, PollMode, Poller};
use rustix_openpty::openpty;
use rustix_openpty::rustix::termios::Winsize;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix_openpty::rustix::termios::{self, InputModes, OptionalActions};
use signal_hook::low_level::{pipe as signal_pipe, unregister as unregister_signal};
use signal_hook::{SigId, consts as sigconsts};

use crate::event::{OnResize, WindowSize};
use crate::tty::{ChildEvent, EventedPty, EventedReadWrite, Options};

// Interest in PTY read/writes.
pub(crate) const PTY_READ_WRITE_TOKEN: usize = 0;

// Interest in new child events.
pub(crate) const PTY_CHILD_EVENT_TOKEN: usize = 1;

macro_rules! die {
    ($($arg:tt)*) => {{
        error!($($arg)*);
        std::process::exit(1);
    }};
}

/// Really only needed on BSD, but should be fine elsewhere.
fn set_controlling_terminal(fd: c_int) -> Result<()> {
    let res = unsafe {
        // TIOSCTTY changes based on platform and the `ioctl` call is different
        // based on architecture (32/64). So a generic cast is used to make sure
        // there are no issues. To allow such a generic cast the clippy warning
        // is disabled.
        #[allow(clippy::cast_lossless)]
        libc::ioctl(fd, TIOCSCTTY as _, 0)
    };

    if res == 0 { Ok(()) } else { Err(Error::last_os_error()) }
}

#[derive(Debug)]
struct Passwd<'a> {
    name: &'a str,
    dir: &'a str,
    shell: &'a str,
}

/// Return a Passwd struct with pointers into the provided buf.
///
/// # Unsafety
///
/// If `buf` is changed while `Passwd` is alive, bad thing will almost certainly happen.
fn get_pw_entry(buf: &mut [i8; 1024]) -> Result<Passwd<'_>> {
    // Create zeroed passwd struct.
    let mut entry: MaybeUninit<libc::passwd> = MaybeUninit::uninit();

    let mut res: *mut libc::passwd = ptr::null_mut();

    // Try and read the pw file.
    let uid = unsafe { libc::getuid() };
    let status =
        unsafe { libc::getpwuid_r(uid, entry.as_mut_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), &mut res) };
    if status != 0 {
        return Err(Error::from_raw_os_error(status));
    }
    if res.is_null() {
        return Err(Error::other("pw not found"));
    }
    // getpwuid_r initializes the entry only on success with a found record.
    let entry = unsafe { entry.assume_init() };

    if entry.pw_uid != uid || entry.pw_name.is_null() || entry.pw_dir.is_null() || entry.pw_shell.is_null() {
        return Err(Error::new(ErrorKind::InvalidData, "invalid passwd record"));
    }

    // Build a borrowed Passwd struct.
    Ok(Passwd {
        name: unsafe { CStr::from_ptr(entry.pw_name).to_str() }
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
        dir: unsafe { CStr::from_ptr(entry.pw_dir).to_str() }
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
        shell: unsafe { CStr::from_ptr(entry.pw_shell).to_str() }
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
    })
}

pub struct Pty {
    child: Option<Child>,
    #[cfg(target_os = "linux")]
    identity: Option<File>,
    #[cfg(target_os = "linux")]
    descendants: Vec<File>,
    reaped: bool,
    file: File,
    signals: UnixStream,
    sig_id: SigId,
}

impl Pty {
    pub fn child(&self) -> &Child {
        self.child.as_ref().expect("PTY owns its child until drop")
    }

    pub fn file(&self) -> &File {
        &self.file
    }
}

/// User information that is required for a new shell session.
struct ShellUser {
    user: String,
    home: String,
    shell: String,
}

impl ShellUser {
    /// look for shell, username, longname, and home dir in the respective environment variables
    /// before falling back on looking into `passwd`.
    fn from_env() -> Result<Self> {
        let mut buf = [0; 1024];
        let pw = get_pw_entry(&mut buf);

        let user = match env::var("USER") {
            Ok(user) => user,
            Err(_) => match pw {
                Ok(ref pw) => pw.name.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let home = match env::var("HOME") {
            Ok(home) => home,
            Err(_) => match pw {
                Ok(ref pw) => pw.dir.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let shell = match env::var("SHELL") {
            Ok(shell) => shell,
            Err(_) => match pw {
                Ok(ref pw) => pw.shell.to_owned(),
                Err(err) => return Err(err),
            },
        };

        Ok(Self { user, home, shell })
    }
}

#[cfg(not(target_os = "macos"))]
fn default_shell_command(shell: &str, _user: &str, _home: &str) -> Command {
    Command::new(shell)
}

#[cfg(target_os = "macos")]
fn default_shell_command(shell: &str, user: &str, home: &str) -> Command {
    let shell_name = shell.rsplit('/').next().unwrap();

    // On macOS, use the `login` command so the shell will appear as a tty session.
    let mut login_command = Command::new("/usr/bin/login");

    // Exec the shell with argv[0] prepended by '-' so it becomes a login shell.
    // `login` normally does this itself, but `-l` disables this.
    let exec = format!("exec -a -{} {}", shell_name, shell);

    // Since we use -l, `login` will not change directory to the user's home. However,
    // `login` only checks the current working directory for a .hushlogin file, causing
    // it to miss any in the user's home directory. We can fix this by doing the check
    // ourselves and passing `-q`
    let has_home_hushlogin = Path::new(home).join(".hushlogin").exists();

    // -f: Bypasses authentication for the already-logged-in user.
    // -l: Skips changing directory to $HOME and prepending '-' to argv[0].
    // -p: Preserves the environment.
    // -q: Act as if `.hushlogin` exists.
    //
    // XXX: we use zsh here over sh due to `exec -a`.
    let flags = if has_home_hushlogin { "-qflp" } else { "-flp" };
    login_command.args([flags, user, "/bin/zsh", "-fc", &exec]);
    login_command
}

/// Create a new TTY and return a handle to interact with it.
pub fn new(config: &Options, window_size: WindowSize, window_id: u64) -> Result<Pty> {
    let pty = openpty(None, Some(&window_size.to_winsize()))?;
    let (master, slave) = (pty.controller, pty.user);
    from_fd(config, window_id, master, slave)
}

/// Create a new TTY from a PTY's file descriptors.
pub fn from_fd(config: &Options, window_id: u64, master: OwnedFd, slave: OwnedFd) -> Result<Pty> {
    let master_fd = master.as_raw_fd();
    let slave_fd = slave.as_raw_fd();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(mut termios) = termios::tcgetattr(&master) {
        // Set character encoding to UTF-8.
        termios.input_modes.set(InputModes::IUTF8, true);
        let _ = termios::tcsetattr(&master, OptionalActions::Now, &termios);
    }

    let user = ShellUser::from_env()?;

    let mut builder = if let Some(shell) = config.shell.as_ref() {
        let mut cmd = Command::new(&shell.program);
        cmd.args(shell.args.as_slice());
        cmd
    } else {
        default_shell_command(&user.shell, &user.user, &user.home)
    };

    // Setup child stdin/stdout/stderr as slave fd of PTY.
    builder.stdin(slave.try_clone()?);
    builder.stderr(slave.try_clone()?);
    builder.stdout(slave);

    // Setup shell environment.
    window_environment(&mut builder, window_id);
    builder.env("USER", user.user);
    builder.env("HOME", user.home);
    for (key, value) in &config.env {
        builder.env(key, value);
    }

    // Prevent child processes from inheriting linux-specific startup notification env.
    builder.env_remove("XDG_ACTIVATION_TOKEN");
    builder.env_remove("DESKTOP_STARTUP_ID");

    let working_directory = config
        .working_directory
        .as_ref()
        .map(|path| CString::new(path.as_os_str().as_bytes()))
        .transpose()
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "working directory contains NUL"))?;

    unsafe {
        builder.pre_exec(move || {
            // Create a new process group.
            let err = libc::setsid();
            if err == -1 {
                return Err(Error::last_os_error());
            }

            // Fail the spawn if the folder disappeared or is inaccessible;
            // silently inheriting codn's folder could operate on the wrong repo.
            if let Some(working_directory) = working_directory.as_ref() {
                if libc::chdir(working_directory.as_ptr()) != 0 {
                    return Err(Error::last_os_error());
                }
            }

            set_controlling_terminal(slave_fd)?;

            // No longer need slave/master fds.
            libc::close(slave_fd);
            libc::close(master_fd);

            // codn: nor anything else the app has open. A library may
            // leave a file open without the flag that closes it here, as
            // drivers of graphics do. They are marked and not closed, so
            // that the way an error of the start is told still works. A
            // kernel that does not know the call leaves them as before.
            #[cfg(target_os = "linux")]
            libc::syscall(
                libc::SYS_close_range,
                3 as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_uint,
            );

            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGALRM, libc::SIG_DFL);

            Ok(())
        });
    }

    // Prepare signal handling before spawning child.
    let (signals, sig_id) = {
        let (sender, recv) = UnixStream::pair()?;

        // Register the recv end of the pipe for SIGCHLD.
        let sig_id = signal_pipe::register(sigconsts::SIGCHLD, sender)?;
        recv.set_nonblocking(true)?;
        (recv, sig_id)
    };

    match builder.spawn() {
        Ok(child) => {
            unsafe {
                // Maybe this should be done outside of this function so nonblocking
                // isn't forced upon consumers. Although maybe it should be?
                set_nonblocking(master_fd);
            }

            #[cfg(target_os = "linux")]
            let identity = process_identity(child.id());
            Ok(Pty {
                child: Some(child),
                #[cfg(target_os = "linux")]
                identity,
                #[cfg(target_os = "linux")]
                descendants: Vec::new(),
                reaped: false,
                file: File::from(master),
                signals,
                sig_id,
            })
        }
        Err(err) => {
            unregister_signal(sig_id);
            Err(Error::new(
                err.kind(),
                format!("Failed to spawn command '{}': {}", builder.get_program().to_string_lossy(), err),
            ))
        }
    }
}

// Linux pidfds name a process identity, not a PID which can be reused.
#[cfg(target_os = "linux")]
fn process_identity(pid: u32) -> Option<File> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 { None } else { Some(unsafe { File::from_raw_fd(fd as c_int) }) }
}

#[cfg(target_os = "linux")]
fn identity_exited(identity: &File) -> bool {
    let mut poll = libc::pollfd { fd: identity.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    unsafe { libc::poll(&mut poll, 1, 0) > 0 && poll.revents != 0 }
}

/// Whether a child has ended and waits to be reaped. It stays that way:
/// `WNOWAIT` leaves the status for `try_wait`, and with it the PID.
#[cfg(target_os = "linux")]
fn ended_unreaped(pid: u32) -> bool {
    let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
    let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
    // Safety: `info` is a valid place for the result. A zeroed siginfo_t is
    // a valid value, and waitid leaves the PID at zero when nothing ended.
    unsafe {
        libc::waitid(libc::P_PID, pid as libc::id_t, info.as_mut_ptr(), options) == 0
            && info.assume_init().si_pid() != 0
    }
}

#[cfg(target_os = "linux")]
fn signal_identity(identity: &File, signal: c_int) {
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            identity.as_raw_fd(),
            signal,
            ptr::null::<libc::siginfo_t>(),
            0,
        );
    }
}

/// Snapshot only members of the PTY's own session while its unreaped leader
/// still reserves that session ID. Check start time on both sides of pidfd_open
/// so PID reuse during enumeration cannot select an unrelated process.
#[cfg(target_os = "linux")]
fn session_members(leader: u32) -> Vec<File> {
    fn stat(pid: u32) -> Option<(u32, u64)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, fields) = text.rsplit_once(')')?;
        let fields: Vec<&str> = fields.split_whitespace().collect();
        Some((fields.get(3)?.parse().ok()?, fields.get(19)?.parse().ok()?))
    }
    let entries = match std::fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            if pid == leader {
                return None;
            }
            let before = stat(pid)?;
            if before.0 != leader {
                return None;
            }
            let identity = process_identity(pid)?;
            (stat(pid) == Some(before)).then_some(identity)
        })
        .collect()
}

impl Drop for Pty {
    fn drop(&mut self) {
        unregister_signal(self.sig_id);
        let Some(mut child) = self.child.take() else { return };
        #[cfg(target_os = "linux")]
        {
            if !self.reaped {
                self.descendants.extend(session_members(child.id()));
            }
            let identities: Vec<&File> = self.identity.iter().chain(self.descendants.iter()).collect();
            if self.identity.is_none() && !self.reaped {
                // Old kernels can lack pidfds. Child still owns an unreaped
                // process, so Child::kill cannot target a reused PID.
                let _ = child.kill();
            }
            for (signal, grace) in [
                (libc::SIGHUP, Duration::from_secs(2)),
                (libc::SIGTERM, Duration::from_secs(2)),
                (libc::SIGKILL, Duration::from_millis(200)),
            ] {
                for identity in &identities {
                    signal_identity(identity, signal);
                }
                let until = Instant::now() + grace;
                while identities.iter().any(|identity| !identity_exited(identity)) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if identities.iter().all(|identity| identity_exited(identity)) {
                    break;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        if !self.reaped {
            let _ = child.kill();
        }
        if child.try_wait().ok().flatten().is_none() {
            // A process stuck in the kernel can survive SIGKILL. Reap it off
            // the reader thread so its emulator and scrollback are released.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

impl EventedReadWrite for Pty {
    type Reader = File;
    type Writer = File;

    #[inline]
    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        poll_opts: PollMode,
    ) -> Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        unsafe {
            poll.add_with_mode(&self.file, interest, poll_opts)?;
        }

        unsafe { poll.add_with_mode(&self.signals, Event::readable(PTY_CHILD_EVENT_TOKEN), PollMode::Level) }
    }

    #[inline]
    fn reregister(&mut self, poll: &Arc<Poller>, mut interest: Event, poll_opts: PollMode) -> Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        poll.modify_with_mode(&self.file, interest, poll_opts)?;

        poll.modify_with_mode(&self.signals, Event::readable(PTY_CHILD_EVENT_TOKEN), PollMode::Level)
    }

    #[inline]
    fn deregister(&mut self, poll: &Arc<Poller>) -> Result<()> {
        poll.delete(&self.file)?;
        poll.delete(&self.signals)
    }

    #[inline]
    fn reader(&mut self) -> &mut File {
        &mut self.file
    }

    #[inline]
    fn writer(&mut self) -> &mut File {
        &mut self.file
    }
}

impl EventedPty for Pty {
    #[inline]
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        // See if there has been a SIGCHLD.
        let mut buf = [0u8; 1];
        if let Err(err) = self.signals.read(&mut buf) {
            if err.kind() != ErrorKind::WouldBlock {
                error!("Error reading from signal pipe: {err}");
            }
            return None;
        }

        // Match on the child process.
        // Snapshot before try_wait reaps the leader and frees its PID. The
        // signal says that some child of the process ended: looking at
        // every process is only worth it when it was this one.
        #[cfg(target_os = "linux")]
        let members = if self.reaped {
            Vec::new()
        } else if ended_unreaped(self.child().id()) {
            session_members(self.child().id())
        } else {
            return None;
        };
        match self.child.as_mut().expect("PTY owns child").try_wait() {
            Err(err) => {
                error!("Error checking child process termination: {err}");
                None
            }
            Ok(None) => None,
            Ok(exit_status) => {
                self.reaped = true;
                #[cfg(target_os = "linux")]
                self.descendants.extend(members);
                Some(ChildEvent::Exited(exit_status))
            }
        }
    }
}

impl OnResize for Pty {
    /// Resize the PTY.
    ///
    /// Tells the kernel that the window size changed with the new pixel
    /// dimensions and line/column counts.
    fn on_resize(&mut self, window_size: WindowSize) {
        let win = window_size.to_winsize();

        let res = unsafe { libc::ioctl(self.file.as_raw_fd(), libc::TIOCSWINSZ, &win as *const _) };

        if res < 0 {
            die!("ioctl TIOCSWINSZ failed: {}", Error::last_os_error());
        }
    }
}

/// Types that can produce a `Winsize`.
pub trait ToWinsize {
    /// Get a `Winsize`.
    fn to_winsize(self) -> Winsize;
}

impl ToWinsize for WindowSize {
    fn to_winsize(self) -> Winsize {
        let ws_row = self.num_lines as libc::c_ushort;
        let ws_col = self.num_cols as libc::c_ushort;

        let ws_xpixel = ws_col * self.cell_width as libc::c_ushort;
        let ws_ypixel = ws_row * self.cell_height as libc::c_ushort;
        Winsize { ws_row, ws_col, ws_xpixel, ws_ypixel }
    }
}

unsafe fn set_nonblocking(fd: c_int) {
    let res = unsafe { fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK) };
    assert_eq!(res, 0);
}

/// Zero means the caller has no native window id. Do not advertise an
/// Alacritty window or inherit the id of the terminal which started codn.
fn window_environment(builder: &mut Command, id: u64) {
    if id == 0 {
        builder.env_remove("ALACRITTY_WINDOW_ID");
        builder.env_remove("WINDOWID");
    } else {
        let id = id.to_string();
        builder.env("ALACRITTY_WINDOW_ID", &id);
        builder.env("WINDOWID", id);
    }
}

#[test]
fn no_native_window_id_removes_foreign_terminal_identifiers() {
    let mut command = Command::new("/bin/sh");
    command.env("WINDOWID", "foreign");
    command.env("ALACRITTY_WINDOW_ID", "foreign");
    window_environment(&mut command, 0);
    assert!(command.get_envs().filter(|(name, _)| {
        *name == "WINDOWID" || *name == "ALACRITTY_WINDOW_ID"
    }).all(|(_, value)| value.is_none()));
    window_environment(&mut command, 42);
    assert_eq!(command.get_envs().filter(|(_, value)| {
        *value == Some(std::ffi::OsStr::new("42"))
    }).count(), 2);
}

#[test]
fn test_get_pw_entry() {
    let mut buf: [i8; 1024] = [0; 1024];
    let _pw = get_pw_entry(&mut buf).unwrap();
}

#[cfg(all(test, target_os = "linux"))]
mod codn_cleanup_tests {
    use super::*;

    fn size() -> WindowSize {
        WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 }
    }

    #[test]
    fn a_missing_working_directory_fails_instead_of_inheriting_the_parent_folder() {
        let config = Options {
            shell: Some(crate::tty::Shell::new("/bin/sh".into(), vec!["-c".into(), "exit 0".into()])),
            working_directory: Some(
                std::env::temp_dir()
                    .join(format!("codn-missing-cwd-{}", std::process::id()))
                    .join("not-present"),
            ),
            ..Options::default()
        };
        assert!(new(&config, size(), 0).is_err());
    }

    #[test]
    fn hung_session_cleanup_escalates_and_leaves_unrelated_processes_alive() {
        let root = std::env::temp_dir().join(format!("codn-pty-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("ready");
        let child_marker = root.join("child");
        let script = format!(
            "trap '' HUP TERM; sleep 60 & echo $! > '{}'; echo ready > '{}'; wait",
            child_marker.display(),
            marker.display()
        );
        let config = Options {
            shell: Some(crate::tty::Shell::new("/bin/sh".into(), vec!["-c".into(), script])),
            ..Options::default()
        };
        let pty = new(&config, size(), 0).unwrap();
        let leader = pty.identity.as_ref().unwrap().try_clone().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            assert!(Instant::now() < until, "throwaway session did not become ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        let descendant_pid = std::fs::read_to_string(&child_marker).unwrap().trim().parse().unwrap();
        let descendant = process_identity(descendant_pid).unwrap();
        let mut unrelated = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let begun = Instant::now();
        drop(pty);
        let elapsed = begun.elapsed();
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            elapsed >= Duration::from_millis(3500) && elapsed < Duration::from_secs(6),
            "cleanup took {elapsed:?}"
        );
        assert!(identity_exited(&leader) && identity_exited(&descendant));
        assert!(unrelated_alive, "cleanup signalled a process outside the session");
    }
}
