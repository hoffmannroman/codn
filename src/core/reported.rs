//! What a session's program reports about itself: the model it runs, how
//! full its context is, how much of its limits is used and when they are
//! whole again.
//!
//! codn names a file for every program it starts in the variable
//! `CODN_STATUS`, the program writes there, and codn reads it. The format
//! is codn's own, a few keys of TOML, so nothing here knows an agent:
//! whatever runs in a session can write it, an agent's plugin as well as a
//! line of shell.
//!
//! A program that runs agents beside itself can also say here what each
//! of them is doing, by the id it reports the agent under to the terminal
//! (`program.rs`): the rows below a session show it. That stands last in
//! the file, behind a line `[agents]`, one agent to a line, and is read
//! line by line: see `agents`.

use std::collections::BTreeMap;
use std::fs::DirBuilder;
use std::io::{self, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::program::is_id;
use super::{SessionId, config, open_unblocking};
use crate::log::log;

/// The variable that names the file to the program of a session.
pub const VARIABLE: &str = "CODN_STATUS";

/// A report is a few lines. A file larger than this is not one.
const LIMIT: u64 = 16 * 1024;
/// How many of the characters that open a level a report may hold: the
/// dot of a key, a brace, a bracket. A level of TOML needs one of them, so
/// this is also how deep a report can go. The parser builds a tree and
/// walks it level by level on the stack: a few kilobytes of keys with dots
/// around braces go thousands of levels deep and would end the thread, and
/// with it the app. All of them count, also those in a text: which of
/// them stand in one only the parser knows.
const MAX_NESTING: usize = 64;
/// Of how many agents a report says what they are doing. A program
/// reports as many to its terminal at the most.
const MAX_AGENTS: usize = 32;
/// The table behind whose line a report says what the agents are doing,
/// and how many lines are looked at there.
const AGENTS: &str = "agents";
const AGENT_LINES: usize = 4 * MAX_AGENTS;
/// How long a model's name or the name of a limit may be, in characters.
const MAX_NAME: usize = 48;
/// How many limits are shown, and how many of the options a program runs
/// with.
const MAX_LIMITS: usize = 4;
const MAX_OPTIONS: usize = 4;
/// The most a limit can be used by, in percent: three digits to show.
const MAX_USED: f64 = 999.0;
/// The most tokens that are a number of tokens: what `count` has words for.
const MAX_TOKENS: f64 = 999e6;
/// More tokens a second than this are not shown.
const MAX_SPEED: f64 = 99_999.0;
const DAY: i64 = 24 * 60 * 60;
/// How far ahead a reset can lie. What says more counts in something else
/// than seconds.
const MAX_AHEAD: i64 = 100 * 366 * DAY;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reported {
    pub model: Option<String>,
    /// What the program runs with besides the model, in its own words: how
    /// hard it thinks, a mode it is in.
    pub options: Vec<String>,
    /// How many tokens the context holds now, and how many it can hold.
    pub tokens: Option<u64>,
    pub window: Option<u64>,
    /// How many tokens a second the last answer of the model came at.
    pub speed: Option<u32>,
    /// The limits, by the name each is shown under.
    pub limits: BTreeMap<String, Limit>,
    /// What the agents the program runs beside itself are doing, in its
    /// own words, by the id it reports each under to its terminal.
    pub agents: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limit {
    /// How much of it is used, in percent.
    pub used: f64,
    /// When it is whole again, as a time of day. Not a time that is left:
    /// that would be wrong a minute after it was read, and nothing redraws
    /// a window in which nothing happens.
    pub resets: Option<String>,
}

/// One figure of a report as it is shown: how much of something is used,
/// what it is, and what there is to add.
#[derive(Clone, Debug, PartialEq)]
pub struct Gauge {
    pub percent: f64,
    pub name: String,
    pub detail: Option<String>,
}

/// The folder of the files this run of codn names. One for every run: what
/// an earlier run left behind is told apart by it and swept away.
fn folder() -> PathBuf {
    config::state_dir().join("status").join(std::process::id().to_string())
}

/// The file of a program: of the session with this number, started as its
/// `generation`. Every start has its own, so what a program of an earlier
/// start still writes, or left behind, is never read as the next one's.
pub fn file(id: SessionId, generation: u64) -> PathBuf {
    folder().join(format!("{}-{generation}.toml", id.0))
}

/// Makes room for a report: the folder, and no file by that name. Waits for
/// the disk, so it is for a worker.
pub fn prepare(file: &Path) {
    let made =
        file.parent().map_or(Ok(()), |folder| DirBuilder::new().recursive(true).mode(0o700).create(folder));
    let removed = match std::fs::remove_file(file) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    if let Err(e) = made.and(removed) {
        log(format_args!("{}: {e}", file.display()));
    }
}

/// Removes what earlier runs of codn left in the folder of the reports.
/// Waits for the disk, so it is for a worker, and it is for the one codn
/// that owns the state.
pub fn sweep() {
    sweep_but(&folder());
}

fn sweep_but(own: &Path) {
    let Some(all) = own.parent() else { return };
    // Only in a folder of its own: one that is a link leads to a folder
    // that is somebody else's, with whatever lies there.
    if !all.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(all) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == own {
            continue;
        }
        // A link is removed, never followed.
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(e) = removed {
            log(format_args!("{}: {e}", path.display()));
        }
    }
}

/// Reads a report. Nothing if there is none; an error if the file cannot be
/// read as one, which it cannot while its program is writing it.
pub fn read(path: &Path) -> io::Result<Option<Reported>> {
    let file = match open_unblocking(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut text = String::new();
    file.take(LIMIT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > LIMIT {
        return Ok(None);
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs() as i64);
    parse(&text, now)
}

/// Takes from a report what fits, at the time `now` in seconds since
/// 1970. A key that holds something else than expected is left out, as is
/// one codn does not know.
fn parse(text: &str, now: i64) -> io::Result<Option<Reported>> {
    // A program empties its file before it writes it.
    if text.trim().is_empty() {
        return Err(io::Error::other("the report is empty"));
    }
    // What the agents are doing is no part of what the parser is handed
    // whole: it names files, and every dot of a name would count. Where
    // a text of several lines may stand before their line, that line may
    // be a line of the text: such a report is handed over whole.
    let lines = || text.split_inclusive('\n');
    let before = lines().position(heads_agents).map(|at| lines().take(at).map(str::len).sum::<usize>());
    let (text, doing) = match before.filter(|before| !several(&text[..*before])) {
        Some(before) => {
            let line = text[before..].split_inclusive('\n').next().map_or(0, str::len);
            (&text[..before], &text[before + line..])
        }
        None => (text, ""),
    };
    if levels(text) > MAX_NESTING {
        return Ok(None);
    }
    let table: toml::Table = text.parse().map_err(io::Error::other)?;
    let count = |key: &str| table.get(key).and_then(number).filter(|n| *n <= MAX_TOKENS).map(|n| n as u64);
    let limits = table.get("limits").and_then(toml::Value::as_table).into_iter().flatten();
    let limits = limits.filter_map(|(key, value)| Some((name(key)?, limit(value, now)?)));
    let options = table.get("options").and_then(toml::Value::as_array).into_iter().flatten();
    let reported = Reported {
        model: table.get("model").and_then(toml::Value::as_str).and_then(name),
        options: options.filter_map(|option| name(option.as_str()?)).take(MAX_OPTIONS).collect(),
        tokens: count("tokens"),
        window: count("window"),
        speed: table.get("speed").and_then(number).filter(|n| *n <= MAX_SPEED).map(|n| n.round() as u32),
        limits: distinct(limits),
        agents: agents(doing),
    };
    Ok(Some(reported).filter(|reported| *reported != Reported::default()))
}

/// The first limits there is room for, by their names as they are shown:
/// names that read alike are one, of which the last counts, and take one
/// place.
fn distinct(limits: impl Iterator<Item = (String, Limit)>) -> BTreeMap<String, Limit> {
    let mut shown = BTreeMap::new();
    for (name, limit) in limits {
        if shown.len() < MAX_LIMITS || shown.contains_key(&name) {
            shown.insert(name, limit);
        }
    }
    shown
}

/// Whether a line is the one behind which the agents stand: the line a
/// table of that name begins with, as TOML lets it be written, with
/// room around the name and a comment behind it.
fn heads_agents(line: &str) -> bool {
    // A carriage return that does not end the line ends a comment for
    // the parser, and what stands behind it would be dropped with the
    // comment here: such a line is none of theirs.
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.contains('\r') {
        return false;
    }
    let head = line.split('#').next().unwrap_or_default().trim();
    let name = head.strip_prefix('[').and_then(|head| head.strip_suffix(']'));
    name.is_some_and(|name| name.trim() == AGENTS)
}

/// Whether a text of several lines may stand in a report: only three
/// quotes in a row begin one.
fn several(text: &str) -> bool {
    text.contains("\"\"\"") || text.contains("'''")
}

/// How many characters of a text can open a level of TOML: every dot,
/// brace and bracket, wherever it stands.
fn levels(text: &str) -> usize {
    text.chars().filter(|c| matches!(c, '.' | '{' | '[')).count()
}

/// What the agents of a program are doing, from the lines behind
/// `[agents]`: each `id = "text"`, the id that of the record the program
/// reports the agent under.
///
/// Every line is read by itself, as a report of that one line, and held
/// against the same bound before the parser gets it. So a text may name
/// files, with a dot each, without the dots of all texts adding up to a
/// report that is dropped, and no line can be deeper than a report may
/// be. A line that is not one agent and its text is passed over, and the
/// next table ends them.
fn agents(lines: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    // Lines that say nothing are passed over before the parser is asked,
    // and do not count against the lines it is asked of.
    let says = |line: &&str| !line.is_empty() && !line.starts_with('#');
    for line in lines.lines().map(str::trim).filter(says).take(AGENT_LINES) {
        if line.starts_with('[') || found.len() == MAX_AGENTS {
            break;
        }
        if levels(line) > MAX_NESTING {
            continue;
        }
        let Ok(table) = line.parse::<toml::Table>() else { continue };
        let mut said = table.iter();
        let (Some((id, doing)), None) = (said.next(), said.next()) else { continue };
        if let Some(doing) = doing.as_str().and_then(name).filter(|_| is_id(id.as_bytes())) {
            found.insert(id.clone(), doing);
        }
    }
    found
}

/// A limit: how much of it is used, as a number alone or under `used`
/// beside `resets`, the second since 1970 at which it is whole again.
fn limit(value: &toml::Value, now: i64) -> Option<Limit> {
    let Some(table) = value.as_table() else {
        return Some(Limit { used: number(value)?.min(MAX_USED), resets: None });
    };
    let resets = table.get("resets").and_then(number).and_then(|at| moment(at as i64, now));
    Some(Limit { used: table.get("used").and_then(number)?.min(MAX_USED), resets })
}

/// A moment that lies ahead, as the time of day it has here, with its day
/// of the week if that is not within a day.
fn moment(at: i64, now: i64) -> Option<String> {
    if at <= now || at - now > MAX_AHEAD {
        return None;
    }
    let time = libc::time_t::try_from(at).ok()?;
    // SAFETY: `tm` is plain numbers and a pointer the call sets or leaves
    // null, and both arguments live through the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return None;
    }
    Some(clock(tm.tm_hour, tm.tm_min, (at - now >= DAY).then_some(tm.tm_wday)))
}

fn clock(hour: i32, minute: i32, weekday: Option<i32>) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let time = format!("{hour:02}:{minute:02}");
    match weekday.and_then(|day| DAYS.get(day as usize)) {
        Some(day) => format!("{day} {time}"),
        None => time,
    }
}

/// A number that can be shown: finite and not below zero.
fn number(value: &toml::Value) -> Option<f64> {
    let number = value.as_float().or_else(|| value.as_integer().map(|n| n as f64))?;
    // Without its sign: a zero may have one, and would be shown with it.
    (number.is_finite() && number >= 0.0).then_some(number.abs())
}

/// A name as it is shown: one line of what can be seen, of a length that
/// leaves room for the rest. Also the title of a record a program reports
/// to its terminal (`src/core/program.rs`).
pub(super) fn name(text: &str) -> Option<String> {
    let seen: String = text.chars().filter(|c| !c.is_control() && !steers(*c)).collect();
    let name: String = seen.trim().chars().take(MAX_NAME).collect();
    (!name.is_empty()).then_some(name)
}

/// Characters that are not seen: those that change how what follows is
/// laid out (no width, the direction of writing, the separators of lines
/// and paragraphs), and those that draw nothing though their kind is that
/// of a letter or a mark (fillers, the empty pattern of Braille, what
/// chooses the form of the character before it, tags).
pub(super) fn steers(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{34f}'
            | '\u{61c}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202f}'
            | '\u{2060}'..='\u{206f}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fffb}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

impl Reported {
    /// What the agent the program reports under this id is doing, where
    /// the program said so.
    pub fn doing(&self, id: &str) -> Option<&str> {
        self.agents.get(id).map(String::as_str)
    }

    /// What the program runs with, in a few words: the model, then its
    /// options. Nothing for a program that reported neither.
    pub fn runs(&self) -> Option<String> {
        let words: Vec<&str> = self.model.iter().chain(&self.options).map(String::as_str).collect();
        (!words.is_empty()).then(|| words.join(" · "))
    }

    /// The figures of the report, each one a part of a whole: the context
    /// first, then the limits.
    pub fn gauges(&self) -> Vec<Gauge> {
        let context =
            self.tokens.zip(self.window.filter(|window| *window > 0)).map(|(tokens, window)| Gauge {
                percent: (tokens as f64 * 100.0 / window as f64).min(MAX_USED),
                name: "context".into(),
                detail: Some(format!("{} / {}", count(tokens), count(window))),
            });
        let limits = self.limits.iter().map(|(name, limit)| Gauge {
            percent: limit.used,
            name: name.clone(),
            detail: limit.resets.as_ref().map(|at| format!("resets {at}")),
        });
        context.into_iter().chain(limits).collect()
    }
}

impl Gauge {
    /// The figure as it is written beside its ring.
    pub fn figure(&self) -> String {
        format!("{}%", self.whole())
    }

    /// The whole percent that is shown, as a figure and as a ring: both
    /// from one number, so that a ring shows nothing its figure denies.
    pub fn whole(&self) -> f64 {
        self.percent.round_ties_even()
    }
}

/// A number of tokens, up to `MAX_TOKENS`, in at most four characters and
/// a unit: from a hundred million on without a tenth.
fn count(n: u64) -> String {
    let short = |value: f64, unit: &str| {
        let text = format!("{value:.1}");
        format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
    };
    match n {
        0..1000 => n.to_string(),
        // 999,500 and more would read "1000k".
        1000..999_500 => format!("{:.0}k", n as f64 / 1e3),
        // 99,950,000 and more would read "100.0M".
        999_500..99_950_000 => short(n as f64 / 1e6, "M"),
        _ => format!("{:.0}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// A moment at which every test reads its report.
    const NOW: i64 = 1_791_290_000;

    fn said(text: &str) -> Reported {
        parse(text, NOW).unwrap().unwrap()
    }

    fn shown(reported: &Reported) -> Vec<String> {
        let line =
            |gauge: Gauge| format!("{} {} {}", gauge.figure(), gauge.name, gauge.detail.unwrap_or_default());
        reported.gauges().into_iter().map(line).collect()
    }

    #[test]
    fn a_report_is_read_key_by_key() {
        let full = said(
            "model = \"opus\"\ntokens = 124000\nwindow = 200000\n\n[limits.5h]\nused = 34\n\n[limits.7d]\nused = 12.4\n",
        );
        assert_eq!(full.model.as_deref(), Some("opus"));
        assert_eq!((full.tokens, full.window), (Some(124_000), Some(200_000)));
        assert_eq!(shown(&full), ["62% context 124k / 200k", "34% 5h ", "12% 7d "]);
        assert_eq!(full.runs().as_deref(), Some("opus"));
        // What does not fit is left out, the rest is taken.
        let partly = said("model = 7\ntokens = \"many\"\nwindow = 200000.0\nlater = true\n");
        assert_eq!(partly, Reported { window: Some(200_000), ..Default::default() });
        // A part of nothing, or of what is not known, is no figure.
        assert!(partly.gauges().is_empty());
        assert!(said("tokens = 512").gauges().is_empty());
        assert!(said("tokens = 512\nwindow = 0").gauges().is_empty());
    }

    #[test]
    fn the_speed_is_a_whole_number_of_tokens_a_second() {
        assert_eq!(said("speed = 78").speed, Some(78));
        assert_eq!(said("speed = 77.5").speed, Some(78));
        assert_eq!(said("speed = 0").speed, Some(0));
        assert_eq!(said("speed = 99999").speed, Some(99_999));
        // What is no speed is left out, and the rest is taken.
        for none in ["speed = 100000", "speed = -1", "speed = nan", "speed = inf", "speed = \"fast\""] {
            let text = format!("model = \"opus\"\n{none}\n");
            assert_eq!(said(&text), Reported { model: Some("opus".into()), ..Default::default() }, "{none}");
        }
        // It is no part of a whole: no ring is drawn for it.
        assert!(said("speed = 78").gauges().is_empty());
    }

    #[test]
    fn what_the_plugin_for_claude_code_writes_is_read() {
        // The texts that mods/codn-status/tests/register.test.ts expects
        // the plugin to write. Change both together.
        let usual = said(
            "model = \"opus\"\noptions = [\"high\", \"think\"]\ntokens = 124000\nwindow = 200000\n\n\
             [limits.\"5h\"]\nused = 34\n",
        );
        assert_eq!(usual.runs().as_deref(), Some("opus · high · think"));
        assert_eq!(shown(&usual), ["62% context 124k / 200k", "34% 5h "]);
        assert_eq!(usual.speed, None);
        // With the speed of the last answer, which stands before the limits.
        let fast = said(
            "model = \"opus\"\noptions = [\"high\", \"think\"]\ntokens = 124000\nwindow = 200000\nspeed = 75\n\n\
             [limits.\"5h\"]\nused = 34\n",
        );
        assert_eq!(fast, Reported { speed: Some(75), ..usual.clone() });
        let odd = said(
            "model = \"opus \\\"5\\\"\"\n\n[limits.\"5h\"]\nused = 999\nresets = 1791315361\n\n\
             [limits.\"__proto__\"]\nused = 3\n\n[limits.\"constructor\"]\nused = 4\n\n[limits.\"ab\"]\nused = 5\n",
        );
        assert_eq!(odd.model.as_deref(), Some("opus \"5\""));
        assert_eq!(odd.limits.keys().collect::<Vec<_>>(), ["5h", "__proto__", "ab", "constructor"]);
        assert_eq!((odd.limits["5h"].used, &odd.limits["5h"].resets), (999.0, &moment(1_791_315_361, NOW)));
        assert!(odd.limits["5h"].resets.is_some());
        // A quote in a name is written as its number: three of them in
        // a row would begin a text of several lines, behind which no
        // agents are read.
        let quotes = said("model = \"o\\u0027\\u0027\\u0027pus\"\n\n[agents]\n\"a1\" = \"Read a.rs\"\n");
        assert_eq!((quotes.model.as_deref(), quotes.doing("a1")), (Some("o'''pus"), Some("Read a.rs")));
        // With what the agents of the session are doing behind it.
        let agents = said(
            "model = \"opus\"\noptions = [\"high\", \"think\"]\ntokens = 124000\nwindow = 200000\n\n\
             [limits.\"5h\"]\nused = 34\n\n[agents]\n\"a89b5d4516757b675\" = \"Show current time\"\n",
        );
        assert_eq!(agents.doing("a89b5d4516757b675"), Some("Show current time"));
        assert_eq!(shown(&agents), shown(&usual));
        let odd =
            said("[agents]\n\"a1\" = \"say \\\"hi\\\" \\\\ there\"\n\"team-one--x-\" = \"Read a.rs\"\n");
        assert_eq!(
            (odd.doing("a1"), odd.doing("team-one--x-")),
            (Some("say \"hi\" \\ there"), Some("Read a.rs"))
        );
    }

    #[test]
    fn what_the_reporter_for_codex_writes_is_read() {
        // The text that mods/codn-codex/tests/test_report.py expects the
        // reporter to write for a turn, from the file both tests read.
        let usual = said(include_str!("../../mods/codn-codex/tests/report.toml"));
        assert_eq!(usual.runs().as_deref(), Some("gpt-6.1-sol · high"));
        assert_eq!((usual.tokens, usual.window), (Some(21_382), Some(760_000)));
        assert_eq!(usual.limits.keys().collect::<Vec<_>>(), ["5h", "7d"]);
        assert_eq!(
            (usual.limits["5h"].used, &usual.limits["5h"].resets),
            (33.0, &moment(1_791_310_998, NOW))
        );
        assert_eq!(
            (usual.limits["7d"].used, &usual.limits["7d"].resets),
            (18.0, &moment(1_791_833_854, NOW))
        );
        // And texts it expects for names and figures that are out of the
        // ordinary. Change both together.
        let odd = said(
            "model = \"gpt \\\"6\\\"[31m\"\noptions = [\"high\"]\n\n[limits.\"5h\"]\nused = 12.50001\n\n\
             [limits.\"3d\"]\nused = 999\nresets = 1791833854\n\n\
             [limits.\"odd \\\"name\\\"\"]\nused = 4\n\n[limits.\"90m\"]\nused = 1\n",
        );
        assert_eq!(odd.model.as_deref(), Some("gpt \"6\"[31m"));
        assert_eq!(odd.limits.keys().collect::<Vec<_>>(), ["3d", "5h", "90m", "odd \"name\""]);
        assert_eq!((odd.limits["5h"].used, odd.limits["3d"].used), (12.50001, 999.0));
        assert!(shown(&odd).iter().any(|gauge| gauge.starts_with("13% 5h")), "rounded once, and here");
    }

    #[test]
    fn what_the_plugin_for_opencode_writes_is_read() {
        // The text that mods/codn-opencode/tests/report.test.ts expects the
        // plugin to write for a turn, from the file both tests read.
        let usual = said(include_str!("../../mods/codn-opencode/tests/report.toml"));
        assert_eq!(usual.runs().as_deref(), Some("big-pickle · build"));
        assert_eq!((usual.tokens, usual.window, usual.speed), (Some(1_900), Some(200_000), Some(50)));
        assert!(usual.limits.is_empty());
        assert_eq!(usual.agents.get("ses_0a1b2c3d4e60NoPqRsTuVwXyZa").map(String::as_str), Some("Read HEAD"));
    }

    #[test]
    fn what_a_program_runs_with_follows_its_model() {
        let both = said("model = \"opus\"\noptions = [\"high\", \"think\"]\n");
        assert_eq!(both.runs().as_deref(), Some("opus · high · think"));
        // Options alone are a report too, and tokens alone say nothing here.
        assert_eq!(said("options = [\"plan\"]").runs().as_deref(), Some("plan"));
        assert_eq!(said("tokens = 5").runs(), None);
        // What is no name is left out, and no more than four are taken.
        let odd = said("options = [\"a\\u202E\\n\", 7, \"\", [\"b\"], \"c\", \"d\", \"e\", \"f\"]\n");
        assert_eq!(odd.options, ["a", "c", "d", "e"]);
        assert_eq!(parse("options = \"high\"\n", NOW).unwrap(), None);
    }

    #[test]
    fn numbers_are_shown_only_within_what_they_can_be() {
        // Below zero, not a number, without end, beyond counting: none of
        // them is a number of tokens, and of a limit only the last is used.
        for bad in ["-1", "nan", "inf", "-inf", "1e30"] {
            let reported =
                said(&format!("model = \"m\"\ntokens = {bad}\nwindow = {bad}\n[limits]\na = {bad}\n"));
            let most = (bad == "1e30").then_some(MAX_USED);
            assert_eq!((reported.tokens, reported.window), (None, None), "{bad}");
            assert_eq!(reported.limits.get("a").map(|limit| limit.used), most, "{bad}");
        }
        // A figure has three digits at most, whatever the file says.
        let over = said("tokens = 5000\nwindow = 1\n[limits]\na = 1e300\nb = { used = 1e300 }\n");
        assert_eq!(shown(&over), ["999% context 5k / 1", "999% a ", "999% b "]);
        assert_eq!(said("tokens = 999e6").tokens, Some(999_000_000));
    }

    #[test]
    fn a_limit_is_a_number_or_says_when_it_resets() {
        let soon = NOW + 3600;
        let limits = said(&format!(
            "[limits]\nplain = 5\npast = {{ used = 6, resets = {} }}\nsoon = {{ used = 7, resets = {soon} }}\n\
             far = {{ used = 108, resets = {} }}\nnone = {{ resets = {soon} }}\n",
            NOW - 1,
            NOW + 3 * DAY
        ))
        .limits;
        assert_eq!(limits.keys().collect::<Vec<_>>(), ["far", "past", "plain", "soon"]);
        assert_eq!((limits["plain"].used, &limits["plain"].resets), (5.0, &None));
        assert_eq!(limits["past"].resets, None);
        assert_eq!(limits["soon"].resets, moment(soon, NOW));
        assert_eq!((limits["far"].used, &limits["far"].resets), (108.0, &moment(NOW + 3 * DAY, NOW)));
        assert_eq!(clock(9, 5, None), "09:05");
        assert_eq!(clock(19, 36, Some(1)), "Mon 19:36");
    }

    /// What `date` makes of a moment, in the zone the test runs in.
    fn date(at: i64, format: &str) -> String {
        let said =
            Command::new("date").env("LC_ALL", "C").args(["-d", &format!("@{at}"), format]).output().unwrap();
        String::from_utf8(said.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn a_reset_is_the_time_of_day_it_has_here() {
        // Hour and minute differ, and so do the hour here and elsewhere
        // wherever the test does not run on Greenwich time.
        let at = NOW + 3 * 3600 + 17 * 60;
        assert_eq!(moment(at, NOW), Some(date(at, "+%H:%M")));
        // From a day ahead on, the day of the week says which day.
        assert_eq!(moment(NOW + DAY - 1, NOW), Some(date(NOW + DAY - 1, "+%H:%M")));
        assert_eq!(moment(NOW + DAY, NOW), Some(date(NOW + DAY, "+%a %H:%M")));
        assert_eq!(moment(NOW + 5 * DAY, NOW), Some(date(NOW + 5 * DAY, "+%a %H:%M")));
        // Now is not ahead, and a moment counted in milliseconds is none.
        assert_eq!((moment(NOW, NOW), moment(NOW - 1, NOW)), (None, None));
        assert_eq!(moment(NOW * 1000, NOW), None);
        assert!(moment(NOW + MAX_AHEAD, NOW).is_some());
        assert_eq!(moment(NOW + MAX_AHEAD + 1, NOW), None);
    }

    #[test]
    fn nothing_and_half_a_report_are_told_apart() {
        assert_eq!(parse("limits = 3\nmodel = \"\"\n", NOW).unwrap(), None);
        assert_eq!(parse("# nothing yet\n", NOW).unwrap(), None);
        // A file its program emptied to write it, or is in the middle of.
        assert!(parse("", NOW).is_err());
        assert!(parse(" \n", NOW).is_err());
        assert!(parse("model = \"op", NOW).is_err());
    }

    #[test]
    fn a_report_cannot_go_deeper_than_a_worker_can_follow() {
        // 79 keys of 79 parts around 79 braces: 12 KiB, 6,241 levels. The
        // parser takes each of the two alone and overflows the stack of a
        // thread on both together.
        let key = vec!["a"; 79].join(".");
        let deep = format!("{}x=0{}", format!("{key}={{").repeat(79), "}".repeat(79));
        assert!((deep.len() as u64) < LIMIT);
        assert_eq!(parse(&deep, NOW).unwrap(), None);
        // As deep as it may go is read, also by a worker.
        let most = format!("model = \"m\"\n{} = 1\n", vec!["a"; MAX_NESTING + 1].join("."));
        let worker = std::thread::Builder::new().spawn(move || parse(&most, NOW).unwrap());
        assert_eq!(worker.unwrap().join().unwrap().unwrap().model.as_deref(), Some("m"));
        let over = format!("model = \"m\"\n{} = 1\n", vec!["a"; MAX_NESTING + 2].join("."));
        assert_eq!(parse(&over, NOW).unwrap(), None);
        // A report as the plugin for Claude Code writes it is far from that.
        let usual = "model = \"claude-opus-5.5\"\ntokens = 1\nwindow = 2\n\n[limits.\"5h\"]\nused = 3.5\nresets = 9\n";
        assert!(levels(&usual.repeat(4)) <= MAX_NESTING);
    }

    #[test]
    fn what_agents_are_doing_may_name_files_and_no_line_of_it_goes_deep() {
        // The names of files in what agents are doing: dots by the dozen,
        // which would end a report if they counted together.
        let doing: String =
            (0..MAX_AGENTS).map(|n| format!("a{n} = \"Read a.b.c/sidebar.rs [1]\"\n")).collect();
        assert!(levels(&doing) > MAX_NESTING);
        let full = said(&format!("model = \"m\"\n[agents]\n{doing}"));
        assert_eq!((full.agents.len(), full.model.as_deref()), (MAX_AGENTS, Some("m")));
        // A report that goes deep is none, whatever it is hidden in:
        // 6,241 levels end a worker. In a comment that a carriage return
        // ends for the parser, with quotes around the depth, which ended
        // the app while the dots in a text of one line were not counted.
        let key = vec!["a"; 79].join(".");
        let deep = format!("{}x=0{}", format!("{key}={{").repeat(79), "}".repeat(79));
        for hidden in
            [format!("# \"\r{deep}\"\n"), format!("# '\r{deep}'\n"), format!("s = \"\"\" \" \"\"\"\n{deep}")]
        {
            let read = std::thread::Builder::new().spawn(move || parse(&hidden, NOW).unwrap());
            assert_eq!(read.unwrap().join().unwrap(), None);
        }
        // Nor behind the line of the agents, where every line is held
        // against the bound by itself: such a line is passed over, and
        // the agents around it are read.
        for hidden in
            [deep.clone(), format!("# \"\r{deep}\""), format!("a9 = \"\r{deep}\""), format!("\r{deep}")]
        {
            let text = format!("model = \"m\"\n[agents]\na1 = \"one\"\n{hidden}\na2 = \"two\"\n");
            let read = std::thread::Builder::new().spawn(move || parse(&text, NOW).unwrap().unwrap());
            let read = read.unwrap().join().unwrap();
            assert_eq!(read.agents.keys().collect::<Vec<_>>(), ["a1", "a2"]);
        }
        // A text may hold whatever a line of TOML may: three quotes in
        // a row, which begin a text of several lines where they stand
        // alone, are part of it here.
        let quotes = said(
            "[agents]\na1 = \"python -c '''print(a.b)'''\"\na2 = 'say \"\"\" x.y'\na3 = \"\"\"\na4 = \"Read a.rs\"\n",
        );
        assert_eq!(quotes.agents.keys().collect::<Vec<_>>(), ["a1", "a2", "a4"]);
        assert_eq!(quotes.doing("a1"), Some("python -c '''print(a.b)'''"));
        // As deep as a line may go is read, and is no agent.
        let most = format!("[agents]\na1 = \"one\"\n{} = 1\n", vec!["a"; MAX_NESTING + 1].join("."));
        let read = std::thread::Builder::new().spawn(move || parse(&most, NOW).unwrap().unwrap());
        assert_eq!(read.unwrap().join().unwrap().agents.len(), 1);
    }

    #[test]
    fn a_report_says_what_the_agents_of_its_program_are_doing() {
        let said = said(
            "model = \"opus\"\n\n[agents]\na89b5d4516757b675 = \"Show current time\"\n\"aad878bc0972d0d46\" = \"Read sidebar.rs\"\n",
        );
        assert_eq!(said.doing("a89b5d4516757b675"), Some("Show current time"));
        assert_eq!(said.doing("aad878bc0972d0d46"), Some("Read sidebar.rs"));
        assert_eq!(said.doing("other"), None);
        // As a line of shell writes it.
        let shell = super::parse(
            "model = \"probe\"\n[agents]\na1 = \"Show current time in HH:MM:SS\"\na2 = \"Read sidebar.rs\"\n",
            NOW,
        );
        assert_eq!(shell.unwrap().unwrap().doing("a1"), Some("Show current time in HH:MM:SS"));
        // The bar is none the wiser for it.
        assert_eq!((said.runs().as_deref(), said.gauges().len()), (Some("opus"), 0));
        // A report of nothing else is one.
        let alone = parse("[agents]\na1 = \"Grep\"\n", NOW).unwrap().unwrap();
        assert_eq!(alone.doing("a1"), Some("Grep"));
        // What is no id of a record or no text that can be shown is left
        // out, and the rest is taken.
        let odd = super::parse(
            "[agents]\n\"a b\" = \"space\"\n\"\" = \"none\"\na2 = 7\na3 = \"\"\na4 = \" Re\\u202Ead\\u200B \"\n\"a/x\" = \"below\"\n",
            NOW,
        );
        let odd = odd.unwrap().unwrap();
        assert_eq!(odd.agents.keys().collect::<Vec<_>>(), ["a/x", "a4"]);
        assert_eq!(odd.doing("a4"), Some("Read"));
        // No more than a program reports to its terminal, and each no
        // longer than a name.
        let many: String = (0..40).map(|n| format!("a{n:03} = \"{}\"\n", "x".repeat(60))).collect();
        let many = parse(&format!("[agents]\n{many}"), NOW);
        let many = many.unwrap().unwrap();
        assert_eq!(many.agents.len(), MAX_AGENTS);
        assert!(many.agents.values().all(|doing| doing.chars().count() == MAX_NAME));
        // Something else under that key is nothing, and so are agents
        // written another way than a line each behind their own line.
        assert_eq!(parse("model = \"m\"\nagents = 3\n", NOW).unwrap().unwrap().agents.len(), 0);
        assert_eq!(
            super::parse("model = \"m\"\nagents = { a1 = \"one\" }\n", NOW).unwrap().unwrap().agents.len(),
            0
        );
        // The line they stand behind, as TOML lets a table begin: with
        // room around its name and a comment behind it. Not a list of
        // tables, and not words that only hold it.
        for line in
            ["[agents]", "  [ agents ]  ", "[agents]   # what they are doing, by their ids", "[agents]#x"]
        {
            let read = super::parse(&format!("model = \"m\"\n{line}\na1 = \"Read a.b.c\"\n"), NOW);
            assert_eq!(read.unwrap().unwrap().doing("a1"), Some("Read a.b.c"), "{line}");
        }
        for line in ["[[agents]]", "[agents.x]", "# [agents]", "[agents] x", "[\"agents\"]"] {
            let read = super::parse(&format!("model = \"m\"\n{line}\na1 = \"one\"\n"), NOW);
            // No agents are read: of a report without them, or of what
            // is no report at all.
            assert!(!read.is_ok_and(|read| read.is_some_and(|read| !read.agents.is_empty())), "{line}");
        }
        // A line that looks like theirs in a text of several lines is
        // part of that text: the report is read, and has no agents.
        for quotes in ["'''", "\"\"\""] {
            let within = format!("model = \"m\"\nnote = {quotes}\n[agents]\na1 = \"one\"\n{quotes}\n");
            let read = super::parse(&within, NOW).unwrap().unwrap();
            assert_eq!((read.model.as_deref(), read.agents.len()), (Some("m"), 0), "{quotes}");
        }
        // A carriage return alone ends a comment for the parser: a line
        // of theirs with one in it is not their line, and no agent
        // behind it goes missing unseen.
        let turned = super::parse("model = \"m\"\n[agents] # x\ra1 = \"one\"\na2 = \"two\"\n", NOW);
        assert!(!turned.is_ok_and(|read| read.is_some_and(|read| read.doing("a2").is_some())));
        // Lines that say nothing do not count against the lines that
        // are looked at: as many agents as may be are read behind them.
        let spaced: String =
            (0..MAX_AGENTS).map(|n| format!("\n# agent {n}\n\n\n\na{n:02} = \"x\"\n")).collect();
        assert_eq!(
            super::parse(&format!("[agents]\n{spaced}"), NOW).unwrap().unwrap().agents.len(),
            MAX_AGENTS
        );
        // But no more lines are handed to the parser than that.
        let junk: String = (0..AGENT_LINES).map(|n| format!("j{n} = 1\n")).collect();
        assert_eq!(
            super::parse(&format!("model = \"m\"\n[agents]\n{junk}a1 = \"late\"\n"), NOW)
                .unwrap()
                .unwrap()
                .agents
                .len(),
            0
        );
        // They stand last: the next table ends them, and is not read.
        let last = super::parse(
            "[agents]\r\n a1 = \"one\" # the first\r\n\n# two more\na.b = \"no id\"\na2 = 'two'\na3 = \"x\"\na3 = \"y\"\n[limits]\n5h = 3\na4 = \"late\"\n",
            NOW,
        );
        let last = last.unwrap().unwrap();
        assert_eq!(
            last.agents.iter().map(|(id, doing)| (id.as_str(), doing.as_str())).collect::<Vec<_>>(),
            [("a1", "one"), ("a2", "two"), ("a3", "y")]
        );
        assert!(last.limits.is_empty());
    }

    #[test]
    fn what_is_shown_is_one_short_line() {
        let long = format!("model = \"a\\nb\\u001b[31m{}\"\n", "x".repeat(500));
        let model = said(&long).model.unwrap();
        assert!(model.starts_with("ab[31mx") && model.chars().count() == 48);
        // Nothing that is not seen: not what turns the writing around, not
        // what has no width, and no room before a name that pushes it out.
        let hidden = format!("model = \"{}o\\u202Ep\\u200Bu\\u2028s\\uFEFF\"\n", " ".repeat(60));
        assert_eq!(said(&hidden).model.as_deref(), Some("opus"));
        assert_eq!(parse("model = \"\\u200B\\u202E \"\n", NOW).unwrap(), None);
        // The names of limits are names too.
        let limits = said("[limits]\n\"a\\u202Eb\\n\" = 1\n\"\\u0007\" = 2\n").limits;
        assert_eq!(limits.keys().collect::<Vec<_>>(), ["ab"]);
        let many: String = (0..9).map(|n| format!("l{n} = {n}\n")).collect();
        assert_eq!(said(&format!("[limits]\n{many}")).limits.len(), 4);
    }

    #[test]
    fn a_name_of_fillers_and_marks_that_draw_nothing_is_none() {
        // Characters that are letters or marks by their kind and draw
        // nothing: fillers, the empty pattern of Braille, what chooses
        // the form of the character before it, tags. A name of them alone
        // would be a row with nothing in it.
        let unseen = [
            '\u{34f}',
            '\u{115f}',
            '\u{1160}',
            '\u{17b4}',
            '\u{17b5}',
            '\u{180b}',
            '\u{180e}',
            '\u{2800}',
            '\u{3164}',
            '\u{fe00}',
            '\u{fe0f}',
            '\u{ffa0}',
            '\u{fff9}',
            '\u{1d173}',
            '\u{e0001}',
            '\u{e0041}',
            '\u{e0100}',
        ];
        for c in unseen {
            assert_eq!(name(&c.to_string()), None, "U+{:04X}", c as u32);
            assert_eq!(name(&format!("o{c}pus")).as_deref(), Some("opus"), "U+{:04X}", c as u32);
        }
        // What is seen stays, also beside them: letters of every script.
        assert_eq!(name("한글 ㄱ ⠁ ॐ").as_deref(), Some("한글 ㄱ ⠁ ॐ"));
    }

    #[test]
    fn token_counts_are_short() {
        let counts = [0, 999, 1000, 124_499, 999_499, 999_500, 1_000_000, 1_250_000, 20_000_000, 999_000_000];
        let shown: Vec<String> = counts.into_iter().map(count).collect();
        assert_eq!(shown, ["0", "999", "1k", "124k", "999k", "1M", "1M", "1.2M", "20M", "999M"]);
        // From a hundred million on there is no room for a tenth.
        let large: Vec<String> = [99_949_999, 99_950_000, 123_456_789].into_iter().map(count).collect();
        assert_eq!(large, ["99.9M", "100M", "123M"]);
    }

    #[test]
    fn a_figure_and_its_ring_say_the_same() {
        let of = |used: &str| {
            let gauges = said(&format!("[limits]\na = {used}\n")).gauges();
            (gauges[0].figure(), gauges[0].whole())
        };
        // Half a percent is none in both, and a half goes to the even
        // number in both: a ring must not show what its figure denies.
        assert_eq!(of("0.5"), ("0%".into(), 0.0));
        assert_eq!(of("12.5"), ("12%".into(), 12.0));
        assert_eq!(of("1.5"), ("2%".into(), 2.0));
        // Nothing used is not less than nothing.
        assert_eq!(of("-0.0"), ("0%".into(), 0.0));
    }

    #[test]
    fn limits_whose_names_read_alike_take_no_place_from_others() {
        // Four limits are shown. Names that differ only in what is not
        // seen are one name, and count as one of the four.
        let report =
            "[limits]\n\"5h\" = 1\n\"5h\\u200B\" = 2\n\"5h\\u200C\" = 3\n\"5h\\u200D\" = 4\n\"7d\" = 5\n";
        assert_eq!(said(report).limits.keys().collect::<Vec<_>>(), ["5h", "7d"]);
    }

    #[test]
    fn files_are_bounded_and_need_not_exist() {
        let dir = std::env::temp_dir().join(format!("codn-reported-{}", std::process::id()));
        let path = dir.join("status").join("1").join("1-1.toml");
        prepare(&path);
        assert_eq!(read(&path).unwrap(), None);
        // Only the user reads what a program reports.
        let mode =
            |path: &Path| std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(path).unwrap()) & 0o777;
        assert_eq!((mode(path.parent().unwrap()), mode(&dir.join("status"))), (0o700, 0o700));
        std::fs::write(&path, "tokens = 5\n").unwrap();
        assert_eq!(read(&path).unwrap().unwrap().tokens, Some(5));
        // An earlier program's report does not outlast the start of the next.
        prepare(&path);
        assert_eq!(read(&path).unwrap(), None);
        // As much as a report may hold is read, a byte more is not one:
        // 16 KiB, whatever the constant says.
        let filled = |bytes: usize| format!("tokens = 5\n#{}\n", "x".repeat(bytes - 13));
        std::fs::write(&path, filled(16 * 1024)).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().tokens, Some(5));
        std::fs::write(&path, filled(16 * 1024 + 1)).unwrap();
        assert_eq!(read(&path).unwrap(), None);
        // What is not a file is not read: a folder, what never ends, and a
        // pipe, which would read as a file that is being written.
        assert_eq!(read(&dir).unwrap(), None);
        std::os::unix::fs::symlink("/dev/zero", dir.join("endless")).unwrap();
        assert_eq!(read(&dir.join("endless")).unwrap(), None);
        let pipe = std::ffi::CString::new(dir.join("pipe").to_str().unwrap()).unwrap();
        // SAFETY: the path is a string that ends in a zero and lives through the call.
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
        assert_eq!(read(&dir.join("pipe")).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_earlier_runs_left_is_swept_and_links_are_not_followed() {
        let dir = std::env::temp_dir().join(format!("codn-sweep-{}", std::process::id()));
        let (all, kept) = (dir.join("status"), dir.join("kept"));
        let own = all.join("7");
        for folder in [&own, &all.join("3"), &kept] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("1-1.toml"), "tokens = 5\n").unwrap();
        }
        std::fs::write(all.join("2.toml"), "tokens = 5\n").unwrap();
        std::os::unix::fs::symlink(&kept, all.join("link")).unwrap();
        sweep_but(&own);
        let left: Vec<_> = std::fs::read_dir(&all).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(left, ["7"]);
        assert!(own.join("1-1.toml").exists() && kept.join("1-1.toml").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nothing_is_swept_where_the_folder_of_the_reports_is_a_link() {
        // Whoever keeps the reports off the disk makes that folder a link
        // to another, which may hold more than reports: all of it would go
        // at every start.
        let dir = std::env::temp_dir().join(format!("codn-sweep-link-{}", std::process::id()));
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("work")).unwrap();
        std::fs::write(elsewhere.join("work/text"), "kept").unwrap();
        std::fs::write(elsewhere.join("notes"), "kept").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("status")).unwrap();
        sweep_but(&dir.join("status/7"));
        assert!(elsewhere.join("work/text").exists() && elsewhere.join("notes").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
