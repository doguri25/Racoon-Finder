//! The main window's state, and what its keys, clicks and the loader's events do to it.
//!
//! [`AppState`] is plain data on the UI thread: the two panes and their tabs (module `tabs`
//! for duplicating, reopening, reordering, pinning and moving them), which one has the keyboard,
//! whether hidden items show, how the window lays them out ([`Layout`]) and whether navigation
//! sync is on, the folder tree of the sidebar, the jobs of the job panel, the
//! window that shows over the main window (a [`Modal`]), the destination picker's folder
//! tree and recent targets ([`AppState::shown_picker`]), the items a drag of rows carries
//! ([`AppState::drag`]), and the address bar's lists (module `address`: a folder's subfolders
//! under the `›` of a piece of the path row, and the path field's completions). Its methods
//! change it and return the [`Effect`]s that need a worker
//! thread, the job engine or the screen, which the
//! [`Controller`](crate::controller::Controller) carries out: a listing to start, a link to
//! follow or a vanished folder's nearest parent to find, a file to open, a job to submit or a
//! question to answer, a notice, a row to scroll to. Keeping the two apart lets the tests
//! drive the state without a window.
//!
//! Every change of a file is a job ([`Effect::Job`]); nothing here or in the controller writes
//! a file itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use rf_core::{ErrorKind, VPath};
use rf_jobs::{CreatedItem, TransferMode, UndoRecord};
use rf_vfs::{SortKey, VisibilityFilter};

use crate::dialogs::{Failures, Modal};
use crate::jobs::{History, JobEffect, JobRows};
use crate::keys::{Command, Mods};
use crate::loader::{Followed, LoadEvent, LoadRequest, NamesRequest};
use crate::pane::{Handled, LoadStatus, OpenAction, Pane, Tab, ancestors};
use crate::places::Place;
use crate::settings::{Found, Session, SessionPane, Waiting};
use crate::tree::{NodeId, Tree};
use crate::words;

mod address;
mod copies;
mod drag;
mod jobs;
mod layout;
mod pick;
mod sidebar;
mod tabs;

pub use address::{
    Completion, Environment, MAX_NAMES, Names, Subfolders, Typed, expand_user_path, is_remote_url,
    read_typed, split_last,
};
pub use copies::copy_origins;
pub use drag::{Drag, DropOffer, DropSpot};
use layout::Step;
pub use layout::{EVEN_SHARE, Layout, MAX_SHARE, MIN_SHARE};

/// The left pane's position in [`AppState::panes`].
pub const LEFT: usize = 0;
/// The right pane's position in [`AppState::panes`].
pub const RIGHT: usize = 1;

/// The shortest time between two automatic reloads of a folder whose watch ended while the
/// folder is still there, so that a watch that keeps failing cannot reload it in a loop.
pub const AUTO_RELOAD_GAP: Duration = Duration::from_secs(5);

/// What a change of the [`AppState`] needs done outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// List and watch a tab's folder ([`Loader::request`](crate::loader::Loader::request)).
    Load(LoadRequest),
    /// The watch of the tab with `generation` ended, or the folder of a tab reopened under it is
    /// gone: find on a worker thread where to go
    /// ([`Loader::nearest_existing_parent`](crate::loader::Loader::nearest_existing_parent) of
    /// `path`, after [`VANISHED_SETTLE`](crate::controller::VANISHED_SETTLE)), then call
    /// [`AppState::found_existing`].
    FindExisting { generation: u64, path: VPath },
    /// The row opened in the tab with `generation` is a link: follow it on a worker thread
    /// ([`Loader::follow`](crate::loader::Loader::follow)), then hand the result, which
    /// carries `generation`, to [`AppState::followed`].
    Follow { generation: u64, link: VPath },
    /// Open the file with the system's default app.
    OpenFile(VPath),
    /// The folder of the tab with `generation` came back after its watch ended, within
    /// [`AUTO_RELOAD_GAP`] of its last automatic reload: call [`AppState::reload_due`] after
    /// `after` (a one-shot timer on the UI thread).
    ReloadLater {
        generation: u64,
        path: VPath,
        after: Duration,
    },
    /// Show a notice in the corner of the window.
    Notice(String),
    /// Show the path of pane `side` as text to edit.
    EditPath(usize),
    /// Give the toolbar's filter box (이 폴더에서 거르기) the keys.
    FocusFilter,
    /// Open the 보기 menu.
    ShowViewMenu,
    /// Give the sidebar's folder tree the keys (module `state::sidebar`).
    FocusTree,
    /// Give the keys back to the panes (from the tree, or from a part of the window that is
    /// gone).
    FocusPanes,
    /// Scroll pane `side` so that `row` shows.
    ScrollTo { side: usize, row: usize },
    /// List the folders in a folder of the tree
    /// ([`Loader::list_folders`](crate::loader::Loader::list_folders)); the listing comes
    /// back to [`AppState::loaded`].
    LoadChildren(LoadRequest),
    /// Scroll the folder tree so that row `row`, the active pane's folder just found there,
    /// shows.
    ScrollTree(usize),
    /// List the names of folders for a list of the address bar
    /// ([`Loader::list_names`](crate::loader::Loader::list_names)); they come back to
    /// [`AppState::loaded`].
    ListNames(NamesRequest),
    /// Find out on a worker thread what is at `path`, the destination picker's target or a
    /// path typed into it ([`Loader::inspect_target`](crate::loader::Loader::inspect_target),
    /// with `from_dir`, the folder the items are in), and hand it to
    /// [`AppState::target_inspected`] with `generation`.
    InspectTarget {
        generation: u64,
        path: VPath,
        from_dir: VPath,
    },
    /// Scroll the picker's folder tree so that row `row`, its target just found there, shows.
    ScrollPickTree(usize),
    /// Ask the job engine for something ([`Jobs::carry_out`](crate::jobs::Jobs::carry_out)).
    /// A job it starts goes to [`AppState::track`], and what the history can do next then to
    /// [`AppState::set_history`].
    Job(JobEffect),
    /// Find on a worker thread whether the location of `folder` has a trash
    /// (`rf_platform::trash_supported`), then call [`AppState::trash_checked`] with `paths`.
    CheckTrash { folder: VPath, paths: Vec<VPath> },
    /// Read again on a worker thread the free space of the folders of `tabs` (with each tab's
    /// generation, for [`AppState::loaded`] as a [`LoadEvent::Space`], which only a tab that
    /// still has that generation takes) and of the drives `drives` (for
    /// [`AppState::drive_space`]): a job that ended has changed it.
    ReadSpace {
        tabs: Vec<(u64, VPath)>,
        drives: Vec<VPath>,
    },
    /// Find on a worker thread how each copy of `copies` (as its job left it, then its
    /// original) is now, that job having ended at `at`
    /// ([`Loader::check_copy`](crate::loader::Loader::check_copy)), then call
    /// [`AppState::copies_checked`] with `token` and what was found, in order. The look stops
    /// when the app ends.
    CheckCopies {
        token: u64,
        copies: Vec<(CreatedItem, VPath)>,
        at: Option<SystemTime>,
    },
    /// Show a toast with the message `text` and the button `action`, whose click comes back
    /// to [`AppState::toast_clicked`] with `token`.
    Toast {
        token: u64,
        text: String,
        action: String,
    },
    /// Show `text` where the toasts show, without a button, until [`Effect::NotBusy`] with
    /// `token` comes or a toast takes its place: a worker is at something the user waits for.
    Busy { token: u64, text: String },
    /// The work of the notice [`Effect::Busy`] `token` is over: the notice goes, unless a
    /// toast has taken its place.
    NotBusy(u64),
    /// End the app ([`crate::jobs::close_down`] on a worker thread, then the end of the event
    /// loop). [`AppState::begin_quit`] has closed the windows and stopped taking questions.
    Quit,
}

