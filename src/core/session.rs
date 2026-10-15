//! A session: one program of an agent in a project, with what is known of
//! its conversation and of the state of its process.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::program::{Kind, State, Sub};
use super::{Activity, ProjectId, Reported, SessionId, Status, config, uuid};
use crate::log::log;
use crate::term::Terminal;

/// How long after the last input or output an agent is asked which
/// conversation it is in. It has written that down by then.
const ASK_AFTER: Duration = Duration::from_secs(2);

/// How often in a row that question is asked again when no worker took
/// it: for a minute. Workers that are busy are free again long before,
/// and workers that hang do not keep a window that rests waking for good.
const ASK_AGAIN: u8 = 30;

/// How long after input or output the file is read in which a program
/// reports about itself, and how often at most while there is either.
const READ_AFTER: Duration = Duration::from_secs(1);

/// How long a program has to write its report after it printed. A read
/// that comes later than this has seen it; one that comes sooner is
/// followed by another.
const WRITTEN_AFTER: Duration = Duration::from_millis(500);

/// A process that ends this soon after its start is treated as a failed start.
const FAILED_START: Duration = Duration::from_secs(3);

/// The kinds of news of a session a user who is elsewhere is told of:
/// that it waits for him, that it is done, that it failed. Each is told
/// once until he has looked at the session, however often a program says
/// it: every telling is a notification on the desktop.
const WAITS: u8 = 1;
const DONE: u8 = 2;
const FAILED: u8 = 4;

/// How many of the things a program runs beside itself have a row each.
/// A program can run more of them than a list of projects has room for.
pub const SUBS: usize = 8;

/// What a session is shown as doing: what its program says of itself, or
/// else what the behaviour of its terminal lets guess.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Doing {
    Idle,
    Working,
    /// It stopped while nobody looked. Whether it is done or asks
    /// something is not known.
    Attention,
    /// The program says that it waits for the user.
    Blocked,
    /// The program says that it is done, and nobody has looked since.
    Done,
    /// The program says that it failed.
    Failed,
}

impl Doing {
    /// What a program is shown as doing that says so itself.
    pub fn said(state: State) -> Self {
        match state {
            State::Blocked(_) => Self::Blocked,
            State::Error => Self::Failed,
            State::Working => Self::Working,
            State::Done => Self::Done,
            State::Idle => Self::Idle,
        }
    }
}

pub struct Session {
    pub id: SessionId,
    /// Identifies the current process, including restarts of this row.
    pub generation: u64,
    /// Latest history request. Restarting invalidates pending answers.
    pub follow_request: u64,
    pub starting: bool,
    pub start_gate: Option<StartGate>,
    pub project: ProjectId,
    pub agent: String,
    /// The id the agent resumes the conversation by: handed to it at the
    /// start, or taken from what the agent wrote down.
    pub uuid: String,
    /// The agent has a conversation under `uuid`. Not so for an agent that
    /// chooses its ids itself, until its conversation was found.
    pub known: bool,
    /// What was set for the session before it started, like the model.
    pub chosen: config::Chosen,
    /// Name given by the user, shown instead of the title.
    pub name: Option<String>,
    /// Title set by the program through the terminal.
    pub title: Option<String>,
    /// `None` until the session is first shown. Kept after the process
    /// ended with an error, so its last output stays readable.
    pub term: Option<Terminal>,
    /// Start with the agent's resume arguments instead of fresh.
    pub resume: bool,
    pub spawned: Option<Instant>,
    /// Why the session is not running: it could not start, or its process
    /// ended with an error. The row stays so it can be started again.
    pub ended: Option<String>,
    pub activity: Activity,
    /// The state the program says it is in, as it was heard last. None:
    /// it says nothing, and `activity` is all there is to go by.
    pub program: Option<State>,
    /// What the program says it runs beside itself, the agents it sent
    /// off: each a row below that of the session.
    pub subs: Vec<Sub>,
    /// The session was looked at since its program said it is done.
    looked: bool,
    /// What the user was told of the session since he looked at it last:
    /// `WAITS`, `DONE` and `FAILED`.
    told: u8,
    /// The agent's own files say which conversation it is in, and codn
    /// reads that. Set when the session starts.
    pub followed: bool,
    /// When to read it next. Nothing while the session is quiet.
    pub ask_at: Option<Instant>,
    /// How often in a row no worker took that question.
    ask_refused: u8,
    /// What the program reports about itself, as last read.
    pub reported: Option<Reported>,
    /// The file it reports in, named to it when it was started. Every
    /// start has its own.
    pub report_file: Option<PathBuf>,
    /// Counts the programs of this row that were named a file. An answer
    /// to a read carries it, and one for an earlier program counts no more.
    pub read_request: u64,
    /// When to read the report next. Nothing while the session is quiet.
    pub read_at: Option<Instant>,
    /// A read is out. There is one at a time: an answer that takes long
    /// must not be overtaken by the next read and dropped for it.
    reading: bool,
    /// The report was due again while a read was out.
    read_waits: bool,
    /// The last input or output.
    touch: Option<Instant>,
}

