//! Derives a session's status from how its terminal behaves.
//!
//! No agent-specific protocol is involved: an agent that is working keeps
//! redrawing (spinner, streamed text), one that waits for the user is quiet.
//! That holds for any CLI, so new agents need no code here.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Idle,
    Working,
    /// Stopped while the user was not looking: finished or asking something.
    Attention,
}

/// Output this soon after a keystroke is the echo of typing.
const ECHO: Duration = Duration::from_millis(300);
/// A longer pause than this starts a new burst of output.
const GAP: Duration = Duration::from_millis(500);
/// Output must keep coming this long to count as working.
const SUSTAIN: Duration = Duration::from_millis(800);
/// Silence this long ends the work.
const QUIET: Duration = Duration::from_millis(1500);
/// Shorter work than this ends without asking for attention.
const NOTABLE: Duration = Duration::from_secs(2);

pub struct Activity {
    pub status: Status,
    last_output: Option<Instant>,
    last_input: Option<Instant>,
    /// Start of the current run of output.
    burst: Option<Instant>,
    /// When the status became `Working`.
    working_since: Option<Instant>,
}

impl Default for Activity {
    fn default() -> Self {
        Self { status: Status::Idle, last_output: None, last_input: None, burst: None, working_since: None }
    }
}

impl Activity {
    pub fn input(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    /// The terminal produced output. Returns whether the status changed.
    pub fn output(&mut self, now: Instant) -> bool {
        let echo = self.last_input.is_some_and(|t| now.duration_since(t) < ECHO);
        if echo && self.status != Status::Working {
            return false;
        }
        if self.last_output.is_none_or(|t| now.duration_since(t) > GAP) {
            self.burst = Some(now);
        }
        self.last_output = Some(now);
        let sustained = self.burst.is_some_and(|t| now.duration_since(t) >= SUSTAIN);
        // A session that asks for attention keeps asking until it was looked
        // at, whatever it prints in the meantime.
        if sustained && self.status == Status::Idle {
            self.status = Status::Working;
            self.working_since = self.burst;
            return true;
        }
        false
    }

    /// When `settle` should run next.
    pub(super) fn deadline(&self) -> Option<Instant> {
        if self.status == Status::Working { self.last_output.map(|t| t + QUIET) } else { None }
    }

    /// Ends the work once the terminal went quiet. `seen` says whether the
    /// user is looking at this session. Returns whether the status changed.
    pub fn settle(&mut self, now: Instant, seen: bool) -> bool {
        let (Some(last), Some(since)) = (self.last_output, self.working_since) else { return false };
        if self.status != Status::Working || now < last + QUIET {
            return false;
        }
        // Measured over the whole work, pauses in the output included.
        let notable = last.duration_since(since) >= NOTABLE;
        self.status = if notable && !seen { Status::Attention } else { Status::Idle };
        self.burst = None;
        self.working_since = None;
        true
    }

    /// The terminal rang its bell.
    pub fn bell(&mut self, seen: bool) -> bool {
        let changed = !seen && self.status != Status::Attention;
        if changed {
            self.status = Status::Attention;
        }
        changed
    }

    /// The user looked at the session.
    pub fn seen(&mut self) -> bool {
        let changed = self.status == Status::Attention;
        if changed {
            self.status = Status::Idle;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Feeds output every 100 ms from `from` to `to` milliseconds.
    fn stream(a: &mut Activity, t0: Instant, from: u64, to: u64) {
        for t in (from..=to).step_by(100) {
            a.output(t0 + ms(t));
        }
    }

    #[test]
    fn sustained_output_is_work_and_silence_asks_for_attention() {
        let (mut a, t0) = (Activity::default(), Instant::now());
        stream(&mut a, t0, 0, 700);
        assert_eq!(a.status, Status::Idle);
        stream(&mut a, t0, 800, 5000);
        assert_eq!(a.status, Status::Working);
        assert!(!a.settle(t0 + ms(6000), false));
        assert!(a.settle(t0 + ms(6500), false));
        assert_eq!(a.status, Status::Attention);
        assert!(a.seen());
        assert_eq!(a.status, Status::Idle);
    }

    #[test]
    fn watched_or_short_work_just_goes_idle() {
        let (mut a, t0) = (Activity::default(), Instant::now());
        stream(&mut a, t0, 0, 5000);
        assert!(a.settle(t0 + ms(7000), true));
        assert_eq!(a.status, Status::Idle);

        let (mut a, t0) = (Activity::default(), Instant::now());
        stream(&mut a, t0, 0, 1000);
        assert_eq!(a.status, Status::Working);
        assert!(a.settle(t0 + ms(3000), false));
        assert_eq!(a.status, Status::Idle);
    }

    #[test]
    fn typing_echo_and_single_lines_are_not_work() {
        let (mut a, t0) = (Activity::default(), Instant::now());
        for t in (0..3000).step_by(100) {
            a.input(t0 + ms(t));
            a.output(t0 + ms(t + 5));
        }
        assert_eq!(a.status, Status::Idle);
        // A log line every second never sustains a burst.
        for t in (10_000..20_000).step_by(1000) {
            a.output(t0 + ms(t));
        }
        assert_eq!(a.status, Status::Idle);
    }

    #[test]
    fn a_pause_in_long_work_does_not_hide_that_it_was_long() {
        let (mut a, t0) = (Activity::default(), Instant::now());
        stream(&mut a, t0, 0, 60_000);
        // A break of 0.7 s, then a last second of output.
        stream(&mut a, t0, 60_700, 61_700);
        assert!(a.settle(t0 + ms(64_000), false));
        assert_eq!(a.status, Status::Attention);
    }

    #[test]
    fn attention_lasts_until_seen_and_is_raised_once() {
        let (mut a, t0) = (Activity::default(), Instant::now());
        stream(&mut a, t0, 0, 5000);
        assert!(a.settle(t0 + ms(7000), false));
        // More output, even sustained, changes nothing until someone looks.
        for t in (8000..12_000).step_by(100) {
            assert!(!a.output(t0 + ms(t)));
        }
        assert!(!a.settle(t0 + ms(20_000), false));
        assert_eq!(a.status, Status::Attention);
        assert!(!a.bell(false), "already asking");
        assert!(a.seen());
    }

    #[test]
    fn bell_only_matters_when_unseen() {
        let mut a = Activity::default();
        assert!(!a.bell(true));
        assert!(a.bell(false));
        assert_eq!(a.status, Status::Attention);
    }
}
