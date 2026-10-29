//! Carries out the main window's [`AppState`] changes: Slint's callbacks, the loader's
//! tickets, the worker threads, the job engine, and what the screen shows.
//!
//! The [`Controller`] lives in a thread-local of the UI thread: Slint's callbacks reach it
//! there, and so do the results of worker threads, through [`post`], and the events and
//! questions of the job engine, through [`job_event`] and [`post_question`].
//!
//! It also keeps the [`Settings`] (module `persist`): it opens the tabs of the last run, shows the
//! theme chosen, and after a change writes them again on a worker thread, [`SAVE_DELAY`] later,
//! once for every change in that time; and a last time as the app ends.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use rf_core::VPath;
use rf_jobs::{JobEvent, TransferMode};
use rf_platform::KnownFolder;
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, VecModel};

use crate::dialogs::{ConflictChoice, Modal};
use crate::jobs::{Jobs, QUIT_TIMEOUT};
use crate::keys::{self, Command, Mods};
use crate::loader::{LoadEvent, LoadTicket, Loader};
use crate::pick::POLICIES;
use crate::places::{self, Place};
use crate::resolver::Question;
use crate::settings::{Saver, Session, SessionPane, Settings, ThemeChoice, Waiting};
use crate::state::{AppState, Effect, LEFT, RIGHT, start_folders};
use crate::ui_bridge::{self, PaneModels, PickModels, TreeRows, post};
use crate::{DialogKind, JobRowView, MainWindow, open, words};

mod persist;

pub use persist::{RESTORE_WAIT, SAVE_DELAY};
use persist::{logical, look_for, theme_number, theme_of, wait_for};

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

/// Hands an event of the job engine to the controller, from any thread (the bridge thread of
/// [`Jobs::start`]).
pub fn job_event(event: JobEvent) {
    post(move |c| c.job_event(event));
}

/// Hands a question of a job to the controller, from the job's worker thread; `false` when the
/// event loop has ended, and nobody is left to answer it.
pub fn post_question(question: Question) -> bool {
    slint::invoke_from_event_loop(move || {
        // Without a controller (the window has closed) the question stays open until the
        // engine's end releases it.
        with(|c| c.question(question));
    })
    .is_ok()
}

/// What the window starts with, besides the job engine.
#[derive(Debug)]
pub struct Startup {
    /// The folder the app's first argument names: the left pane opens it alone, in place of
    /// the tabs of the last run.
    pub start: Option<PathBuf>,
    /// The settings of the last run ([`crate::settings::load`]).
    pub settings: Settings,
    /// Writes them back.
    pub saver: Saver,
}

/// Connects the window's callbacks to a new controller with the job engine `jobs`, gives the
/// window the theme and the size of the settings (before it shows), starts finding the folders
/// to show and the sidebar's places, and returns the controller. It stays reachable through
/// [`with`] until it is dropped with [`end`].
pub fn begin(window: &MainWindow, startup: Startup, jobs: Jobs) -> Rc<Controller> {
    let Startup {
        start,
        settings,
        saver,
    } = startup;
    if let Some((width, height)) = settings.window_size() {
        window.set_start_width(logical(width));
        window.set_start_height(logical(height));
    }
    let mut session = settings.session();
    if start.is_some() {
        session.panes[LEFT] = SessionPane::default();
    }
    window.set_theme_choice(theme_number(settings.theme));
    let controller = Rc::new(Controller::new(window, jobs, settings, saver));
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&controller)));
    connect(window);

    let loader = controller.loader.clone();
    spawn_worker("rf-ui-start", move || {
        let home = rf_platform::known_folder(KnownFolder::Home);
        let documents = rf_platform::known_folder(KnownFolder::Documents);
        let root = std::path::absolute(std::path::MAIN_SEPARATOR_STR)
            .ok()
            .and_then(|path| VPath::local(path).ok());
        let current_dir = std::env::current_dir().ok();
        let tz = TimeZone::try_system().unwrap_or(TimeZone::UTC);
        let folders = start_folders(
            start.as_deref(),
            current_dir.as_deref(),
            home.as_ref(),
            documents.as_ref(),
            root.as_ref(),
        );
        let (Some((left, right)), Some(fallback)) = (folders, home.clone().or(root)) else {
            return;
        };
        let checks = look_for(&loader, session.folders());
        let found = wait_for(&checks, session.folders().len(), RESTORE_WAIT);
        let (session, waiting) = session.resolve(&fallback, |path| found.get(path).copied());
        post(move |c| c.open(left, right, session, &waiting, home, &tz));
        // The folders not known in time, as they come.
        for (path, exists) in checks {
            post(move |c| c.update(|state| state.restored_folder(&path, exists)));
        }
    });
    spawn_worker("rf-ui-places", || {
        let mut found = places::favorites();
        found.extend(places::drives());
        post(move |c| c.set_places(&found));
    });
    controller
}