/// What the button of the toast on screen does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToastAction {
    /// Undo the job that left `record`, if it is still the next one to undo.
    Undo(UndoRecord),
    /// Show the items the job could not process.
    Details(Failures),
}

/// The window's state on the UI thread: two panes, the one with the keyboard, whether hidden
/// items show, and the folder tree that follows the active pane.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact of the window, not states of one machine"
)]
pub struct AppState {
    panes: [Pane; 2],
    active: usize,
    show_hidden: bool,
    /// One pane or two, how they share the room, and the side panels.
    layout: Layout,
    /// Navigation sync is on ([`layout`]).
    sync: bool,
    /// What navigation sync waits for the other pane's listing to decide.
    sync_wait: Option<layout::SyncWait>,
    /// The sidebar's folder tree, whose roots are the places.
    tree: Tree,
    /// The node of the tree's keyboard cursor ([`sidebar`]), once the tree had the keys.
    tree_cursor: Option<NodeId>,
    /// The folder tree of the destination picker, whose roots are the places too: made anew
    /// for each picker, and dropped when it closes.
    pick_tree: Tree,
    /// The sidebar's places, once found: the roots of both trees, and the favorites the
    /// picker offers.
    places: Vec<Place>,
    /// The folders the picker offers as recent targets, the last one chosen first.
    recent: Vec<VPath>,
    /// Where a tab goes when its folder and every parent of it are gone.
    home: Option<VPath>,
    /// When each folder was last reloaded because its watch ended ([`AUTO_RELOAD_GAP`]).
    auto_reloads: HashMap<VPath, Instant>,
    /// The tabs, by generation, with a reload waiting for its gap ([`Effect::ReloadLater`]):
    /// one at most each.
    pending_reloads: HashSet<u64>,
    /// The jobs of the job panel.
    jobs: JobRows,
    /// What the engine's history can undo and redo next, as the engine last told.
    history: History,
    /// The window over the main window, if one shows.
    modal: Option<Modal>,
    /// The windows waiting for the one that shows to close, in the order they came.
    later: VecDeque<Modal>,
    /// The toast on screen whose button does something, by its token.
    toast: Option<(u64, ToastAction)>,
    /// The token of the last toast.
    toast_token: u64,
    /// The app is ending: no window opens and no question shows any more.
    quitting: bool,
    /// Whether the notice of a journal error has shown (once is enough).
    journal_error_told: bool,
    /// The copies an undo of a copy could not trash, and where they came from.
    copies: copies::Copies,
    /// The items the last press on a row readied a drag of, under the id its payload carries.
    drag: Option<Drag>,
    /// The click a press on a selected row left for its release.
    pending_click: Option<drag::PendingClick>,
    /// The list of a folder's subfolders that the `›` after a piece of a path row opened.
    subfolders: Option<Subfolders>,
    /// The completion list of the path field being edited.
    completion: Option<Completion>,
    /// Where `~` and the variables of a typed path come from.
    environment: Environment,
}

