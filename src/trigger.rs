//! Trigger-key state machine. Pure logic (no OS calls), so it is fully tested.
//!
//! - Double-tap  -> hands-free recording; the next press stops it.
//! - Hold        -> push-to-talk; release stops it.
//! - Esc         -> cancel.
//!
//! Key events pass through to the OS untouched. For a toggle key like Caps
//! Lock we count the presses in a session; if the count is odd the LED/state
//! was flipped, so we emit `RestoreToggle` (after the key is physically up).

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    HandsFree,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Down,
    Up,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start(Mode),
    Stop,
    Cancel,
    /// Tap the toggle key once to undo the state change our presses caused.
    RestoreToggle,
}

#[derive(Debug, Clone)]
pub struct Params {
    pub double_tap: Duration,
    pub hold: Duration,
    pub max: Duration,
    /// True for keys that toggle OS state (Caps Lock, Scroll Lock).
    pub toggle_key: bool,
    pub esc_cancels: bool,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    FirstDown(Instant),
    WaitSecond(Instant),
    Recording { mode: Mode, since: Instant },
}

pub struct Trigger {
    p: Params,
    state: State,
    down: bool,
    toggles: u32,
    ignore_up: bool,
    restore_pending: bool,
}

impl Trigger {
    pub fn new(p: Params) -> Self {
        Trigger { p, state: State::Idle, down: false, toggles: 0, ignore_up: false, restore_pending: false }
    }