/// Drops the controller of this thread, which stops every listing and watch.
pub fn end() {
    let controller = CONTROLLER.with(|cell| cell.borrow_mut().take());
    drop(controller);
}

/// Runs `f` on a new thread. A thread that cannot start (the system is out of threads or
/// memory) leaves its work undone, and a notice says so.
fn spawn_worker(name: &str, f: impl FnOnce() + Send + 'static) {
    if std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(f)
        .is_err()
    {
        post(|c| c.notice(words::background_failed()));
    }
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
    window.on_theme_chosen(|choice| {
        if let Some(theme) = theme_of(choice) {
            with(|c| c.choose_theme(theme));
        }
    });
    window.on_window_resized(|| {
        with(Controller::plan_save);
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
    window.on_tree_clicked(|i| {
        with(|c| c.update_at(i, AppState::open_tree_row));
    });
    window.on_tree_toggled(|i| {
        with(|c| c.update_at(i, AppState::toggle_tree_row));
    });
    connect_jobs(window);
    connect_pick(window);
}

/// The callbacks of the destination picker.
fn connect_pick(window: &MainWindow) {
    window.on_pick_target_clicked(|i| {
        with(|c| c.update_at(i, AppState::pick_clicked));
    });
    window.on_pick_step(|delta| {
        let delta = if delta < 0 { -1 } else { 1 };
        with(|c| c.update(|state| state.pick_step(delta)));
    });
    window.on_pick_enter(|| {
        with(|c| c.update(AppState::pick_enter));
    });
    window.on_pick_confirmed(|| {
        with(|c| c.update(AppState::pick_confirmed));
    });
    window.on_pick_mode(|is_move| {
        let mode = if is_move {
            TransferMode::Move
        } else {
            TransferMode::Copy
        };
        with(|c| {
            c.update(|state| {
                state.pick_mode(mode);
                Vec::new()
            });
        });
    });
    window.on_pick_policy(|i| {
        if let Some(&policy) = index(i).and_then(|i| POLICIES.get(i)) {
            with(|c| {
                c.update(|state| {
                    state.pick_policy(policy);
                    Vec::new()
                });
            });
        }
    });
    window.on_pick_filter_edited(|text| {
        with(|c| c.update(|state| state.pick_filter(&text)));
    });
    window.on_pick_path_edited(|text| {
        with(|c| {
            c.update(|state| {
                state.pick_path_edited(&text);
                Vec::new()
            });
        });
    });
    window.on_pick_path_entered(|text| {
        with(|c| c.update(|state| state.pick_path_entered(&text)));
    });
    window.on_pick_tree_clicked(|i| {
        with(|c| c.update_at(i, AppState::pick_tree_clicked));
    });
    window.on_pick_tree_toggled(|i| {
        with(|c| c.update_at(i, AppState::pick_tree_toggled));
    });
    window.on_pick_new_folder(|| {
        with(|c| c.update(AppState::pick_new_folder));
    });
}

/// The callbacks of the dialogs, the job panel and the toast, and the close button.
fn connect_jobs(window: &MainWindow) {
    window.on_row_menu(|s, i| {
        with(|c| c.update_at(i, |state, i| state.row_menu(side(s), i)));
    });
    window.on_name_edited(|text| {
        with(|c| {
            c.update(|state| {
                state.name_edited(&text);
                Vec::new()
            });
        });
    });
    window.on_name_accepted(|text| {
        with(|c| c.update(|state| state.name_confirmed(&text)));
    });
    window.on_dialog_cancelled(|| {
        with(|c| c.update(AppState::dialog_cancelled));
    });
    window.on_confirm_accepted(|| {
        with(|c| c.update(AppState::confirm_accepted));
    });
    window.on_conflict_answered(|choice, apply_all| {
        let choice = match choice {
            0 => ConflictChoice::Overwrite,
            1 => ConflictChoice::Skip,
            _ => ConflictChoice::Rename,
        };
        with(|c| c.update(|state| state.conflict_answered(choice, apply_all)));
    });
    window.on_preflight_answered(|choice| {
        with(|c| c.update(|state| state.preflight_answered(choice)));
    });
    window.on_recovery_chosen(|i, run_again| {
        with(|c| c.update_at(i, |state, i| state.recovery_chosen(i, run_again)));
    });
    window.on_job_pause(|i| {
        with(|c| c.update_at(i, |state, i| state.job_control(i, 0)));
    });
    window.on_job_resume(|i| {
        with(|c| c.update_at(i, |state, i| state.job_control(i, 1)));
    });
    window.on_job_cancel(|i| {
        with(|c| c.update_at(i, |state, i| state.job_control(i, 2)));
    });
    window.on_notice_action(|token| {
        if let Ok(token) = u64::try_from(token) {
            with(|c| c.update(|state| state.toast_clicked(token)));
        }
    });
    window.on_notice_closed(|token| {
        if let Ok(token) = u64::try_from(token) {
            with(|c| {
                c.update(|state| {
                    state.toast_closed(token);
                    Vec::new()
                });
            });
        }
    });
    // The window stays until the jobs have stopped (carry-m1-03 "quit order"); the controller
    // ends the event loop then. Without a controller nothing holds it.
    window.window().on_close_requested(|| {
        if with(Controller::close_requested).is_some() {
            CloseRequestResponse::KeepWindowShown
        } else {
            CloseRequestResponse::HideWindow
        }
    });
}

/// Carries out the [`AppState`]'s effects for the window: the loader's tickets, the worker
/// threads, and what the screen shows.
pub struct Controller {
    window: slint::Weak<MainWindow>,
    loader: Loader,
    jobs: Jobs,
    /// `None` until the folders to start at are known.
    state: RefCell<Option<AppState>>,
    /// The engine's events and questions that came before the state; it takes them when it
    /// comes (the journal's recovery is announced at once).
    early: RefCell<Vec<Early>>,
    /// The time zone of the dates the dialogs show, once known.
    tz: RefCell<TimeZone>,
    job_rows: Rc<VecModel<JobRowView>>,
    /// The dialog the screen shows, as it was last handed over: a view is set again only when
    /// it changed, so a list in it keeps its scroll position while the jobs report progress.
    shown_modal: RefCell<Option<(DialogKind, Option<Modal>)>>,
    /// The models of the destination picker's view.
    pick: PickModels,
    /// The running loads of the tabs and the tree, by the generation of their request.
    tickets: RefCell<HashMap<u64, LoadTicket>>,
    /// The sidebar's places, the roots of the tree, once found.
    places: RefCell<Vec<Place>>,
    tree_rows: Rc<TreeRows>,
    panes: [PaneModels; 2],
    /// The tab (its position) and folder each pane showed at the last redraw: a pane that
    /// shows another folder or tab scrolls to its cursor, or its top.
    shown: RefCell<[Option<(usize, VPath)>; 2]>,
    /// The settings as last read or written, with the keys of a newer version: a save writes
    /// them with what the window shows then, and only when that differs.
    settings: RefCell<Settings>,
    saver: Saver,
    /// Runs [`SAVE_DELAY`] after the first change since the last save.
    save_timer: slint::Timer,
    /// The settings have been taken for the last write, as the app ends: none is planned any
    /// more.
    settings_closed: Cell<bool>,
    /// Whether a failed save has been told (once is enough).
    save_failure_told: Cell<bool>,
    /// The theme chosen in the 보기 menu.
    theme: Cell<ThemeChoice>,
}

impl fmt::Debug for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Controller")
            .field("state", &self.state)
            .field("tickets", &self.tickets)
            .finish_non_exhaustive()
    }
}

