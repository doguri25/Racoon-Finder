//! The main window's state, and what its keys, clicks and the loader's events do to it.
//!
//! [`AppState`] is plain data on the UI thread: the two panes, which one has the keyboard and
//! whether hidden items show. Its methods change it and return the [`Effect`]s that need a
//! worker thread or the screen, which the [`Controller`] carries out: a listing to start, a
//! link to follow or a vanished folder's nearest parent to find, a file to open, a notice, a
//! row to scroll to. Keeping the two apart lets the tests drive the state without a window.
//!
//! The [`Controller`] lives in a thread-local of the UI thread: Slint's callbacks reach it
//! there, and so do the results of worker threads, through [`post`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use rf_core::{ErrorKind, VPath};
use rf_platform::KnownFolder;
use rf_vfs::{SortKey, VisibilityFilter};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::keys::{self, Command, Mods};
use crate::loader::{LoadEvent, LoadRequest, LoadTicket, Loader};
use crate::pane::{Handled, OpenAction, Pane, Tab};
use crate::places::{self, Place};
use crate::ui_bridge::{self, PaneModels, post};
use crate::{MainWindow, PlaceRow, open, words};

/// The left pane's position in [`AppState::panes`].
pub const LEFT: usize = 0;
/// The right pane's position in [`AppState::panes`].
pub const RIGHT: usize = 1;

/// The shortest time between two automatic reloads of a folder whose watch ended while the
/// folder is still there, so that a watch that keeps failing cannot reload it in a loop.
pub const AUTO_RELOAD_GAP: Duration = Duration::from_secs(5);

/// How long the worker waits after a folder's watch ended before it looks for the folder:
/// a folder replaced in one go (`rm -rf dist && mkdir dist`) is back by then, and is listed
/// again instead of being left for its parent.
pub const VANISHED_SETTLE: Duration = Duration::from_millis(300);

/// What a change of the [`AppState`] needs done outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// List and watch a tab's folder ([`Loader::request`]).
    Load(LoadRequest),
    /// The watch of the tab with `generation` ended: find on a worker thread where to go
    /// ([`Loader::nearest_existing_parent`] of `path`, after [`VANISHED_SETTLE`]), then call
    /// [`AppState::found_existing`].
    FindExisting { generation: u64, path: VPath },
    /// The row opened in the tab with `generation` is a link: follow it on a worker thread
    /// ([`Loader::follow`]), then call [`AppState::followed`].
    Follow { generation: u64, link: VPath },
    /// Open the file with the system's default app.
    OpenFile(VPath),
    /// Show a notice in the corner of the window.
    Notice(String),
    /// Show the path of pane `side` as text to edit.
    EditPath(usize),
    /// Scroll pane `side` so that `row` shows.
    ScrollTo { side: usize, row: usize },
}

/// The window's state on the UI thread: two panes, the one with the keyboard, and whether
/// hidden items show.
#[derive(Debug)]
pub struct AppState {
    panes: [Pane; 2],
    active: usize,
    show_hidden: bool,
    /// Where a tab goes when its folder and every parent of it are gone.
    home: Option<VPath>,
    /// When each folder was last reloaded because its watch ended ([`AUTO_RELOAD_GAP`]).
    auto_reloads: HashMap<VPath, Instant>,
}

