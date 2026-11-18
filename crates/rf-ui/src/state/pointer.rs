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
//! - **The visit history's menu** (NAV-04): a right click or a long press on 뒤로 or 앞으로 lists
//!   that way's folders of the active pane's tab, nearest first, [`HISTORY_MENU`] at most
//!   ([`AppState::history_entries`]); the key of [`Command::HistoryMenu`](crate::keys::Command)
//!   shows both ways around the current folder ([`MenuShown::History`]). Choosing one goes there in
//!   one move ([`AppState::history_chosen`]), past the folders between, which stay in the history
//!   (`History::go_to`).
//! - **A slow second click** (OPS-02) on the name of the item that is already the only one
//!   selected renames it, as in File Explorer and Finder. [`SlowClick`] is the pure part, which the
//!   controller drives with the times of the presses; [`AppState::row_press`] tells it what a
//!   press is on, and [`AppState::slow_rename`] opens the name window once it says so.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use rf_core::{EntryKind, VPath};

use super::layout::Step;
use super::long_press::Spot;
use super::{AppState, Effect, MenuShown};
use crate::dialogs::{Modal, NameDialog};
use crate::history::{HISTORY_MENU, Way};

/// How far, in logical pixels, a swipe goes before it moves in the history.
pub const SWIPE_DISTANCE: f32 = 80.0;

/// How long after a press a second one is the second click of a double click, for the slow
/// second click: the interval of Slint's own double click (its platform's `click_interval`,
/// which the winit backend leaves at its default), so that the two agree on which second click
/// opens. rf-platform does not read the system's double-click time (yet).
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// The item a slow second click is on: the pane, and the row's name, by which it is found once
/// the rename is due (rows may come and go meanwhile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlowTarget {
    pub side: usize,
    pub name: OsString,
}

/// A left press on a row, as the slow second click needs to know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowPress {
    pub target: SlowTarget,
    /// The press is on the row's name (its text), not on its icon or another column.
    pub on_name: bool,
    /// A modifier key is held (Ctrl, Shift, Alt, the system key): the press changes the
    /// selection.
    pub modified: bool,
    /// The row was the only one selected before the press.
    pub sole: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum SlowPhase {
    Idle,
    /// A press that may be a slow second click, not let go yet.
    Pressed {
        target: SlowTarget,
        at: Spot,
        since: Instant,
    },
    /// The click came; the rename waits for the double-click time to pass, in case a second
    /// click makes it a double click (an open).
    Waiting {
        target: SlowTarget,
        until: Instant,
    },
}

/// The slow second click (OPS-02): a click on the name of the item that is already the only
/// selection, when the double-click time has passed since the press before, renames it once the
/// double-click time has passed again with no other press.
///
/// - A press within the double-click time of the one before is a double click's second click:
///   the row opens ([`AppState::open_row`]), and nothing is renamed.
/// - A press that moves farther than the threshold is a drag ([`SlowClick::moved`]).
/// - A press with a modifier key, on an item that was not the only selection, or off its name
///   (the icon, another column) only selects.
/// - Any other press while the rename waits (the second click of a double click, or a press
///   anywhere else), a key, the wheel, a menu or a dialog drop it ([`SlowClick::cancel`]).
///
/// It knows nothing of the window: the controller hands it the presses with their times.
#[derive(Clone, Debug)]
pub struct SlowClick {
    double_click: Duration,
    threshold: f32,
    /// When the last press on a row came.
    last_press: Option<Instant>,
    phase: SlowPhase,
}

impl SlowClick {
    /// A machine with `double_click` as the double-click time, whose press may move
    /// `threshold_px` before it is a drag.
    pub fn new(double_click: Duration, threshold_px: f32) -> SlowClick {
        SlowClick {
            double_click,
            threshold: threshold_px,
            last_press: None,
            phase: SlowPhase::Idle,
        }
    }

    /// A left press on a row at `at`, `now`, as `press` tells it. A rename that waits is
    /// dropped: this press may be the second click of a double click, or a press elsewhere.
    pub fn press(&mut self, press: RowPress, at: Spot, now: Instant) {
        let slow = self
            .last_press
            .is_none_or(|last| now.saturating_duration_since(last) >= self.double_click);
        self.last_press = Some(now);
        self.phase = if slow && press.on_name && press.sole && !press.modified {
            SlowPhase::Pressed {
                target: press.target,
                at,
                since: now,
            }
        } else {
            SlowPhase::Idle
        };
    }

    /// The pointer is at `at`: a press moved farther than the threshold is a drag.
    pub fn moved(&mut self, at: Spot) {
        if let SlowPhase::Pressed { at: from, .. } = &self.phase
            && from.distance(at) > self.threshold
        {
            self.phase = SlowPhase::Idle;
        }
    }