/// What the job engine told before the state was there.
#[derive(Debug)]
enum Early {
    Event(JobEvent),
    Question(Question),
}

impl Controller {
    fn new(window: &MainWindow, jobs: Jobs, settings: Settings, saver: Saver) -> Controller {
        let tree_rows = Rc::new(TreeRows::default());
        window.set_tree(ModelRc::from(Rc::clone(&tree_rows)));
        let job_rows = Rc::new(VecModel::default());
        window.set_job_rows(ModelRc::from(Rc::clone(&job_rows)));
        Controller {
            window: window.as_weak(),
            loader: Loader::new(rf_vfs::Vfs::with_local()),
            jobs,
            state: RefCell::new(None),
            early: RefCell::new(Vec::new()),
            tz: RefCell::new(TimeZone::UTC),
            job_rows,
            shown_modal: RefCell::new(None),
            pick: PickModels::default(),
            tickets: RefCell::new(HashMap::new()),
            places: RefCell::new(Vec::new()),
            tree_rows,
            panes: [PaneModels::default(), PaneModels::default()],
            shown: RefCell::new([None, None]),
            theme: Cell::new(settings.theme),
            settings: RefCell::new(settings),
            saver,
            save_timer: slint::Timer::default(),
            settings_closed: Cell::new(false),
            save_failure_told: Cell::new(false),
        }
    }