/// What to do with a session whose process ended.
#[derive(Debug, PartialEq)]
pub enum Exit {
    /// Resuming did not work, start it fresh.
    Retry,
    /// It ended with an error: keep the row and the output.
    Keep,
    /// It ended on its own accord: remove it.
    Close,
}

impl Session {
    /// A session that has not been started. `uuid` names its conversation,
    /// `resume` says to continue that one instead of beginning it.
    pub fn new(id: SessionId, project: ProjectId, agent: &str, uuid: String, resume: bool) -> Self {
        Self {
            id,
            generation: 0,
            follow_request: 0,
            starting: false,
            start_gate: None,
            project,
            agent: agent.to_owned(),
            uuid,
            known: false,
            chosen: Default::default(),
            name: None,
            title: None,
            term: None,
            resume,
            spawned: None,
            ended: None,
            activity: Activity::default(),
            program: None,
            subs: Vec::new(),
            looked: false,
            told: 0,
            followed: false,
            ask_at: None,
            ask_refused: 0,
            reported: None,
            report_file: None,
            read_request: 0,
            read_at: None,
            reading: false,
            read_waits: false,
            touch: None,
        }
    }

    /// The process is being started, behind `gate`. Returns the generation
    /// its reports and the answer of the start carry. What is still out for
    /// an earlier one counts no more.
    pub fn begin_start(&mut self, gate: StartGate, followed: bool) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.next_follow();
        self.forget_report();
        self.forget_program();
        self.starting = true;
        self.start_gate = Some(gate);
        self.followed = followed;
        self.generation
    }

    /// Gives up the start that is under way. Its reader is woken and
    /// reports nothing.
    pub fn cancel_start(&mut self) {
        self.start_gate = None;
        self.starting = false;
    }

    /// The process runs. What its reader held back is let through.
    pub fn started(&mut self, term: Terminal) {
        self.starting = false;
        self.term = Some(term);
        // From here: a resume that fails is told by how soon after this
        // the program ended.
        self.spawned = Some(Instant::now());
        if let Some(gate) = self.start_gate.take() {
            gate.ready();
        }
    }

    /// The process could not be started. The row stays and says why.
    pub fn failed(&mut self, why: String) {
        self.cancel_start();
        log(format_args!("session not started: {why}"));
        self.ended = Some(why);
    }

    /// The number of a new question about the conversation. Answers to
    /// the questions before it count no more.
    pub fn next_follow(&mut self) -> u64 {
        self.follow_request = self.follow_request.wrapping_add(1);
        self.follow_request
    }

    /// Lets go of what the program before reported, and of what is still
    /// out for it: the next program has a file of its own.
    fn forget_report(&mut self) {
        self.read_request = self.read_request.wrapping_add(1);
        (self.reported, self.report_file, self.read_at) = (None, None, None);
        (self.reading, self.read_waits) = (false, false);
    }

    /// Whether the program says what it is doing. What it finished says
    /// nothing more once it was looked at: the session is told by its
    /// terminal again, or a shell in which one program once said "done"
    /// would never again be seen to work.
    pub fn reports(&self) -> bool {
        match self.program {
            None => false,
            Some(State::Done) => !self.looked,
            Some(_) => true,
        }
    }

    /// What the session is shown as doing. A program that says so itself
    /// is believed before what its terminal lets guess.
    pub fn doing(&self) -> Doing {
        match (self.program.filter(|_| self.reports()), self.activity.status) {
            (Some(state), _) => Doing::said(state),
            (None, Status::Working) => Doing::Working,
            (None, Status::Attention) => Doing::Attention,
            (None, Status::Idle) => Doing::Idle,
        }
    }

    /// The program says it is in another state, or that something is done
    /// again, or with None nothing any more. `seen` says whether the user
    /// is looking at the session. Returns what to tell a user who is not,
    /// if anything.
    pub fn said(&mut self, state: Option<State>, seen: bool) -> Option<&'static str> {
        (self.program, self.looked) = (state, seen);
        let (kind, news) = match state? {
            State::Blocked(Some(Kind::Permission)) => (WAITS, "Asks for permission"),
            State::Blocked(Some(Kind::Question)) => (WAITS, "Asks a question"),
            State::Blocked(Some(Kind::Auth)) => (WAITS, "Needs a login"),
            State::Blocked(None) => (WAITS, "Waiting for you"),
            State::Done => (DONE, "Done"),
            State::Error => (FAILED, "Failed"),
            State::Idle | State::Working => return None,
        };
        (!seen && self.tell(kind)).then_some(news)
    }

    /// The session stopped while nobody looked, by what its terminal lets
    /// guess. Returns whether to tell the user: not of a session whose
    /// program says what it does, and not of one he was told something
    /// of and has not looked at since. The program says the same a moment
    /// later or said it a moment before, in its own words, and that was
    /// two notifications for one thing.
    pub fn stopped(&mut self) -> bool {
        !self.reports() && self.told == 0 && self.tell(WAITS)
    }

    /// The program ended with an error. Returns whether to tell a user
    /// who is not looking: not one who was told already that it failed,
    /// by the program itself a moment before.
    pub fn crashed(&mut self) -> bool {
        self.tell(FAILED)
    }

    /// Notes that the user is told news of this kind. Returns whether he
    /// was not told it already since he looked.
    fn tell(&mut self, kind: u8) -> bool {
        let new = self.told & kind == 0;
        self.told |= kind;
        new
    }

    /// The user looked at the session. Returns whether that changed what
    /// it is shown as doing.
    pub fn seen(&mut self) -> bool {
        let before = self.doing();
        self.activity.seen();
        (self.looked, self.told) = (true, 0);
        self.doing() != before
    }

    /// The program says it runs other things beside itself now, or says
    /// something else of them. Returns whether that changed what is shown.
    pub fn runs(&mut self, subs: Vec<Sub>) -> bool {
        let changed = self.subs != subs;
        self.subs = subs;
        changed
    }

    /// The rows below that of the session: one for each thing its program
    /// runs beside itself, up to `SUBS`, and how many more there are, which
    /// one more row says.
    pub fn sub_rows(&self) -> (&[Sub], usize) {
        let shown = self.subs.len().min(SUBS);
        (&self.subs[..shown], self.subs.len() - shown)
    }

    /// What a row below that of the session says its agent is doing:
    /// what the program reports of it in its file, by the id of the
    /// record, and else the title the terminal was told.
    pub fn doing_of<'a>(&'a self, sub: &'a Sub) -> &'a str {
        self.reported.as_ref().and_then(|reported| reported.doing(&sub.id)).unwrap_or(&sub.title)
    }

    /// Whether a mark of the session moves: its own, or one of a row
    /// below it, which can work while the session itself waits.
    pub fn moves(&self) -> bool {
        let works = |sub: &Sub| sub.state == State::Working;
        self.ended.is_none() && (self.doing() == Doing::Working || self.sub_rows().0.iter().any(works))
    }

    /// Lets go of what the program said of itself: it is gone, and the
    /// next one in this row has said nothing yet.
    fn forget_program(&mut self) {
        (self.program, self.looked, self.told) = (None, false, 0);
        self.subs.clear();
    }

    /// Something was typed for the program.
    pub fn input(&mut self, now: Instant) {
        self.activity.input(now);
        self.touched(now);
    }

    /// The program printed something. Returns whether that changed the
    /// status of the session.
    pub fn output(&mut self, now: Instant) -> bool {
        let changed = self.activity.output(now);
        self.touched(now);
        changed
    }

    /// Something was typed or printed. Whatever changes the conversation
    /// an agent is in does one of the two, and so does whatever changes
    /// what a program reports about itself.
    pub fn touched(&mut self, now: Instant) {
        if self.followed {
            self.ask_at = Some(now + ASK_AFTER);
            self.ask_refused = 0;
        }
        self.touch = Some(now);
        self.read_at.get_or_insert(now + READ_AFTER);
    }

    /// No worker took the question which conversation the agent is in.
    /// It is asked again, or what the agent went on to would not be known
    /// after a restart, but not without end: after `ASK_AGAIN` times it
    /// waits for the next input or output, and for the window to close.
    pub fn ask_refused(&mut self, now: Instant) {
        if self.followed && self.ask_refused < ASK_AGAIN {
            self.ask_refused += 1;
            self.ask_at = Some(now + ASK_AFTER);
        }
    }

    /// Whether the report is to be read now: `READ_AFTER` after input or
    /// output, and while either goes on once in that time. A program
    /// writes its report when it has printed, so a read that comes right
    /// after the last of it is followed by one more.
    pub fn read_due(&mut self, now: Instant) -> bool {
        if self.read_at.is_none_or(|at| at > now) {
            return false;
        }
        let fresh = self.touch.is_some_and(|at| now.duration_since(at) < WRITTEN_AFTER);
        self.read_at = fresh.then_some(now + READ_AFTER);
        true
    }

    /// The file to read the report from now, and the number its answer is
    /// to carry. Nothing for a program that is not running or was named
    /// no file, and nothing while a read is out: that one is followed by
    /// another when it comes back.
    pub fn begin_read(&mut self) -> Option<(PathBuf, u64)> {
        if self.term.is_none() || self.ended.is_some() {
            return None;
        }
        let file = self.report_file.clone()?;
        if self.reading {
            self.read_waits = true;
            return None;
        }
        self.reading = true;
        Some((file, self.read_request))
    }

    /// No worker was free to read the report. It is read with the next
    /// input or output: asking again without either would run while idle.
    pub fn read_refused(&mut self) {
        (self.reading, self.read_waits) = (false, false);
    }

    /// The answer to a read: what the program reports, nothing if there is
    /// no report, or `None` for a file that could not be read as one. That
    /// leaves what is shown: its program may be in the middle of writing
    /// it. Returns whether what is shown changed.
    pub fn read_done(&mut self, request: u64, found: Option<Option<Reported>>, now: Instant) -> bool {
        // An answer for a program before this one.
        if request != self.read_request {
            return false;
        }
        self.reading = false;
        if std::mem::take(&mut self.read_waits) {
            self.read_at = Some(now);
        }
        if self.term.is_none() || self.ended.is_some() {
            return false;
        }
        match found {
            Some(found) if found != self.reported => {
                self.reported = found;
                true
            }
            _ => false,
        }
    }

    /// Decides what happens after the process ended with `code`.
    pub fn exited(&mut self, code: Option<i32>, now: Instant) -> Exit {
        let code = code.unwrap_or(1);
        if code == 0 {
            return Exit::Close;
        }
        // What was derived from its output ends with it. A program that
        // was at work when it ended went on being at work, until the
        // loop woke for it once more and the user was told that the
        // session waits for him; and the program started in its place
        // began with the work of the one before.
        self.activity = Activity::default();
        let quick = self.spawned.is_some_and(|t| now.duration_since(t) < FAILED_START);
        if quick && self.resume {
            // Nothing to resume, for example a conversation that never got
            // its first message.
            self.resume = false;
            self.term = None;
            self.forget_report();
            self.forget_program();
            return Exit::Retry;
        }
        // An agent that crashed must not take its conversation with it: the
        // row keeps the id it can be resumed with.
        self.ended = Some(format!("The program ended with exit code {code}."));
        // Whoever was told that it failed, by the program itself, is
        // not told so again for its end.
        let told = self.told & FAILED;
        self.forget_program();
        self.told = told;
        Exit::Keep
    }

    /// The row no longer stands for the conversation it was opened with:
    /// it gets an id nothing is known under, and loses that one's title.
    pub fn forget_conversation(&mut self) {
        self.uuid = uuid();
        self.known = false;
        self.title = None;
    }

    /// Clears the way for starting the process again.
    pub fn restart(&mut self) {
        self.cancel_start();
        self.next_follow();
        self.forget_report();
        self.forget_program();
        self.ask_at = None;
        self.ended = None;
        self.term = None;
        self.resume = true;
        self.activity = Activity::default();
    }
}