impl AppState {
    /// The left pane at `left`, the right one at `right`, the left one active, and the
    /// requests for their listings.
    pub fn new(left: VPath, right: VPath, home: Option<VPath>) -> (AppState, Vec<Effect>) {
        AppState::restore(left, right, Session::default(), &[], home)
    }

    /// The window as `session` left it: the tabs of each pane in their order, pinned or not,
    /// with the one that showed, the pane with the keyboard, the hidden items shown or not, the
    /// layout; and the requests for the tabs' listings. Navigation sync starts off. A pane
    /// without tabs opens at `left` or `right`. The tabs of `waiting` show home in place of their
    /// folder, which they stand in for ([`Tab::stand_in_for`]) until they move, or
    /// [`AppState::restored_folder`] takes what is found.
    pub fn restore(
        left: VPath,
        right: VPath,
        session: Session,
        waiting: &[Waiting],
        home: Option<VPath>,
    ) -> (AppState, Vec<Effect>) {
        let filter = VisibilityFilter {
            show_hidden: session.show_hidden,
            ..VisibilityFilter::default()
        };
        let [left_saved, right_saved] = session.panes;
        let mut effects = Vec::new();
        let mut panes = [(left_saved, left), (right_saved, right)].map(|(saved, default)| {
            let SessionPane {
                tabs,
                active,
                pinned,
            } = saved;
            let mut tabs = tabs.into_iter();
            let (mut pane, request) = Pane::new(tabs.next().unwrap_or(default), filter);
            effects.push(Effect::Load(request));
            for tab in tabs {
                effects.push(Effect::Load(pane.add_tab(tab)));
            }
            for (tab, pinned) in pane.tabs_mut().iter_mut().zip(pinned) {
                tab.set_pinned(pinned);
            }
            pane.select_tab(active.min(pane.tabs().len() - 1));
            pane
        });
        for waiting in waiting {
            if let Some(tab) = panes
                .get_mut(waiting.side)
                .and_then(|pane| pane.tabs_mut().get_mut(waiting.tab))
            {
                tab.stand_in_for(waiting.path.clone());
            }
        }
        let state = AppState {
            panes,
            active: session.active_pane.min(RIGHT),
            show_hidden: filter.show_hidden,
            layout: session.layout,
            sync: false,
            sync_wait: None,
            tree: Tree::new(filter),
            tree_cursor: None,
            pick_tree: Tree::new(filter),
            places: Vec::new(),
            recent: Vec::new(),
            home,
            auto_reloads: HashMap::new(),
            pending_reloads: HashSet::new(),
            jobs: JobRows::default(),
            history: History::default(),
            modal: None,
            later: VecDeque::new(),
            toast: None,
            toast_token: 0,
            quitting: false,
            journal_error_told: false,
            copies: copies::Copies::default(),
            drag: None,
            pending_click: None,
            subfolders: None,
            completion: None,
            environment: Environment::default(),
        };
        (state, effects)
    }

    /// What the window shows, to keep for the next run: the tabs in their order, which of them
    /// are pinned (a flag for each, as far as the last pinned one), the one that shows. A tab
    /// that stands in at home for a folder of the last run (a slow drive, an unplugged one)
    /// keeps that folder.
    pub fn session(&self) -> Session {
        let pane = |pane: &Pane| SessionPane {
            tabs: pane
                .tabs()
                .iter()
                .map(|tab| tab.stands_in_for().unwrap_or(tab.location()).clone())
                .collect(),
            active: pane.active(),
            pinned: crate::settings::pinned_flags(pane.tabs().iter().map(Tab::pinned)),
        };
        Session {
            panes: [pane(&self.panes[LEFT]), pane(&self.panes[RIGHT])],
            active_pane: self.active,
            show_hidden: self.show_hidden,
            layout: self.layout,
        }
    }

    /// What a worker found at `path`, a folder of the last run that tabs stand in for at home
    /// (a slow drive's): each tab that still stands in for it goes to it when it is there,
    /// stops standing in when it is gone, and keeps standing in when its drive is not there.
    pub fn restored_folder(&mut self, path: &VPath, found: Found) -> Vec<Effect> {
        let mut effects = Vec::new();
        for tab in self.panes.iter_mut().flat_map(Pane::tabs_mut) {
            if tab.stands_in_for() != Some(path) {
                continue;
            }
            match found {
                Found::Folder => effects.push(Effect::Load(tab.navigate(path.clone()))),
                Found::Gone => tab.end_stand_in(),
                Found::Offline => {}
            }
        }
        effects
    }

    pub fn panes(&self) -> &[Pane; 2] {
        &self.panes
    }

