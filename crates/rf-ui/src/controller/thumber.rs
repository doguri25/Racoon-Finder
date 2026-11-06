//! The thumbnails of the icon views (VIEW-09) on the controller's side, and the icon size that
//! Ctrl+wheel and a pinch change (COMP-04).
//!
//! Each pane has a [`ThumbPool`] of its own, started the first time the pane wants a thumbnail
//! (so 자세히 alone starts no thread), each asked with the generation of its tab's listing. What
//! the panes show is asked for at most every [`THUMB_BATCH`]: after the list of icons tells that
//! the items on screen changed (a scroll, another size), and after every redraw (another folder,
//! other rows, another view, the setting). Only the files not yet showing their thumbnail at the
//! size asked go, and nothing goes when that is what was asked last. What the pools made is
//! taken every [`THUMB_HARVEST`] while any of them works, and shown when the tab that asked
//! still shows that listing. With 보기 ▸ 설정 ▸ 썸네일 보이기 off, the pools are let go and the
//! thumbnails shown are forgotten.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, TimerMode};

use super::{Controller, with};
use crate::DropPlace;
use crate::state::{LEFT, RIGHT};
use crate::thumbs::{self, ThumbPool, ThumbProvider, ThumbWant};
use crate::ui_bridge::ThumbStamp;

/// How long the controller gathers the changes of what shows before it asks the pools.
pub const THUMB_BATCH: Duration = Duration::from_millis(100);
/// How often it takes what the pools made while they work.
pub const THUMB_HARVEST: Duration = Duration::from_millis(50);
/// The items a pane is taken to show before its list has told which ones it does.
const FIRST_SCREEN: usize = 64;
/// How far a trackpad's two fingers scroll, in logical pixels, for one icon size with Ctrl.
const WHEEL_STEP_PIXELS: f64 = 60.0;
/// How much a pinch scales for one icon size.
const PINCH_STEP: f64 = 0.25;

/// The thumbnails' part of the controller.
pub(super) struct Thumber {
    /// The app's cache folder, which the disk cache is under; until it is known (and in the
    /// tests of the controller) no pool starts.
    pub(super) cache_dir: RefCell<Option<PathBuf>>,
    pools: [RefCell<Option<ThumbPool>>; 2],
    /// The first and the last item each pane's icons show, as they last told.
    ranges: [Cell<Option<(usize, usize)>>; 2],
    /// What each pane's pool was last asked for, and with which generation.
    asked: [RefCell<(u64, Vec<ThumbWant>)>; 2],
    /// The row of each file asked for, to find it when its thumbnail comes.
    rows: [RefCell<HashMap<PathBuf, usize>>; 2],
    batch: slint::Timer,
    harvest: slint::Timer,
    /// Ctrl (macOS ⌘) is held, and the wheel's and the pinch's way toward the next size.
    zoom: RefCell<ZoomSteps>,
}

impl Thumber {
    pub(super) fn new() -> Thumber {
        Thumber {
            cache_dir: RefCell::new(None),
            pools: [RefCell::new(None), RefCell::new(None)],
            ranges: [Cell::new(None), Cell::new(None)],
            asked: [RefCell::default(), RefCell::default()],
            rows: [RefCell::default(), RefCell::default()],
            batch: slint::Timer::default(),
            harvest: slint::Timer::default(),
            zoom: RefCell::new(ZoomSteps::default()),
        }
    }
}

impl Thumber {
    /// No thumbnail is made or kept from now on: the pools end, none starts again, and the disk
    /// cache is not written (the removal of the app deletes its folder).
    pub(super) fn stop(&self) {
        *self.cache_dir.borrow_mut() = None;
        for pool in &self.pools {
            pool.borrow_mut().take();
        }
        self.batch.stop();
        self.harvest.stop();
    }
}

impl std::fmt::Debug for Thumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumber")
            .field("cache_dir", &self.cache_dir)
            .finish_non_exhaustive()
    }
}

/// What the thumbnail of `want` is for.
fn stamp_of(want: &ThumbWant) -> ThumbStamp {
    ThumbStamp {
        size: want.size,
        len: want.len,
        mtime: want.mtime,
    }
}

/// The system's thumbnails, which the pool guards.
fn system_provider() -> ThumbProvider {
    Arc::new(rf_platform::thumbs::os_thumbnail)
}

impl Controller {
    /// The icons of pane `side` show the items `first..=last` now (`last` -1: none).
    pub(super) fn visible_range(&self, side: usize, first: i32, last: i32) {
        // No item on screen is the empty range (1, 0).
        let range = usize::try_from(first)
            .ok()
            .zip(usize::try_from(last).ok())
            .filter(|(first, last)| first <= last)
            .unwrap_or((1, 0));
        self.thumber.ranges[side].set(Some(range));
        self.schedule_thumbs();
    }

    /// Asks the pools for what the panes show [`THUMB_BATCH`] from now, unless that is asked
    /// already.
    pub(super) fn schedule_thumbs(&self) {
        if !self.thumber.batch.running() {
            self.thumber
                .batch
                .start(TimerMode::SingleShot, THUMB_BATCH, || {
                    with(Controller::ask_thumbs);
                });
        }
    }

