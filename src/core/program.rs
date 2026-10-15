//! What a program says of its own state: the reports of OSC 7501, the
//! program status protocol in its revision 0.3,
//! https://www.superlogical.com/rex/docs/build/program-status.
//!
//! A report is `key=value` pairs with `:` between them. It is the whole of
//! one record, the one its `id` names, and a terminal keeps the records.
//! Of all of them a session shows one state, see `Records::state`.
//!
//! What a program sends is not believed: every length is held against a
//! limit before anything is kept, and a report that breaks one is dropped
//! whole. Of a report the state is kept, and its title as a name that can
//! be shown: a record with an id is a row below its session, see
//! `Records::subs`. Its message is checked, because a report with a bad
//! one does not count, and shown nowhere.

use super::reported::name;

/// The longest report, and the longest key in one. A whole sequence may
/// have 4096 bytes, and what begins and ends it is eight of them at the
/// least. The emulator, which knows how a sequence ended, holds each
/// against its own length before it passes one on.
const REPORT: usize = 4096 - 8;
const KEY: usize = 16;
/// The longest name of the program that reports.
const APP: usize = 32;
/// The longest id, the longest of its segments and how many it may have.
const ID: usize = 128;
const SEGMENT: usize = 32;
const DEPTH: usize = 8;
/// The longest title and message, as they come and as the text they are.
const TITLE: (usize, usize) = (256, 192);
const MESSAGE: (usize, usize) = (2732, 2048);
/// How many records of a terminal are kept. The protocol asks for 64 at
/// the least.
const RECORDS: usize = 64;

/// The state a program says it is in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// At rest.
    Idle,
    Working,
    /// Finished, with a result nobody has looked at.
    Done,
    /// Failed, and stopped.
    Error,
    /// It waits for the user, and for what, where it said so.
    Blocked(Option<Kind>),
}

/// What a program that is blocked waits for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

/// What a report asks of the records.
#[derive(Debug, PartialEq)]
enum Report {
    /// The record of this id is in this state, under this title if it has
    /// one. The id of the record that has none is empty.
    Set(String, State, Option<String>),
    /// The record of this id goes, with those below it. Without an id
    /// all of them go.
    Clear(Option<String>),
}

/// Whether a byte may stand in a name: of the program, or in a segment of
/// an id.
fn named(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte)
}

/// Whether a value is the id of a record.
pub(super) fn is_id(id: &[u8]) -> bool {
    let fits = |segment: &[u8]| (1..=SEGMENT).contains(&segment.len()) && segment.iter().all(|&b| named(b));
    id.len() <= ID && id.split(|&b| b == b'/').count() <= DEPTH && id.split(|&b| b == b'/').all(fits)
}

