//! The mouse's fourth and fifth buttons (NAV-04), a trackpad's swipe (COMP-04), the long press
//! (PICK-02) and the slow second click (OPS-02), from the window's own events.
//!
//! The window's winit hook ([`watch_winit_events`](super)) sees these before Slint does, and
//! calls the methods here:
//!
//! - **Back and Forward** go to the active pane's history ([`AppState::mouse_back`]); the window
//!   tells whether a text field has the keys and whether a drag is under way first, as they
//!   decide whether the button acts.
//! - **A swipe** (winit's `PanGesture`) adds up in the state ([`AppState::swipe`]) and ends with
//!   the gesture.
//! - **A long press** is armed by a left press (or a first finger) on a pane's list, on a row or
//!   on the part without rows ([`Controller::press_began`]), but not on the heads, the tabs, the
//!   sidebar, a scroll bar, a text field, or while a dialog shows (`MainWindow.press-at`). A timer
//!   of 30 ms runs while it waits ([`LongPress`]): it shows the ring beside the pointer, and at
//!   its time opens the menu of the row or of the folder where the press was, as a right click
//!   does ([`Controller::press_open`]). The pointer's moves and the release are the window's own
//!   events too, so that they reach it whatever Slint makes of the press: a move past the
//!   threshold ends it ([`Controller::press_moved`]), a second button, a key, another finger or
//!   the pointer leaving the window cancel it ([`Controller::press_cancel`]). A long press on
//!   뒤로 or 앞으로 opens the visit history's menu that way instead (NAV-04), as a right click on
//!   them does.
//! - **A slow second click** on the name of the item that is already the only selection renames
//!   it (OPS-02, [`SlowClick`]): a row's press tells what it is on ([`Controller::slow_press`]),
//!   the pointer's moves and the cancels of the long press reach it too, its click
//!   ([`Controller::slow_release`]) renames at once when the double-click time has passed since
//!   the press, else a timer waits for that time, unless another press comes first (a double
//!   click opens).
//!
//! The menu opens while the button is still down. The menus the system shows (Windows, macOS) run
//! a loop of their own that takes the button's release, which neither winit nor Slint would then
//! hear; so just before the menu opens, Slint is told the button was let go (the row's click that
//! this makes is dropped, [`Controller::release_swallowed`]), and the release that really comes,
//! if it comes, is kept from Slint.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use rf_platform::shell::WindowPoint;
use slint::ComponentHandle;

use super::{Controller, side_number, with};
use crate::history::Way;
use crate::state::long_press::{LongPress, Outcome, Spot, THRESHOLD_PX, TICK};
use crate::state::{DOUBLE_CLICK, SlowClick, SlowTarget};
use crate::{DragPointer, DropPlace, LongPressRing};

/// Where a long press began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PressTarget {
    /// A row of a pane's list, or the part of it without rows.
    List { side: usize, row: Option<usize> },
    /// 뒤로 or 앞으로, whose visit history that way it lists (NAV-04).
    Visits(Way),
}

/// The number the window gives the button of `way` (`MainWindow.visits-button-at`).
fn way_number(way: Way) -> i32 {
    match way {
        Way::Back => 0,
        Way::Forward => 1,
    }
}

/// How many fingers count as down once the menu of a long press returned: when a finger armed
/// the press, the menu's loop may have taken its end, so none counts (a finger still down
/// brings the count back to none at its own end, which never goes below none).
fn fingers_after_menu(armed_by_finger: bool, down: usize) -> usize {
    if armed_by_finger { 0 } else { down }
}

/// What the controller keeps of the long press under way.
pub(super) struct PressWork {
    pub(super) machine: RefCell<LongPress>,
    target: Cell<Option<PressTarget>>,
    /// The finger that armed the press, when a finger did.
    finger: Cell<Option<u64>>,
    /// How many fingers are down.
    fingers: Cell<usize>,
    /// Runs while a press waits.
    pub(super) timer: slint::Timer,
    /// The release Slint is being told of is the long press's own: it clicks nothing.
    pub(super) swallow: Cell<bool>,
    /// The slow second click (OPS-02), and the timer that waits out the double-click time
    /// after its click.
    slow: RefCell<SlowClick>,
    slow_timer: slint::Timer,
}

