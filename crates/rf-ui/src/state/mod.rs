//! The main window's state, and what its keys, clicks and the loader's events do to it.
//!
//! [`AppState`] is plain data on the UI thread: the two panes, which one has the keyboard,
//! whether hidden items show, the folder tree of the sidebar, the jobs of the job panel, the
//! window that shows over the main window (a [`Modal`]), and the destination picker's folder
//! tree and recent targets ([`pick`]). Its methods change it and return
//! the [`Effect`]s that need a worker thread, the job engine or the screen, which the
//! [`Controller`](crate::controller::Controller) carries out: a listing to start, a link to
//! follow or a vanished folder's nearest parent to find, a file to open, a job to submit or a
//! question to answer, a notice, a row to scroll to. Keeping the two apart lets the tests
//! drive the state without a window.
//!
//! Every change of a file is a job ([`Effect::Job`]); nothing here or in the controller writes
//! a file itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use rf_core::{ErrorKind, VPath};
use rf_jobs::{TransferMode, UndoRecord};
use rf_vfs::{SortKey, VisibilityFilter};

use crate::dialogs::{Failures, Modal};
use crate::jobs::{History, JobEffect, JobRows};
use crate::keys::{Command, Mods};
use crate::loader::{Followed, LoadEvent, LoadRequest};
use crate::pane::{Handled, LoadStatus, OpenAction, Pane, Tab, ancestors};
use crate::places::Place;
use crate::settings::{Session, SessionPane, Waiting};
use crate::tree::Tree;
use crate::words;

mod jobs;
mod pick;

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
    /// The watch of the tab with `generation` ended: find on a worker thread where to go
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
    /// Scroll pane `side` so that `row` shows.
    ScrollTo { side: usize, row: usize },
    /// List the folders in a folder of the tree
    /// ([`Loader::list_folders`](crate::loader::Loader::list_folders)); the listing comes
    /// back to [`AppState::loaded`].
    LoadChildren(LoadRequest),
    /// Scroll the folder tree so that row `row`, the active pane's folder just found there,
    /// shows.
    ScrollTree(usize),
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
    /// Show a toast with the message `text` and the button `action`, whose click comes back
    /// to [`AppState::toast_clicked`] with `token`.
    Toast {
        token: u64,
        text: String,
        action: String,
    },
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
pub struct AppState {
    panes: [Pane; 2],
    active: usize,
    show_hidden: bool,
    /// The sidebar's folder tree, whose roots are the places.
    tree: Tree,
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
    /// The tabs of the last run that show home while their folder is looked for, by the
    /// generation of their request, with that folder ([`AppState::restored_folder`]).
    restoring: Vec<(u64, VPath)>,
}

impl AppState {
    /// The left pane at `left`, the right one at `right`, the left one active, and the
    /// requests for their listings.
    pub fn new(left: VPath, right: VPath, home: Option<VPath>) -> (AppState, Vec<Effect>) {
        AppState::restore(left, right, Session::default(), &[], home)
    }

