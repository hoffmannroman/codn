//! Desktop notifications through `notify-send`, silently skipped if missing.

use std::process::Command;

use super::start;

pub fn send(title: &str, body: &str) {
    let mut command = Command::new("notify-send");
    // The title starts with a folder name, which may begin with a dash.
    command.args(["--app-name", "codn", "--", title, body]);
    start::apart(command, "notify-send");
}
