//! The user's shell: its search path, and whether a command is found on one.

use std::ffi::CString;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// The shell the user logs in with, if the environment says.
pub fn name() -> Option<String> {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty())
}

/// Where commands are looked for: on `own` if there is one, else on the
/// search path codn was started with.
pub fn search_path(own: Option<&str>) -> String {
    own.map(str::to_owned).or_else(|| std::env::var("PATH").ok()).unwrap_or_default()
}

pub fn which(cmd: &str, path: &str) -> bool {
    let executable = |path: &Path| {
        if !path.is_file() {
            return false;
        }
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else { return false };
        // Effective credentials and ACLs, not just whether some execute bit
        // is set. This checks access without running the configured program.
        unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0 }
    };
    if cmd.contains('/') {
        return executable(Path::new(cmd));
    }
    std::env::split_paths(path).any(|dir| executable(&dir.join(cmd)))
}

const MARK: &str = "__codn_path__";

/// The command that makes a shell print its search path between two marks,
/// so that whatever else its startup files print can be cut away.
fn path_query(shell: &str) -> String {
    if shell.ends_with("fish") {
        format!("echo {MARK}(string join : $PATH){MARK}")
    } else {
        // Braces, or the mark would read as part of the variable's name.
        format!("echo {MARK}${{PATH}}{MARK}")
    }
}

/// What a shell prints, up to the second mark. Startup files print what
/// they like, and as much. What they leave running can hold the output
/// open long after the path was printed, so its end is not waited for.
fn read_marked(stdout: impl Read) -> Vec<u8> {
    let (mut out, mut chunk) = (Vec::new(), [0; 4096]);
    let (mut marks, mut looked) = (0, 0);
    let mut stdout = stdout.take(1024 * 1024);
    while marks < 2
        && let Ok(n @ 1..) = stdout.read(&mut chunk)
    {
        out.extend_from_slice(&chunk[..n]);
        while let Some(at) = out[looked..].windows(MARK.len()).position(|part| part == MARK.as_bytes()) {
            marks += 1;
            looked += at + MARK.len();
        }
        // A mark may still be arriving: its start is looked at again.
        looked = looked.max(out.len().saturating_sub(MARK.len() - 1));
    }
    out
}

/// The search path an interactive login shell ends up with. A desktop
/// launcher starts codn without the user's shell setup, so tools installed
/// under the home directory are not found until this is asked.
pub fn shell_path() -> Option<String> {
    let shell = name()?;
    let print = path_query(&shell);
    let mut child = Command::new(&shell)
        .args(["-ilc", &print])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(stdout.map(read_marked).unwrap_or_default());
    });
    let out = rx.recv_timeout(Duration::from_secs(10));
    // A startup file can close the output and keep the shell running. It
    // gets a moment to end by itself, nobody waits longer than that: the
    // sessions that wait for this answer would wait with it.
    let grace = if out.is_ok() { Duration::from_millis(500) } else { Duration::ZERO };
    let _ = super::wait_until(child, Instant::now() + grace);
    between(&String::from_utf8_lossy(&out.ok()?))
}

/// The search path in what a shell printed: what stands between the
/// two marks, on one line. With one mark alone the answer was cut off,
/// and what follows it is a part of a path at the best: it would be the
/// search path of every session.
fn between(out: &str) -> Option<String> {
    let mut parts = out.split(MARK);
    let (_, path, _) = (parts.next()?, parts.next()?.trim(), parts.next()?);
    (!path.is_empty() && !path.contains(['\n', '\r', '\0'])).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_search_path_is_what_stands_between_two_marks_on_one_line() {
        let said = |text: &str| between(&text.replace('|', MARK));
        assert_eq!(said("a greeting\n|/a:/b|\n").as_deref(), Some("/a:/b"));
        assert_eq!(said("| /a:/b |").as_deref(), Some("/a:/b"));
        // Cut off behind the first mark: not the half of a path.
        assert_eq!(said("|/cut/off/in/the/mid"), None);
        assert_eq!(said("|/a\n/b:/c|"), None, "more than one line");
        assert_eq!(said("||"), None);
        assert_eq!(said("no mark at all"), None);
    }

    #[test]
    fn command_lookup_requires_an_executable_file() {
        let root = std::env::temp_dir().join(format!("codn-which-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("agent");
        std::fs::write(&file, "not executable").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!which(root.to_str().unwrap(), ""));
        assert!(!which(file.to_str().unwrap(), ""));
        assert!(!which("agent", root.to_str().unwrap()));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o001)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(!which("agent", root.to_str().unwrap()));
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(which("agent", root.to_str().unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_path_query_survives_a_real_shell() {
        let out = Command::new("/bin/sh")
            .args(["-c", &path_query("/bin/sh")])
            .env("PATH", "/a:/b")
            .output()
            .unwrap();
        let out = String::from_utf8(out.stdout).unwrap();
        assert_eq!(out.split(MARK).nth(1), Some("/a:/b"));
        assert!(path_query("/usr/bin/fish").contains("string join"));
    }

    #[test]
    fn the_path_is_taken_while_the_output_stays_open() {
        use std::io::Write;
        // What a startup file left running keeps the other end, and never
        // writes again. The marks arrive in pieces.
        let (reader, mut writer) = std::io::pipe().unwrap();
        let text = format!("motd\n{MARK}/a:/b{MARK}\nmore");
        let (first, second) = text.split_at(text.find(MARK).unwrap() + 5);
        let (tx, rx) = channel();
        std::thread::spawn(move || tx.send(read_marked(reader)));
        writer.write_all(first.as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        writer.write_all(second.as_bytes()).unwrap();
        let out = rx.recv_timeout(Duration::from_secs(3)).expect("the answer waits for the end");
        assert_eq!(String::from_utf8(out).unwrap().split(MARK).nth(1), Some("/a:/b"));
        drop(writer);
    }

    #[test]
    fn commands_are_looked_up_on_the_given_path() {
        assert!(which("sh", "/nonexistent:/bin:/usr/bin"));
        assert!(!which("sh", "/nonexistent"));
        assert!(which("/bin/sh", ""));
        assert!(!which("no-such-command-here", "/bin:/usr/bin"));
    }
}