    /// The pane with the keyboard: [`LEFT`] or [`RIGHT`].
    pub fn active(&self) -> usize {
        self.active
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The sidebar's folder tree.
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The generation of every tab's last request, of every listing the trees wait for and of
    /// the listings of the address bar's lists: the loads still worth running.
    pub fn generations(&self) -> HashSet<u64> {
        self.panes
            .iter()
            .flat_map(Pane::tabs)
            .map(Tab::generation)
            .chain(self.tree.generations())
            .chain(self.pick_tree.generations())
            .chain(self.address_generations())
            .collect()
    }

    /// Gives pane `side` the keyboard.
    pub fn activate(&mut self, side: usize) {
        self.active = side.min(RIGHT);
    }

    /// Carries out the shortcut `command`, held with `mods`, on the active pane. `page` is how
    /// many rows `PageUp` and `PageDown` move. `None` for a command of the shortcut table this
    /// window does not do yet (`docs/ux/keyboard.md` says which task brings each): the key goes
    /// on to the system.
    pub fn key(
        &mut self,
        command: Command,
        mods: Mods,
        page: usize,
        now: Instant,
    ) -> Option<Vec<Effect>> {
        // A command of the window closes the list of subfolders under a `›` (the keys of the
        // list itself, `keys::list_key`, go to `AppState::subfolders_key` instead).
        self.subfolders = None;
        // A window over the main window takes the keys itself; one that comes here anyway (or
        // one pressed while the app ends) does nothing.
        if self.modal.is_some() || self.quitting {
            return Some(Vec::new());
        }
        let side = self.active;
        let page = isize::try_from(page.max(1)).unwrap_or(isize::MAX);
        let effects = match command {
            // With one pane, Tab stays on it: the other is hidden.
            Command::SwitchPane => {
                if self.layout.dual {
                    self.active = RIGHT - side;
                }
                Vec::new()
            }
            Command::NextTab => self.step_tab(side, 1),
            Command::PrevTab => self.step_tab(side, -1),
            Command::ReopenTab => self.reopen_tab(side),
            Command::MoveTabLeft => self.shift_tab(side, self.panes[side].active(), -1),
            Command::MoveTabRight => self.shift_tab(side, self.panes[side].active(), 1),
            Command::FocusFilter => vec![Effect::FocusFilter],
            Command::ViewMenu => vec![Effect::ShowViewMenu],
            Command::Open => self.open_cursor(side),
            Command::Up => self.go(side, Step::Up),
            Command::Back => self.go(side, Step::Back),
            Command::Forward => self.go(side, Step::Forward),
            Command::CursorUp => self.move_cursor(side, -1, mods.shift),
            Command::CursorDown => self.move_cursor(side, 1, mods.shift),
            Command::CursorHome => self.move_cursor(side, isize::MIN, mods.shift),
            Command::CursorEnd => self.move_cursor(side, isize::MAX, mods.shift),
            Command::PageUp => self.move_cursor(side, -page, mods.shift),
            Command::PageDown => self.move_cursor(side, page, mods.shift),
            Command::ToggleSelect => {
                let (selection, rows) = self.tab_mut(side).selection_mut();
                if let Some(cursor) = selection.cursor() {
                    selection.toggle(cursor, rows);
                }
                Vec::new()
            }
            Command::SelectAll => {
                let (selection, rows) = self.tab_mut(side).selection_mut();
                selection.select_all(rows);
                Vec::new()
            }
            Command::ClearSelection => {
                self.tab_mut(side).selection_mut().0.clear();
                Vec::new()
            }
            Command::EditPath => vec![Effect::EditPath(side)],
            Command::NewTab => self.add_tab(side),
            Command::CloseTab => {
                let tab = self.panes[side].active();
                self.close_tab(side, tab)
            }
            Command::ToggleHidden => self.toggle_hidden(),
            Command::TypeToSelect(text) => self
                .tab_mut(side)
                .type_to_select(&text, now)
                .map(|row| Effect::ScrollTo { side, row })
                .into_iter()
                .collect(),
            Command::NewFolder => self.new_folder(side),
            Command::Rename => self.rename(side),
            Command::Trash => self.trash(side),
            Command::DeletePermanently => self.delete_permanently(side),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::CopyTo => self.open_picker(side, TransferMode::Copy),
            Command::MoveTo => self.open_picker(side, TransferMode::Move),
            Command::ToggleDualPane => self.toggle_dual(),
            Command::ToggleSplit => self.toggle_split(),
            Command::ToggleTree => self.toggle_tree(),
            Command::FocusTree => self.focus_tree(),
            Command::TogglePreview => self.toggle_preview(),
            Command::SwapPanes => self.swap_panes(),
            Command::SameFolderInOther => self.same_folder_in_other(side),
            Command::SyncNavigation => self.toggle_sync(),
            Command::Refresh
            | Command::Copy
            | Command::Cut
            | Command::Paste
            | Command::PasteMove
            | Command::CopyPath
            | Command::Properties
            | Command::ContextMenu
            | Command::QuickLook => return None,
        };
        Some(effects)
    }

    /// Shows the tab `delta` places after (a negative `delta`: before) the one pane `side`
    /// shows, going round from the last tab to the first and back.
    fn step_tab(&mut self, side: usize, delta: isize) -> Vec<Effect> {
        let pane = &mut self.panes[side];
        let count = pane.tabs().len();
        let next = pane.active().cast_signed().saturating_add(delta);
        pane.select_tab(usize::try_from(next.rem_euclid(count.cast_signed())).unwrap_or(0));
        Vec::new()
    }

    /// A right click on row `row` of pane `side`, before its menu opens: the pane gets the
    /// keyboard, and the row the cursor. A row that is not selected becomes the selection, as
    /// a click would make it; a selected one leaves the selection as it is, so that the menu
    /// acts on all of it.
    pub fn row_menu(&mut self, side: usize, row: usize) -> Vec<Effect> {
        self.activate(side);
        let (selection, rows) = self.tab_mut(side).selection_mut();
        match rows.get(row) {
            Some(entry) if selection.is_selected(&entry.name) => selection.set_cursor(row, rows),
            Some(_) => selection.click(row, rows),
            None => {}
        }
        Vec::new()
    }

    /// A click on row `row` of pane `side`: Ctrl flips the row, Shift selects the range from
    /// the anchor, a plain click selects the row alone. The pane gets the keyboard.
    pub fn click_row(&mut self, side: usize, row: usize, ctrl: bool, shift: bool) -> Vec<Effect> {
        self.activate(side);
        let (selection, rows) = self.tab_mut(side).selection_mut();
        if shift {
            selection.extend_to(row, rows);
        } else if ctrl {
            selection.toggle(row, rows);
        } else {
            selection.click(row, rows);
        }
        Vec::new()
    }

    /// Opens row `row` of pane `side` (Enter or a double click): a folder shows in the tab, or
    /// in a new tab beside a pinned one (and in the other pane with navigation sync), a file
    /// opens in its app, and a link is followed first.
    pub fn open_row(&mut self, side: usize, row: usize) -> Vec<Effect> {
        self.activate(side);
        let tab = self.tab_mut(side);
        match tab.open_target(row) {
            Some(OpenAction::Navigate(folder)) => {
                let name = folder.file_name().map(std::ffi::OsStr::to_os_string);
                let mut effects = load(self.navigate_shown(side, folder));
                if let Some(name) = name {
                    effects.extend(self.sync_open(side, &name));
                }
                effects
            }
            Some(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            Some(OpenAction::Follow(link)) => vec![Effect::Follow {
                generation: tab.generation(),
                link,
            }],
            None => Vec::new(),
        }
    }

    /// Shows tab `index` of pane `side`.
    pub fn select_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        self.panes[side].select_tab(index);
        Vec::new()
    }

    /// Closes tab `index` of pane `side`, pinned or not, unless it is the pane's last one; the
    /// pane keeps it to reopen.
    pub fn close_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        self.close_one(side, index);
        Vec::new()
    }