impl PressWork {
    pub(super) fn new() -> PressWork {
        PressWork {
            machine: RefCell::new(LongPress::new(
                std::time::Duration::from_millis(u64::from(crate::settings::LONG_PRESS_DEFAULT_MS)),
                THRESHOLD_PX,
            )),
            target: Cell::new(None),
            finger: Cell::new(None),
            fingers: Cell::new(0),
            timer: slint::Timer::default(),
            swallow: Cell::new(false),
            slow: RefCell::new(SlowClick::new(DOUBLE_CLICK, THRESHOLD_PX)),
            slow_timer: slint::Timer::default(),
        }
    }
}

impl Controller {
    /// The mouse's fourth (`back`) or fifth button went down.
    pub(super) fn mouse_nav(&self, back: bool) {
        self.pointer_context();
        self.update(|state| {
            if back {
                state.mouse_back()
            } else {
                state.mouse_forward()
            }
        });
    }

    /// A swipe of a trackpad moved `dx` logical pixels sideways (`started`: a new gesture;
    /// `ended`: its last).
    pub(super) fn swipe(&self, dx: f32, started: bool, ended: bool) {
        if started {
            self.update(|state| {
                state.swipe_end();
                Vec::new()
            });
        }
        if dx != 0.0 {
            self.pointer_context();
            self.update(|state| state.swipe(dx));
        }
        if ended {
            self.update(|state| {
                state.swipe_end();
                Vec::new()
            });
        }
    }

    /// Tells the state what the window knows before a button or a gesture of the pointer acts.
    fn pointer_context(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let typing = window.get_typing();
        let drag = window.global::<DragPointer<'_>>();
        let dragging = drag.get_rows() || drag.get_over();
        if let Some(state) = self.state.borrow_mut().as_mut() {
            state.set_pointer_context(typing, dragging);
        }
    }

    /// A left press at `at` (a finger when `finger` has its id): a long press is armed when it is
    /// on a pane's list and the setting is on.
    pub(super) fn press_began(&self, at: Spot, finger: Option<u64>) {
        // A new press forgets the last one, a menu that opened included: its release may never
        // have come (an OS menu's loop takes it), and waiting for it would withhold this
        // press's release.
        self.press_reset();
        self.press.finger.set(finger);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let (allowed, ms) = {
            let state = self.state.borrow();
            let Some(state) = state.as_ref() else {
                return;
            };
            (
                state.long_press() && state.modal().is_none(),
                state.long_press_ms(),
            )
        };
        if !allowed {
            return;
        }
        let target = match window.invoke_visits_button_at(at.x, at.y) {
            0 => PressTarget::Visits(Way::Back),
            1 => PressTarget::Visits(Way::Forward),
            _ => {
                let found = window.invoke_press_at(at.x, at.y);
                if !found.found || found.place != DropPlace::List {
                    return;
                }
                PressTarget::List {
                    side: usize::try_from(found.side).unwrap_or(0),
                    row: usize::try_from(found.index).ok(),
                }
            }
        };
        self.press.target.set(Some(target));
        {
            let mut machine = self.press.machine.borrow_mut();
            machine.set_delay(std::time::Duration::from_millis(u64::from(ms)));
            machine.press(at, Instant::now());
        }
        self.press
            .timer
            .start(slint::TimerMode::Repeated, TICK, || {
                with(Controller::press_tick);
            });
    }

    /// The pointer is at `at`: a press that waits and was moved too far is no long press, nor a
    /// slow second click (it is a drag).
    pub(super) fn press_moved(&self, at: Spot) {
        self.press.slow.borrow_mut().moved(at);
        let cancelled = self
            .press
            .machine
            .borrow_mut()
            .moved(at, Instant::now())
            .is_some();
        if cancelled {
            self.press_stop();
        }
    }

    /// The left button (or the finger that armed the press) was let go: `true` when the press had
    /// opened its menu, whose release then goes no further.
    pub(super) fn press_released(&self) -> bool {
        let outcome = self.press.machine.borrow_mut().release(Instant::now());
        self.press_stop();
        outcome == Outcome::Opened
    }

    /// Another button, a key, another finger, the pointer leaving the window or a dialog: a press
    /// that waits is no long press.
    pub(super) fn press_cancel(&self) {
        self.press.machine.borrow_mut().cancel();
        self.press_stop();
        self.slow_cancel();
    }

