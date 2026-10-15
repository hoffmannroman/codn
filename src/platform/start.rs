//! Starts a program of the desktop that codn hands something to and then
//! leaves alone: `xdg-open` and `notify-send`.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// Starts `command` with nothing of codn's: no input, no output, and none
/// of the files codn holds open. Says whether it was started.
pub(super) fn apart(mut command: Command, name: &str) -> bool {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // What such a program starts may run for hours, a browser, and is to
    // hold nothing codn has open. A library may leave a file open without
    // the flag that closes it here, as drivers of graphics do. They are
    // marked and not closed, so that the way an error of the start is
    // told still works. A kernel that does not know the call leaves them
    // as before.
    // SAFETY: one call of the system between fork and exec, which takes
    // no lock and asks for no memory.
    unsafe {
        command.pre_exec(|| {
            libc::syscall(
                libc::SYS_close_range,
                3 as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_uint,
            );
            Ok(())
        })
    };
    match command.spawn() {
        Ok(mut child) => {
            // Reap it off the main thread. Where no thread can be made
            // the child is left as it is, until codn ends: asking for
            // one with `thread::spawn` would end codn and every session.
            let _ = std::thread::Builder::new().name(name.into()).spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn a_program_that_is_started_holds_no_file_of_the_app() {
        let dir = std::env::temp_dir().join(format!("codn-start-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (stub, seen) = (dir.join("stub"), dir.join("seen"));
        // A file that is open as a library may leave one: without the
        // flag that closes it when a program is started.
        let held = std::fs::File::open(&dir).unwrap();
        // SAFETY: a flag of a descriptor that lives through the call.
        assert_eq!(unsafe { libc::fcntl(held.as_raw_fd(), libc::F_SETFD, 0) }, 0);
        // The program says whether it has that file, and what it was
        // handed, once both are known.
        let script = format!(
            "#!/bin/sh\nif [ -e /proc/$$/fd/{} ]; then held=held; else held=free; fi\n\
             printf '%s|' \"$held\" \"$@\" > '{}.new' && mv '{}.new' '{}'\n",
            held.as_raw_fd(),
            seen.display(),
            seen.display(),
            seen.display()
        );
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new(&stub);
        command.args(["--", "a b"]);
        assert!(apart(command, "stub"));
        let until = Instant::now() + Duration::from_secs(5);
        let said = loop {
            match std::fs::read_to_string(&seen) {
                Ok(said) => break said,
                Err(_) if Instant::now() > until => break String::new(),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(said, "free|--|a b|");
        // A program that is not there is not started, and says so.
        assert!(!apart(Command::new("/nonexistent/codn-start"), "none"));
    }
}
