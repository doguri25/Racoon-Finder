//! The layout of the main window, and the commands between its two panes. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The [`Layout`] (LAY-01~03, LAY-06, LAY-09, LAY-10): one pane or two, side by side or one
//! above the other, how the divider shares the room between them, how the sidebar with the
//! folder tree shows ([`TreeMode`], LAY-03: one tree for the active pane, a tree beside each
//! pane, or none), the preview panel's place (LAY-06), whether the job panel shows under the
//! panes or in a window of its own (LAY-09), and whether the toolbar shows (LAY-10). The
//! settings keep it.
//!
//! Where the window opens (LAY-07): [`placement`] puts the place the settings kept for it on the
//! screens there are now.
//!
//! The commands between the panes (LAY-05): swapping their tab sets, opening the active
//! pane's folder in the other one, and navigation sync. The last two act on the other pane, so
//! with one pane they only say they cannot be used, and going to one pane turns sync off. Sync
//! starts off at every start; while it is on, four moves of the active pane are made in the tab
//! the other pane shows too:
//!
//! - opening a folder of the list (Enter, a double click, 열기): the other pane opens the
//!   folder of the same name among its rows, hidden ones too ([`same_folder`]). While the
//!   other pane's listing is on its way, the decision waits for it ([`SyncWait`]), through a
//!   reload of that listing too; further folders opened meanwhile are opened there in turn;
//! - Up, Back and Forward: the other pane makes the same move from where it is. While folders
//!   wait to be opened there, Back takes the last of them off instead, and Forward puts back the
//!   last one Back took off; Back past the first of them, and Up (a move to the parent, which
//!   the history keeps as one), turn sync off, as the other pane's history is not in step there.
//!   So does the waiting tab's folder vanishing, when folders still waited to be opened there.
//!   Any other move of the active pane while a decision waits leaves the waiting folders as they
//!   are, as it would leave the other pane where it is.
//!
//! When the other pane cannot follow (no folder of that name among its rows, a listing that
//! failed, a root to go up from, nothing to go back or forward to), sync turns off and a notice
//! says why; the active pane moves all the same. Any other move (a piece of the path row, a
//! folder of the address bar's lists, a typed path, the tree, a link, another tab) moves the
//! active pane alone, and sync stays on;
//! a move of the other pane drops a decision that waits for its listing, and a swap of the
//! panes turns sync off while one waits.

use std::ffi::{OsStr, OsString};

use rf_core::{Entry, EntryKind, Os};
use rf_vfs::{NameFolding, fold_name};

use super::{AppState, Effect, LEFT, RIGHT, load};
use crate::history::Way;
use crate::loader::LoadRequest;
use crate::pane::{LoadStatus, Tab};
use crate::words;

/// The least share of the room the divider leaves the first pane (the left or top one).
pub const MIN_SHARE: f64 = 0.1;
/// The most share of the room the divider gives the first pane.
pub const MAX_SHARE: f64 = 0.9;
/// An even split: a double click on the divider, and the 보기 menu, go back to it.
pub const EVEN_SHARE: f64 = 0.5;

/// How the folder tree shows (LAY-03), with the sidebar it is in: the places (the favorites,
/// the recent folders, the drives) and the folders open below them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TreeMode {
    /// One tree, at the window's left, that follows the pane with the keyboard (LAY-03 a).
    #[default]
    Shared,
    /// A tree beside each pane that follows that pane alone (LAY-03 b): the left pane's is
    /// tree 0, the right pane's tree 1, each opened and closed on its own. With one pane, the
    /// tree of the pane that shows.
    PerPane,
    /// No tree, and no sidebar (LAY-03 c).
    Hidden,
}

impl TreeMode {
    /// This mode, where it shows a tree; the shared tree for none.
    #[must_use]
    pub fn shown(self) -> TreeMode {
        match self {
            TreeMode::Hidden => TreeMode::Shared,
            shown => shown,
        }
    }
}

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
    /// How the folder tree shows (LAY-03).
    pub tree: TreeMode,
    /// The preview panel's place shows, at the right end (LAY-06).
    pub preview: bool,
    /// The job panel shows in a window of its own rather than under the panes (LAY-09).
    pub jobs_window: bool,
    /// The toolbar shows, the row of buttons over the panes (LAY-10). Hidden, the panes' own
    /// path rows (the address bars) stay.
    pub toolbar: bool,
    /// The first pane's share of the room both have, in thousandths: from 100 to 900.
    thousandths: u16,
}