impl AppState {
    /// The left pane at `left`, the right one at `right`, the left one active, and the
    /// requests for their listings.
    pub fn new(left: VPath, right: VPath, home: Option<VPath>) -> (AppState, Vec<Effect>) {
        let filter = VisibilityFilter::default();
        let (left_pane, left_request) = Pane::new(left, filter);
        let (right_pane, right_request) = Pane::new(right, filter);
        let state = AppState {
            panes: [left_pane, right_pane],
            active: LEFT,
            show_hidden: filter.show_hidden,
            home,
            auto_reloads: HashMap::new(),
        };
        (
            state,
            vec![Effect::Load(left_request), Effect::Load(right_request)],
        )
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

    /// The generation of every tab's last request: the loads still worth running.
    pub fn generations(&self) -> HashSet<u64> {
        self.panes
            .iter()
            .flat_map(Pane::tabs)
            .map(Tab::generation)
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
            Command::ToggleHidden => {
                self.toggle_hidden();
                Vec::new()
            }
            Command::TypeToSelect(text) => self
                .tab_mut(side)
                .type_to_select(&text, now)
                .map(|row| Effect::ScrollTo { side, row })
                .into_iter()
                .collect(),
            // Commands that change files run through the job engine, which this window does
            // not have yet; their keys are left to the system.
            Command::Rename
            | Command::Trash
            | Command::DeletePermanently
            | Command::CopyTo
            | Command::MoveTo
            | Command::NewFolder
            | Command::Undo
            | Command::Redo => return None,
        };
        Some(effects)
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

    /// Shows `folder` in the active pane (a sidebar place).
    pub fn go_to(&mut self, folder: VPath) -> Vec<Effect> {
        let side = self.active;
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Takes an event of the loader. A rescan lists the folder again; a vanished folder is
    /// looked for on a worker thread. The ticket of a vanished folder stays: its listing, if
    /// still running, still comes.
    pub fn loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        let Some(side) = self.side_of(event.generation()) else {
            return Vec::new();
        };
        match self.panes[side].handle(event) {
            Handled::Stale | Handled::Applied { .. } => Vec::new(),
            Handled::Reload { request, .. } => vec![Effect::Load(request)],
            Handled::Vanished {
                generation, path, ..
            } => vec![Effect::FindExisting { generation, path }],
        }
    }

    /// Takes where the tab with `generation`, whose folder `path` vanished, can go: `found`,
    /// the nearest folder of `path` that is still there, or home when none is. A tab that has
    /// moved on since takes nothing. When `found` is `path` itself (the folder was replaced,
    /// or only its watch failed), the folder is listed again, at most once per
    /// [`AUTO_RELOAD_GAP`]; otherwise the tab goes there.
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
            let recent = self
                .auto_reloads
                .get(path)
                .is_some_and(|&at| now.saturating_duration_since(at) < AUTO_RELOAD_GAP);
            if recent || self.tab_by_generation(generation).is_none() {
                return Vec::new();
            }
            self.auto_reloads
                .retain(|_, &mut at| now.saturating_duration_since(at) < AUTO_RELOAD_GAP);
            self.auto_reloads.insert(path.clone(), now);
            let Some(tab) = self.tab_by_generation(generation) else {
                return Vec::new();
            };
            return vec![Effect::Load(tab.reload())];
        }
        match self.tab_by_generation(generation) {
            Some(tab) => vec![Effect::Load(tab.navigate(target))],
            None => Vec::new(),
        }
    }

    /// Takes what following a link opened in the tab with `generation` gave. A tab that has
    /// moved on since takes nothing.
    pub fn followed(
        &mut self,
        generation: u64,
        result: Result<OpenAction, ErrorKind>,
    ) -> Vec<Effect> {
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

    /// Shows or hides the hidden items in every tab of both panes.
    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        for pane in &mut self.panes {
            for tab in pane.tabs_mut() {
                tab.set_show_hidden(self.show_hidden);
            }
        }
    }
}

/// `path` and the folders above it, the root first.
pub fn ancestors(path: &VPath) -> Vec<VPath> {
    let mut chain = vec![path.clone()];
    while let Some(parent) = chain.last().and_then(VPath::parent) {
        chain.push(parent);
    }
    chain.reverse();
    chain
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

thread_local! {
    static CONTROLLER: RefCell<Option<Rc<Controller>>> = const { RefCell::new(None) };
}

/// Calls `f` with the controller of this thread's window; `None`, without calling it, when
/// there is none (before the window opens, after it closes, or on another thread).
pub fn with<R>(f: impl FnOnce(&Controller) -> R) -> Option<R> {
    let controller = CONTROLLER.with(|cell| cell.borrow().clone())?;
    Some(f(&controller))
}

/// Connects the window's callbacks to a new controller, starts finding the folders to show
/// and the sidebar's places, and returns the controller. It stays reachable through [`with`]
/// until it is dropped with [`end`].
pub fn begin(window: &MainWindow, start: Option<PathBuf>) -> Rc<Controller> {
    let controller = Rc::new(Controller::new(window));
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&controller)));
    connect(window);

    spawn_worker("rf-ui-start", move || {
        let home = rf_platform::known_folder(KnownFolder::Home);
        let documents = rf_platform::known_folder(KnownFolder::Documents);
        let root = std::path::absolute(std::path::MAIN_SEPARATOR_STR)
            .ok()
            .and_then(|path| VPath::local(path).ok());
        let current_dir = std::env::current_dir().ok();
        let tz = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
        let folders = start_folders(
            start.as_deref(),
            current_dir.as_deref(),
            home.as_ref(),
            documents.as_ref(),
            root.as_ref(),
        );
        if let Some((left, right)) = folders {
            post(move |c| c.open(left, right, home, &tz));
        }
    });
    spawn_worker("rf-ui-places", || {
        let mut found = places::favorites();
        found.extend(places::drives());
        post(move |c| c.set_places(found));
    });
    controller
}