    /// Opens a tab in pane `side` at the folder the pane shows.
    pub fn add_tab(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let here = self.tab(side).location().clone();
        vec![Effect::Load(self.panes[side].add_tab(here))]
    }

    /// Goes to piece `index` of the path row of pane `side` (0 is the root).
    pub fn open_crumb(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        let Some(folder) = ancestors(self.tab(side).location()).into_iter().nth(index) else {
            return Vec::new();
        };
        load(self.navigate_shown(side, folder))
    }

    /// Sorts pane `side` by `key`, the other way round when it already sorts by it.
    pub fn sort(&mut self, side: usize, key: SortKey) -> Vec<Effect> {
        self.activate(side);
        self.tab_mut(side).set_sort(key);
        Vec::new()
    }

    /// The toolbar's filter box (이 폴더에서 거르기) changed to `text`: the active pane shows
    /// only the rows whose names hold it (all of them for an empty one), and scrolls to its
    /// cursor.
    pub fn filter_edited(&mut self, text: &str) -> Vec<Effect> {
        let side = self.active;
        let tab = self.tab_mut(side);
        tab.set_name_filter(text);
        let row = tab.selection().cursor().unwrap_or(0);
        vec![Effect::ScrollTo { side, row }]
    }

    /// What the filter box shows: the filter of the active pane's tab.
    pub fn filter_text(&self) -> &str {
        self.tab(self.active).name_filter()
    }

    /// Lists the folder of pane `side` again ("다시 시도").
    pub fn retry(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        vec![Effect::Load(self.tab_mut(side).reload())]
    }

    /// Shows `folder` in the active pane (in a new tab beside a pinned one).
    pub fn go_to(&mut self, folder: VPath) -> Vec<Effect> {
        let side = self.active;
        load(self.navigate_shown(side, folder))
    }

    /// Makes `places` (the favorites, then the drives) the roots of the folder tree, which
    /// starts again from them, closed, and finds the folder it follows again; and of the
    /// picker's tree, when a picker is open.
    pub fn set_places(&mut self, places: &[Place]) -> Vec<Effect> {
        places.clone_into(&mut self.places);
        // The new tree's nodes are other ones.
        self.tree_cursor = None;
        let mut effects = self.with_tree(|tree| tree.set_roots(pick::tree_roots(places)));
        if self.shown_picker().is_some() {
            effects.extend(self.with_pick_tree(|tree| tree.set_roots(pick::tree_roots(places))));
        }
        effects
    }

    /// The free space of the drive at `path`, as a worker read it again
    /// ([`Effect::ReadSpace`]): the sidebar shows it from now on. Nothing for a drive that is no
    /// longer among the places.
    pub fn drive_space(&mut self, path: &VPath, space: rf_platform::DiskSpace) {
        for place in &mut self.places {
            if place.kind == crate::places::PlaceKind::Drive && place.path == *path {
                place.space = Some(space);
            }
        }
    }