    /// The window as `session` left it: the tabs of each pane with the one that showed, the
    /// pane with the keyboard, the hidden items shown or not; and the requests for the tabs'
    /// listings. A pane without tabs opens at `left` or `right`. The tabs of `waiting` show
    /// home in the session while their folder is looked for; [`AppState::restored_folder`]
    /// takes what is found.
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
        let panes = [(left_saved, left), (right_saved, right)].map(|(saved, default)| {
            let SessionPane { tabs, active } = saved;
            let mut tabs = tabs.into_iter();
            let (mut pane, request) = Pane::new(tabs.next().unwrap_or(default), filter);
            effects.push(Effect::Load(request));
            for tab in tabs {
                effects.push(Effect::Load(pane.add_tab(tab)));
            }
            pane.select_tab(active.min(pane.tabs().len() - 1));
            pane
        });
        let restoring = waiting
            .iter()
            .filter_map(|waiting| {
                let tab = panes.get(waiting.side)?.tabs().get(waiting.tab)?;
                Some((tab.generation(), waiting.path.clone()))
            })
            .collect();
        let state = AppState {
            panes,
            active: session.active_pane.min(RIGHT),
            show_hidden: filter.show_hidden,
            tree: Tree::new(filter),
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
            restoring,
        };
        (state, effects)
    }

    /// What the window shows, to keep for the next run. A tab that stands in at home for a
    /// folder of the last run still being looked for keeps that folder.
    pub fn session(&self) -> Session {
        let pane = |pane: &Pane| SessionPane {
            tabs: pane
                .tabs()
                .iter()
                .map(|tab| {
                    self.restoring
                        .iter()
                        .find(|(generation, _)| *generation == tab.generation())
                        .map_or_else(|| tab.location().clone(), |(_, path)| path.clone())
                })
                .collect(),
            active: pane.active(),
        };
        Session {
            panes: [pane(&self.panes[LEFT]), pane(&self.panes[RIGHT])],
            active_pane: self.active,
            show_hidden: self.show_hidden,
        }
    }

    /// Whether `path`, a folder of the last run that tabs stand in for at home, is there, as a
    /// worker found: each such tab that has not moved since goes to it. Without it they stay.
    pub fn restored_folder(&mut self, path: &VPath, exists: bool) -> Vec<Effect> {
        let (found, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.restoring)
            .into_iter()
            .partition(|(_, waited)| waited == path);
        self.restoring = rest;
        if !exists {
            return Vec::new();
        }
        found
            .into_iter()
            .filter_map(|(generation, _)| {
                let tab = self.tab_by_generation(generation)?;
                Some(Effect::Load(tab.navigate(path.clone())))
            })
            .collect()
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

    /// The generation of every tab's last request and of every listing the tree waits for:
    /// the loads still worth running.
    pub fn generations(&self) -> HashSet<u64> {
        self.panes
            .iter()
            .flat_map(Pane::tabs)
            .map(Tab::generation)
            .chain(self.tree.generations())
            .chain(self.pick_tree.generations())
            .collect()
    }

    /// Gives pane `side` the keyboard.
    pub fn activate(&mut self, side: usize) {
        self.active = side.min(RIGHT);
    }

    /// Carries out the shortcut `command`, held with `mods`, on the active pane. `page` is how
    /// many rows `PageUp` and `PageDown` move. `None` for a command this window does not take
    /// (yet): the key goes on to the system.
    pub fn key(
        &mut self,
        command: Command,
        mods: Mods,
        page: usize,
        now: Instant,
    ) -> Option<Vec<Effect>> {
        // A window over the main window takes the keys itself; one that comes here anyway (or
        // one pressed while the app ends) does nothing.
        if self.modal.is_some() || self.quitting {
            return Some(Vec::new());
        }
        let side = self.active;
        let page = isize::try_from(page.max(1)).unwrap_or(isize::MAX);
        let effects = match command {
            Command::SwitchPane => {
                self.active = RIGHT - side;
                Vec::new()
            }
            Command::Open => match self.tab(side).selection().cursor() {
                Some(row) => self.open_row(side, row),
                None => Vec::new(),
            },
            Command::Up => self.go(side, Tab::go_up),
            Command::Back => self.go(side, Tab::go_back),
            Command::Forward => self.go(side, Tab::go_forward),
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
        };
        Some(effects)
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

    /// Opens row `row` of pane `side` (Enter or a double click): a folder shows in the tab, a
    /// file opens in its app, and a link is followed first.
    pub fn open_row(&mut self, side: usize, row: usize) -> Vec<Effect> {
        self.activate(side);
        let tab = self.tab_mut(side);
        match tab.open_target(row) {
            Some(OpenAction::Navigate(folder)) => vec![Effect::Load(tab.navigate(folder))],
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

    /// Closes tab `index` of pane `side`, unless it is the pane's last one.
    pub fn close_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        // Its load stops once the controller sees its generation is gone.
        drop(self.panes[side].close_tab(index));
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
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Goes to the path typed into the path row of pane `side`. Text that is no path gives
    /// a notice and changes nothing.
    pub fn enter_path(&mut self, side: usize, text: &str) -> Vec<Effect> {
        self.activate(side);
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        match VPath::parse(text) {
            Ok(folder) => vec![Effect::Load(self.tab_mut(side).navigate(folder))],
            Err(_) => vec![Effect::Notice(words::cannot_open(ErrorKind::InvalidPath))],
        }
    }

    /// Sorts pane `side` by `key`, the other way round when it already sorts by it.
    pub fn sort(&mut self, side: usize, key: SortKey) -> Vec<Effect> {
        self.activate(side);
        self.tab_mut(side).set_sort(key);
        Vec::new()
    }

    /// Lists the folder of pane `side` again ("다시 시도").
    pub fn retry(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        vec![Effect::Load(self.tab_mut(side).reload())]
    }

    /// Shows `folder` in the active pane.
    pub fn go_to(&mut self, folder: VPath) -> Vec<Effect> {
        let side = self.active;
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Makes `places` (the favorites, then the drives) the roots of the folder tree, which
    /// starts again from them, closed, and finds the folder it follows again; and of the
    /// picker's tree, when a picker is open.
    pub fn set_places(&mut self, places: &[Place]) -> Vec<Effect> {
        places.clone_into(&mut self.places);
        let mut effects = self.with_tree(|tree| tree.set_roots(pick::tree_roots(places)));
        if self.shown_picker().is_some() {
            effects.extend(self.with_pick_tree(|tree| tree.set_roots(pick::tree_roots(places))));
        }
        effects
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
    /// user closed in the tree stays closed.
    pub fn follow_active(&mut self) -> Vec<Effect> {
        let location = self.tab(self.active).location();
        if self.tree.target() == Some(location) {
            return Vec::new();
        }
        let location = location.clone();
        self.with_tree(|tree| tree.reveal(&location))
    }

    /// Takes an event of the loader. A rescan lists the folder again; a vanished folder is
    /// looked for on a worker thread. The ticket of a vanished folder stays: its listing, if
    /// still running, still comes. An event that no tab asked for goes to the folder tree.
    pub fn loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        let Some(side) = self.side_of(event.generation()) else {
            return self.tree_loaded(event);
        };
        let new_rows = matches!(event, LoadEvent::Listed { .. } | LoadEvent::Changes { .. });
        match self.panes[side].handle(event) {
            Handled::Stale => Vec::new(),
            // A row a job made may have come: it was selected, and shows.
            Handled::Applied { tab } => {
                // The trees' nodes of the folder take its folders, once the listing is whole.
                let listed = &self.panes[side].tabs()[tab];
                if new_rows && listed.status() == LoadStatus::Ready {
                    self.tree.sync_folder(listed.location(), listed.all());
                    self.pick_tree.sync_folder(listed.location(), listed.all());
                }
                let shows = tab == self.panes[side].active();
                self.panes[side].tabs_mut()[tab]
                    .take_revealed()
                    .filter(|_| shows)
                    .map(|row| Effect::ScrollTo { side, row })
                    .into_iter()
                    .collect()
            }
            Handled::Reload { request, .. } => vec![Effect::Load(request)],
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
    /// in quick succession still shows the second one. Otherwise the tab goes to `found`.
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
        match self.tab_by_generation(generation) {
            Some(tab) => vec![Effect::Load(tab.navigate(target))],
            None => Vec::new(),
        }
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
        self.auto_reloads
            .retain(|_, &mut at| now.saturating_duration_since(at) < AUTO_RELOAD_GAP);
        self.auto_reloads.insert(path.clone(), now);
        vec![Effect::Load(request)]
    }

    /// Takes what following a link gave ([`Loader::follow`](crate::loader::Loader::follow)).
    /// Only the tab that still has the generation the result carries takes it: a tab that has
    /// moved on since takes nothing.
    pub fn followed(&mut self, followed: Followed) -> Vec<Effect> {
        let Followed { generation, result } = followed;
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        match result {
            Ok(OpenAction::Navigate(folder)) => vec![Effect::Load(tab.navigate(folder))],
            Ok(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            // Following never gives another link to follow.
            Ok(OpenAction::Follow(_)) => Vec::new(),
            Err(kind) => vec![Effect::Notice(words::cannot_open(kind))],
        }
    }

    /// Takes an event of a listing of a folder tree: the picker's, when it waits for it, or
    /// else the sidebar's. One that no node waits for is dropped. A listing that failed leaves
    /// the node empty, with no notice.
    fn tree_loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        let generation = event.generation();
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
            | LoadEvent::Space { .. } => Vec::new(),
        };
        if self
            .pick_tree
            .generations()
            .any(|waited| waited == generation)
        {
            self.with_pick_tree(take)
        } else {
            self.with_tree(take)
        }
    }

    /// Changes the tree with `f` and turns what it asks for into effects: the listings to
    /// start, and a scroll to the row of the active pane's folder when the tree has just
    /// found it.
    fn with_tree(&mut self, f: impl FnOnce(&mut Tree) -> Vec<LoadRequest>) -> Vec<Effect> {
        let before = self.tree.selected();
        let mut effects: Vec<Effect> = f(&mut self.tree)
            .into_iter()
            .map(Effect::LoadChildren)
            .collect();
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
        self.panes
            .iter()
            .position(|pane| pane.tabs().iter().any(|tab| tab.generation() == generation))
    }

    /// Moves the active tab of pane `side` with `step` (back, forward, up), if it can go.
    fn go(&mut self, side: usize, step: fn(&mut Tab) -> Option<LoadRequest>) -> Vec<Effect> {
        step(self.tab_mut(side))
            .map(Effect::Load)
            .into_iter()
            .collect()
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