/// Drops the controller of this thread, which stops every listing and watch.
pub fn end() {
    let controller = CONTROLLER.with(|cell| cell.borrow_mut().take());
    drop(controller);
}

/// Runs `f` on a new thread. A thread that cannot start leaves its work undone: the window
/// stays as it is (the system is out of threads).
fn spawn_worker(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.to_owned()).spawn(f);
}

/// The pane a callback's `side` names: 1 is the right one, anything else the left one.
fn side(side: i32) -> usize {
    if side == 1 { RIGHT } else { LEFT }
}

/// A row or tab position from Slint; `None` for a negative one.
fn index(index: i32) -> Option<usize> {
    usize::try_from(index).ok()
}

fn connect(window: &MainWindow) {
    window.on_key(|text, ctrl, shift, alt| {
        with(|c| c.key(&text, Mods { ctrl, shift, alt })).unwrap_or(false)
    });
    window.on_command(|name| {
        with(|c| c.command(&name));
    });
    window.on_pane_activated(|s| {
        with(|c| {
            c.update(|state| {
                state.activate(side(s));
                Vec::new()
            });
        });
    });
    window.on_tab_selected(|s, i| {
        with(|c| c.update_at(i, |state, i| state.select_tab(side(s), i)));
    });
    window.on_tab_closed(|s, i| {
        with(|c| c.update_at(i, |state, i| state.close_tab(side(s), i)));
    });
    window.on_tab_added(|s| {
        with(|c| c.update(|state| state.add_tab(side(s))));
    });
    window.on_crumb_clicked(|s, i| {
        with(|c| c.update_at(i, |state, i| state.open_crumb(side(s), i)));
    });
    window.on_path_entered(|s, text| {
        with(|c| c.update(|state| state.enter_path(side(s), &text)));
    });
    window.on_sort(|s, column| {
        if let Some(key) = ui_bridge::sort_key(column) {
            with(|c| c.update(|state| state.sort(side(s), key)));
        }
    });
    window.on_row_clicked(|s, i, ctrl, shift| {
        with(|c| c.update_at(i, |state, i| state.click_row(side(s), i, ctrl, shift)));
    });
    window.on_row_activated(|s, i| {
        with(|c| c.update_at(i, |state, i| state.open_row(side(s), i)));
    });
    window.on_retry(|s| {
        with(|c| c.update(|state| state.retry(side(s))));
    });
    window.on_place_clicked(|i| {
        with(|c| c.open_place(i));
    });
}

/// Carries out the [`AppState`]'s effects for the window: the loader's tickets, the worker
/// threads, and what the screen shows.
pub struct Controller {
    window: slint::Weak<MainWindow>,
    loader: Loader,
    /// `None` until the folders to start at are known.
    state: RefCell<Option<AppState>>,
    /// The running loads, by the generation of their request.
    tickets: RefCell<HashMap<u64, LoadTicket>>,
    places: RefCell<Vec<Place>>,
    place_rows: Rc<VecModel<PlaceRow>>,
    panes: [PaneModels; 2],
    /// The tab (its position) and folder each pane showed at the last redraw: a pane that
    /// shows another folder or tab scrolls to its cursor, or its top.
    shown: RefCell<[Option<(usize, VPath)>; 2]>,
}

impl fmt::Debug for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Controller")
            .field("state", &self.state)
            .field("tickets", &self.tickets)
            .finish_non_exhaustive()
    }
}

impl Controller {
    fn new(window: &MainWindow) -> Controller {
        let place_rows = Rc::new(VecModel::default());
        window.set_places(ModelRc::from(Rc::clone(&place_rows)));
        Controller {
            window: window.as_weak(),
            loader: Loader::new(rf_vfs::Vfs::with_local()),
            state: RefCell::new(None),
            tickets: RefCell::new(HashMap::new()),
            places: RefCell::new(Vec::new()),
            place_rows,
            panes: [PaneModels::default(), PaneModels::default()],
            shown: RefCell::new([None, None]),
        }
    }

    /// Shows `left` and `right` in the panes, with dates in `tz`, once they are known.
    fn open(&self, left: VPath, right: VPath, home: Option<VPath>, tz: &jiff::tz::TimeZone) {
        for pane in &self.panes {
            pane.set_time_zone(tz.clone());
        }
        let (state, effects) = AppState::new(left, right, home);
        *self.state.borrow_mut() = Some(state);
        self.apply(effects);
    }

    fn set_places(&self, found: Vec<Place>) {
        ui_bridge::set_rows(
            &self.place_rows,
            found.iter().map(ui_bridge::place_row).collect(),
        );
        *self.places.borrow_mut() = found;
    }