impl Default for Layout {
    /// Two panes side by side, split evenly, the shared tree shown, the preview panel not, the
    /// job panel under the panes, and the toolbar.
    fn default() -> Layout {
        Layout {
            dual: true,
            stacked: false,
            tree: TreeMode::Shared,
            preview: false,
            jobs_window: false,
            toolbar: true,
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

    /// Whether tree `tree` shows while pane `active` has the keyboard: tree 0 alone as the
    /// shared one, the tree of each pane that shows with a tree for each, none while hidden.
    pub fn tree_shows(&self, tree: usize, active: usize) -> bool {
        match self.tree {
            TreeMode::Shared => tree == LEFT,
            TreeMode::PerPane => self.shows(tree, active),
            TreeMode::Hidden => false,
        }
    }

    /// The pane tree `tree` follows while pane `active` has the keyboard: the active one for
    /// the shared tree, its own pane for a tree of each pane.
    pub fn tree_pane(&self, tree: usize, active: usize) -> usize {
        match self.tree {
            TreeMode::PerPane => tree.min(RIGHT),
            TreeMode::Shared | TreeMode::Hidden => active,
        }
    }

    /// The trees that show while pane `active` has the keyboard, in the order F6 visits them:
    /// the left one first.
    pub fn shown_trees(&self, active: usize) -> Vec<usize> {
        [LEFT, RIGHT]
            .into_iter()
            .filter(|&tree| self.tree_shows(tree, active))
            .collect()
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

    /// 보기 ▸ 작업 패널 ▸ 별도 창 (LAY-09): the job panel shows in a window of its own
    /// (`in_window`), or under the panes again, which the close button of that window asks for
    /// too. Either place shows the same jobs. The keys the panel under the panes had go back to
    /// the panes as it leaves (the window does that, as for a panel whose last job ended).
    pub fn set_jobs_window(&mut self, in_window: bool) {
        self.layout.jobs_window = in_window;
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

    /// Ctrl+B (macOS ⌃⌘S): hides the sidebar, or shows it again in the mode it showed in last
    /// (the shared tree when it has not shown since the start). See [`AppState::set_tree_mode`].
    pub(super) fn toggle_tree(&mut self) -> Vec<Effect> {
        let mode = match self.layout.tree {
            TreeMode::Hidden => self.tree_back,
            TreeMode::Shared | TreeMode::PerPane => TreeMode::Hidden,
        };
        self.set_tree_mode(mode)
    }

    /// 보기 ▸ 폴더 트리 ▸ 공유 트리, 패널별 트리 or 트리 숨김 (LAY-03): the sidebar shows in
    /// `mode`. A tree that no longer shows stops following its pane
    /// ([`AppState::follow_active`]); hidden, the sidebar gives the keys back to the panes if
    /// one of its trees had them (the window does the same for a tree of a pane that goes,
    /// `TreeBox.gone`). Each tree that shows from now on finds its pane's folder, and keeps
    /// what is open in it: the left pane's tree is the shared one.
    pub fn set_tree_mode(&mut self, mode: TreeMode) -> Vec<Effect> {
        self.layout.tree = mode;
        if mode == TreeMode::Hidden {
            return vec![Effect::FocusPanes];
        }
        self.tree_back = mode;
        Vec::new()
    }

    /// 보기 ▸ 툴바 (LAY-10): the toolbar shows, or not. The panes keep their path rows (the
    /// address bars) either way, and every button of the toolbar has its key.
    pub fn toggle_toolbar(&mut self) -> Vec<Effect> {
        self.layout.toolbar = !self.layout.toolbar;
        Vec::new()
    }

    /// Ctrl+F (macOS ⌘F), F3, Ctrl+E: the toolbar's filter box (이 폴더에서 거르기) takes the
    /// keys. A hidden toolbar shows again for it, as Finder's toolbar does for its search field:
    /// the box is in it.
    pub(super) fn focus_filter(&mut self) -> Vec<Effect> {
        self.layout.toolbar = true;
        vec![Effect::FocusFilter]
    }

    /// Shows or hides the preview panel. Turned on in a window too narrow for it, a notice says
    /// why it does not show, each time: the user asked for it (the window hiding it by itself
    /// says so once, [`AppState::set_preview_cramped`]).
    pub(super) fn toggle_preview(&mut self) -> Vec<Effect> {
        self.layout.preview = !self.layout.preview;
        self.tell_cramped(true)
    }

    /// The two panes trade their tab sets, each tab with its folder, rows, history and
    /// listing; the keyboard stays on its side, which shows the other set now. With one pane,
    /// it shows the hidden pane's tabs.
    pub(super) fn swap_panes(&mut self) -> Vec<Effect> {
        self.panes.swap(LEFT, RIGHT);
        // A press waiting for its release names a row of the pane it was on.
        self.pending_click = None;
        // What sync waited to do in the other pane would now be the active one's. A wait for a
        // tab that has moved on is no longer anything.
        let Some(wait) = self.sync_wait.take() else {
            return Vec::new();
        };
        let live = [LEFT, RIGHT]
            .into_iter()
            .any(|side| self.tab(side).generation() == wait.generation);
        if live {
            return self.stop_sync(words::sync_off_by_swap().to_owned());
        }
        Vec::new()
    }

    /// The tab the other pane shows goes to the active pane's folder. With one pane, a notice
    /// says it cannot be used.
    pub(super) fn same_folder_in_other(&mut self, side: usize) -> Vec<Effect> {
        if !self.layout.dual {
            return one_pane_only();
        }
        let here = self.tab(side).location().clone();
        load(self.navigate_shown(RIGHT - side, here))
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
        let mut wait = self
            .sync_wait
            .take()
            .filter(|wait| wait.generation == generation);
        if wait.is_none() && status != LoadStatus::Loading {
            return self.sync_enter(other, name, SyncWait::default());
        }
        let waiting = wait.get_or_insert_with(|| SyncWait {
            generation,
            ..SyncWait::default()
        });
        waiting.names.push(name.to_os_string());
        // Opening a folder ends the active pane's way forward.
        waiting.undone.clear();
        self.sync_wait = wait;
        self.sync_due(other)
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
        if !self.sync {
            return Vec::new();
        }
        self.sync_wait = Some(wait);
        self.sync_due(other)
    }

    /// The tab whose request is `generation` has left the pane it was in, or closed: sync waits
    /// for it no more.
    pub(super) fn forget_sync_wait(&mut self, generation: u64) {
        self.sync_wait.take_if(|wait| wait.generation == generation);
    }

    /// The folder of the tab whose request is `generation` is gone, and the tab goes elsewhere:
    /// sync waits for it no more. `true` when folders still waited to be opened there (not only
    /// ones Back took off), which the other pane cannot follow any more.
    pub(super) fn sync_wait_vanished(&mut self, generation: u64) -> bool {
        self.sync_wait
            .take_if(|wait| wait.generation == generation)
            .is_some_and(|wait| self.sync && !wait.names.is_empty())
    }

    /// The tab a pane listed again under `generation`, having waited under `old` (a rescan, or
    /// a reload after its watch ended): sync's wait for it goes on under the new listing.
    pub(super) fn sync_relisted(&mut self, old: u64, generation: u64) {
        if let Some(wait) = self
            .sync_wait
            .as_mut()
            .filter(|wait| wait.generation == old)
        {
            wait.generation = generation;
        }
    }

    /// Pane `other` opens the first folder sync waits to open there, once its rows are in
    /// (they are when its listing is not on its way); nothing while they are not, or while no
    /// folder waits. The wait stays meanwhile, and so does one that keeps only folders Back
    /// took off, for Forward.
    fn sync_due(&mut self, other: usize) -> Vec<Effect> {
        let Some(mut wait) = self.sync_wait.take() else {
            return Vec::new();
        };
        if wait.names.is_empty() || self.tab(other).status() == LoadStatus::Loading {
            self.sync_wait = (!wait.names.is_empty() || !wait.undone.is_empty()).then_some(wait);
            return Vec::new();
        }
        let first = wait.names.remove(0);
        self.sync_enter(other, &first, wait)
    }

    /// Pane `other`, whose rows are in, opens its folder `name`, and goes on to wait for the
    /// folders of `then` (any generation it has is replaced by that of the new listing); or
    /// sync turns off when its rows have no such folder, or its listing failed.
    fn sync_enter(&mut self, other: usize, name: &OsStr, mut then: SyncWait) -> Vec<Effect> {
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
        if !then.names.is_empty() || !then.undone.is_empty() {
            then.generation = request.generation;
            self.sync_wait = Some(then);
        }
        vec![Effect::Load(request)]
    }

    /// Pane `side` has just made the move `step`: with sync on, the other pane makes it too, or
    /// sync turns off when it cannot. While folders wait to be opened in the other pane, Back
    /// takes the last of them off instead and keeps it for Forward, which leaves that pane where
    /// it is, and Forward puts back the last one Back took off. Up while folders wait, Back past
    /// the first of them, or a Forward with nothing to put back, turns sync off: the other
    /// pane's history is not in step there (Up is a move of its own in the active pane's
    /// history, which the other pane, short of the folders it waits to open, cannot make).
    pub(super) fn sync_step(&mut self, side: usize, step: Step) -> Vec<Effect> {
        if !self.sync {
            return Vec::new();
        }
        let other = RIGHT - side;
        let generation = self.tab(other).generation();
        // A wait for another listing is stale: that tab has moved on.
        if let Some(mut wait) = self.sync_wait.take()
            && wait.generation == generation
        {
            match step {
                Step::Back if !wait.names.is_empty() => {
                    wait.undone.extend(wait.names.pop());
                    self.sync_wait = Some(wait);
                    return self.sync_due(other);
                }
                Step::Up if !wait.names.is_empty() => {
                    return self.stop_sync(words::sync_cannot_follow().to_owned());
                }
                Step::Forward if !wait.undone.is_empty() => {
                    wait.names.extend(wait.undone.pop());
                    self.sync_wait = Some(wait);
                    return self.sync_due(other);
                }
                // Up from where the wait began: the other pane is in step there.
                Step::Up => {}
                // A jump of the visit history's menu past folders the other pane still waits
                // to open: its history is not in step there.
                Step::Back | Step::Forward | Step::Jump(..) => {
                    return self.stop_sync(words::sync_cannot_follow().to_owned());
                }
            }
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
    /// To the folder that many places along the history that way, from the visit history's
    /// menu (NAV-04): 1 or more (0 is [`Step::Back`] or [`Step::Forward`]).
    Jump(Way, usize),
}

impl Step {
    /// Makes the move in `tab`, if it can go; the request for the listing.
    pub(super) fn run(self, tab: &mut Tab) -> Option<LoadRequest> {
        match self {
            Step::Up => tab.go_up(),
            Step::Back => tab.go_back(),
            Step::Forward => tab.go_forward(),
            Step::Jump(way, index) => tab.go_to(way, index),
        }
    }

    /// The move to the folder `index` places along the history `way`: a step for the nearest
    /// one, so that navigation sync follows it as it follows the keys.
    pub(super) fn to(way: Way, index: usize) -> Step {
        match (way, index) {
            (Way::Back, 0) => Step::Back,
            (Way::Forward, 0) => Step::Forward,
            _ => Step::Jump(way, index),
        }
    }
}

/// What navigation sync waits to do in the other pane's tab, whose listing `generation` is on
/// its way or came last: the folders to open there in turn (the decision waits for its rows),
/// and those Back took off them, which Forward puts back, the last one last.
#[derive(Debug, Default)]
pub(super) struct SyncWait {
    generation: u64,
    names: Vec<OsString>,
    undone: Vec<OsString>,
}

/// The notice for a command for the other pane while one pane shows.
fn one_pane_only() -> Vec<Effect> {
    vec![Effect::Notice(words::one_pane_only().to_owned())]
}

/// A screen of the desktop as the window system tells it (LAY-07): its top left corner on the
/// desktop and its size, in physical pixels, and how many physical pixels a logical one takes
/// there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

/// Where the window was when the app last ended, as the settings keep it (LAY-07): its outer
/// top left corner on the desktop in physical pixels (`None` before the first save, and where
/// the window system does not tell it, as on Wayland), its size in logical pixels (the last
/// one it had while neither maximized nor full screen, kept between `settings::MIN_WINDOW` and
/// `settings::MAX_WINDOW`), and whether it was maximized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SavedPlace {
    pub position: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub maximized: bool,
}

/// Where the window opens ([`placement`]): its outer top left corner (`None`: where the window
/// system puts a new window), its size in logical pixels (`None`: the window's own), and
/// whether it opens maximized, on the screen of that corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub position: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub maximized: bool,
}

/// The size the main window opens at without a saved one, in logical pixels (its
/// `start-width` and `start-height`).
pub const DEFAULT_WINDOW: (u32, u32) = (1280, 800);

/// How much of the window, in logical pixels across and down, has to be on a screen for the
/// window to count as on it: enough of its title bar to take hold of and drag.
pub const ON_SCREEN: (u32, u32) = (64, 32);

/// Where the window opens on `screens` (the first one the system's main screen), from where it
/// was (`saved`), as LAY-07 has it:
///
/// - on a screen, it opens where it was; partly off it (by more than all but [`ON_SCREEN`] of
///   it), it is moved onto the screen that holds most of it, as little as it takes to be whole
///   there;
/// - off every screen (the screen it was on is gone), or nowhere known, it opens where the
///   window system puts a new window;
/// - larger than the screen it opens on, it is made as large as that screen, but never smaller
///   than `settings::MIN_WINDOW`;
/// - maximized, it opens maximized on that screen (its size the one it goes back to).
///
/// Without a screen the system tells of, the place is as saved: nothing to check it against.
/// Pure: it asks the system nothing.
pub fn placement(saved: SavedPlace, screens: &[Screen]) -> Placement {
    let Some(main) = screens.first() else {
        return Placement {
            position: saved.position,
            size: saved.size,
            maximized: saved.maximized,
        };
    };
    let wanted = saved.size.unwrap_or(DEFAULT_WINDOW);
    let on = saved.position.and_then(|corner| {
        screens
            .iter()
            .filter_map(|screen| {
                let (across, down) = overlap(corner, physical(wanted, screen.scale), screen);
                let least = physical(ON_SCREEN, screen.scale);
                (across >= least.0 && down >= least.1)
                    .then(|| (u64::from(across) * u64::from(down), screen))
            })
            .max_by_key(|&(area, _)| area)
            .map(|(_, screen)| screen)
    });
    let (position, screen) = match (saved.position, on) {
        (Some(corner), Some(screen)) => {
            let size = physical(fit(wanted, screen), screen.scale);
            (Some(inside(corner, size, screen)), screen)
        }
        _ => (None, main),
    };
    Placement {
        position,
        size: Some(fit(wanted, screen)),
        maximized: saved.maximized,
    }
}

/// `size` (logical) made no larger than `screen`, and no smaller than the window's least.
fn fit(size: (u32, u32), screen: &Screen) -> (u32, u32) {
    let least = crate::settings::MIN_WINDOW;
    let room = logical((screen.width, screen.height), screen.scale);
    (
        size.0.min(room.0).max(least.0),
        size.1.min(room.1).max(least.1),
    )
}

/// The top left corner `corner` of a window of `size` (physical) moved as little as it takes
/// for the window to be whole on `screen`; at the screen's corner when it is larger.
fn inside(corner: (i32, i32), size: (u32, u32), screen: &Screen) -> (i32, i32) {
    let along = |at: i32, length: u32, start: i32, room: u32| {
        let end = i64::from(start) + i64::from(room) - i64::from(length);
        let end = i32::try_from(end.max(i64::from(start))).unwrap_or(start);
        at.clamp(start, end)
    };
    (
        along(corner.0, size.0, screen.x, screen.width),
        along(corner.1, size.1, screen.y, screen.height),
    )
}

/// How much of a window at `corner` of `size` (physical) is on `screen`, across and down.
fn overlap(corner: (i32, i32), size: (u32, u32), screen: &Screen) -> (u32, u32) {
    let span = |at: i32, length: u32, start: i32, room: u32| {
        let low = i64::from(at).max(i64::from(start));
        let high = (i64::from(at) + i64::from(length)).min(i64::from(start) + i64::from(room));
        u32::try_from((high - low).max(0)).unwrap_or(u32::MAX)
    };
    (
        span(corner.0, size.0, screen.x, screen.width),
        span(corner.1, size.1, screen.y, screen.height),
    )
}

/// `size` in logical pixels as physical ones on a screen of `scale`.
fn physical(size: (u32, u32), scale: f64) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let one = |length: u32| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a length of a window on a screen, kept in u32 by the min"
        )]
        let pixels = (f64::from(length) * scale).round().min(f64::from(u32::MAX)) as u32;
        pixels
    };
    (one(size.0), one(size.1))
}

