//! Hands an address to the desktop through `xdg-open`, which opens it in
//! the browser the user chose there.

use std::os::unix::process::CommandExt;
use std::process::Command;

use super::start;

/// Asks the desktop to open an address of the web. Says whether
/// `xdg-open` was started; what becomes of the address it tells nobody.
pub fn address(address: &str) -> bool {
    start("xdg-open", address)
}

/// Starts a program with an address of the web as its one argument.
/// Says whether it was started.
fn start(program: &str, address: &str) -> bool {
    // The address is text a program printed. Nothing but the web is
    // handed on: what `xdg-open` does with a file or with a scheme of
    // some application is not for a click on a terminal to start. An
    // address of the web is no option of `xdg-open` either.
    if !web(address) {
        return false;
    }
    let mut command = Command::new(program);
    // Where no browser runs yet, `xdg-open` may start one as its child.
    // It is not to end with a signal meant for codn.
    command.arg(address).process_group(0);
    start::apart(command, "xdg-open")
}

/// Whether a text is an address of the web, by how it begins.
fn web(address: &str) -> bool {
    let begins =
        |scheme: &str| address.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme));
    (begins("http://") || begins("https://")) && !address.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn nothing_but_an_address_of_the_web_is_handed_on() {
        assert!(
            web("https://example.org/a?b=c") && web("http://localhost:3000") && web("HTTPS://EXAMPLE.ORG")
        );
        // What is started here is a program of the test's own, which writes
        // down what it was handed: with `xdg-open` itself, a fault in what
        // is held back would open a file on the desktop of whoever runs the
        // tests.
        let dir = std::env::temp_dir().join(format!("codn-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (stub, seen) = (dir.join("open"), dir.join("seen"));
        let script = format!("#!/bin/sh\nprintf '%s|' \"$#\" \"$@\" >> '{}'\n", seen.display());
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let stub = stub.to_str().unwrap();
        for other in [
            "",
            "--help",
            "-https://example.org",
            "file:///etc/passwd",
            "/etc/passwd",
            "javascript:alert(1)",
            "mailto:a@example.org",
            "vscode://file/etc/passwd",
            "https:/example.org",
            " https://example.org",
            "https://example.org/\n--help",
            "h\u{e4}ttps://example.org",
        ] {
            assert!(!web(other), "{other:?}");
            assert!(!start(stub, other), "{other:?}");
        }
        // An address of the web is its one argument, as it stands. It is
        // started last, so that whatever was started before it by
        // mistake has been written down by the time it is.
        assert!(start(stub, "https://example.org/a b?c=$HOME&d=`e`#f"));
        let until = Instant::now() + Duration::from_secs(5);
        let handed = loop {
            let handed = std::fs::read_to_string(&seen).unwrap_or_default();
            if handed.contains("#f|") || Instant::now() > until {
                break handed;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(handed, "1|https://example.org/a b?c=$HOME&d=`e`#f|");
        // A program that is not there is not started, and says so.
        assert!(!start("/nonexistent/codn-open", "https://example.org"));
    }
}