    fn open_place(&self, i: i32) {
        let folder = index(i).and_then(|i| Some(self.places.borrow().get(i)?.path.clone()));
        if let Some(folder) = folder {
            self.update(|state| state.go_to(folder));
        }
    }

    /// A key; `true` when the window used it.
    fn key(&self, text: &str, mods: Mods) -> bool {
        let Some(command) = keys::command_for(text, mods) else {
            return false;
        };
        let page = self
            .window
            .upgrade()
            .and_then(|window| usize::try_from(window.get_page_rows()).ok())
            .unwrap_or(1);
        let mut used = false;
        self.update(|state| {
            let effects = state.key(command, mods, page, Instant::now());
            used = effects.is_some();
            effects.unwrap_or_default()
        });
        used
    }

    /// A toolbar button: the same command as its key.
    fn command(&self, name: &str) {
        let command = match name {
            "back" => Command::Back,
            "forward" => Command::Forward,
            "up" => Command::Up,
            "new-folder" => Command::NewFolder,
            "copy" => Command::CopyTo,
            "move" => Command::MoveTo,
            "delete" => Command::Trash,
            "rename" => Command::Rename,
            "undo" => Command::Undo,
            _ => return,
        };
        self.update(|state| {
            state
                .key(command, Mods::default(), 1, Instant::now())
                .unwrap_or_default()
        });
    }

    /// Changes the state with `f`, then carries out what it returns and redraws.
    fn update(&self, f: impl FnOnce(&mut AppState) -> Vec<Effect>) {
        let effects = {
            let mut state = self.state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            f(state)
        };
        self.apply(effects);
    }

    /// [`Controller::update`] for a callback about position `i`; nothing for a negative one.
    fn update_at(&self, i: i32, f: impl FnOnce(&mut AppState, usize) -> Vec<Effect>) {
        if let Some(i) = index(i) {
            self.update(|state| f(state, i));
        }
    }

    fn loaded(&self, event: LoadEvent) {
        self.update(|state| state.loaded(event));
    }

    fn apply(&self, effects: Vec<Effect>) {
        let mut on_screen = Vec::new();
        for effect in effects {
            match effect {
                Effect::Load(request) => {
                    let generation = request.generation;
                    let ticket = self.loader.request(request, |event| {
                        post(move |c| c.loaded(event));
                    });
                    self.tickets.borrow_mut().insert(generation, ticket);
                }
                Effect::FindExisting { generation, path } => {
                    let loader = self.loader.clone();
                    spawn_worker("rf-ui-vanished", move || {
                        std::thread::sleep(VANISHED_SETTLE);
                        let found = loader.nearest_existing_parent(&path);
                        post(move |c| {
                            c.update(|state| {
                                state.found_existing(generation, &path, found, Instant::now())
                            });
                        });
                    });
                }
                Effect::Follow { generation, link } => {
                    let loader = self.loader.clone();
                    spawn_worker("rf-ui-follow", move || {
                        let result = loader.follow(&link);
                        post(move |c| c.update(|state| state.followed(generation, result)));
                    });
                }
                Effect::OpenFile(file) => open::open_file(file, |kind| {
                    post(move |c| c.notice(&words::cannot_open(kind)));
                }),
                Effect::Notice(_) | Effect::EditPath(_) | Effect::ScrollTo { .. } => {
                    on_screen.push(effect);
                }
            }
        }
        self.drop_stale_tickets();
        self.redraw();
        // After the redraw, so that the rows to scroll to are there.
        for effect in on_screen {
            match effect {
                Effect::Notice(text) => self.notice(&text),
                Effect::EditPath(side) => {
                    if let Some(window) = self.window.upgrade() {
                        window.invoke_edit_path(side_number(side));
                    }
                }
                Effect::ScrollTo { side, row } => self.scroll_to(side, row),
                // Carried out above.
                Effect::Load(_)
                | Effect::FindExisting { .. }
                | Effect::Follow { .. }
                | Effect::OpenFile(_) => {}
            }
        }
    }

    /// Stops the loads of requests no tab waits for any more.
    fn drop_stale_tickets(&self) {
        let live = match self.state.borrow().as_ref() {
            Some(state) => state.generations(),
            None => return,
        };
        let stale: Vec<LoadTicket> = {
            let mut tickets = self.tickets.borrow_mut();
            let gone: Vec<u64> = tickets
                .keys()
                .filter(|generation| !live.contains(generation))
                .copied()
                .collect();
            gone.iter()
                .filter_map(|generation| tickets.remove(generation))
                .collect()
        };
        // Dropped outside the borrow: a drop waits for a sink call in progress.
        drop(stale);
    }

