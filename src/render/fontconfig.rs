//! What fontconfig knows, asked through its command line tools: the font
//! for a pattern, and every installed font with the characters it covers.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use crate::core::wait_until;
use crate::log::log;

/// A font file as fontconfig names it.
#[derive(Clone)]
pub(super) struct Match {
    pub(super) file: PathBuf,
    pub(super) index: u32,
    pub(super) family: String,
}

/// An installed font and the characters it has.
pub(super) struct Candidate {
    pub(super) font: Match,
    pub(super) color: bool,
    /// Inclusive code point ranges, ascending.
    ranges: Vec<(u32, u32)>,
}

impl Candidate {
    pub(super) fn covers(&self, c: char) -> bool {
        let c = c as u32;
        let next = self.ranges.partition_point(|&(_, end)| end < c);
        self.ranges.get(next).is_some_and(|&(start, _)| start <= c)
    }
}

/// Runs a fontconfig tool and returns the fonts it names, one per line as
/// file, index, color, family and (only for lists) charset, tab-separated.
fn fc_lines(tool: &str, args: &[&str], charset: bool) -> Vec<String> {
    let format = if charset {
        "%{file}\t%{index}\t%{color}\t%{family}\t%{charset}\n"
    } else {
        "%{file}\t%{index}\t%{color}\t%{family}\n"
    };
    // Fontconfig may itself wait on a broken font directory. Startup joins
    // the lookups, so they need the same deadline as other helpers. The
    // list of every font is read beside the running app, and takes its
    // time where many fonts are installed and none is cached yet.
    const LIMIT: u64 = 16 * 1024 * 1024;
    let timeout = if charset { LIST } else { LOOKUP };
    let Ok(mut child) = Command::new(tool)
        .args(["-f", format])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return Vec::new();
    };
    let deadline = Instant::now() + timeout;
    let Some(stdout) = child.stdout.take() else { return Vec::new() };
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(LIMIT + 1).read_to_end(&mut bytes);
        let _ = tx.send(result.map(|_| bytes));
    });
    let bytes = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok().and_then(Result::ok);
    if bytes.as_ref().is_none_or(|bytes| bytes.len() as u64 > LIMIT) {
        let _ = child.kill();
    }
    let status = wait_until(child, deadline);
    let Some(bytes) = bytes
        .filter(|bytes| bytes.len() as u64 <= LIMIT)
        .filter(|_| status.is_some_and(|status| status.success()))
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes).lines().map(str::to_owned).collect()
}

fn parse(line: &str) -> Option<(Match, bool, &str)> {
    let mut parts = line.splitn(5, '\t');
    let (file, index, color, family) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let font = Match { file: file.into(), index: index.parse().unwrap_or(0), family: family.to_owned() };
    Some((font, color == "True", parts.next().unwrap_or("")))
}

/// The font fontconfig answers a pattern with.
pub(super) fn best(pattern: &str) -> Option<Match> {
    fc_lines("fc-match", &[pattern], false)
        .iter()
        .filter_map(|line| parse(line))
        .map(|(font, ..)| font)
        .next()
}

/// The family to ask fontconfig for: the configured one if it is installed,
/// else the generic one. fontconfig answers every request with some font,
/// so a missing terminal font would otherwise come back as a proportional
/// one.
pub(super) fn installed(family: &str, generic: &str) -> String {
    if family.is_empty() {
        return generic.to_owned();
    }
    let wanted = family.to_lowercase();
    let begun = Instant::now();
    let found = best(family).is_some_and(|m| m.family.to_lowercase().split(',').any(|f| f == wanted));
    if !found {
        // No answer is not the answer that the font is missing.
        let why = if begun.elapsed() < LOOKUP { "is not installed" } else { "was not found in time" };
        log(format_args!("the font \"{family}\" {why}, using the system's {generic} font"));
        return generic.to_owned();
    }
    family.to_owned()
}

/// How long fontconfig may take for one font, while the window waits.
pub(super) const LOOKUP: Duration = Duration::from_secs(3);
/// How long it may take to list every font, which nothing waits for.
const LIST: Duration = Duration::from_secs(20);

/// Every installed font with its coverage, in fontconfig's order for
/// monospace text.
pub(super) fn candidates() -> Vec<Candidate> {
    let lines = fc_lines("fc-match", &["-s", "monospace"], true);
    let parsed = lines.iter().filter_map(|line| {
        let (font, color, charset) = parse(line)?;
        Some(Candidate { font, color, ranges: ranges(charset) })
    });
    parsed.collect()
}

/// Parses fontconfig's charset notation: hexadecimal code points and ranges
/// separated by spaces, like "20-7e a0 2190-21ff".
fn ranges(charset: &str) -> Vec<(u32, u32)> {
    let number = |s: &str| u32::from_str_radix(s, 16).ok();
    let mut ranges: Vec<(u32, u32)> = charset
        .split_whitespace()
        .filter_map(|part| match part.split_once('-') {
            Some((start, end)) => Some((number(start)?, number(end)?)),
            None => number(part).map(|c| (c, c)),
        })
        .collect();
    ranges.sort_unstable();
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_is_read_from_fontconfig_charsets() {
        let line = "/fonts/a b.ttf\t3\tTrue\tNoto Sans,Noto\t20-7e a0 2190-21ff 1f600-1f64f";
        let (font, color, charset) = parse(line).unwrap();
        assert_eq!((font.file.to_str(), font.index, color), (Some("/fonts/a b.ttf"), 3, true));
        let candidate = Candidate { font, color, ranges: ranges(charset) };
        for (c, covered) in [('a', true), ('~', true), ('\u{7f}', false), ('\u{a0}', true), ('\u{a1}', false)]
        {
            assert_eq!(candidate.covers(c), covered, "{c:?}");
        }
        assert!(candidate.covers('→') && candidate.covers('😀') && !candidate.covers('你'));
        assert!(ranges("").is_empty() && ranges("zz 41").len() == 1);
    }

    #[test]
    fn fontconfig_helpers_have_an_exit_deadline_and_output_limit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("codn-font-helper-{}", crate::core::uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let helper = dir.join("helper");
        std::fs::write(&helper, "#!/bin/sh\nexec 1>&-\nexec /bin/sleep 60\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let begun = Instant::now();
        assert!(fc_lines(helper.to_str().unwrap(), &[], false).is_empty());
        assert!(begun.elapsed() < Duration::from_secs(5), "helper closed output without exiting");
        std::fs::write(&helper, "#!/bin/sh\nexec /usr/bin/yes\n").unwrap();
        let begun = Instant::now();
        assert!(fc_lines(helper.to_str().unwrap(), &[], false).is_empty());
        assert!(begun.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
