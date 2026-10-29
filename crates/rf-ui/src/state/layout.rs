//! The layout of the main window, and the commands between its two panes. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The [`Layout`] (LAY-01~03, LAY-06): one pane or two, side by side or one above the other,
//! how the divider shares the room between them, whether the sidebar with the folder tree
//! shows (LAY-03 a and c), and the preview panel's place (LAY-06). The settings keep it.
//!
//! The commands between the panes (LAY-05): swapping their tab sets, opening the active
//! pane's folder in the other one, and navigation sync. The last two act on the other pane, so
//! with one pane they only say they cannot be used, and going to one pane turns sync off. Sync
//! starts off at every start; while it is on, four moves of the active pane are made in the tab
//! the other pane shows too:
//!
//! - opening a folder of the list (Enter, a double click, 열기): the other pane opens the
//!   folder of the same name among its rows, hidden ones too ([`same_folder`]). While the
//!   other pane's listing is on its way, the decision waits for it ([`SyncWait`]); further
//!   folders opened meanwhile are opened there in turn, and Up or Back undoes the last of them;
//! - Up, Back and Forward: the other pane makes the same move from where it is.
//!
//! When the other pane cannot follow (no folder of that name among its rows, a listing that
//! failed, a root to go up from, nothing to go back or forward to), sync turns off and a notice
//! says why; the active pane moves all the same. Any other move (a piece of the path row, a
//! typed path, the tree, a link, another tab) moves the active pane alone, and sync stays on;
//! a move of the other pane drops a decision that waits for its listing.

use std::ffi::{OsStr, OsString};

use rf_core::{Entry, EntryKind, Os};
use rf_vfs::fold_name;

use super::{AppState, Effect, LEFT, RIGHT};
use crate::loader::LoadRequest;
use crate::pane::{LoadStatus, Tab};
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

    /// F12: one pane or two. The pane with the keyboard is the one that stays; sync, which
    /// moves the other one, turns off.
    pub(super) fn toggle_dual(&mut self) -> Vec<Effect> {
        self.layout.dual = !self.layout.dual;
        if !self.layout.dual && self.sync {
            return self.stop_sync(words::sync_off().to_owned());
        }
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
        self.panes.swap(LEFT, RIGHT);
        // A press waiting for its release names a row of the pane it was on.
        self.pending_click = None;
        Vec::new()
    }

    /// The tab the other pane shows goes to the active pane's folder. With one pane, a notice
    /// says it cannot be used.
    pub(super) fn same_folder_in_other(&mut self, side: usize) -> Vec<Effect> {
        if !self.layout.dual {
            return one_pane_only();
        }
        let here = self.tab(side).location().clone();
        self.navigate_shown(RIGHT - side, here)
            .map(Effect::Load)
            .into_iter()
            .collect()
    }

    /// Shift+F9: navigation sync on or off, and a notice that says which. With one pane, a
    /// notice says it cannot be used.
    pub(super) fn toggle_sync(&mut self) -> Vec<Effect> {
        if self.sync {
            return self.stop_sync(words::sync_off().to_owned());
        }
        if !self.layout.dual {
            return one_pane_only();
        }
        self.sync = true;
        vec![Effect::Notice(words::sync_on().to_owned())]
    }

    /// Turns sync off, dropping what it waits to do, with the notice `text`.
    fn stop_sync(&mut self, text: String) -> Vec<Effect> {
        self.sync = false;
        self.sync_wait = None;
        vec![Effect::Notice(text)]
    }

    /// Pane `side` has just opened its folder `name`: with sync on, the other pane opens its
    /// folder of that name, or sync turns off when its rows have none. While the other pane's
    /// listing is on its way, that waits for it ([`AppState::sync_listed`]).
    pub(super) fn sync_open(&mut self, side: usize, name: &OsStr) -> Vec<Effect> {
        if !self.sync {
            return Vec::new();
        }
        let other = RIGHT - side;
        let (generation, status) = {
            let tab = self.tab(other);
            (tab.generation(), tab.status())
        };
        if let Some(wait) = self
            .sync_wait
            .as_mut()
            .filter(|wait| wait.generation == generation)
        {
            wait.names.push(name.to_os_string());
            return Vec::new();
        }
        if status == LoadStatus::Loading {
            self.sync_wait = Some(SyncWait {
                generation,
                names: vec![name.to_os_string()],
            });
            return Vec::new();
        }
        self.sync_enter(other, name, Vec::new())
    }

    /// The listing of request `generation` has come, or failed: when sync waits for it in the
    /// tab a pane shows, that pane opens the folders sync waited to open there, the first now
    /// and the others as their listings come; or sync turns off when it cannot. A tab that no
    /// longer shows, or has moved on, has its wait dropped.
    pub(super) fn sync_listed(&mut self, generation: u64) -> Vec<Effect> {
        let Some(wait) = self.sync_wait.take_if(|wait| wait.generation == generation) else {
            return Vec::new();
        };
        let Some(other) = [LEFT, RIGHT]
            .into_iter()
            .find(|&side| self.tab(side).generation() == generation)
        else {
            return Vec::new();
        };
        let mut names = wait.names.into_iter();
        match names.next() {
            Some(first) if self.sync => self.sync_enter(other, &first, names.collect()),
            _ => Vec::new(),
        }
    }

    /// The tab whose request is `generation` has left the pane it was in, or closed: sync waits
    /// for it no more.
    pub(super) fn forget_sync_wait(&mut self, generation: u64) {
        self.sync_wait.take_if(|wait| wait.generation == generation);
    }

    /// The tab a pane listed again (a rescan) under `generation`, having waited under `old`:
    /// sync's wait for it goes on under the new listing.
    pub(super) fn sync_relisted(&mut self, old: u64, generation: u64) {
        if let Some(wait) = self
            .sync_wait
            .as_mut()
            .filter(|wait| wait.generation == old)
        {
            wait.generation = generation;
        }
    }

    /// Pane `other`, whose rows are in, opens its folder `name`, and waits to open `then` in
    /// turn; or sync turns off when its rows have no such folder, or its listing failed.
    fn sync_enter(&mut self, other: usize, name: &OsStr, then: Vec<OsString>) -> Vec<Effect> {
        let tab = self.tab(other);
        if let LoadStatus::Failed(_) = tab.status() {
            return self.stop_sync(words::sync_cannot_follow().to_owned());
        }
        let folder = same_folder(tab.all(), name, Os::CURRENT)
            .and_then(|found| tab.location().join(found).ok());
        let Some(folder) = folder else {
            return self.stop_sync(words::sync_no_folder(&name.to_string_lossy()));
        };
        // A pinned tab there stays: a tab beside it goes in, and is the one sync follows.
        let Some(request) = self.navigate_shown(other, folder) else {
            return Vec::new();
        };
        if !then.is_empty() {
            self.sync_wait = Some(SyncWait {
                generation: request.generation,
                names: then,
            });
        }
        vec![Effect::Load(request)]
    }

    /// Pane `side` has just made the move `step`: with sync on, the other pane makes it too, or
    /// sync turns off when it cannot. While the other pane waits to open folders, Up or Back
    /// undoes the last of them there instead, which leaves it where it is.
    pub(super) fn sync_step(&mut self, side: usize, step: Step) -> Vec<Effect> {
        if !self.sync {
            return Vec::new();
        }
        let other = RIGHT - side;
        let generation = self.tab(other).generation();
        // A wait for another listing is stale: that tab has moved on.
        if let Some(wait) = self.sync_wait.take()
            && wait.generation == generation
        {
            let mut names = wait.names;
            if step == Step::Forward || names.pop().is_none() {
                return self.stop_sync(words::sync_cannot_follow().to_owned());
            }
            if !names.is_empty() {
                self.sync_wait = Some(SyncWait { generation, names });
            }
            return Vec::new();
        }
        match self.move_shown(other, |tab| step.run(tab)) {
            Some(request) => vec![Effect::Load(request)],
            None => self.stop_sync(words::sync_cannot_follow().to_owned()),
        }
    }
}