    /// Hands each pane's pool the files its icons show that do not show their thumbnail yet.
    fn ask_thumbs(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let scale = window.window().scale_factor();
        let state = self.state.borrow();
        let Some(state) = state.as_ref() else {
            return;
        };
        let thumber = &self.thumber;
        if !state.show_thumbnails() {
            for side in [LEFT, RIGHT] {
                thumber.pools[side].borrow_mut().take();
                *thumber.asked[side].borrow_mut() = (0, Vec::new());
                self.panes[side].clear_thumbs();
            }
            return;
        }
        let mut asked_any = false;
        for side in [LEFT, RIGHT] {
            // A pane that is not shown (the other one has the window alone) asks for nothing.
            if !state.layout().shows(side, state.active()) {
                thumber.pools[side].borrow_mut().take();
                *thumber.asked[side].borrow_mut() = (0, Vec::new());
                thumber.rows[side].borrow_mut().clear();
                continue;
            }
            let tab = state.panes()[side].active_tab();
            let generation = tab.generation();
            let side_px = tab.view().mode.thumb_side();
            let wants = match (side_px, tab.location().as_local()) {
                (Some(side_px), Some(folder)) => {
                    let (first, last) = thumber.ranges[side].get().unwrap_or((0, FIRST_SCREEN - 1));
                    let size = physical(side_px, scale);
                    thumbs::visible_wants(tab.visible(), folder, first, last, size)
                        .into_iter()
                        .filter(|want| !self.panes[side].has_thumb(&want.path, stamp_of(want)))
                        .map(|want| (want, first))
                        .collect::<Vec<_>>()
                }
                _ => Vec::new(),
            };
            let mut rows = thumber.rows[side].borrow_mut();
            rows.clear();
            let wants: Vec<ThumbWant> = wants
                .into_iter()
                .map(|(want, first)| {
                    rows.insert(want.path.clone(), first + want.visible_rank);
                    want
                })
                .collect();
            let mut asked = thumber.asked[side].borrow_mut();
            if asked.0 == generation && asked.1 == wants {
                continue;
            }
            let mut pool = thumber.pools[side].borrow_mut();
            if pool.is_none() {
                let Some(cache_dir) = thumber.cache_dir.borrow().clone() else {
                    continue;
                };
                if wants.is_empty() {
                    continue;
                }
                *pool = Some(ThumbPool::new(
                    thumbs::worker_count(),
                    cache_dir,
                    system_provider(),
                ));
            }
            let Some(pool) = pool.as_ref() else {
                continue;
            };
            pool.want(generation, wants.clone());
            *asked = (generation, wants);
            asked_any = true;
        }
        if asked_any && !thumber.harvest.running() {
            thumber
                .harvest
                .start(TimerMode::Repeated, THUMB_HARVEST, || {
                    with(Controller::harvest_thumbs);
                });
        }
    }

    /// Shows the thumbnails the pools made for the listings the panes still show; stops looking
    /// once every pool is idle.
    fn harvest_thumbs(&self) {
        let scale = self
            .window
            .upgrade()
            .map_or(1.0, |window| window.window().scale_factor());
        let state = self.state.borrow();
        let Some(state) = state.as_ref() else {
            return;
        };
        let mut busy = false;
        for side in [LEFT, RIGHT] {
            let pool = self.thumber.pools[side].borrow();
            let Some(pool) = pool.as_ref() else {
                continue;
            };
            let tab = state.panes()[side].active_tab();
            let generation = tab.generation();
            // A slow result of an older size must not replace the one of the size now.
            let size = tab
                .view()
                .mode
                .thumb_side()
                .map(|side_px| physical(side_px, scale));
            let rows = self.thumber.rows[side].borrow();
            for ready in pool.take_ready() {
                if ready.generation != generation || Some(ready.size) != size {
                    continue;
                }
                if let Some(image) = ready.image {
                    let row = rows.get(&ready.path).copied();
                    let stamp = ThumbStamp {
                        size: ready.size,
                        len: ready.len,
                        mtime: ready.mtime,
                    };
                    self.panes[side].set_thumb(row, &ready.path, stamp, image);
                }
            }
            busy |= !pool.is_idle();
        }
        if !busy {
            self.thumber.harvest.stop();
        }
    }

    /// Ctrl (macOS ⌘) is held now, or not, for the wheel.
    pub(super) fn zoom_key(&self, held: bool) {
        self.thumber.zoom.borrow_mut().held = held;
    }

    /// The wheel turned by `lines` notches, or a trackpad scrolled `pixels` logical pixels (up
    /// positive). With Ctrl (macOS ⌘) held over a pane's list, the icons of that pane step a size
    /// up or down, and the list does not scroll: `true`.
    pub(super) fn zoom_wheel(&self, lines: Option<f64>, pixels: f64) -> bool {
        let steps = {
            let mut zoom = self.thumber.zoom.borrow_mut();
            if !zoom.held {
                return false;
            }
            match lines {
                Some(lines) => ZoomSteps::notches(lines),
                None => zoom.scrolled(pixels),
            }
        };
        let Some(side) = self.pane_under_pointer() else {
            return false;
        };
        self.step_sizes(side, steps);
        true
    }