    /// Shows the tabs of `session` in the panes (`left` and `right` for a pane without), with
    /// dates in `tz`, once they are known, and the tree of the places if they came first. The
    /// tabs of `waiting` show home until their folders are found.
    fn open(
        &self,
        left: VPath,
        right: VPath,
        session: Session,
        waiting: &[Waiting],
        home: Option<VPath>,
        tz: &TimeZone,
    ) {
        for pane in &self.panes {
            pane.set_time_zone(tz.clone());
        }
        *self.tz.borrow_mut() = tz.clone();
        let (mut state, mut effects) = AppState::restore(left, right, session, waiting, home);
        state.set_recent_targets(self.settings.borrow().recent());
        let places = self.places.borrow().clone();
        if !places.is_empty() {
            effects.extend(state.set_places(&places));
        }
        effects.extend(state.follow_active());
        state.set_history(self.jobs.history());
        for early in self.early.take() {
            effects.extend(match early {
                Early::Event(event) => state.job_event(event),
                Early::Question(question) => state.question(question),
            });
        }
        *self.state.borrow_mut() = Some(state);
        self.apply(effects);
    }

    /// An event of the job engine. Progress only redraws the job panel; an end, the journal's
    /// news and the like change the state and the whole window.
    fn job_event(&self, event: JobEvent) {
        if self.state.borrow().is_none() {
            self.early.borrow_mut().push(Early::Event(event));
            return;
        }
        let quiet = matches!(
            event,
            JobEvent::Progress { .. }
                | JobEvent::Planned { .. }
                | JobEvent::State { .. }
                | JobEvent::Queue { .. }
        );
        if quiet {
            let effects = match self.state.borrow_mut().as_mut() {
                Some(state) => state.job_event(event),
                None => return,
            };
            if effects.is_empty() {
                self.redraw_jobs();
            } else {
                self.apply(effects);
            }
            return;
        }
        // The engine notes a job's record before its end is announced, so the history is up
        // to date now.
        let history = self.jobs.history();
        self.update(|state| {
            state.set_history(history);
            state.job_event(event)
        });
    }

    /// A question of a job.
    fn question(&self, question: Question) {
        if self.state.borrow().is_none() {
            self.early.borrow_mut().push(Early::Question(question));
            return;
        }
        self.update(|state| state.question(question));
    }

    /// The window's close button: the state asks first if jobs run, or ends the app.
    fn close_requested(&self) {
        if self.state.borrow().is_none() {
            // Nothing shows yet, so nothing can be under way.
            self.quit();
            return;
        }
        self.update(AppState::request_close);
    }

    /// Ends the app: on a worker thread the engine shuts down and the questions are released
    /// ([`Jobs::close_down`]), then the settings are written a last time, and then the event
    /// loop ends. The window shows the closing note meanwhile. A worker that cannot start
    /// leaves the shutdown and the settings to [`crate::run`], after the event loop.
    fn quit(&self) {
        let jobs = self.jobs.clone();
        let saver = self.saver.clone();
        let last = self.final_settings();
        let taken = last.is_some();
        let started = std::thread::Builder::new()
            .name("rf-ui-quit".to_owned())
            .spawn(move || {
                jobs.close_down(QUIT_TIMEOUT);
                if let Some(settings) = last {
                    // After a write in progress; the app ends either way.
                    let _ = saver.save(&settings);
                }
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            });
        if started.is_err() {
            if taken {
                self.settings_closed.set(false);
            }
            let _ = slint::quit_event_loop();
        }
    }