/// A move of a tab that navigation sync makes in the other pane too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Up,
    Back,
    Forward,
}

impl Step {
    /// Makes the move in `tab`, if it can go; the request for the listing.
    pub(super) fn run(self, tab: &mut Tab) -> Option<LoadRequest> {
        match self {
            Step::Up => tab.go_up(),
            Step::Back => tab.go_back(),
            Step::Forward => tab.go_forward(),
        }
    }
}

/// The folders navigation sync is to open, in turn, in the tab whose listing `generation` is
/// on its way (the other pane's): the decision waits for its rows.
#[derive(Debug)]
pub(super) struct SyncWait {
    generation: u64,
    names: Vec<OsString>,
}

/// The notice for a command for the other pane while one pane shows.
fn one_pane_only() -> Vec<Effect> {
    vec![Effect::Notice(words::one_pane_only().to_owned())]
}

/// The folder among `entries` that navigation sync takes for `name` on `os`: the one of that
/// very name; or else, on Windows and macOS, whose file systems mostly ignore case, the only
/// one whose name differs from it in case alone (folded as `rf_vfs` folds names). `None` when
/// there is none, or several.
pub(super) fn same_folder<'a>(entries: &'a [Entry], name: &OsStr, os: Os) -> Option<&'a OsStr> {
    let folders = entries.iter().filter(|entry| entry.kind == EntryKind::Dir);
    if let Some(exact) = folders.clone().find(|entry| entry.name == name) {
        return Some(&exact.name);
    }
    if os == Os::Other {
        return None;
    }
    let wanted = fold_name(name);
    let mut alike = folders.filter(|entry| fold_name(&entry.name) == wanted);
    match (alike.next(), alike.next()) {
        (Some(only), None) => Some(&only.name),
        _ => None,
    }
}