    /// The press under way is forgotten, whatever it was (see [`LongPress::reset`]); a rename a
    /// slow second click left waiting does not come (this may be a double click's second press,
    /// whose row then says what it is, [`Controller::slow_press`]).
    pub(super) fn press_reset(&self) {
        self.press.machine.borrow_mut().reset();
        self.press_stop();
        self.slow_cancel();
        // Every left press: a rename the name window applied opens no other window over what
        // it does. Not while the state is borrowed (it never is here, outside a change).
        if let Ok(mut state) = self.state.try_borrow_mut()
            && let Some(state) = state.as_mut()
        {
            state.left_pressed();
        }
    }

    /// A left press at `now` on row `row` of pane `side`, with Ctrl or Shift held (`modified`)
    /// and on the text of its name or not, before the state takes it: whether it may be a slow
    /// second click ([`AppState::row_press`](crate::state::AppState::row_press)). The window
    /// gives the time of its event; a test chooses it.
    pub(super) fn slow_press(
        &self,
        side: usize,
        row: usize,
        modified: bool,
        on_name: bool,
        now: Instant,
    ) {
        self.press.slow_timer.stop();
        let press = self
            .state
            .borrow()
            .as_ref()
            .and_then(|state| state.row_press(side, row, on_name, modified));
        let at = self
            .pointer
            .get()
            .map_or(Spot::new(0.0, 0.0), |at| Spot::new(at.x, at.y));
        let mut slow = self.press.slow.borrow_mut();
        match press {
            Some(press) => slow.press(press, at, now),
            None => slow.cancel(),
        }
    }

    /// A press on a row was let go as a click at `now` (Slint's, after no drag): the rename of
    /// a slow second click, now or once the double-click time has passed.
    pub(super) fn slow_release(&self, now: Instant) {
        let due = self.press.slow.borrow_mut().release(now);
        match due {
            Some(target) => self.slow_due(&target),
            None => self.slow_wait(),
        }
    }

    /// Waits for the rename a slow second click left waiting, while one does.
    fn slow_wait(&self) {
        let Some(until) = self.press.slow.borrow().waiting_until() else {
            return;
        };
        let wait = until.saturating_duration_since(Instant::now());
        self.press
            .slow_timer
            .start(slint::TimerMode::SingleShot, wait, || {
                with(Controller::slow_tick);
            });
    }

    /// The timer of the slow second click: its rename, once the double-click time has passed.
    fn slow_tick(&self) {
        self.slow_tick_at(Instant::now());
    }

    /// [`Controller::slow_tick`] at `now`, which the tests choose.
    pub(super) fn slow_tick_at(&self, now: Instant) {
        let due = self.press.slow.borrow_mut().tick(now);
        match due {
            Some(target) => self.slow_due(&target),
            None => self.slow_wait(),
        }
    }

    /// When the rename a slow second click left waiting is due; `None` while none waits. A test
    /// takes the time of its tick from this, not from a clock of its own.
    #[cfg(test)]
    pub(super) fn slow_waits_until(&self) -> Option<Instant> {
        self.press.slow.borrow().waiting_until()
    }

    /// The slow second click came due on `target`: its name window opens.
    fn slow_due(&self, target: &SlowTarget) {
        self.update(|state| state.slow_rename(target));
    }

    /// No rename comes of the slow second click under way: a double click opened the row, or
    /// the press was cancelled.
    pub(super) fn slow_cancel(&self) {
        self.press.slow.borrow_mut().cancel();
        self.press.slow_timer.stop();
    }

    /// The window lost the focus: no press is under way, no finger counts as down (their ends
    /// go to the window that has the focus now).
    pub(super) fn press_focus_lost(&self) {
        self.press_reset();
        self.press.fingers.set(0);
    }

    /// A finger touched the window (`down`) or left it; the number that is down. A second
    /// finger cancels a press that waits.
    pub(super) fn fingers_changed(&self, down: bool) -> usize {
        let now = if down {
            self.press.fingers.get().saturating_add(1)
        } else {
            self.press.fingers.get().saturating_sub(1)
        };
        self.press.fingers.set(now);
        now
    }

    /// Whether `finger` is the one that armed the press under way.
    pub(super) fn is_press_finger(&self, finger: u64) -> bool {
        self.press.finger.get() == Some(finger)
    }