    /// Makes `found` the places of the sidebar: the roots of the tree, now or once the panes'
    /// folders are known.
    fn set_places(&self, found: &[Place]) {
        found.clone_into(&mut self.places.borrow_mut());
        self.update(|state| state.set_places(found));
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

    /// A toolbar button or a menu item: the same command as its key.
    fn command(&self, name: &str) {
        let command = match name {
            "back" => Command::Back,
            "forward" => Command::Forward,
            "up" => Command::Up,
            "new-folder" => Command::NewFolder,
            "copy" => Command::CopyTo,
            "move" => Command::MoveTo,
            "delete" => Command::Trash,
            "delete-permanently" => Command::DeletePermanently,
            "rename" => Command::Rename,
            "open" => Command::Open,
            "undo" => Command::Undo,
            "redo" => Command::Redo,
            "toggle-hidden" => Command::ToggleHidden,
            _ => return,
        };
        self.update(|state| {
            state
                .key(command, Mods::default(), 1, Instant::now())
                .unwrap_or_default()
        });
    }

    /// Changes the state with `f`, lets the tree follow the active pane, then carries out what
    /// both return and redraws.
    fn update(&self, f: impl FnOnce(&mut AppState) -> Vec<Effect>) {
        let effects = {
            let mut state = self.state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            let mut effects = f(state);
            effects.extend(state.follow_active());
            effects
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
        let mut job_asked = false;
        for effect in effects {
            match effect {
                Effect::Job(effect) => {
                    job_asked = true;
                    if let Some(started) = self.jobs.carry_out(effect)
                        && let Some(state) = self.state.borrow_mut().as_mut()
                    {
                        state.track(started);
                    }
                }
                Effect::CheckTrash { folder, paths } => {
                    spawn_worker("rf-ui-trash", move || {
                        let supported = rf_platform::trash_supported(&folder);
                        post(move |c| c.update(|state| state.trash_checked(paths, supported)));
                    });
                }
                Effect::Quit => self.quit(),
                Effect::Load(request) => {
                    let generation = request.generation;
                    let ticket = self.loader.request(request, |event| {
                        post(move |c| c.loaded(event));
                    });
                    self.tickets.borrow_mut().insert(generation, ticket);
                }
                Effect::LoadChildren(request) => {
                    let generation = request.generation;
                    let ticket = self.loader.list_folders(request, |event| {
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
                Effect::ReloadLater {
                    generation,
                    path,
                    after,
                } => slint::Timer::single_shot(after, move || {
                    with(|c| c.update(|state| state.reload_due(generation, &path, Instant::now())));
                }),
                Effect::InspectTarget {
                    generation,
                    path,
                    from_dir,
                } => {
                    let loader = self.loader.clone();
                    spawn_worker("rf-ui-pick", move || {
                        let facts = loader.inspect_target(&path, &from_dir);
                        post(move |c| {
                            c.update(|state| state.target_inspected(generation, &path, facts));
                        });
                    });
                }
                Effect::Notice(_)
                | Effect::EditPath(_)
                | Effect::ScrollTo { .. }
                | Effect::ScrollTree(_)
                | Effect::ScrollPickTree(_)
                | Effect::Toast { .. } => {
                    on_screen.push(effect);
                }
            }
        }
        if job_asked {
            // An undo or a redo took its record off the history; a new job will note one when
            // it ends.
            let history = self.jobs.history();
            if let Some(state) = self.state.borrow_mut().as_mut() {
                state.set_history(history);
            }
        }
        self.drop_stale_tickets();
        self.redraw();
        // After the redraw, so that the rows to scroll to are there.
        for effect in on_screen {
            self.show(effect);
        }
        // Any change may be one the settings keep (a tab, the hidden items, a recent target).
        self.plan_save();
    }

    /// Does an effect that shows on the screen.
    fn show(&self, effect: Effect) {
        match effect {
            Effect::Notice(text) => self.notice(&text),
            Effect::EditPath(side) => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_edit_path(side_number(side));
                }
            }
            Effect::ScrollTo { side, row } => self.scroll_to(side, row),
            Effect::ScrollTree(row) => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_tree_scroll_to(i32::try_from(row).unwrap_or(i32::MAX));
                }
            }
            Effect::ScrollPickTree(row) => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_pick_tree_scroll_to(i32::try_from(row).unwrap_or(i32::MAX));
                }
            }
            Effect::Toast {
                token,
                text,
                action,
            } => {
                if let Some(window) = self.window.upgrade() {
                    // A token past the range of a Slint int has no button to click.
                    let token = i32::try_from(token).unwrap_or(0);
                    window.invoke_show_toast(text.into(), action.into(), token);
                }
            }
            // Carried out by `apply`.
            Effect::Load(_)
            | Effect::LoadChildren(_)
            | Effect::FindExisting { .. }
            | Effect::Follow { .. }
            | Effect::ReloadLater { .. }
            | Effect::OpenFile(_)
            | Effect::Job(_)
            | Effect::CheckTrash { .. }
            | Effect::InspectTarget { .. }
            | Effect::Quit => {}
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

    /// Shows the state: both panes, the toolbar's buttons for the active one, and the tree.
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
            window.set_show_hidden(state.show_hidden());
            self.tree_rows.sync(state.tree(), &self.places.borrow());
            self.show_jobs(&window, state);
            self.show_dialog(&window, state);
        }
        for (side, row) in scrolls {
            self.scroll_to(side, row);
        }
    }

    /// Shows the job panel and the toolbar's Undo, after a job's progress or state changed.
    fn redraw_jobs(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if let Some(state) = self.state.borrow().as_ref() {
            self.show_jobs(&window, state);
        }
    }

    fn show_jobs(&self, window: &MainWindow, state: &AppState) {
        let rows = state.job_rows();
        ui_bridge::set_rows(
            &self.job_rows,
            rows.iter().map(ui_bridge::job_row_view).collect(),
        );
        window.set_can_undo(state.can_undo());
        window.set_notice_action_live(state.toast_button_live());
    }

    /// Shows the dialog of the state, setting its view before its kind (the dialog reads the
    /// view when it opens), and only when it changed. The destination picker's view is set at
    /// every redraw while it is on screen, under another dialog too: its tree and what a
    /// worker found at its target change without a change of the dialog.
    fn show_dialog(&self, window: &MainWindow, state: &AppState) {
        let kind = ui_bridge::dialog_kind(state.modal(), state.quitting());
        let picker = state.shown_picker();
        if let Some(picker) = picker {
            let found = state.pick_found();
            window.set_pick_view(self.pick.view(
                picker,
                state.pick_tree(),
                found.as_deref(),
                &self.places.borrow(),
            ));
        }
        // The picker goes under the other dialog before that one shows, and comes on top
        // after it is the dialog again, so that it stays one picker all along.
        let behind = picker.is_some() && kind != DialogKind::Pick;
        if behind {
            window.set_pick_behind(true);
        }
        let now = (kind, state.modal().cloned());
        if self.shown_modal.borrow().as_ref() == Some(&now) {
            if !behind {
                window.set_pick_behind(false);
            }
            return;
        }
        let tz = self.tz.borrow();
        match &now.1 {
            _ if kind == DialogKind::Closing => {
                window.set_closing_text(words::closing().into());
            }
            Some(Modal::Name(dialog)) => window.set_name_view(ui_bridge::name_view(dialog)),
            Some(Modal::Confirm(confirm)) => {
                window.set_confirm_view(ui_bridge::confirm_view(confirm));
            }
            Some(Modal::Failures(failures)) => {
                window.set_message_view(ui_bridge::message_view(failures));
            }
            Some(Modal::Recovery(recovery)) => {
                window.set_recovery_view(ui_bridge::recovery_view(recovery, &tz));
            }
            Some(Modal::Conflict(question)) => {
                window.set_conflict_view(ui_bridge::conflict_view(question, &tz));
            }
            Some(Modal::Preflight(question)) => {
                window.set_preflight_view(ui_bridge::preflight_view(question));
            }
            // Set above.
            Some(Modal::Pick(_)) | None => {}
        }
        window.set_dialog(kind);
        if !behind {
            window.set_pick_behind(false);
        }
        *self.shown_modal.borrow_mut() = Some(now);
    }
}

/// The number Slint uses for pane `side`.
fn side_number(side: usize) -> i32 {
    i32::from(side == RIGHT)
}
