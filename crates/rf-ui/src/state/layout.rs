//! The layout of the main window, and the commands between its two panes. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The [`Layout`] (LAY-01~03, LAY-06): one pane or two, side by side or one above the other,
//! how the divider shares the room between them, whether the sidebar with the folder tree
//! shows (LAY-03 a and c), and the preview panel's place (LAY-06). The settings keep it.
//!
//! The commands between the panes (LAY-05): swapping their tab sets, opening the active
//! pane's folder in the other one, and navigation sync. Sync starts off at every start; while
//! it is on, four moves of the active pane are made in the tab the other pane shows too:
//!
//! - opening a folder of the list (Enter, a double click, 열기): the other pane opens the
//!   folder of the same name (the same spelling) among its rows, hidden ones too;
//! - Up, Back and Forward: the other pane makes the same move from where it is.
//!
//! When the other pane cannot follow (no folder of that name among its rows, a root to go up
//! from, nothing to go back or forward to), sync turns off and a notice says why; the active
//! pane moves all the same. Any other move (a piece of the path row, a typed path, the tree, a
//! link, another tab) moves the active pane alone, and sync stays on.

use std::ffi::OsStr;

use rf_core::EntryKind;

use super::{AppState, Effect, RIGHT};
use crate::loader::LoadRequest;
use crate::pane::Tab;
use crate::words;

/// The least share of the room the divider leaves the first pane (the left or top one).
pub const MIN_SHARE: f64 = 0.1;
/// The most share of the room the divider gives the first pane.
pub const MAX_SHARE: f64 = 0.9;
/// An even split: a double click on the divider, and the 보기 menu, go back to it.
pub const EVEN_SHARE: f64 = 0.5;

/// How the main window lays out its panes and side panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate choice of the layout, not states of one machine"
)]
pub struct Layout {
    /// Two panes (LAY-01). With one, only the pane with the keyboard shows; the other keeps
    /// its tabs, and is the picker's "반대편 패널".
    pub dual: bool,
    /// The two panes one above the other, the left one on top, rather than side by side
    /// (LAY-02).
    pub stacked: bool,
    /// The sidebar, with the places and the folder tree that follows the active pane, shows
    /// (LAY-03 a); or not (LAY-03 c).
    pub tree: bool,
    /// The preview panel's place shows, at the right end (LAY-06).
    pub preview: bool,
    /// The first pane's share of the room both have, in thousandths: from 100 to 900.
    thousandths: u16,
}

impl Default for Layout {
    /// Two panes side by side, split evenly, the tree shown and the preview panel not.
    fn default() -> Layout {
        Layout {
            dual: true,
            stacked: false,
            tree: true,
            preview: false,
            thousandths: 500,
        }
    }
}

impl Layout {
    /// The first pane's share of the room both have, from [`MIN_SHARE`] to [`MAX_SHARE`].
    pub fn share(&self) -> f32 {
        f32::from(self.thousandths) / 1000.0
    }

    /// The share in thousandths, from 100 to 900: as the settings keep it, a decimal with no
    /// more than three places.
    pub fn thousandths(&self) -> u16 {
        self.thousandths
    }

    /// Gives the first pane `share` of the room, to the thousandth, kept between [`MIN_SHARE`]
    /// and [`MAX_SHARE`]. A share that is no number changes nothing.
    pub fn set_share(&mut self, share: f64) {
        if share.is_nan() {
            return;
        }
        let kept = (share.clamp(MIN_SHARE, MAX_SHARE) * 1000.0).round();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a share kept between 0.1 and 0.9 makes 100 to 900"
        )]
        let thousandths = kept as u16;
        self.thousandths = thousandths;
    }

    /// Whether pane `side` shows while pane `active` has the keyboard: both do with two panes,
    /// the active one alone with one.
    pub fn shows(&self, side: usize, active: usize) -> bool {
        self.dual || side == active
    }
}

