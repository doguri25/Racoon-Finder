//! What the mouse's own buttons and a trackpad do (NAV-04, COMP-04), and the settings of the
//! long press (PICK-02) and of the checkboxes of the rows (NAV-15). A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! - **The mouse's fourth and fifth buttons** are the active pane's Back and Forward
//!   ([`AppState::mouse_back`], [`AppState::mouse_forward`]), as the keys are. They do nothing
//!   while a window shows over the main one, while a text field has the keys (the path field,
//!   the filter box: the window tells, [`AppState::set_pointer_context`]), while a drag is under
//!   way (a drag of rows, or one of another app over the window), and while the app ends.
//! - **A trackpad's horizontal swipe** adds up ([`AppState::swipe`]): once the distance reaches
//!   [`SWIPE_DISTANCE`] the active pane goes back (to the right) or forward (to the left), once,
//!   however far the same gesture goes on ([`AppState::swipe_end`] ends it).

use super::layout::Step;
use super::{AppState, Effect};

/// How far, in logical pixels, a swipe goes before it moves in the history.
pub const SWIPE_DISTANCE: f32 = 80.0;

/// What the window knows that decides whether the pointer's buttons and gestures act.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PointerContext {
    /// A text field has the keys.
    text_field: bool,
    /// A drag of rows is under way, or the left button is down on one.
    dragging: bool,
}

/// The swipe under way.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Swipe {
    /// How far the gesture has gone sideways, to the right positive.
    sum: f32,
    /// The gesture has moved the history already.
    spent: bool,
}

impl AppState {
    /// Tells what the window knows before a button or a gesture of the pointer: whether a text
    /// field has the keys, and whether a drag of rows is under way.
    pub fn set_pointer_context(&mut self, text_field: bool, dragging: bool) {
        self.pointer = PointerContext {
            text_field,
            dragging,
        };
    }

    /// Whether the pointer's buttons and gestures leave the history alone now.
    fn pointer_nav_blocked(&self) -> bool {
        self.modal.is_some()
            || self.quick_look_open()
            || self.quitting
            || self.pointer.text_field
            || self.pointer.dragging
            || self.outside.is_some()
    }

    /// The mouse's fourth button: the active pane goes back (NAV-04).
    pub fn mouse_back(&mut self) -> Vec<Effect> {
        self.mouse_step(Step::Back)
    }

    /// The mouse's fifth button: the active pane goes forward (NAV-04).
    pub fn mouse_forward(&mut self) -> Vec<Effect> {
        self.mouse_step(Step::Forward)
    }

    fn mouse_step(&mut self, step: Step) -> Vec<Effect> {
        if self.pointer_nav_blocked() {
            return Vec::new();
        }
        // As a command of the keys closes the list of subfolders under a `›`.
        self.subfolders = None;
        let side = self.active;
        self.go(side, step)
    }

    /// A trackpad's swipe went `dx` logical pixels sideways (to the right positive, as the
    /// fingers move). Once the gesture has gone [`SWIPE_DISTANCE`] it moves the active pane in
    /// its history, to the right back and to the left forward, and then does nothing more until
    /// [`AppState::swipe_end`].
    pub fn swipe(&mut self, dx: f32) -> Vec<Effect> {
        if self.swipe.spent || self.pointer_nav_blocked() || !dx.is_finite() {
            return Vec::new();
        }
        self.swipe.sum += dx;
        if self.swipe.sum.abs() < SWIPE_DISTANCE {
            return Vec::new();
        }
        let step = if self.swipe.sum > 0.0 {
            Step::Back
        } else {
            Step::Forward
        };
        self.swipe.spent = true;
        self.mouse_step(step)
    }

    /// The gesture ended (or was cancelled): the next one starts from nothing.
    pub fn swipe_end(&mut self) {
        self.swipe = Swipe::default();
    }

    /// Whether a long press opens the menu of the row it is held on (보기 ▸ 설정, PICK-02).
    pub fn long_press(&self) -> bool {
        self.long_press
    }

    pub fn set_long_press(&mut self, on: bool) {
        self.long_press = on;
    }

    /// How long the press is held, in milliseconds.
    pub fn long_press_ms(&self) -> u16 {
        self.long_press_ms
    }

    pub fn set_long_press_ms(&mut self, ms: u16) {
        self.long_press_ms = ms.clamp(
            crate::settings::LONG_PRESS_MIN_MS,
            crate::settings::LONG_PRESS_MAX_MS,
        );
    }
}