    /// A pinch (a trackpad's, macOS) scaled by `delta` more (`started`: a new pinch): the icons
    /// of the pane under the pointer, or the active one, step a size every [`PINCH_STEP`].
    pub(super) fn zoom_pinch(&self, delta: f64, started: bool) {
        let steps = self.thumber.zoom.borrow_mut().pinched(delta, started);
        if steps == 0 {
            return;
        }
        let side = self.pane_under_pointer().or_else(|| {
            self.state
                .borrow()
                .as_ref()
                .map(crate::state::AppState::active)
        });
        if let Some(side) = side {
            self.step_sizes(side, steps);
        }
    }

    /// The pane whose list is under the pointer.
    fn pane_under_pointer(&self) -> Option<usize> {
        let window = self.window.upgrade()?;
        let at = self.pointer.get()?;
        let spot = window.invoke_spot_at(at.x, at.y);
        (spot.found && spot.place == DropPlace::List).then_some(super::side(spot.side))
    }

    /// The icons of pane `side` `steps` sizes up (down when negative).
    fn step_sizes(&self, side: usize, steps: i32) {
        for _ in 0..steps.unsigned_abs() {
            self.update(|state| state.step_icon_size(side, steps > 0));
        }
    }
}

/// `side` logical pixels at the window's `scale`, in whole pixels.
fn physical(side: u32, scale: f32) -> u32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a thumbnail's side at any scale a window has is a few hundred pixels"
    )]
    let pixels = (f64::from(side) * scale).round().clamp(1.0, 4096.0) as u32;
    pixels
}

/// What the wheel with Ctrl and a pinch have moved toward the next icon size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ZoomSteps {
    /// Ctrl (macOS ⌘) is held.
    held: bool,
    /// A trackpad's scroll since the last step, in logical pixels.
    pixels: f64,
    /// A pinch's scale since the last step.
    pinch: f64,
}

impl ZoomSteps {
    /// A wheel's notches: a size for each turn, up or down.
    fn notches(lines: f64) -> i32 {
        if lines > 0.0 {
            1
        } else if lines < 0.0 {
            -1
        } else {
            0
        }
    }

    /// A trackpad's scroll by `pixels`: a size every [`WHEEL_STEP_PIXELS`].
    fn scrolled(&mut self, pixels: f64) -> i32 {
        self.pixels += pixels;
        take_steps(&mut self.pixels, WHEEL_STEP_PIXELS)
    }

    /// A pinch by `delta` (a new one when `started`): a size every [`PINCH_STEP`].
    fn pinched(&mut self, delta: f64, started: bool) -> i32 {
        if started {
            self.pinch = 0.0;
        }
        if delta.is_finite() {
            self.pinch += delta;
        }
        take_steps(&mut self.pinch, PINCH_STEP)
    }
}

/// The whole steps of `step` in `amount` (up positive), which keeps what is left over.
fn take_steps(amount: &mut f64, step: f64) -> i32 {
    let mut steps = 0;
    while *amount >= step {
        *amount -= step;
        steps += 1;
    }
    while *amount <= -step {
        *amount += step;
        steps -= 1;
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinch_steps_a_size_every_quarter() {
        let mut zoom = ZoomSteps::default();
        assert_eq!(zoom.pinched(0.1, true), 0);
        assert_eq!(zoom.pinched(0.1, false), 0);
        assert_eq!(zoom.pinched(0.1, false), 1, "0.3 in all");
        assert_eq!(zoom.pinched(0.15, false), 0, "0.05 left over, then 0.2");
        assert_eq!(zoom.pinched(0.06, false), 1);
        assert_eq!(zoom.pinched(-0.6, false), -2);
        // A new pinch starts from nothing.
        assert_eq!(zoom.pinched(-0.2, true), 0);
        assert_eq!(zoom.pinched(0.5, false), 1);
        assert_eq!(zoom.pinched(f64::NAN, false), 0);
    }

    #[test]
    fn the_wheel_steps_a_size_a_notch_or_a_stretch_of_a_trackpad() {
        assert_eq!(ZoomSteps::notches(1.0), 1);
        assert_eq!(ZoomSteps::notches(-3.0), -1);
        assert_eq!(ZoomSteps::notches(0.0), 0);
        let mut zoom = ZoomSteps::default();
        assert_eq!(zoom.scrolled(25.0), 0);
        assert_eq!(zoom.scrolled(40.0), 1);
        assert_eq!(zoom.scrolled(-130.0), -2);
    }

    #[test]
    fn a_thumbnail_is_asked_for_in_the_screen_s_pixels() {
        assert_eq!(physical(96, 1.0), 96);
        assert_eq!(physical(96, 1.5), 144);
        assert_eq!(physical(160, 2.0), 320);
        assert_eq!(physical(48, f32::NAN), 48);
        assert_eq!(physical(48, 0.0), 48);
    }
}