    /// What a job that changed the folders `folders` has to have read again: the free space of
    /// the drives they are on (the deepest drive of the places that holds each), and of every
    /// listed tab on those drives, with its generation. Without known drives, a tab counts as on
    /// the drive of a folder when both have one root.
    fn space_to_read(&self, folders: &[VPath]) -> Effect {
        let drives: Vec<&VPath> = self
            .places
            .iter()
            .filter(|place| place.kind == crate::places::PlaceKind::Drive)
            .map(|place| &place.path)
            .collect();
        let drive_of = |path: &VPath| -> Option<VPath> {
            let deepest = drives
                .iter()
                .filter(|drive| path.starts_with(drive))
                .max_by_key(|drive| ancestors(drive).len())
                .map(|drive| (*drive).clone());
            if drives.is_empty() {
                ancestors(path).into_iter().next()
            } else {
                deepest
            }
        };
        let mut touched: Vec<VPath> = Vec::new();
        for drive in folders.iter().filter_map(drive_of) {
            if !touched.contains(&drive) {
                touched.push(drive);
            }
        }
        let tabs = self
            .panes
            .iter()
            .flat_map(Pane::tabs)
            .filter(|tab| tab.status() == LoadStatus::Ready)
            .filter(|tab| drive_of(tab.location()).is_some_and(|drive| touched.contains(&drive)))
            .map(|tab| (tab.generation(), tab.location().clone()))
            .collect();
        let drives = if drives.is_empty() {
            Vec::new()
        } else {
            touched
        };
        Effect::ReadSpace { tabs, drives }
    }

    /// Shows the folder of row `row` of the tree in the active pane (a click on the row). The
    /// row is the tree's selection from then on, even when another row shows the same folder.
    pub fn open_tree_row(&mut self, row: usize) -> Vec<Effect> {
        let Some(id) = self.tree.node_at(row) else {
            return Vec::new();
        };
        let Some(folder) = self.tree.node(id).map(|node| node.path.clone()) else {
            return Vec::new();
        };
        self.tree.select(id);
        self.go_to(folder)
    }

    /// Opens or closes row `row` of the tree (a click on its chevron); the first opening
    /// lists the folder.
    pub fn toggle_tree_row(&mut self, row: usize) -> Vec<Effect> {
        match self.tree.node_at(row) {
            Some(id) => self.with_tree(|tree| tree.toggle(id).into_iter().collect()),
            None => Vec::new(),
        }
    }

    /// Makes the tree follow the active pane (LAY-03 a): when the folder the active pane
    /// shows is not the one the tree follows (the pane, its tab or its folder changed), the
    /// tree opens the folders on the way to it and selects it. The controller calls this
    /// after every change; it does nothing while the folder stays the same, so a folder the
    /// user closed in the tree stays closed, and nothing while the tree is hidden, which finds
    /// the folder once it shows again.
    pub fn follow_active(&mut self) -> Vec<Effect> {
        let location = self.tab(self.active).location();
        if !self.layout.tree || self.tree.target() == Some(location) {
            return Vec::new();
        }
        let location = location.clone();
        self.with_tree(|tree| tree.reveal(&location))
    }

