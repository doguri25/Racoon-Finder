//! Carries out the main window's [`AppState`] changes: Slint's callbacks, the loader's
//! tickets, the worker threads, and what the screen shows.
//!
//! The [`Controller`] lives in a thread-local of the UI thread: Slint's callbacks reach it
//! there, and so do the results of worker threads, through [`post`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rf_core::VPath;
use rf_platform::KnownFolder;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::keys::{self, Command, Mods};
use crate::loader::{LoadEvent, LoadTicket, Loader};
use crate::places::{self, Place};
use crate::state::{AppState, Effect, LEFT, RIGHT, start_folders};
use crate::ui_bridge::{self, PaneModels, post};
use crate::{MainWindow, PlaceRow, open, words};

/// How long the worker waits after a folder's watch ended before it looks for the folder:
/// a folder replaced in one go (`rm -rf dist && mkdir dist`) is back by then, and is listed
/// again instead of being left for its parent.
pub const VANISHED_SETTLE: Duration = Duration::from_millis(300);
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
                    self.loader.follow(generation, link, |followed| {
                        post(move |c| c.update(|state| state.followed(followed)));
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