    /// Whether the release Slint is told of now is the long press's own: the row it came of
    /// clicks nothing.
    pub(super) fn release_swallowed(&self) -> bool {
        self.press.swallow.get()
    }

    /// Stops the timer and the ring, and forgets where the press began.
    fn press_stop(&self) {
        self.press.timer.stop();
        self.press.target.set(None);
        if let Some(window) = self.window.upgrade() {
            window.global::<LongPressRing<'_>>().set_shown(false);
        }
    }

    /// The timer's turn: the ring, or the menu at the time.
    fn press_tick(&self) {
        self.press_tick_at(Instant::now());
    }

    /// [`Controller::press_tick`] at `now`, which the tests choose.
    pub(super) fn press_tick_at(&self, now: Instant) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        // A drag of rows began meanwhile (Slint's own threshold is past the press's): it is
        // that, and no menu.
        if window.global::<DragPointer<'_>>().get_rows() {
            return self.press_cancel();
        }
        let (open, progress, at) = {
            let mut machine = self.press.machine.borrow_mut();
            let at = machine.armed_at();
            (machine.tick(now), machine.progress(now), at)
        };
        if let Some(open) = open {
            return self.press_open(open.at);
        }
        let ring = window.global::<LongPressRing<'_>>();
        match (progress, at) {
            (Some(progress), Some(at)) => {
                ring.set_x(at.x);
                ring.set_y(at.y);
                ring.set_progress(progress);
                ring.set_shown(true);
            }
            _ => ring.set_shown(false),
        }
    }

    /// The press held its time: the menu of what it is on opens at `at`, as a right click would
    /// open it.
    fn press_open(&self, at: Spot) {
        let Some(target) = self.press.target.take() else {
            return self.press_stop();
        };
        self.press_stop();
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let still_allowed = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.long_press() && state.modal().is_none());
        if !still_allowed {
            // No menu opens, so no release is to be withheld.
            self.press.machine.borrow_mut().reset();
            return;
        }
        // The list moved or was listed again during the hold (a scroll of the keys, a relist):
        // the menu is of what is under the pointer now, or of nothing; a button that went off
        // meanwhile lists nothing.
        let same = match target {
            PressTarget::List { side, row } => {
                let under = window.invoke_press_at(at.x, at.y);
                under.found
                    && usize::try_from(under.side).ok() == Some(side)
                    && usize::try_from(under.index).ok() == row
            }
            PressTarget::Visits(way) => {
                window.invoke_visits_button_at(at.x, at.y) == way_number(way)
            }
        };
        if !same {
            self.press.machine.borrow_mut().reset();
            return;
        }
        // The press is over for the window before a menu of the system's takes the release
        // (see the module's text).
        self.left_down.set(false);
        self.press.swallow.set(true);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(at.x, at.y),
                button: slint::platform::PointerEventButton::Left,
            });
        self.press.swallow.set(false);
        // Where Windows' own menu shows first, it shows where a right click would show it (on a
        // list: the buttons have the app's menu alone).
        self.pointer.set(Some(WindowPoint { x: at.x, y: at.y }));
        if matches!(target, PressTarget::List { .. }) && self.right_pressed() {
            return;
        }
        match target {
            PressTarget::List {
                side,
                row: Some(row),
            } => {
                self.update(|state| state.row_menu(side, row));
                window.invoke_show_item_menu_at(side_number(side), at.x, at.y);
            }
            PressTarget::List { side, row: None } => {
                window.invoke_show_blank_menu_at(side_number(side), at.x, at.y);
            }
            PressTarget::Visits(way) => window.invoke_show_visits_menu(way_number(way)),
        }
        // A menu of the system's may have taken the finger's end: the count is forgotten.
        self.press.fingers.set(fingers_after_menu(
            self.press.finger.get().is_some(),
            self.press.fingers.get(),
        ));
    }
}

#[cfg(test)]
mod finger_tests {
    use super::fingers_after_menu;

    #[test]
    fn a_menu_of_a_finger_s_press_forgets_the_fingers_it_may_have_lost() {
        assert_eq!(fingers_after_menu(true, 1), 0);
        assert_eq!(fingers_after_menu(true, 2), 0);
        assert_eq!(
            fingers_after_menu(false, 1),
            1,
            "a mouse press leaves the count"
        );
        assert_eq!(fingers_after_menu(false, 0), 0);
    }
}