    fn notice(&self, text: &str) {
        if let Some(window) = self.window.upgrade() {
            window.invoke_show_notice(text.into());
        }
    }

    fn scroll_to(&self, side: usize, row: usize) {
        if let Some(window) = self.window.upgrade() {
            window.invoke_scroll_to(side_number(side), i32::try_from(row).unwrap_or(i32::MAX));
        }
    }

    /// Shows the state: both panes, and the toolbar's buttons for the active one.
    fn redraw(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let mut scrolls = Vec::new();
        {
            let state = self.state.borrow();
            let Some(state) = state.as_ref() else {
                return;
            };
            let mut shown = self.shown.borrow_mut();
            for side in [LEFT, RIGHT] {
                let pane = &state.panes()[side];
                let view = self.panes[side].view(pane, side == state.active());
                if side == LEFT {
                    window.set_left_pane(view);
                } else {
                    window.set_right_pane(view);
                }
                let tab = pane.active_tab();
                let now_shown = Some((pane.active(), tab.location().clone()));
                if shown[side] != now_shown {
                    shown[side] = now_shown;
                    scrolls.push((side, tab.selection().cursor().unwrap_or(0)));
                }
            }
            let tab = state.panes()[state.active()].active_tab();
            window.set_can_back(tab.history().can_back());
            window.set_can_forward(tab.history().can_forward());
            window.set_can_up(tab.location().parent().is_some());
        }
        for (side, row) in scrolls {
            self.scroll_to(side, row);
        }
    }
}

/// The number Slint uses for pane `side`.
fn side_number(side: usize) -> i32 {
    i32::from(side == RIGHT)
}

#[cfg(test)]
mod tests {
    use rf_core::{Attributes, Entry, EntryKind};
    use rf_vfs::SortOrder;
    use slint::platform::Key;