impl AppState {
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Whether navigation sync is on.
    pub fn syncing(&self) -> bool {
        self.sync
    }

    /// The divider was dragged to `share` of the room, or double-clicked ([`EVEN_SHARE`]).
    pub fn set_split_share(&mut self, share: f64) {
        self.layout.set_share(share);
    }

    /// F12: one pane or two. The pane with the keyboard is the one that stays.
    pub(super) fn toggle_dual(&mut self) -> Vec<Effect> {
        self.layout.dual = !self.layout.dual;
        Vec::new()
    }

    /// Shift+F12: side by side, or one above the other.
    pub(super) fn toggle_split(&mut self) -> Vec<Effect> {
        self.layout.stacked = !self.layout.stacked;
        Vec::new()
    }

    /// Shows or hides the sidebar. Hidden, its tree stops following the active pane
    /// ([`AppState::follow_active`]), and gives the keys back to the panes if it had them;
    /// shown again, it finds the active pane's folder.
    pub(super) fn toggle_tree(&mut self) -> Vec<Effect> {
        self.layout.tree = !self.layout.tree;
        if self.layout.tree {
            Vec::new()
        } else {
            vec![Effect::FocusPanes]
        }
    }

    /// Shows or hides the preview panel's place.
    pub(super) fn toggle_preview(&mut self) -> Vec<Effect> {
        self.layout.preview = !self.layout.preview;
        Vec::new()
    }

    /// The two panes trade their tab sets, each tab with its folder, rows, history and
    /// listing; the keyboard stays on its side, which shows the other set now. With one pane,
    /// it shows the hidden pane's tabs.
    pub(super) fn swap_panes(&mut self) -> Vec<Effect> {
        self.panes.swap(super::LEFT, RIGHT);
        // A press waiting for its release names a row of the pane it was on.
        self.pending_click = None;
        Vec::new()
    }

    /// The tab the other pane shows goes to the active pane's folder.
    pub(super) fn same_folder_in_other(&mut self, side: usize) -> Vec<Effect> {
        let here = self.tab(side).location().clone();
        vec![Effect::Load(self.tab_mut(RIGHT - side).navigate(here))]
    }

    /// Shift+F9: navigation sync on or off, and a notice that says which.
    pub(super) fn toggle_sync(&mut self) -> Vec<Effect> {
        self.sync = !self.sync;
        let text = if self.sync {
            words::sync_on()
        } else {
            words::sync_off()
        };
        vec![Effect::Notice(text.to_owned())]
    }

    /// Pane `side` has just opened its folder `name`: with sync on, the other pane opens its
    /// folder of that name, or sync turns off when its rows have none.
    pub(super) fn sync_open(&mut self, side: usize, name: &OsStr) -> Vec<Effect> {
        if !self.sync {
            return Vec::new();
        }
        let other = self.tab_mut(RIGHT - side);
        let has = other
            .all()
            .iter()
            .any(|entry| entry.kind == EntryKind::Dir && entry.name == name);
        if let (true, Ok(folder)) = (has, other.location().join(name)) {
            return vec![Effect::Load(other.navigate(folder))];
        }
        self.sync = false;
        vec![Effect::Notice(words::sync_no_folder(
            &name.to_string_lossy(),
        ))]
    }

    /// Pane `side` has just made the move `step` (Up, Back, Forward): with sync on, the other
    /// pane makes it too, or sync turns off when it cannot.
    pub(super) fn sync_step(
        &mut self,
        side: usize,
        step: fn(&mut Tab) -> Option<LoadRequest>,
    ) -> Vec<Effect> {
        if !self.sync {
            return Vec::new();
        }
        if let Some(request) = step(self.tab_mut(RIGHT - side)) {
            return vec![Effect::Load(request)];
        }
        self.sync = false;
        vec![Effect::Notice(words::sync_cannot_follow().to_owned())]
    }
}