    /// The press was let go as a click (no drag came of it), `now`: the item to rename now,
    /// when the double-click time has passed since the press already; otherwise the rename
    /// waits for it ([`SlowClick::tick`]).
    pub fn release(&mut self, now: Instant) -> Option<SlowTarget> {
        let SlowPhase::Pressed { target, since, .. } =
            std::mem::replace(&mut self.phase, SlowPhase::Idle)
        else {
            return None;
        };
        let until = since.checked_add(self.double_click).unwrap_or(since);
        if now >= until {
            return Some(target);
        }
        self.phase = SlowPhase::Waiting { target, until };
        None
    }

    /// The timer's look at `now`: the item to rename, once, when the double-click time has
    /// passed with no other press.
    pub fn tick(&mut self, now: Instant) -> Option<SlowTarget> {
        match &self.phase {
            SlowPhase::Waiting { until, .. } if now >= *until => {}
            _ => return None,
        }
        match std::mem::replace(&mut self.phase, SlowPhase::Idle) {
            SlowPhase::Waiting { target, .. } => Some(target),
            _ => None,
        }
    }

    /// When the rename that waits is due; `None` while none waits.
    pub fn waiting_until(&self) -> Option<Instant> {
        match &self.phase {
            SlowPhase::Waiting { until, .. } => Some(*until),
            _ => None,
        }
    }

    /// A key, the wheel, another button, a press off the rows, a menu or a dialog: no rename
    /// comes of the press under way or the click before. The time of the last press stays, so
    /// that a press right after is still the second click of a double click.
    pub fn cancel(&mut self) {
        self.phase = SlowPhase::Idle;
    }
}

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

    /// The folders the visit history's menu lists `way` for the active pane's tab: the nearest
    /// first, [`HISTORY_MENU`] at most.
    pub fn history_entries(&self, way: Way) -> Vec<VPath> {
        self.tab(self.active)
            .history()
            .list(way)
            .take(HISTORY_MENU)
            .cloned()
            .collect()
    }

    /// The key of the visit history's menu: it shows when the active pane's tab has a folder
    /// to go to either way.
    pub(super) fn history_menu(&mut self) -> Vec<Effect> {
        let history = self.tab(self.active).history();
        if history.can_back() || history.can_forward() {
            vec![Effect::ShowMenu(MenuShown::History)]
        } else {
            Vec::new()
        }
    }

    /// Entry `index` of the visit history's menu `way` was chosen (0 the nearest): the active
    /// pane goes there in one move, in a new tab beside a pinned one, and navigation sync makes
    /// the same move in the other pane. Nothing while a window shows over the main one, or past
    /// the entries the menu lists.
    pub fn history_chosen(&mut self, way: Way, index: usize) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting || index >= HISTORY_MENU {
            return Vec::new();
        }
        self.subfolders = None;
        let side = self.active;
        self.go(side, Step::to(way, index))
    }

    /// What a left press on row `row` of pane `side` is to the slow second click, before the
    /// press changes the selection: the row's item, whether it was the only selection, and
    /// what the window says of the press (`on_name`, `modified`). `None` for a row the state
    /// does not have.
    pub fn row_press(
        &self,
        side: usize,
        row: usize,
        on_name: bool,
        modified: bool,
    ) -> Option<RowPress> {
        let tab = self.tab(side);
        let entry = tab.visible().get(row)?;
        let selection = tab.selection();
        Some(RowPress {
            target: SlowTarget {
                side,
                name: entry.name.clone(),
            },
            on_name,
            modified: modified || self.held_modifiers(),
            sole: selection.len() == 1 && selection.is_selected(&entry.name),
        })
    }

    /// The slow second click came due on `target`: its name window opens, when the item is
    /// still in its pane and the only one selected, and nothing else is under way (a window
    /// over the main one, quick look, a drag of another app, the app's end).
    pub fn slow_rename(&mut self, target: &SlowTarget) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting || self.quick_look_open() || self.outside.is_some()
        {
            return Vec::new();
        }
        let tab = self.tab(target.side);
        let selection = tab.selection();
        if selection.len() != 1 || !selection.is_selected(&target.name) {
            return Vec::new();
        }
        let Some(entry) = tab.visible().iter().find(|entry| entry.name == target.name) else {
            return Vec::new();
        };
        let is_dir = entry.kind == EntryKind::Dir;
        let Ok(path) = tab.location().join(&entry.name) else {
            return Vec::new();
        };
        if let Some(dialog) = NameDialog::rename(path, is_dir, target.side) {
            self.open(Modal::Name(dialog));
        }
        Vec::new()
    }

    /// Whether a modifier key is held, as the window last told.
    fn held_modifiers(&self) -> bool {
        let mods = self.drag_mods;
        mods.ctrl || mods.shift || mods.alt || mods.logo
    }
}