/// The text a value is in base64, with or without its padding: of
/// `limits.0` bytes at the most as it came and `limits.1` as text, and
/// without a character that controls a terminal. None for a value that is
/// no such text.
fn text(value: &[u8], limits: (usize, usize)) -> Option<String> {
    let end = value.iter().position(|&b| b == b'=').unwrap_or(value.len());
    let (digits, padding) = value.split_at(end);
    let padded = padding.is_empty() || (padding.len() <= 2 && value.len().is_multiple_of(4));
    if value.len() > limits.0 || !padded || padding.iter().any(|&b| b != b'=') || digits.len() % 4 == 1 {
        return None;
    }
    let mut bytes = Vec::with_capacity(digits.len() / 4 * 3 + 2);
    let (mut bits, mut held) = (0u32, 0);
    for &digit in digits {
        let six = match digit {
            b'A'..=b'Z' => digit - b'A',
            b'a'..=b'z' => digit - b'a' + 26,
            b'0'..=b'9' => digit - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        (bits, held) = (bits << 6 | six as u32, held + 6);
        if held >= 8 {
            held -= 8;
            bytes.push((bits >> held) as u8);
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    (text.len() <= limits.1 && !text.chars().any(char::is_control)).then_some(text)
}

/// Reads a report: what stood behind `OSC 7501 ;`. None for one that says
/// nothing that counts.
fn parse(report: &[u8]) -> Option<Report> {
    if report.len() > REPORT {
        return None;
    }
    // Of a key that stands twice the last one counts, so nothing is
    // judged before the end.
    let [mut state, mut id, mut kind, mut app, mut title, mut message] = [None; 6];
    for pair in report.split(|&b| b == b':') {
        let Some(at) = pair.iter().position(|&b| b == b'=') else { continue };
        let (key, value) = (pair[..at].trim_ascii(), pair[at + 1..].trim_ascii());
        // A key that is too long breaks a limit, and the report with it.
        if key.len() > KEY {
            return None;
        }
        // A pair that is not made as pairs are is passed over, and so is
        // a key nobody knows. Not an id: a report whose id is none says
        // nothing, and passed over it would be said of the program
        // itself. Its bytes are judged below, with the rest of it.
        let valued = |&b: &u8| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b);
        if key != b"id" && !value.iter().all(valued) {
            continue;
        }
        match key {
            b"state" => state = Some(value),
            b"id" => id = Some(value),
            b"kind" => kind = Some(value),
            b"app" => app = Some(value),
            b"title" => title = Some(value),
            b"msg" => message = Some(value),
            _ => {}
        }
    }
    // A name of the program with other characters than a name has was
    // not said, which changes nothing here. One that is too long breaks
    // a limit.
    let title = title.map(|title| text(title, TITLE));
    let sound = id.is_none_or(is_id)
        && app.is_none_or(|app| app.len() <= APP)
        && title.as_ref().is_none_or(Option::is_some)
        && message.is_none_or(|message| text(message, MESSAGE).is_some());
    if !sound {
        return None;
    }
    // A program's words, shown in a row: one line of what can be seen.
    let title = title.flatten().and_then(|title| name(&title));
    // An id was held against `named`, which lets only ASCII through.
    let id = id.map(|id| String::from_utf8_lossy(id).into_owned());
    let state = match state? {
        b"idle" => State::Idle,
        b"working" => State::Working,
        b"done" => State::Done,
        b"error" => State::Error,
        b"blocked" => State::Blocked(match kind {
            Some(b"permission") => Some(Kind::Permission),
            Some(b"question") => Some(Kind::Question),
            Some(b"auth") => Some(Kind::Auth),
            _ => None,
        }),
        b"clear" => return Some(Report::Clear(id)),
        _ => return None,
    };
    Some(Report::Set(id.unwrap_or_default(), state, title))
}

/// One record: what a program said last under one id.
struct Record {
    /// Empty for the record that has none: the program itself.
    id: String,
    state: State,
    title: Option<String>,
    /// The place of the record among all a terminal ever had, by when it
    /// was first said. A report that replaces it leaves it where it is.
    since: u64,
}

/// A record that has an id, as it is shown: something the program runs
/// beside itself, an agent it sent off.
#[derive(Clone, Debug, PartialEq)]
pub struct Sub {
    /// The id of the record: what the program knows it by.
    pub id: String,
    pub state: State,
    /// Its title, and for one without a title its id.
    pub title: String,
    /// How many of the records above it there are.
    pub depth: usize,
}

/// What a report changed beside the one state that is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct News {
    /// It says that something is done: that is news each time, also where
    /// the one state shown stays what it was.
    pub done: bool,
    /// The records that have an id are others now, or say something else.
    pub subs: bool,
}

/// The records of one terminal, the one that was written last at the end.
#[derive(Default)]
pub struct Records {
    all: Vec<Record>,
    /// How many records there ever were.
    said: u64,
}

impl Records {
    /// Takes in a report: what stood behind `OSC 7501 ;`. Returns what of
    /// it is news by itself.
    pub fn report(&mut self, report: &[u8]) -> News {
        match parse(report) {
            Some(Report::Set(id, state, title)) => {
                let known = self.all.iter().position(|record| record.id == id);
                let before = known.map(|at| self.all.remove(at));
                let same =
                    before.as_ref().is_some_and(|record| record.state == state && record.title == title);
                // No room: the one nothing was said of for longest goes.
                let gone = (self.all.len() == RECORDS).then(|| self.all.remove(0));
                let since = before.map_or(self.said, |record| record.since);
                self.said += 1;
                let subs = gone.is_some_and(|record| !record.id.is_empty()) || (!id.is_empty() && !same);
                self.all.push(Record { id, state, title, since });
                News { done: state == State::Done, subs }
            }
            Some(Report::Clear(None)) => News { done: false, subs: self.clear() },
            Some(Report::Clear(Some(id))) => {
                let below =
                    |known: &str| known.strip_prefix(id.as_str()).is_some_and(|rest| rest.starts_with('/'));
                // An id is never empty: all that goes here was shown.
                let before = self.all.len();
                self.all.retain(|record| record.id != id && !below(&record.id));
                News { done: false, subs: self.all.len() != before }
            }
            None => News::default(),
        }
    }

    /// The terminal was put back as it was built. Returns whether a
    /// record that has an id went with it.
    pub fn clear(&mut self) -> bool {
        let shown = self.all.iter().any(|record| !record.id.is_empty());
        self.all.clear();
        shown
    }

    /// A shell asks for the next command: whatever ran is gone, and what
    /// it said it was doing, it does no longer. What it finished or failed
    /// at stays. Returns whether a record that has an id went.
    pub fn expire(&mut self) -> bool {
        let shown = |records: &[Record]| records.iter().filter(|record| !record.id.is_empty()).count();
        let before = shown(&self.all);
        self.all.retain(|record| matches!(record.state, State::Done | State::Error));
        shown(&self.all) != before
    }

    /// The records that have an id, in the order they were first said,
    /// each one behind the record it lies below.
    pub fn subs(&self) -> Vec<Sub> {
        let since = |id: &str| self.all.iter().find(|record| record.id == id).map(|record| record.since);
        // Where a record stands: behind the ones above it that there are,
        // by when each of them was first said.
        let place = |record: &Record| -> Vec<u64> {
            let above = record.id.match_indices('/').filter_map(|(at, _)| since(&record.id[..at]));
            above.chain([record.since]).collect()
        };
        let mut shown: Vec<_> = self
            .all
            .iter()
            .filter(|record| !record.id.is_empty())
            .map(|record| (place(record), record))
            .collect();
        shown.sort_by(|a, b| a.0.cmp(&b.0));
        shown
            .into_iter()
            .map(|(place, record)| Sub {
                id: record.id.clone(),
                state: record.state,
                title: record.title.clone().unwrap_or_else(|| record.id.clone()),
                depth: place.len() - 1,
            })
            .collect()
    }

    /// The one state that is shown of all records: what waits for the user
    /// before what failed, that before what is worked at, that before what
    /// is done, and rest last. Of several alike the one said last. None
    /// while the program has said nothing that holds.
    pub fn state(&self) -> Option<State> {
        let rank = |state: &State| match state {
            State::Idle => 0,
            State::Done => 1,
            State::Working => 2,
            State::Error => 3,
            State::Blocked(_) => 4,
        };
        self.all.iter().map(|record| record.state).max_by_key(rank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(id: &str, state: State) -> Option<Report> {
        Some(Report::Set(id.into(), state, None))
    }

    /// Text as a report carries it.
    fn base64(text: &str) -> String {
        const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for three in text.as_bytes().chunks(3) {
            let bits = three.iter().fold(0u32, |bits, &b| bits << 8 | b as u32) << (8 * (3 - three.len()));
            for n in 0..=three.len() {
                out.push(DIGITS[(bits >> (18 - 6 * n) & 63) as usize] as char);
            }
        }
        out
    }

    fn titled(id: &str, state: &str, title: &str) -> Vec<u8> {
        format!("state={state}:id={id}:title={}", base64(title)).into_bytes()
    }

    fn sub(id: &str, state: State, title: &str, depth: usize) -> Sub {
        Sub { id: id.into(), state, title: title.into(), depth }
    }

    #[test]
    fn a_report_is_read_as_the_protocol_says() {
        assert_eq!(parse(b"state=working"), set("", State::Working));
        assert_eq!(parse(b"state=idle:app=claude-code:id=a/b.c"), set("a/b.c", State::Idle));
        assert_eq!(parse(b"state=done"), set("", State::Done));
        assert_eq!(parse(b"state=error"), set("", State::Error));
        // What it waits for counts only where it waits, and only where
        // the word is known.
        assert_eq!(parse(b"state=blocked:kind=question"), set("", State::Blocked(Some(Kind::Question))));
        assert_eq!(parse(b"state=blocked:kind=auth"), set("", State::Blocked(Some(Kind::Auth))));
        assert_eq!(parse(b"state=blocked:kind=permission"), set("", State::Blocked(Some(Kind::Permission))));
        assert_eq!(parse(b"state=blocked:kind=mood"), set("", State::Blocked(None)));
        assert_eq!(parse(b"state=working:kind=question"), set("", State::Working));
        // Room around keys and values is not part of them, the last of
        // two counts, and what is not a pair or not known is passed over.
        assert_eq!(parse(b" state = idle : state = done "), set("", State::Done));
        assert_eq!(parse(b"state=done:later:colour=red:=x:progress=42:state"), set("", State::Done));
        assert_eq!(parse(b"state=done:state=d\xc3\xb6ne"), set("", State::Done), "no value, no pair");
        assert_eq!(parse(b"state=clear"), Some(Report::Clear(None)));
        assert_eq!(parse(b"state=clear:id=a"), Some(Report::Clear(Some("a".into()))));
        // Without a state, or with one nobody knows, it says nothing.
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"app=a:id=b"), None);
        assert_eq!(parse(b"state=thinking"), None);
        assert_eq!(parse(b"state=Idle"), None);
        assert_eq!(parse(b"STATE=idle"), None, "a key is in small letters");
    }

    #[test]
    fn a_report_whose_id_is_none_is_not_taken_for_the_program_itself() {
        // A pair whose value has a byte no value has is passed over. Were
        // an id passed over so, what a program said of one of its agents
        // would be said of the program: its state, and with `clear` every
        // record of the terminal would go.
        assert_eq!(parse(b"state=done:id=agent 1"), None);
        assert_eq!(parse(b"state=error:id=a;b"), None);
        assert_eq!(parse(b"state=clear:id=a\xc3\xa9"), None);
        assert_eq!(parse(b"state=done:id="), None);
        // Of two the last counts, as of every key, whichever is the bad one.
        assert_eq!(parse(b"state=done:id=a b:id=c"), set("c", State::Done));
        assert_eq!(parse(b"state=done:id=c:id=a b"), None);
    }

    #[test]
    fn a_report_that_breaks_a_limit_is_dropped_whole() {
        let with = |pair: String| parse(format!("state=idle:{pair}").as_bytes());
        // An id: segments of one to 32 characters, eight at the most.
        assert!(with(format!("id={}", "a".repeat(32))).is_some());
        assert!(with(format!("id={}", "a".repeat(33))).is_none());
        assert!(with("id=a/b/c/d/e/f/g/h".into()).is_some());
        assert!(with("id=a/b/c/d/e/f/g/h/i".into()).is_none());
        for bad in ["id=", "id=/a", "id=a/", "id=a//b", "id=a,b", "id=a=b"] {
            assert!(with(bad.into()).is_none(), "{bad}");
        }
        let long = vec!["a".repeat(31); 4].join("/");
        assert_eq!(long.len(), 127);
        assert!(with(format!("id={long}")).is_some() && with(format!("id={long}/b")).is_none());
        // The name of the program: too long breaks a limit, one of other
        // characters or of none is a name that was not said.
        assert!(with(format!("app={}", "a".repeat(32))).is_some());
        assert!(with(format!("app={}", "a".repeat(33))).is_none());
        assert!(with("app=".into()).is_some() && with("app=a/b".into()).is_some());
        assert!(with("app=a,b".into()).is_some());
        // How far it has come is said or not, and never ends a report.
        assert!(with("progress=101".into()).is_some() && with("progress=half".into()).is_some());
        // A key has a longest length, and one over it breaks a limit.
        assert!(with(format!("{}=x", "k".repeat(16))).is_some());
        assert!(with(format!("{}=x", "k".repeat(17))).is_none());
        assert!(with(format!(" {} =x", "k".repeat(16))).is_some(), "the room around it is no part of it");
        // The whole of it: a sequence of 4096 bytes with what begins and
        // ends it, eight bytes at the least.
        let filler = |len: usize| format!("x={}", "a".repeat(len - "state=idle:x=".len()));
        assert_eq!(REPORT, 4088);
        assert!(with(filler(REPORT)).is_some() && with(filler(REPORT + 1)).is_none());
    }

    #[test]
    fn a_title_and_a_message_are_text_in_base64_or_the_report_does_not_count() {
        let titled = |value: &str| parse(format!("state=idle:title={value}").as_bytes()).is_some();
        // "hi", with its padding and without.
        assert!(titled("aGk=") && titled("aGk") && titled(""));
        // "h\u{f6}", and the half of it, which is no text.
        assert!(titled("aMO2") && !titled("aMM="));
        // Not base64: a digit that is none, a length there is none of,
        // padding in the middle or too much of it.
        for bad in ["a-k=", "aGk_", "a", "aGkaa", "a=k=", "aGk==", "aG===", "===="] {
            assert!(!titled(bad), "{bad}");
        }
        // A line feed, an escape and a control character of the upper
        // half, each in a text.
        assert!(!titled("YQpi") && !titled("YRti") && !titled("YcKbYg=="));
        // 192 bytes of text are 256 as they come, and the most there is.
        let most = "YWFh".repeat(64);
        assert!(titled(&most) && !titled(&format!("{most}YQ==")));
        // A message may be longer: 2048 bytes, 2732 with padding.
        let message = |value: &str| parse(format!("state=idle:msg={value}").as_bytes()).is_some();
        let most = format!("{}YWE=", "YWFh".repeat(682));
        assert_eq!(most.len(), MESSAGE.0);
        assert!(message(&most) && !message(&format!("{}YWFh", "YWFh".repeat(682))));
    }

    #[test]
    fn a_report_is_the_whole_of_its_record_and_clearing_takes_what_lies_below() {
        let mut records = Records::default();
        assert_eq!(records.state(), None);
        records.report(b"state=working");
        records.report(b"state=idle:id=agent");
        records.report(b"state=blocked:kind=question:id=agent/one");
        records.report(b"state=done:id=agents");
        assert_eq!(records.state(), Some(State::Blocked(Some(Kind::Question))));
        // What a report leaves out is gone from its record.
        records.report(b"state=blocked:id=agent/one");
        assert_eq!(records.state(), Some(State::Blocked(None)));
        // A report that does not count changes nothing. One with a pair
        // that is none counts by its other pairs.
        records.report(b"state=idle:id=agent/one:title=a");
        assert_eq!(records.state(), Some(State::Blocked(None)));
        records.report(b"state=blocked:id=agent/one:title=!");
        assert_eq!(records.state(), Some(State::Blocked(None)));
        // The record and what lies below it go, not one that begins alike.
        records.report(b"state=clear:id=agent");
        assert_eq!(records.all.iter().map(|record| record.id.as_str()).collect::<Vec<_>>(), ["", "agents"]);
        assert_eq!(records.state(), Some(State::Working));
        records.report(b"state=clear");
        assert_eq!(records.state(), None);
    }

    #[test]
    fn a_report_says_whether_something_is_done_again() {
        let mut records = Records::default();
        // What is done is news each time it is said, also while the one
        // state shown stays what it was: it is another result.
        assert!(records.report(b"state=done").done);
        assert!(records.report(b"state=done").done);
        assert!(records.report(b"state=done:id=sub").done);
        // Nothing else is, by itself: whether the state shown is another
        // is seen by looking at it.
        for other in ["state=idle", "state=working", "state=blocked", "state=error", "state=clear"] {
            assert!(!records.report(other.as_bytes()).done, "{other}");
        }
        assert!(!records.report(b"state=done:title=a").done, "a report that does not count");
    }

    #[test]
    fn one_state_is_shown_of_all_and_of_several_alike_the_last() {
        let mut records = Records::default();
        let mut said = |report: &str| {
            records.report(report.as_bytes());
            records.state()
        };
        assert_eq!(said("state=idle:id=a"), Some(State::Idle));
        assert_eq!(said("state=done:id=b"), Some(State::Done));
        assert_eq!(said("state=working:id=c"), Some(State::Working));
        assert_eq!(said("state=error:id=d"), Some(State::Error));
        assert_eq!(said("state=blocked:kind=auth:id=e"), Some(State::Blocked(Some(Kind::Auth))));
        assert_eq!(said("state=blocked:kind=question:id=f"), Some(State::Blocked(Some(Kind::Question))));
        assert_eq!(said("state=blocked:kind=auth:id=e"), Some(State::Blocked(Some(Kind::Auth))));
        assert_eq!(said("state=clear:id=e"), Some(State::Blocked(Some(Kind::Question))));
        assert_eq!(said("state=clear:id=f"), Some(State::Error));
        // A prompt: what was being done is done no longer. What was
        // finished or failed at stays.
        records.expire();
        assert_eq!(records.state(), Some(State::Error));
        assert_eq!(records.all.len(), 2);
        records.clear();
        assert_eq!(records.state(), None);
    }

    #[test]
    fn the_records_of_a_terminal_have_a_limit() {
        let mut records = Records::default();
        records.report(b"state=blocked:id=first");
        for n in 0..RECORDS {
            records.report(format!("state=idle:id=n{n}").as_bytes());
            // Said again, a record takes no second place.
            records.report(format!("state=idle:id=n{n}").as_bytes());
        }
        // The one nothing was said of for longest gave way.
        assert_eq!(records.all.len(), RECORDS);
        assert_eq!(records.state(), Some(State::Idle));
    }

    #[test]
    fn a_title_is_kept_as_a_name_that_can_be_shown() {
        let title = |text: &str| match parse(&titled("a", "working", text)) {
            Some(Report::Set(_, _, title)) => title,
            other => panic!("{other:?}"),
        };
        assert_eq!(title("Explore the sidebar").as_deref(), Some("Explore the sidebar"));
        assert_eq!(title("  h\u{f6}r  ").as_deref(), Some("h\u{f6}r"));
        // What is not seen and turns the writing round is no part of a
        // name, and a title of nothing else is none.
        assert_eq!(title("a\u{202e}b\u{200b}c").as_deref(), Some("abc"));
        assert_eq!((title(""), title(" \u{feff} ")), (None, None));
        // No longer than a name is.
        assert_eq!(title(&"a".repeat(190)).map(|title| title.len()), Some(48));
        // A title that is no text still ends the report it stands in.
        assert_eq!(parse(b"state=working:id=a:title=YQpi"), None);
    }

    #[test]
    fn the_records_that_have_an_id_are_shown_in_the_order_they_were_first_said() {
        let mut records = Records::default();
        records.report(b"state=working");
        assert_eq!(records.subs(), []);
        records.report(&titled("b", "working", "second"));
        records.report(&titled("a", "working", "first by its id"));
        records.report(b"state=blocked:kind=question:id=c");
        let shown = [
            sub("b", State::Working, "second", 0),
            sub("a", State::Working, "first by its id", 0),
            sub("c", State::Blocked(Some(Kind::Question)), "c", 0),
        ];
        assert_eq!(records.subs(), shown);
        // Said again, a record stays where it is, also as the record of
        // the program itself is said again and again.
        records.report(&titled("b", "blocked", "second"));
        records.report(b"state=idle");
        let Sub { state, title, .. } = &records.subs()[0];
        assert_eq!((*state, title.as_str()), (State::Blocked(None), "second"));
        // One that went and comes again is a new one.
        records.report(b"state=clear:id=b");
        records.report(&titled("b", "working", "second"));
        assert_eq!(records.subs().last(), Some(&sub("b", State::Working, "second", 0)));
        // A record stands behind the one it lies below, wherever it was
        // said, and one whose record above is not there stands alone.
        records.report(b"state=working:id=a/x/y");
        records.report(b"state=working:id=d");
        records.report(b"state=working:id=a/x");
        records.report(b"state=working:id=e/x");
        let places: Vec<_> = records.subs().into_iter().map(|sub| (sub.title, sub.depth)).collect();
        let stands =
            [("first by its id", 0), ("a/x", 1), ("a/x/y", 2), ("c", 0), ("second", 0), ("d", 0), ("e/x", 0)];
        assert_eq!(places, stands.map(|(title, depth)| (title.to_owned(), depth)));
    }

    #[test]
    fn a_report_says_whether_the_records_that_are_shown_changed() {
        let mut records = Records::default();
        let mut said = |report: &[u8]| records.report(report).subs;
        // The record of the program itself is no row.
        assert!(!said(b"state=working") && !said(b"state=done") && !said(b"state=clear:id=a"));
        assert!(said(&titled("a", "working", "one")));
        // The same again is no news, and a program says it by the thousand.
        assert!(!said(&titled("a", "working", "one")));
        assert!(said(&titled("a", "blocked", "one")) && said(&titled("a", "blocked", "two")));
        assert!(said(b"state=blocked:id=a"), "the title is gone");
        assert!(
            !said(b"state=blocked:id=a:title=!"),
            "a pair that is none is passed over, and the rest is not new"
        );
        assert!(said(b"state=working:id=a/b") && said(b"state=clear:id=a") && !said(b"state=clear:id=a"));
        assert!(said(b"state=done:id=kept") && said(b"state=working:id=b"));
        // A prompt takes what was being done, and what is put back as it
        // was built everything.
        assert!(records.expire() && !records.expire());
        assert_eq!(records.subs(), [sub("kept", State::Done, "kept", 0)]);
        assert!(!records.report(b"state=clear:id=other").subs);
        assert!(records.report(b"state=clear").subs && !records.report(b"state=clear").subs);
        records.report(b"state=working");
        assert!(!records.clear());
        // A record that gives way for want of room was shown.
        records.report(b"state=idle:id=first");
        for n in 1..RECORDS {
            records.report(format!("state=idle:id=n{n}").as_bytes());
        }
        assert!(records.report(b"state=idle").subs);
        assert!(!records.report(b"state=idle").subs);
    }
}