    /// Takes an event of the loader. A rescan lists the folder again; a vanished folder is
    /// looked for on a worker thread. The ticket of a vanished folder stays: its listing, if
    /// still running, still comes. The names a list of the address bar waits for go to it; an
    /// event that no tab asked for goes to the folder tree.
    pub fn loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        // The listing of a list of the address bar.
        let Some(event) = self.address_loaded(event) else {
            return Vec::new();
        };
        // A tab reopened at a folder that is gone by now goes where the tab of a vanished
        // folder goes, the nearest folder above it that is left, rather than to the error.
        if let LoadEvent::Failed {
            generation,
            kind: ErrorKind::NotFound,
        } = event
            && let Some(tab) = self.tab_by_generation(generation)
            && tab.take_reopened()
        {
            let path = tab.location().clone();
            return vec![Effect::FindExisting { generation, path }];
        }
        let Some(side) = self.side_of(event.generation()) else {
            return self.tree_loaded(event);
        };
        let new_rows = matches!(event, LoadEvent::Listed { .. } | LoadEvent::Changes { .. });
        let generation = event.generation();
        let finished = matches!(event, LoadEvent::Listed { .. } | LoadEvent::Failed { .. });
        match self.panes[side].handle(event) {
            Handled::Stale => Vec::new(),
            // A row a job made may have come: it was selected, and shows.
            Handled::Applied { tab } => {
                // The trees' nodes of the folder take its folders, once the listing is whole;
                // a reveal that lacked one of them may go on.
                let mut effects = Vec::new();
                let listed = &self.panes[side].tabs()[tab];
                if new_rows && listed.status() == LoadStatus::Ready {
                    let (folder, entries) = (listed.location(), listed.all());
                    let before = (self.tree.selected(), self.pick_tree.selected());
                    let tree = self.tree.sync_folder(folder, entries);
                    let pick = self.pick_tree.sync_folder(folder, entries);
                    effects.extend(self.tree_effects(before.0, tree.unwrap_or_default()));
                    effects.extend(self.pick_tree_effects(before.1, pick.unwrap_or_default()));
                }
                let shows = tab == self.panes[side].active();
                effects.extend(
                    self.panes[side].tabs_mut()[tab]
                        .take_revealed()
                        .filter(|_| shows)
                        .map(|row| Effect::ScrollTo { side, row }),
                );
                // Navigation sync may wait for this listing.
                if finished {
                    effects.extend(self.sync_listed(generation));
                }
                effects
            }
            Handled::Reload { request, .. } => {
                self.sync_relisted(generation, request.generation);
                vec![Effect::Load(request)]
            }
            Handled::Vanished {
                generation, path, ..
            } => vec![Effect::FindExisting { generation, path }],
        }
    }

    /// Takes where the tab with `generation`, whose folder `path` vanished, can go: `found`,
    /// the nearest folder of `path` that is still there, or home when none is. A tab that has
    /// moved on since takes nothing. When `found` is `path` itself (the folder was replaced,
    /// or only its watch failed), the folder is listed again; within [`AUTO_RELOAD_GAP`] of
    /// its last automatic reload, that waits for the gap to pass ([`Effect::ReloadLater`]),
    /// so a watch that keeps failing cannot reload it in a loop and a folder replaced twice
    /// in quick succession still shows the second one. Otherwise the tab goes to `found`, and
    /// when its pane shows it, a notice says why.
    pub fn found_existing(
        &mut self,
        generation: u64,
        path: &VPath,
        found: Option<VPath>,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(target) = found.or_else(|| self.home.clone()) else {
            return Vec::new();
        };
        if target == *path {
            if self.tab_by_generation(generation).is_none() {
                return Vec::new();
            }
            let since = self
                .auto_reloads
                .get(path)
                .map(|&at| now.saturating_duration_since(at))
                .filter(|&since| since < AUTO_RELOAD_GAP);
            if let Some(since) = since {
                if !self.pending_reloads.insert(generation) {
                    return Vec::new();
                }
                return vec![Effect::ReloadLater {
                    generation,
                    path: path.clone(),
                    after: AUTO_RELOAD_GAP.saturating_sub(since),
                }];
            }
            return self.auto_reload(generation, path, now);
        }
        let shown = self
            .panes
            .iter()
            .any(|pane| pane.active_tab().generation() == generation);
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        let to_parent = path.starts_with(&target);
        let mut effects = vec![Effect::Load(tab.navigate(target))];
        // The pane that shows it moves: the user is told why.
        if shown {
            let name = path.file_name().map_or_else(
                || path.to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            effects.push(Effect::Notice(words::folder_vanished(&name, to_parent)));
        }
        effects
    }

    /// The time of the reload [`Effect::ReloadLater`] asked for has come: the tab with
    /// `generation` lists its folder `path` again, unless it has moved on since.
    pub fn reload_due(&mut self, generation: u64, path: &VPath, now: Instant) -> Vec<Effect> {
        if !self.pending_reloads.remove(&generation) {
            return Vec::new();
        }
        self.auto_reload(generation, path, now)
    }

    /// Lists the folder `path` of the tab with `generation` again after its watch ended, and
    /// notes when; nothing for a tab that has moved on.
    fn auto_reload(&mut self, generation: u64, path: &VPath, now: Instant) -> Vec<Effect> {
        self.pending_reloads.remove(&generation);
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        let request = tab.reload();
        // Navigation sync may wait for its listing.
        self.sync_relisted(generation, request.generation);
        self.auto_reloads
            .retain(|_, &mut at| now.saturating_duration_since(at) < AUTO_RELOAD_GAP);
        self.auto_reloads.insert(path.clone(), now);
        vec![Effect::Load(request)]
    }

    /// Takes what following a link gave ([`Loader::follow`](crate::loader::Loader::follow)).
    /// Only the tab that still has the generation the result carries takes it: a tab that has
    /// moved on since takes nothing. A pinned tab stays: the folder opens in a new tab beside
    /// it.
    pub fn followed(&mut self, followed: Followed) -> Vec<Effect> {
        let Followed { generation, result } = followed;
        let Some((side, index)) = self.place_of(generation) else {
            return Vec::new();
        };
        match result {
            Ok(OpenAction::Navigate(folder)) => load(self.navigate_tab(side, index, folder)),
            Ok(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            // Following never gives another link to follow.
            Ok(OpenAction::Follow(_)) => Vec::new(),
            Err(kind) => vec![Effect::Notice(words::cannot_open(kind))],
        }
    }

    /// Takes an event of a listing of a folder tree: the picker's, when it waits for it, or
    /// else the sidebar's. One that no node waits for is dropped. A listing that failed leaves
    /// the node empty, with no notice. Then a tab that shows the folder, whole, gives the node
    /// its folders as they are now when the tree's listing failed, or when the tab's watch
    /// told of a change while that listing was on its way ([`Tree::listing_is_behind`]),
    /// which may have been read before it.
    fn tree_loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        let generation = event.generation();
        let in_picker = self
            .pick_tree
            .generations()
            .any(|waited| waited == generation);
        let tree = if in_picker {
            &self.pick_tree
        } else {
            &self.tree
        };
        let failed = matches!(event, LoadEvent::Failed { .. });
        let folder = tree
            .listing_of(generation)
            .filter(|_| failed || tree.listing_is_behind(generation))
            .cloned();
        let take = |tree: &mut Tree| match event {
            LoadEvent::Listed {
                generation,
                entries,
            } => tree.apply_children(generation, entries),
            LoadEvent::Failed { generation, .. } => {
                tree.listing_failed(generation);
                Vec::new()
            }
            // A listing of folders sends nothing else.
            LoadEvent::Changes { .. }
            | LoadEvent::Rescan { .. }
            | LoadEvent::Vanished { .. }
            | LoadEvent::Space { .. }
            | LoadEvent::Names { .. } => Vec::new(),
        };
        let mut effects = if in_picker {
            self.with_pick_tree(take)
        } else {
            self.with_tree(take)
        };
        let Some(folder) = folder else {
            return effects;
        };
        let Some(shown) = self
            .panes
            .iter()
            .flat_map(Pane::tabs)
            .find(|tab| tab.status() == LoadStatus::Ready && *tab.location() == folder)
        else {
            return effects;
        };
        if in_picker {
            let before = self.pick_tree.selected();
            let requests = self.pick_tree.sync_folder(&folder, shown.all());
            effects.extend(self.pick_tree_effects(before, requests.unwrap_or_default()));
        } else {
            let before = self.tree.selected();
            let requests = self.tree.sync_folder(&folder, shown.all());
            effects.extend(self.tree_effects(before, requests.unwrap_or_default()));
        }
        effects
    }

    /// Changes the tree with `f` and turns what it asks for into effects
    /// ([`AppState::tree_effects`]).
    fn with_tree(&mut self, f: impl FnOnce(&mut Tree) -> Vec<LoadRequest>) -> Vec<Effect> {
        let before = self.tree.selected();
        let requests = f(&mut self.tree);
        self.tree_effects(before, requests)
    }

    /// The effects of `requests`, the listings the tree asked for in a change: those listings,
    /// and a scroll to the row of the active pane's folder when the tree has just found it
    /// (its selection was `before` the change).
    fn tree_effects(&self, before: Option<NodeId>, requests: Vec<LoadRequest>) -> Vec<Effect> {
        let mut effects: Vec<Effect> = requests.into_iter().map(Effect::LoadChildren).collect();
        if self.tree.selected() != before
            && let Some(row) = self.tree.selected_row()
        {
            effects.push(Effect::ScrollTree(row));
        }
        effects
    }

    fn tab(&self, side: usize) -> &Tab {
        self.panes[side].active_tab()
    }

    fn tab_mut(&mut self, side: usize) -> &mut Tab {
        self.panes[side].active_tab_mut()
    }

    fn tab_by_generation(&mut self, generation: u64) -> Option<&mut Tab> {
        self.panes
            .iter_mut()
            .find_map(|pane| pane.tab_by_generation_mut(generation))
    }

    /// The pane one of whose tabs made the request `generation`.
    fn side_of(&self, generation: u64) -> Option<usize> {
        self.place_of(generation).map(|(side, _)| side)
    }

    /// The pane and the position of the tab whose last request is `generation`.
    fn place_of(&self, generation: u64) -> Option<(usize, usize)> {
        self.panes.iter().enumerate().find_map(|(side, pane)| {
            let tabs = pane.tabs();
            let index = tabs.iter().position(|tab| tab.generation() == generation)?;
            Some((side, index))
        })
    }

    /// Moves the active tab of pane `side` with `step` (back, forward, up), if it can go (a
    /// pinned one stays, and a new tab beside it makes the move); with navigation sync, the
    /// other pane makes the same move.
    fn go(&mut self, side: usize, step: Step) -> Vec<Effect> {
        let Some(request) = self.move_shown(side, |tab| step.run(tab)) else {
            return Vec::new();
        };
        let mut effects = vec![Effect::Load(request)];
        effects.extend(self.sync_step(side, step));
        effects
    }

    /// Enter: opens the cursor's row of pane `side`. A folder opens from a cursor the list
    /// placed, which only moves the tab, so the keys go down folder by folder; a file, or a
    /// link that may lead to one, is launched only from a row the user chose
    /// ([`Selection::user_cursor`](crate::selection::Selection::user_cursor)).
    fn open_cursor(&mut self, side: usize) -> Vec<Effect> {
        let tab = self.tab(side);
        let Some(row) = tab.selection().cursor() else {
            return Vec::new();
        };
        let chosen = tab.selection().user_cursor().is_some();
        if !chosen && !matches!(tab.open_target(row), Some(OpenAction::Navigate(_))) {
            return Vec::new();
        }
        self.open_row(side, row)
    }

    fn move_cursor(&mut self, side: usize, delta: isize, extend: bool) -> Vec<Effect> {
        let (selection, rows) = self.tab_mut(side).selection_mut();
        selection.move_cursor(delta, extend, rows);
        selection
            .cursor()
            .map(|row| Effect::ScrollTo { side, row })
            .into_iter()
            .collect()
    }

    /// Shows or hides the hidden items in every tab of both panes, and the hidden folders of
    /// the trees.
    fn toggle_hidden(&mut self) -> Vec<Effect> {
        self.show_hidden = !self.show_hidden;
        for pane in &mut self.panes {
            for tab in pane.tabs_mut() {
                tab.set_show_hidden(self.show_hidden);
            }
        }
        let show = self.show_hidden;
        let mut effects = self.with_tree(|tree| tree.set_show_hidden(show));
        effects.extend(self.with_pick_tree(|tree| tree.set_show_hidden(show)));
        effects
    }
}

/// The listing `request` asks for, if there is one: the effect of a move that may not be made.
fn load(request: Option<LoadRequest>) -> Vec<Effect> {
    request.map(Effect::Load).into_iter().collect()
}

/// The folders the panes start at: the left one at `start` (made absolute against
/// `current_dir`) or else home, the right one at the documents folder or else home. Without
/// a home folder, `root` stands in. `None` when there is no folder at all.
pub fn start_folders(
    start: Option<&Path>,
    current_dir: Option<&Path>,
    home: Option<&VPath>,
    documents: Option<&VPath>,
    root: Option<&VPath>,
) -> Option<(VPath, VPath)> {
    let fallback = home.or(root)?;
    let left = start
        .and_then(|path| match current_dir {
            Some(dir) if path.is_relative() => VPath::local(dir.join(path)).ok(),
            _ => VPath::local(path).ok(),
        })
        .unwrap_or_else(|| fallback.clone());
    let right = documents.unwrap_or(fallback).clone();
    Some((left, right))
}

#[cfg(test)]
mod tests;