/// `size` in physical pixels as logical ones on a screen of `scale`.
fn logical(size: (u32, u32), scale: f64) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let one = |length: u32| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a screen's length over a scale above 0, kept in u32 by the min"
        )]
        let pixels = (f64::from(length) / scale).floor().min(f64::from(u32::MAX)) as u32;
        pixels
    };
    (one(size.0), one(size.1))
}

/// The folder among `entries` that navigation sync takes for `name` on `os`: the one of that
/// very name; or else, on Windows and macOS, whose file systems mostly ignore case, the only
/// one whose name is the same name there ([`rf_vfs::fold_name`] by the rule of `os`): it
/// differs in case alone, or on macOS also in its Unicode form (NFC or NFD, which APFS takes
/// for one name). `None` when there is none, or several.
pub(super) fn same_folder<'a>(entries: &'a [Entry], name: &OsStr, os: Os) -> Option<&'a OsStr> {
    let folders = entries.iter().filter(|entry| entry.kind == EntryKind::Dir);
    if let Some(exact) = folders.clone().find(|entry| entry.name == name) {
        return Some(&exact.name);
    }
    if os == Os::Other {
        return None;
    }
    let folding = NameFolding::of(os);
    let wanted = fold_name(name, folding);
    let mut alike = folders.filter(|entry| fold_name(&entry.name, folding) == wanted);
    match (alike.next(), alike.next()) {
        (Some(only), None) => Some(&only.name),
        _ => None,
    }
}