    pub fn set_params(&mut self, p: Params) {
        self.p = p;
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.state, State::Recording { .. })
    }

    pub fn input(&mut self, input: Input, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        match input {
            Input::Down => {
                if self.down {
                    return out; // auto-repeat
                }
                self.down = true;
                match self.state {
                    State::Idle => {
                        self.toggles = 1;
                        self.state = State::FirstDown(now);
                    }
                    State::WaitSecond(released) => {
                        if now.duration_since(released) < self.p.double_tap {
                            self.toggles += 1;
                            self.ignore_up = true; // release of the second tap
                            self.state = State::Recording { mode: Mode::HandsFree, since: now };
                            out.push(Action::Start(Mode::HandsFree));
                        } else {
                            self.toggles = 1;
                            self.state = State::FirstDown(now);
                        }
                    }
                    State::Recording { mode: Mode::HandsFree, .. } => {
                        self.toggles += 1;
                        self.ignore_up = true;
                        out.push(Action::Stop);
                        self.finish(&mut out);
                    }
                    // Key is already down in these states; unreachable in practice.
                    State::Recording { mode: Mode::PushToTalk, .. } | State::FirstDown(_) => {}
                }
            }
            Input::Up => {
                if !self.down {
                    return out;
                }
                self.down = false;
                if self.ignore_up {
                    self.ignore_up = false;
                    self.flush_restore(&mut out);
                    return out;
                }
                match self.state {
                    State::FirstDown(pressed) => {
                        self.state = if now.duration_since(pressed) < self.p.hold {
                            State::WaitSecond(now)
                        } else {
                            State::Idle
                        };
                    }
                    State::Recording { mode: Mode::PushToTalk, .. } => {
                        out.push(Action::Stop);
                        self.finish(&mut out);
                    }
                    _ => {}
                }
            }
            Input::Esc => {
                if self.is_recording() && self.p.esc_cancels {
                    if self.down {
                        self.ignore_up = true;
                    }
                    out.push(Action::Cancel);
                    self.finish(&mut out);
                }
            }
        }
        out
    }

    /// Call when `deadline()` passes.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        match self.state {
            State::FirstDown(pressed) if self.down && now.duration_since(pressed) >= self.p.hold => {
                self.state = State::Recording { mode: Mode::PushToTalk, since: now };
                out.push(Action::Start(Mode::PushToTalk));
            }
            State::WaitSecond(released) if now.duration_since(released) >= self.p.double_tap => {
                // A lone tap: a normal Caps Lock press. Leave it alone.
                self.state = State::Idle;
                self.toggles = 0;
            }
            State::Recording { since, .. } if now.duration_since(since) >= self.p.max => {
                if self.down {
                    self.ignore_up = true;
                }
                out.push(Action::Stop);
                self.finish(&mut out);
            }
            _ => {}
        }
        out
    }

    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Idle => None,
            State::FirstDown(t) => Some(t + self.p.hold),
            State::WaitSecond(t) => Some(t + self.p.double_tap),
            State::Recording { since, .. } => Some(since + self.p.max),
        }
    }

    /// Force the session to end (e.g. the microphone failed to open).
    pub fn abort(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.is_recording() {
            if self.down {
                self.ignore_up = true;
            }
            self.finish(&mut out);
        }
        out
    }

    fn finish(&mut self, out: &mut Vec<Action>) {
        self.state = State::Idle;
        if self.p.toggle_key && self.toggles % 2 == 1 {
            self.restore_pending = true;
        }
        self.toggles = 0;
        self.flush_restore(out);
    }

    fn flush_restore(&mut self, out: &mut Vec<Action>) {
        if self.restore_pending && !self.down {
            self.restore_pending = false;
            out.push(Action::RestoreToggle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    fn params() -> Params {
        Params {
            double_tap: Duration::from_millis(400),
            hold: Duration::from_millis(300),
            max: Duration::from_secs(60),
            toggle_key: true,
            esc_cancels: true,
        }
    }

    struct Sim {
        t: Trigger,
        now: Instant,
        log: Vec<Action>,
    }

    impl Sim {
        fn new() -> Self {
            Sim { t: Trigger::new(params()), now: Instant::now(), log: vec![] }
        }
        /// Advance time, firing ticks whenever a deadline passes.
        fn wait(&mut self, ms: u64) {
            let end = self.now + Duration::from_millis(ms);
            while let Some(d) = self.t.deadline() {
                if d > end {
                    break;
                }
                self.now = d;
                let a = self.t.tick(d);
                self.log.extend(a);
            }
            self.now = end;
        }
        fn key(&mut self, i: Input) {
            let a = self.t.input(i, self.now);
            self.log.extend(a);
        }
        fn tap(&mut self) {
            self.key(Input::Down);
            self.wait(80);
            self.key(Input::Up);
        }
    }

    #[test]
    fn double_tap_starts_hands_free_and_tap_stops_with_restore() {
        let mut s = Sim::new();
        s.tap();
        s.wait(150);
        s.tap();
        assert_eq!(s.log, vec![Start(Mode::HandsFree)]);
        s.wait(3000);
        s.key(Input::Down);
        assert_eq!(s.log, vec![Start(Mode::HandsFree), Stop]); // restore waits for key-up
        s.wait(80);
        s.key(Input::Up);
        assert_eq!(s.log, vec![Start(Mode::HandsFree), Stop, RestoreToggle]);
        assert!(!s.t.is_recording());
    }

    #[test]
    fn hold_is_push_to_talk() {
        let mut s = Sim::new();
        s.key(Input::Down);
        s.wait(299);
        assert!(s.log.is_empty());
        s.wait(2);
        assert_eq!(s.log, vec![Start(Mode::PushToTalk)]);
        // auto-repeat downs are ignored
        s.key(Input::Down);
        s.key(Input::Down);
        s.wait(2000);
        s.key(Input::Up);
        assert_eq!(s.log, vec![Start(Mode::PushToTalk), Stop, RestoreToggle]);
    }

    #[test]
    fn single_tap_is_left_alone() {
        let mut s = Sim::new();
        s.tap();
        s.wait(1000);
        assert!(s.log.is_empty());
        // A later double-tap still works from a clean state.
        s.tap();
        s.wait(100);
        s.tap();
        assert_eq!(s.log, vec![Start(Mode::HandsFree)]);
    }

    #[test]
    fn slow_double_tap_does_nothing() {
        let mut s = Sim::new();
        s.tap();
        s.wait(500);
        s.tap();
        s.wait(1000);
        assert!(s.log.is_empty());
    }

    #[test]
    fn esc_cancels_hands_free_without_restore() {
        let mut s = Sim::new();
        s.tap();
        s.wait(100);
        s.tap();
        s.wait(500);
        s.key(Input::Esc);
        assert_eq!(s.log, vec![Start(Mode::HandsFree), Cancel]);
    }

    #[test]
    fn esc_cancels_push_to_talk_and_restores_after_release() {
        let mut s = Sim::new();
        s.key(Input::Down);
        s.wait(500);
        s.key(Input::Esc);
        assert_eq!(s.log, vec![Start(Mode::PushToTalk), Cancel]);
        s.key(Input::Up);
        assert_eq!(s.log, vec![Start(Mode::PushToTalk), Cancel, RestoreToggle]);
        // release didn't start anything new
        s.wait(1000);
        assert_eq!(s.log.len(), 3);
    }

    #[test]
    fn max_duration_stops() {
        let mut s = Sim::new();
        s.tap();
        s.wait(100);
        s.tap();
        s.wait(61_000);
        assert_eq!(s.log, vec![Start(Mode::HandsFree), Stop]);
    }

    #[test]
    fn non_toggle_key_never_restores() {
        let mut s = Sim::new();
        s.t.set_params(Params { toggle_key: false, ..params() });
        s.key(Input::Down);
        s.wait(500);
        s.key(Input::Up);
        assert_eq!(s.log, vec![Start(Mode::PushToTalk), Stop]);
    }

    #[test]
    fn esc_when_idle_is_ignored() {
        let mut s = Sim::new();
        s.key(Input::Esc);
        assert!(s.log.is_empty());
    }
}
