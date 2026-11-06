//! The long press (PICK-02): a left press held still on a row, or on the empty part of a list,
//! opens the menu of the item, or of the folder, where the pointer is, as a right click does.
//! It is for touch screens, pens and mice without a right button.
//!
//! [`LongPress`] is the pure part, which the controller drives: it knows nothing of the window.
//! A press arms it ([`LongPress::press`]); the controller's timer asks it every few tens of
//! milliseconds ([`LongPress::tick`]) and shows its progress as a ring beside the pointer
//! ([`LongPress::progress`]); the pointer's moves ([`LongPress::moved`]) and the release
//! ([`LongPress::release`]) come from the window's own events, so that they reach it whatever
//! Slint makes of the press (a drag of rows, a scroll).
//!
//! - A release before the delay is an ordinary click ([`Outcome::Click`]): nothing opened, and
//!   the click goes on as it always does.
//! - A move of more than the threshold before the delay cancels it ([`Cancel`]): the press is
//!   whatever it was going to be, a drag of rows or a scroll, and no menu opens.
//! - At the delay the menu opens once ([`Open`]). The release that follows is then the press's
//!   end and nothing else ([`Outcome::Opened`]): the controller keeps it from clicking or
//!   selecting.
//!
//! A second button or finger, a key, the pointer leaving the window and a dialog cancel the
//! press ([`LongPress::cancel`]).

use std::time::{Duration, Instant};

/// How far, in logical pixels, the pointer may move from where it was pressed before the press
/// is no longer a long press (the drag threshold of Slint's drag area is larger: the press is
/// cancelled before a drag of rows starts).
pub const THRESHOLD_PX: f32 = 8.0;

/// How long a press is held before its progress shows: an ordinary click never flashes a ring.
pub const RING_AFTER: Duration = Duration::from_millis(150);

/// How often the controller's timer asks while a press is armed.
pub const TICK: Duration = Duration::from_millis(30);

/// A point of the window, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
}

impl Spot {
    pub fn new(x: f32, y: f32) -> Spot {
        Spot { x, y }
    }

    fn distance(self, other: Spot) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

/// The pointer moved too far for a long press: it is a drag or a scroll, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancel;

/// What the end of a press was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not a long press (released before the delay, moved too far, cancelled, or never armed):
    /// the release is handled as it always is.
    Click,
    /// The press held its time and opened the menu: the release must do nothing more.
    Opened,
}

/// The delay passed: the menu opens where the press was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Open {
    pub at: Spot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    Armed {
        at: Spot,
        since: Instant,
    },
    /// The menu opened; the release has not come yet.
    Opened,
}

/// The state of one press, from its start to its release.
#[derive(Clone, Debug)]
pub struct LongPress {
    delay: Duration,
    threshold: f32,
    phase: Phase,
}

impl LongPress {
    /// A machine for presses held `delay`, which the pointer may move `threshold_px` from where
    /// it was pressed.
    pub fn new(delay: Duration, threshold_px: f32) -> LongPress {
        LongPress {
            delay,
            threshold: threshold_px,
            phase: Phase::Idle,
        }
    }

    /// Holds the next presses `delay`; a press that is armed keeps the time it began with.
    pub fn set_delay(&mut self, delay: Duration) {
        self.delay = delay;
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }

    /// Whether a press is armed and has not opened its menu yet: the controller's timer runs
    /// only then.
    pub fn is_armed(&self) -> bool {
        matches!(self.phase, Phase::Armed { .. })
    }

    /// A left press at `at`, `now`. It replaces whatever the last press left.
    pub fn press(&mut self, at: Spot, now: Instant) {
        self.phase = Phase::Armed { at, since: now };
    }

    /// The pointer is at `at`. `Some` when it has moved farther than the threshold from the
    /// press while the press waited: the press is cancelled.
    pub fn moved(&mut self, at: Spot, _now: Instant) -> Option<Cancel> {
        let Phase::Armed { at: from, .. } = self.phase else {
            return None;
        };
        if from.distance(at) > self.threshold {
            self.phase = Phase::Idle;
            return Some(Cancel);
        }
        None
    }

    /// The button was let go, `_now`: what the press was. The machine is idle after it.
    pub fn release(&mut self, _now: Instant) -> Outcome {
        let opened = self.phase == Phase::Opened;
        self.phase = Phase::Idle;
        if opened {
            Outcome::Opened
        } else {
            Outcome::Click
        }
    }

    /// The timer's look: `Some` once, when the delay has passed since the press.
    pub fn tick(&mut self, now: Instant) -> Option<Open> {
        let Phase::Armed { at, since } = self.phase else {
            return None;
        };
        if now.saturating_duration_since(since) < self.delay {
            return None;
        }
        self.phase = Phase::Opened;
        Some(Open { at })
    }

    /// Another button or finger, a key, the pointer leaving the window, or a dialog: a press
    /// that waits is no long press. A menu that opened already stays open, and its release
    /// still ends the press ([`Outcome::Opened`]).
    pub fn cancel(&mut self) {
        if self.is_armed() {
            self.phase = Phase::Idle;
        }
    }

    /// The press and its place, while it waits.
    pub fn armed_at(&self) -> Option<Spot> {
        match self.phase {
            Phase::Armed { at, .. } => Some(at),
            Phase::Idle | Phase::Opened => None,
        }
    }

    /// How far the press is from its menu, 0 to 1, for the ring beside the pointer: `None`
    /// while no press waits and for the first [`RING_AFTER`], so that a click shows no ring.
    pub fn progress(&self, now: Instant) -> Option<f32> {
        let Phase::Armed { since, .. } = self.phase else {
            return None;
        };
        let held = now.saturating_duration_since(since);
        if held < RING_AFTER || self.delay.is_zero() {
            return None;
        }
        Some((held.as_secs_f32() / self.delay.as_secs_f32()).clamp(0.0, 1.0))
    }
}