    use super::*;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File)
    }

    fn dir(name: &str) -> Entry {
        Entry::new(name, EntryKind::Dir)
    }

    fn loads(effects: &[Effect]) -> Vec<&LoadRequest> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Load(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    /// The one request among `effects`.
    fn the_load(effects: &[Effect]) -> LoadRequest {
        match loads(effects).as_slice() {
            [request] => (*request).clone(),
            other => panic!("expected one load, got {other:?} in {effects:?}"),
        }
    }

    /// A window state whose left pane at `a` lists `left` and whose right pane at `b` lists
    /// `right`.
    fn ready(left: Vec<Entry>, right: Vec<Entry>) -> AppState {
        let (mut state, effects) = AppState::new(folder("a"), folder("b"), Some(folder("home")));
        let requests = loads(&effects);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, folder("a"));
        assert_eq!(requests[1].path, folder("b"));
        for (request, entries) in requests.into_iter().zip([left, right]) {
            let generation = request.generation;
            let effects = state.loaded(LoadEvent::Listed {
                generation,
                entries,
            });
            assert_eq!(effects, []);
        }
        state
    }

    fn rows(state: &AppState, side: usize) -> Vec<&str> {
        state.panes()[side]
            .active_tab()
            .visible()
            .iter()
            .map(|entry| entry.name.to_str().expect("test names are text"))
            .collect()
    }

    fn selected(state: &AppState, side: usize) -> Vec<usize> {
        let tab = state.panes()[side].active_tab();
        tab.selection().indices(tab.visible())
    }

    fn cursor(state: &AppState, side: usize) -> Option<usize> {
        state.panes()[side].active_tab().selection().cursor()
    }

    fn press(state: &mut AppState, text: &str, mods: Mods) -> Option<Vec<Effect>> {
        let command = keys::command_for(text, mods).expect("a shortcut");
        state.key(command, mods, 10, Instant::now())
    }

    fn special(key: Key) -> String {
        char::from(key).to_string()
    }

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };

    #[test]
    fn startup_lists_both_panes() {
        let state = ready(vec![dir("x"), file("y")], vec![file("z")]);
        assert_eq!(state.active(), LEFT);
        assert_eq!(rows(&state, LEFT), ["x", "y"]);
        assert_eq!(rows(&state, RIGHT), ["z"]);
        assert_eq!(state.generations().len(), 2);
    }

    #[test]
    fn tab_switches_panes_and_clicks_activate() {
        let mut state = ready(vec![file("x")], vec![file("z")]);
        assert_eq!(press(&mut state, &special(Key::Tab), NONE), Some(vec![]));
        assert_eq!(state.active(), RIGHT);
        press(&mut state, &special(Key::Tab), SHIFT);
        assert_eq!(state.active(), LEFT);
        state.click_row(RIGHT, 0, false, false);
        assert_eq!(state.active(), RIGHT);
        assert_eq!(selected(&state, RIGHT), [0]);
    }

    #[test]
    fn keys_move_the_cursor_and_select() {
        let names = ["a", "b", "c", "d", "e"];
        let mut state = ready(names.iter().copied().map(file).collect(), vec![]);
        let down = special(Key::DownArrow);
        assert_eq!(
            press(&mut state, &down, NONE),
            Some(vec![Effect::ScrollTo { side: LEFT, row: 0 }])
        );
        press(&mut state, &down, NONE);
        assert_eq!(
            (cursor(&state, LEFT), selected(&state, LEFT)),
            (Some(1), vec![1])
        );
        // Shift extends from the anchor.
        press(&mut state, &down, SHIFT);
        press(&mut state, &down, SHIFT);
        assert_eq!(selected(&state, LEFT), [1, 2, 3]);
        // Ctrl+Space flips the cursor's row; Esc clears; Ctrl+A selects all.
        press(&mut state, &special(Key::Space), CTRL);
        assert_eq!(selected(&state, LEFT), [1, 2]);
        press(&mut state, &special(Key::Escape), NONE);
        assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
        assert_eq!(cursor(&state, LEFT), Some(3));
        press(&mut state, "a", CTRL);
        assert_eq!(selected(&state, LEFT), [0, 1, 2, 3, 4]);
        // Home, End and the page keys stop at the ends.
        press(&mut state, &special(Key::Home), NONE);
        assert_eq!(cursor(&state, LEFT), Some(0));
        assert_eq!(
            press(&mut state, &special(Key::PageDown), NONE),
            Some(vec![Effect::ScrollTo { side: LEFT, row: 4 }])
        );
        press(&mut state, &special(Key::End), SHIFT);
        assert_eq!(selected(&state, LEFT), [4]);
        // Clicks: Ctrl adds, Shift takes the range from the anchor.
        state.click_row(LEFT, 1, false, false);
        state.click_row(LEFT, 3, true, false);
        assert_eq!(selected(&state, LEFT), [1, 3]);
        state.click_row(LEFT, 4, false, true);
        assert_eq!(selected(&state, LEFT), [3, 4]);
    }

    #[test]
    fn typing_jumps_to_a_name() {
        let mut state = ready(vec![file("apple"), file("banana"), file("berry")], vec![]);
        let now = Instant::now();
        let effects = state.key(Command::TypeToSelect("be".into()), NONE, 10, now);
        assert_eq!(effects, Some(vec![Effect::ScrollTo { side: LEFT, row: 2 }]));
        assert_eq!(selected(&state, LEFT), [2]);
        // No match moves nothing, and the key is still used.
        let effects = state.key(
            Command::TypeToSelect("x".into()),
            NONE,
            10,
            now + Duration::from_secs(5),
        );
        assert_eq!(effects, Some(vec![]));
        assert_eq!(selected(&state, LEFT), [2]);
    }

    #[test]
    fn open_follows_the_row_kind() {
        let link = Entry::new("latest", EntryKind::Symlink);
        let mut state = ready(vec![dir("sub"), file("notes.txt"), link], vec![]);
        let generation = state.panes()[LEFT].active_tab().generation();

        assert_eq!(
            state.open_row(LEFT, 1),
            [Effect::OpenFile(
                folder("a").join("notes.txt").expect("a name")
            )]
        );
        assert_eq!(
            state.open_row(LEFT, 2),
            [Effect::Follow {
                generation,
                link: folder("a").join("latest").expect("a name"),
            }]
        );
        // Enter opens the cursor's row: the folder shows in the tab.
        state.click_row(LEFT, 0, false, false);
        let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a").join("sub").expect("a name"));
        assert_ne!(request.generation, generation);
        // Back and Up go to `a` again.
        let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("used");
        assert_eq!(the_load(&effects).path, folder("a"));
        let effects = press(&mut state, &special(Key::Backspace), NONE).expect("used");
        let temp = VPath::local(std::env::temp_dir()).expect("a local path");
        assert_eq!(the_load(&effects).path, temp);
    }

    #[test]
    fn a_late_link_result_is_dropped() {
        let mut state = ready(vec![Entry::new("latest", EntryKind::Symlink)], vec![]);
        let effects = state.open_row(LEFT, 0);
        let Effect::Follow { generation, link } = effects[0].clone() else {
            panic!("a link is followed: {effects:?}");
        };
        // The user moves on before the link's target is known.
        state.go_to(folder("elsewhere"));
        let late = state.followed(generation, Ok(OpenAction::Navigate(link.clone())));
        assert_eq!(late, []);
        assert_eq!(
            state.panes()[LEFT].active_tab().location(),
            &folder("elsewhere")
        );

        // In time, a link to a folder shows it, one to a file opens it, and a broken one
        // gives a notice.
        let generation = state.panes()[LEFT].active_tab().generation();
        assert_eq!(
            state.followed(generation, Ok(OpenAction::OpenFile(link.clone()))),
            [Effect::OpenFile(link.clone())]
        );
        assert_eq!(
            state.followed(generation, Err(ErrorKind::NotFound)),
            [Effect::Notice(
                "열 수 없습니다: 찾을 수 없습니다".to_owned()
            )]
        );
        let effects = state.followed(generation, Ok(OpenAction::Navigate(link.clone())));
        assert_eq!(the_load(&effects).path, link);
    }

    #[test]
    fn a_vanished_folder_goes_to_the_nearest_parent() {
        let mut state = ready(vec![file("x")], vec![]);
        let generation = state.panes()[LEFT].active_tab().generation();
        let effects = state.loaded(LoadEvent::Vanished { generation });
        assert_eq!(
            effects,
            [Effect::FindExisting {
                generation,
                path: folder("a"),
            }]
        );
        // The ticket stays until the tab moves on: a listing still running still comes.
        assert!(state.generations().contains(&generation));

        let parent = folder("a").parent().expect("a parent");
        let effects = state.found_existing(
            generation,
            &folder("a"),
            Some(parent.clone()),
            Instant::now(),
        );
        assert_eq!(the_load(&effects).path, parent);
        // A second answer for the same generation is late.
        assert_eq!(
            state.found_existing(generation, &folder("a"), Some(parent), Instant::now()),
            []
        );
    }

    #[test]
    fn a_replaced_folder_is_listed_again_but_not_in_a_loop() {
        let mut state = ready(vec![file("old")], vec![]);
        let now = Instant::now();
        let generation = state.panes()[LEFT].active_tab().generation();
        state.loaded(LoadEvent::Vanished { generation });
        let effects = state.found_existing(generation, &folder("a"), Some(folder("a")), now);
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a"));
        // A reload keeps the rows until the new listing comes.
        assert_eq!(rows(&state, LEFT), ["old"]);

        // The new watch ends at once again: no second reload within the gap.
        state.loaded(LoadEvent::Vanished {
            generation: request.generation,
        });
        let soon = now + Duration::from_secs(1);
        assert_eq!(
            state.found_existing(request.generation, &folder("a"), Some(folder("a")), soon),
            []
        );
        // After the gap it may reload again.
        let later = now + AUTO_RELOAD_GAP + Duration::from_secs(1);
        let effects =
            state.found_existing(request.generation, &folder("a"), Some(folder("a")), later);
        assert_eq!(the_load(&effects).path, folder("a"));
    }

    #[test]
    fn a_folder_with_nothing_left_goes_home() {
        let mut state = ready(vec![], vec![]);
        let generation = state.panes()[RIGHT].active_tab().generation();
        let effects = state.found_existing(generation, &folder("b"), None, Instant::now());
        assert_eq!(the_load(&effects).path, folder("home"));
        assert_eq!(
            state.panes()[RIGHT].active_tab().location(),
            &folder("home")
        );
    }

    #[test]
    fn hidden_items_toggle_in_every_tab() {
        let mut dot = file(".dot");
        dot.attributes |= Attributes::HIDDEN;
        let mut state = ready(vec![dot.clone(), file("x")], vec![dot, file("y")]);
        assert_eq!(rows(&state, LEFT), ["x"]);
        assert_eq!(press(&mut state, "h", CTRL), Some(vec![]));
        assert!(state.show_hidden());
        assert_eq!(rows(&state, LEFT), [".dot", "x"]);
        assert_eq!(rows(&state, RIGHT), [".dot", "y"]);
        // A new tab takes the setting.
        let request = the_load(&press(&mut state, "t", CTRL).expect("used"));
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![{
                let mut dot = file(".new");
                dot.attributes |= Attributes::HIDDEN;
                dot
            }],
        });
        assert_eq!(rows(&state, LEFT), [".new"]);
        press(&mut state, "h", CTRL);
        assert_eq!(rows(&state, LEFT), Vec::<&str>::new());
    }

    #[test]
    fn tabs_open_show_and_close() {
        let mut state = ready(vec![file("x")], vec![]);
        let first = state.panes()[LEFT].active_tab().generation();
        let effects = state.add_tab(LEFT);
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a"));
        assert_eq!(state.panes()[LEFT].tabs().len(), 2);
        assert_eq!(state.panes()[LEFT].active(), 1);
        state.select_tab(LEFT, 0);
        assert_eq!(state.panes()[LEFT].active(), 0);
        // Ctrl+W closes the tab that shows; the last tab stays.
        press(&mut state, "w", CTRL);
        assert_eq!(state.panes()[LEFT].tabs().len(), 1);
        assert!(!state.generations().contains(&first));
        assert!(state.generations().contains(&request.generation));
        press(&mut state, "w", CTRL);
        assert_eq!(state.panes()[LEFT].tabs().len(), 1);
        // A click on the other pane's tab makes that pane active.
        state.close_tab(RIGHT, 0);
        assert_eq!(state.active(), RIGHT);
    }

    #[test]
    fn crumbs_and_typed_paths_navigate() {
        let mut state = ready(vec![], vec![]);
        let chain = ancestors(&folder("a"));
        assert_eq!(chain.last(), Some(&folder("a")));
        assert_eq!(chain[0].parent(), None);
        let effects = state.open_crumb(LEFT, 0);
        assert_eq!(the_load(&effects).path, chain[0]);
        assert_eq!(state.open_crumb(LEFT, 99), []);

        let typed = std::env::temp_dir().join("typed");
        let effects = state.enter_path(RIGHT, &format!("  {}  ", typed.display()));
        assert_eq!(the_load(&effects).path, folder("typed"));
        assert_eq!(state.active(), RIGHT);
        assert_eq!(state.enter_path(RIGHT, "   "), []);
        assert_eq!(
            state.enter_path(RIGHT, "relative/path"),
            [Effect::Notice(
                "열 수 없습니다: 쓸 수 없는 이름이나 경로입니다".to_owned()
            )]
        );
        assert_eq!(
            press(&mut state, "l", CTRL),
            Some(vec![Effect::EditPath(RIGHT)])
        );
    }

    #[test]
    fn sort_and_retry_act_on_the_clicked_pane() {
        // The loader sends a listing sorted by name.
        let mut state = ready(vec![file("a"), file("b")], vec![]);
        assert_eq!(rows(&state, LEFT), ["a", "b"]);
        state.sort(LEFT, SortKey::Name);
        assert_eq!(rows(&state, LEFT), ["b", "a"]);
        let sort = state.panes()[LEFT].active_tab().sort();
        assert_eq!(sort.primary.order, SortOrder::Descending);
        let effects = state.retry(RIGHT);
        assert_eq!(the_load(&effects).path, folder("b"));
        assert_eq!(state.active(), RIGHT);
    }

    #[test]
    fn file_commands_are_left_to_the_system() {
        let mut state = ready(vec![file("x")], vec![]);
        for (text, mods) in [
            (special(Key::F2), NONE),
            (special(Key::Delete), NONE),
            (special(Key::Delete), SHIFT),
            (special(Key::F5), NONE),
            (special(Key::F6), NONE),
            (special(Key::F7), NONE),
            ("z".to_owned(), CTRL),
            ("y".to_owned(), CTRL),
        ] {
            assert_eq!(press(&mut state, &text, mods), None, "{text:?}");
        }
        assert_eq!(rows(&state, LEFT), ["x"]);
    }

    #[test]
    fn late_events_of_a_closed_tab_are_dropped() {
        let mut state = ready(vec![file("x")], vec![]);
        let request = the_load(&state.add_tab(LEFT));
        state.close_tab(LEFT, 1);
        let effects = state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![file("late")],
        });
        assert_eq!(effects, []);
        assert_eq!(rows(&state, LEFT), ["x"]);
        assert_eq!(
            state.found_existing(request.generation, &folder("a"), None, Instant::now()),
            []
        );
    }

    #[test]
    fn start_folders_fall_back_to_home() {
        let (home, docs, root) = (folder("home"), folder("docs"), folder("root"));
        let cwd = std::env::temp_dir();
        assert_eq!(
            start_folders(None, Some(&cwd), Some(&home), Some(&docs), Some(&root)),
            Some((home.clone(), docs.clone()))
        );
        assert_eq!(
            start_folders(
                Some(Path::new("rel")),
                Some(&cwd),
                Some(&home),
                None,
                Some(&root)
            ),
            Some((folder("rel"), home.clone()))
        );
        let absolute = std::env::temp_dir().join("abs");
        assert_eq!(
            start_folders(Some(&absolute), None, Some(&home), None, None),
            Some((folder("abs"), home))
        );
        assert_eq!(
            start_folders(None, None, None, None, Some(&root)),
            Some((root.clone(), root))
        );
        assert_eq!(start_folders(None, None, None, None, None), None);
    }
}