/// Keeps a PTY reader's initial reports behind installation on the window
/// thread. The session owns cancellation, so removing its row wakes readers.
pub struct StartGate {
    shared: Arc<(Mutex<Option<bool>>, Condvar)>,
    released: bool,
}

impl StartGate {
    pub fn new() -> Self {
        Self { shared: Arc::new((Mutex::new(None), Condvar::new())), released: false }
    }

    pub fn notifier(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let shared = self.shared.clone();
        move || {
            let (state, wake) = &*shared;
            let mut state = state.lock().unwrap();
            while state.is_none() {
                state = wake.wait(state).unwrap();
            }
            *state == Some(true)
        }
    }

    pub fn cancellation(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let shared = self.shared.clone();
        move || *shared.0.lock().unwrap() == Some(false)
    }

    pub fn aborter(&self) -> impl Fn() + Send + Sync + 'static {
        let shared = self.shared.clone();
        move || {
            let (state, wake) = &*shared;
            *state.lock().unwrap() = Some(false);
            wake.notify_all();
        }
    }

    pub fn ready(mut self) {
        let (state, wake) = &*self.shared;
        *state.lock().unwrap() = Some(true);
        self.released = true;
        wake.notify_all();
    }
}

impl Drop for StartGate {
    fn drop(&mut self) {
        if !self.released {
            let (state, wake) = &*self.shared;
            *state.lock().unwrap() = Some(false);
            wake.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::term::{self, Grid, Options};

    fn session(resume: bool, age: Duration) -> (Session, Instant) {
        let now = Instant::now();
        let mut session = Session::new(SessionId(1), ProjectId(1), "agent", uuid(), resume);
        session.spawned = Some(now);
        (session, now + age)
    }

    #[test]
    fn a_program_that_ended_is_not_at_work_any_more() {
        // It printed for a second and a half, which is work, and ended
        // with an error: the status that is derived from its output went
        // on, woke the loop once more and had the user told that the
        // session waits for him.
        let working = |resume: bool| {
            let (mut s, _) = session(resume, Duration::ZERO);
            let begun = s.spawned.unwrap();
            for tenth in 0..=15 {
                s.output(begun + Duration::from_millis(100 * tenth));
            }
            assert!(s.activity.status == Status::Working && s.activity.deadline().is_some());
            (s, begun + Duration::from_millis(1600))
        };
        let (mut kept, now) = working(false);
        assert_eq!(kept.exited(Some(1), now), Exit::Keep);
        assert!(kept.activity.status == Status::Idle && kept.activity.deadline().is_none());
        assert!(!kept.activity.settle(now + Duration::from_secs(60), false));
        // Nor does the program that is started in its place take the
        // work of the one that could not resume.
        let (mut again, now) = working(true);
        assert_eq!(again.exited(Some(1), now), Exit::Retry);
        assert!(again.activity.status == Status::Idle && again.activity.deadline().is_none());
    }

    #[test]
    fn a_program_that_failed_is_told_once_until_it_is_looked_at() {
        let (mut s, _) = session(false, Duration::ZERO);
        assert!(s.crashed() && !s.crashed(), "once");
        s.seen();
        assert!(s.crashed(), "and again after the user looked");
        // One that said so itself, of which the user was told, is not
        // told a second time when it ends for it.
        let (mut s, now) = session(false, Duration::ZERO);
        assert_eq!(s.said(Some(State::Error), false), Some("Failed"));
        assert_eq!(s.exited(Some(1), now), Exit::Keep);
        assert!(!s.crashed());
    }

    #[test]
    fn a_start_that_is_given_up_wakes_who_waits_for_it() {
        // The three ways a gate is answered: ready, given up by the one
        // who holds its way out, and let go without a word.
        let waits = |answer: fn(StartGate)| {
            let gate = StartGate::new();
            let (notified, cancelled) = (gate.notifier(), gate.cancellation());
            let waiting = std::thread::spawn(notified);
            answer(gate);
            (waiting.join().unwrap(), cancelled())
        };
        assert_eq!(waits(StartGate::ready), (true, false));
        assert_eq!(waits(drop), (false, true));
        assert_eq!(
            waits(|gate| {
                let abort = gate.aborter();
                // Given up from another thread, while the gate is still
                // held: whoever waits does not wait for it to be let go.
                std::thread::spawn(abort).join().unwrap();
                std::mem::forget(gate);
            }),
            (false, true)
        );
    }

    #[test]
    fn a_failed_resume_is_retried_fresh_once() {
        let (mut s, now) = session(true, Duration::from_secs(1));
        assert_eq!(s.exited(Some(1), now), Exit::Retry);
        assert!(!s.resume);
        // For an agent that chooses its ids itself, the row lets go of the
        // conversation that did not resume.
        let before = s.uuid.clone();
        (s.known, s.title) = (true, Some("Old".into()));
        s.forget_conversation();
        assert!(s.uuid != before && !s.known && s.title.is_none());
        s.spawned = Some(now);
        assert_eq!(s.exited(Some(1), now), Exit::Keep);
        assert!(s.ended.is_some());
    }

    #[test]
    fn a_program_that_says_what_it_does_is_believed_before_its_terminal() {
        let (mut s, _) = session(false, Duration::ZERO);
        // Nothing said: the terminal is all there is to go by.
        assert_eq!((s.doing(), s.reports()), (Doing::Idle, false));
        s.activity.bell(false);
        assert_eq!(s.doing(), Doing::Attention);
        // Said: that counts, whatever the terminal lets guess.
        assert_eq!(s.said(Some(State::Working), false), None);
        assert_eq!((s.doing(), s.reports()), (Doing::Working, true));
        assert_eq!(s.said(Some(State::Idle), false), None);
        assert_eq!(s.doing(), Doing::Idle);
        // What a user who is elsewhere is told, in codn's own words.
        assert_eq!(s.said(Some(State::Blocked(Some(Kind::Question))), false), Some("Asks a question"));
        assert_eq!(s.doing(), Doing::Blocked);
        // Looking at it does not answer the question.
        assert!(!s.seen());
        assert_eq!(s.doing(), Doing::Blocked);
        assert_eq!(s.said(Some(State::Error), false), Some("Failed"));
        assert_eq!(s.doing(), Doing::Failed);
        for (kind, news) in [
            (Some(Kind::Permission), "Asks for permission"),
            (Some(Kind::Auth), "Needs a login"),
            (None, "Waiting for you"),
        ] {
            s.seen();
            assert_eq!(s.said(Some(State::Blocked(kind)), false), Some(news));
        }
        // It says nothing any more: back to the terminal, whose bell
        // was seen meanwhile.
        assert_eq!(s.said(None, false), None);
        assert_eq!((s.doing(), s.reports()), (Doing::Idle, false));
    }

    #[test]
    fn each_kind_of_news_is_told_once_until_the_session_was_looked_at() {
        let (mut s, _) = session(false, Duration::ZERO);
        assert_eq!(s.said(Some(State::Done), false), Some("Done"));
        // A program that flips its state tells nobody again, for an hour
        // or for a day.
        for _ in 0..1000 {
            assert_eq!(s.said(Some(State::Working), false), None);
            assert_eq!(s.said(Some(State::Done), false), None);
        }
        // What waits for the user is never lost behind what was done a
        // moment ago, nor is what failed. Each of them once.
        assert_eq!(s.said(Some(State::Blocked(Some(Kind::Permission))), false), Some("Asks for permission"));
        assert_eq!(s.said(Some(State::Blocked(Some(Kind::Question))), false), None);
        assert_eq!(s.said(Some(State::Error), false), Some("Failed"));
        assert_eq!(s.said(Some(State::Error), false), None);
        // He looked: the next of each is news again.
        s.seen();
        assert_eq!(s.said(Some(State::Done), false), Some("Done"));
        assert_eq!(s.said(Some(State::Blocked(None)), false), Some("Waiting for you"));
        // While he looks on, nothing is told and nothing waits to be.
        s.seen();
        assert_eq!(s.said(Some(State::Error), true), None);
        assert_eq!(s.said(Some(State::Error), false), Some("Failed"));
    }

    #[test]
    fn a_stop_that_is_guessed_is_told_only_where_nothing_else_was() {
        // A bell, and a moment later the program says what it rang for:
        // one notification, whichever of the two comes first.
        let (mut s, _) = session(false, Duration::ZERO);
        assert!(s.stopped());
        assert!(!s.stopped(), "told once until looked at");
        assert_eq!(s.said(Some(State::Blocked(Some(Kind::Question))), false), None);
        let (mut s, _) = session(false, Duration::ZERO);
        assert_eq!(s.said(Some(State::Blocked(Some(Kind::Question))), false), Some("Asks a question"));
        assert!(!s.stopped(), "its program says what it does");
        // A program says it is done and takes it back: the silence after
        // it is no second piece of news.
        let (mut s, _) = session(false, Duration::ZERO);
        assert_eq!(s.said(Some(State::Done), false), Some("Done"));
        assert_eq!(s.said(None, false), None);
        assert!(!s.stopped());
        s.seen();
        assert!(s.stopped());
    }

    #[test]
    fn what_is_done_shows_until_it_was_looked_at_and_then_the_terminal_tells_again() {
        let (mut s, now) = session(false, Duration::ZERO);
        s.said(Some(State::Done), false);
        assert_eq!((s.doing(), s.reports()), (Doing::Done, true));
        assert!(s.seen());
        assert_eq!((s.doing(), s.reports()), (Doing::Idle, false));
        assert!(!s.seen());
        // A shell in which a program once said "done": the next one that
        // prints is seen to work, and its end is news.
        for n in 0..30 {
            s.activity.output(now + Duration::from_millis(100 * n));
        }
        assert_eq!(s.doing(), Doing::Working);
        assert!(s.activity.settle(now + Duration::from_secs(10), false));
        assert_eq!(s.doing(), Doing::Attention);
        assert!(s.stopped());
        // Done again: to be looked at again.
        assert_eq!(s.said(Some(State::Done), false), Some("Done"));
        assert_eq!(s.doing(), Doing::Done);
        // Done while he looks on: nothing waits to be looked at.
        s.seen();
        assert_eq!(s.said(Some(State::Done), true), None);
        assert_eq!(s.doing(), Doing::Idle);
    }

    fn sub(state: State) -> Sub {
        Sub { id: "a".into(), state, title: "one".into(), depth: 0 }
    }

    #[test]
    fn what_a_program_runs_beside_itself_has_rows_up_to_a_number() {
        let (mut s, _) = session(false, Duration::from_secs(60));
        assert_eq!(s.sub_rows(), (&[][..], 0));
        assert!(s.runs(vec![sub(State::Working)]));
        // The same again changes nothing that is shown.
        assert!(!s.runs(vec![sub(State::Working)]));
        assert!(s.runs(vec![sub(State::Blocked(None))]));
        assert_eq!(s.sub_rows(), (&[sub(State::Blocked(None))][..], 0));
        s.runs(vec![sub(State::Idle); SUBS]);
        assert_eq!((s.sub_rows().0.len(), s.sub_rows().1), (SUBS, 0));
        s.runs(vec![sub(State::Idle); SUBS + 5]);
        assert_eq!((s.sub_rows().0.len(), s.sub_rows().1), (SUBS, 5));
    }

    #[test]
    fn a_row_says_what_the_program_reports_of_its_agent_and_else_its_title() {
        let (mut s, _) = session(false, Duration::from_secs(60));
        let agent = |id: &str, title: &str| Sub { id: id.into(), title: title.into(), ..sub(State::Working) };
        let (one, two) = (agent("a1", "Explore the sidebar"), agent("a2", "Run the tests"));
        assert_eq!(s.doing_of(&one), "Explore the sidebar");
        let agents = [("a1".to_owned(), "Read sidebar.rs".to_owned())].into();
        s.reported = Some(Reported { agents, ..Default::default() });
        assert_eq!((s.doing_of(&one), s.doing_of(&two)), ("Read sidebar.rs", "Run the tests"));
        // The program is started anew: what the one before reported is gone.
        s.restart();
        assert_eq!(s.doing_of(&one), "Explore the sidebar");
    }

    #[test]
    fn a_mark_moves_for_the_session_or_for_a_row_below_it() {
        let (mut s, now) = session(false, Duration::from_secs(60));
        assert!(!s.moves());
        s.said(Some(State::Working), false);
        assert!(s.moves());
        // The session waits for the user while what it sent off works on.
        s.said(Some(State::Blocked(None)), false);
        assert!(!s.moves());
        s.runs(vec![sub(State::Blocked(None)), sub(State::Working)]);
        assert!(s.moves());
        // Not for a row that is not shown.
        let mut many = vec![sub(State::Idle); SUBS];
        many.push(sub(State::Working));
        s.runs(many);
        assert!(!s.moves());
        // And nothing of a program that is gone.
        s.runs(vec![sub(State::Working)]);
        s.exited(Some(1), now);
        assert!(!s.moves());
    }

    #[test]
    fn a_program_that_is_gone_says_nothing_more() {
        // It ended with an error while it said it worked.
        let (mut s, now) = session(false, Duration::from_secs(60));
        s.said(Some(State::Working), false);
        s.runs(vec![sub(State::Working)]);
        assert_eq!(s.exited(Some(1), now), Exit::Keep);
        assert_eq!((s.program, s.doing()), (None, Doing::Idle));
        assert!(s.subs.is_empty(), "nor does what it ran beside itself");
        // A resume that failed, and a row that is started again.
        let (mut s, now) = session(true, Duration::from_secs(1));
        s.said(Some(State::Blocked(None)), false);
        assert_eq!(s.exited(Some(1), now), Exit::Retry);
        assert_eq!(s.program, None);
        s.said(Some(State::Working), false);
        s.restart();
        assert_eq!((s.program, s.doing()), (None, Doing::Idle));
        // Another program in the row knows nothing of what the one
        // before said, nor of what the user was told of it.
        s.said(Some(State::Done), false);
        s.begin_start(StartGate::new(), false);
        assert_eq!((s.program, s.doing()), (None, Doing::Idle));
        assert_eq!(s.said(Some(State::Done), false), Some("Done"));
    }

    #[test]
    fn only_a_clean_exit_removes_the_session() {
        let (mut s, now) = session(false, Duration::from_secs(1));
        assert_eq!(s.exited(Some(0), now), Exit::Close);
        let (mut unknown, now) = session(false, Duration::from_secs(1));
        assert_eq!(unknown.exited(None, now), Exit::Keep);
        // A crash after an hour of work keeps the row and what resumes it.
        let (mut s, now) = session(true, Duration::from_secs(3600));
        assert_eq!(s.exited(Some(137), now), Exit::Keep);
        assert!(s.ended.as_deref().unwrap().contains("137"));
        s.restart();
        assert!(s.ended.is_none() && s.resume);
        assert_eq!(s.follow_request, 1);
        assert!(s.ask_at.is_none());
    }

    #[test]
    fn a_question_no_worker_takes_is_asked_again_but_not_without_end() {
        let (mut s, t0) = session(false, Duration::ZERO);
        s.followed = true;
        s.touched(t0);
        // Every worker is taken, each time the question is due.
        let mut asked = 0;
        let mut now = t0;
        while let Some(at) = s.ask_at.take() {
            asked += 1;
            assert!(asked < 1000, "the question is asked for ever");
            now = at;
            s.ask_refused(now);
        }
        assert_eq!(asked, 1 + usize::from(ASK_AGAIN));
        assert!(now - t0 <= Duration::from_secs(90));
        // What is typed or printed next asks it anew, as often again.
        s.touched(now);
        assert!(s.ask_at.take().is_some());
        s.ask_refused(now);
        assert!(s.ask_at.is_some() && s.ask_refused == 1);
        // A session whose agent is not followed is never asked.
        let (mut plain, t0) = session(false, Duration::ZERO);
        plain.ask_refused(t0);
        assert!(plain.ask_at.is_none());
    }

    /// The times in milliseconds at which a report is read when there is
    /// output at `outputs`, looked at every 10 ms.
    fn reads(outputs: &[u64]) -> Vec<u64> {
        let (mut s, t0) = session(false, Duration::ZERO);
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut reads = Vec::new();
        for ms in (0..=9000).step_by(10) {
            if outputs.contains(&ms) {
                s.output(at(ms));
            }
            if s.read_due(at(ms)) {
                reads.push(ms);
            }
        }
        assert!(s.read_at.is_none(), "nothing wakes a session that went quiet");
        reads
    }

    #[test]
    fn a_report_is_read_a_second_after_output_and_never_more_often() {
        let steady = |every: usize, until: u64| (0..=until).step_by(every).collect::<Vec<u64>>();
        // The last read comes 700 ms after the last output: late enough
        // for the program to have written.
        assert_eq!(reads(&steady(100, 2300)), [1000, 2000, 3000]);
        // 200 ms after the last output is not, so one more follows.
        assert_eq!(reads(&steady(100, 2800)), [1000, 2000, 3000, 4000]);
        // Output a little faster than the reads does not hurry them.
        assert_eq!(reads(&steady(900, 4500)), [1000, 2000, 3000, 4000, 5000]);
        // A key and its echo are read once, not twice.
        assert_eq!(reads(&[0, 1]), [1000]);
        assert_eq!(reads(&[0]), [1000]);
        assert_eq!(reads(&[0, 990]), [1000, 2000]);
    }

    /// A session whose program runs and was named a file.
    fn reporting() -> (Session, Instant) {
        let (mut s, now) = session(false, Duration::ZERO);
        s.begin_start(StartGate::new(), false);
        s.started(echo());
        s.report_file = Some("/state/status/1/1-1.toml".into());
        (s, now)
    }

    /// A program that ends at once, to stand for one that runs.
    fn echo() -> Terminal {
        let grid = Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
        Terminal::spawn(
            "/bin/sh",
            vec!["-c".into(), ":".into()],
            Path::new("/tmp"),
            grid,
            &Options::default(),
            |_| {},
        )
        .unwrap()
    }

    fn said(model: &str) -> Option<Reported> {
        Some(Reported { model: Some(model.into()), ..Default::default() })
    }

    #[test]
    fn one_read_is_out_at_a_time_and_its_answer_is_taken() {
        let (mut s, now) = reporting();
        let (file, request) = s.begin_read().unwrap();
        assert_eq!(file, Path::new("/state/status/1/1-1.toml"));
        // Due again while the read is out: nothing starts, and the answer
        // that is under way still counts. The next read follows it.
        assert!(s.begin_read().is_none() && s.begin_read().is_none());
        assert!(s.read_done(request, Some(said("a")), now));
        assert_eq!((s.reported.clone(), s.read_at), (said("a"), Some(now)));
        assert!(s.read_due(now) && !s.read_due(now));
        // The same again changes nothing, and nothing follows it.
        let (_, request) = s.begin_read().unwrap();
        assert!(!s.read_done(request, Some(said("a")), now));
        assert!(s.read_at.is_none());
        // A file that could not be read leaves what is shown, one that is
        // gone takes it away.
        let (_, request) = s.begin_read().unwrap();
        assert!(!s.read_done(request, None, now));
        assert_eq!(s.reported, said("a"));
        let (_, request) = s.begin_read().unwrap();
        assert!(s.read_done(request, Some(None), now));
        assert_eq!(s.reported, None);
        // No worker was free: the next input or output asks again.
        assert!(s.begin_read().is_some());
        s.read_refused();
        assert!(s.read_at.is_none() && s.begin_read().is_some());
    }

    #[test]
    fn an_answer_belongs_to_the_program_that_was_asked() {
        let (mut s, now) = reporting();
        let (_, old) = s.begin_read().unwrap();
        assert!(s.read_done(old, Some(said("a")), now));
        let (_, old) = s.begin_read().unwrap();
        // The program is started again: what the one before reported is
        // gone, and so is its file.
        s.restart();
        assert!(s.reported.is_none() && s.report_file.is_none() && s.read_at.is_none());
        assert!(s.begin_read().is_none(), "nothing runs, and no file is named");
        s.begin_start(StartGate::new(), false);
        s.started(echo());
        s.report_file = Some("/state/status/1/1-3.toml".into());
        let (file, new) = s.begin_read().unwrap();
        assert_eq!(file, Path::new("/state/status/1/1-3.toml"));
        // The answer that was still out is for the program before. It is
        // not taken, and it does not end the read that is out now.
        assert!(!s.read_done(old, Some(said("old")), now));
        assert!(s.reported.is_none() && s.begin_read().is_none());
        assert!(s.read_done(new, Some(said("new")), now));
        assert_eq!(s.reported, said("new"));
        // A program that ended reports nothing more.
        let (_, request) = s.begin_read().unwrap();
        s.ended = Some("gone".into());
        assert!(!s.read_done(request, Some(said("late")), now) && s.begin_read().is_none());
        // Every start lets go of the program before, however it came about.
        s.ended = None;
        s.begin_start(StartGate::new(), false);
        assert!(s.reported.is_none() && s.report_file.is_none() && s.read_request != request);
    }

    #[test]
    fn a_resume_that_failed_takes_its_report_with_it() {
        let (mut s, now) = reporting();
        (s.resume, s.spawned) = (true, Some(now));
        let (_, request) = s.begin_read().unwrap();
        assert!(s.read_done(request, Some(said("a")), now));
        s.touched(now);
        assert_eq!(s.exited(Some(1), now + Duration::from_secs(1)), Exit::Retry);
        assert!(s.reported.is_none() && s.report_file.is_none() && s.read_at.is_none());
    }

    #[test]
    fn initial_pty_reports_wait_for_installation_and_keep_order() {
        let gate = StartGate::new();
        let ready = gate.notifier();
        let (send, receive) = std::sync::mpsc::channel();
        let grid = Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
        let mut term = Terminal::spawn(
            "/bin/sh",
            vec!["-c".into(), "printf '\\033]0;first\\007\\033]0;second\\007'; exit 7".into()],
            Path::new("/tmp"),
            grid,
            &Options { scrollback: 100, ..Default::default() },
            move |report| {
                if ready() {
                    let _ = send.send(report);
                }
            },
        )
        .unwrap();
        assert!(receive.recv_timeout(Duration::from_millis(30)).is_err());
        gate.ready();
        let palette = [[0; 4]; term::PALETTE];
        let mut titles = Vec::new();
        loop {
            let report = receive.recv_timeout(Duration::from_secs(3)).unwrap();
            match term.handle(report, &palette) {
                Some(term::Event::Title(Some(title))) => titles.push(title),
                Some(term::Event::Exited(code)) => {
                    assert_eq!(code, Some(7));
                    break;
                }
                _ => {}
            }
        }
        // Titles wait as one, so the first may have been overtaken by the
        // second before it was taken. It never comes after it.
        assert!(titles == ["first", "second"] || titles == ["second"], "{titles:?}");
    }

    #[test]
    fn cancelling_start_wakes_the_pty_reader() {
        let gate = StartGate::new();
        let ready = gate.notifier();
        let cancelled = gate.cancellation();
        let (send, receive) = std::sync::mpsc::channel();
        let term = Terminal::spawn(
            "/bin/sh",
            vec!["-c".into(), "printf '\\033]0;first\\007'; sleep 0.1".into()],
            Path::new("/tmp"),
            Grid { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            &Options::default(),
            move |_| {
                let _ = send.send(ready());
            },
        )
        .unwrap();
        assert!(!cancelled());
        drop(gate);
        assert!(cancelled());
        assert!(!receive.recv_timeout(Duration::from_secs(3)).unwrap());
        drop(term);
    }
}
