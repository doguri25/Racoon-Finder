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
//!
//! The system clipboard has a worker of its own (module `clipboard`), which carries out the
//! state's requests in order; while items the window cut are on the clipboard, the controller
//! asks it for the clipboard's change count every [`CLIPBOARD_POLL`], so that the dimming goes
//! once another app has put something else there.
//!
//! The properties window has two workers (module `properties`): the measure and the checksums.
//! Their news changes only the window, so it redraws the window alone, as the progress of a job
//! redraws the job panel alone.
//!
//! What the window asks of the system itself (연결 프로그램, a program run as administrator, a
//! terminal, the system's properties window) goes to a worker thread of its own each time
//! ([`open::shell`]): none of those calls needs the main thread, and some block until the user
//! closes a dialog.
//!
//! A drag of another app over the window (DND-05) is told by the system: where its pointer is,
//! the keys held, what it allows and what it carries. [`Controller::outside_over`],
//! [`Controller::outside_drop`] and [`Controller::outside_leave`] answer it with the state's
//! module `outside`, and show it as a drag of rows shows: the place under the pointer
//! (`ui_bridge::drop_spot`), the badge beside it, the frame of the place, the time of a rest.
//! Each borrows the state briefly, and answers that nothing is done when it is borrowed already
//! (the system may call it from a loop of its own that runs inside the app's code); a drop's
//! effects are carried out on the next turn of the event loop, once the system has its answer.
//! The system tells of it through the window's drop target (`rf_platform::dnd`), which goes on
//! the window's handle at each show of the window, again for a new handle (module `drops`,
//! [`window_shown`]). Where the platform has none (Linux), and where winit's own drop target
//! still takes the drag (macOS: over the title bar, or everywhere should the platform's view
//! over the content take no drag), winit tells only the files dropped on the window, one by one
//! (`DroppedFile`, without the pointer or the keys), which go into the folder the active pane
//! shows, as a copy (`Controller::file_dropped`).
//!
//! A drag of rows that goes past the window's edges goes on as the system's drag (DND-05 보내기,
//! `Controller::drag_leaves`), on the next turn of the event loop: Slint's drag ends with no
//! drop, what it started stops, and `rf_platform::dnd::drag_out` carries the items to the app
//! that takes them, with no borrow of the state held (on Windows the system's drag runs inside
//! that call, and the window's own drop target answers it there). The app deletes nothing once
//! it ends ([`AppState::drag_out_ended`]). Where the system takes no such drag (Linux), the drag
//! stays Slint's.
//!
//! The system's own menu of items (Windows' shell menu, OPS-05) and its picker of the ways to
//! share them (macOS) run on the UI thread, on the next turn of the event loop after they are
//! asked for, with no borrow of the state held (module `system_menu`).
//!
//! The notifications of jobs that ended go to the system on worker threads too
//! ([`OsNotifier`]), and a job's log is written on one ([`jobs::save_log`]). While the window is
//! hidden and the jobs go on behind the tray icon (XFER-15), the icon follows the state at each
//! redraw ([`Tray`]); the event loop runs until the controller ends it, so the hidden window
//! does not end it, and the app ends as it always does: settings, then the engine.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use rf_core::{Os, VPath};
use rf_jobs::{JobEvent, TransferMode};
use rf_platform::KnownFolder;
use rf_platform::dnd::{
    self, DragMods, DropEffect, DropHint, DropRegistration, DroppedItems, Effects,
};
use rf_platform::shell::{Owner, WindowPoint};
use slint::language::DragAction;
use slint::winit_030::winit::event::{
    ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent as WinitEvent,
};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, DataTransfer, ModelRc, VecModel};

use crate::dialogs::{ConflictChoice, Modal};
use crate::format::folder_label;
use crate::hashing::{self, HASH_CHUNK, HashNews, PROGRESS_GAP};
use crate::instance::RequestStop;
use crate::jobs::{self, Jobs, QUIT_TIMEOUT};
use crate::keys::{self, Command, Mods};
use crate::loader::{CopyChecks, LoadEvent, LoadTicket, Loader};
use crate::notify::{NOTICE_WAIT, Notifier, OsNotifier, SlintTray, Tray};
use crate::open::ShellNews;
use crate::pick::POLICIES;
use crate::places::{self, Place};
use crate::preview::{self, PREVIEW_DELAY};
use crate::properties::{MeasureNews, Tally};
use crate::resolver::Question;
use crate::settings::{Saver, Session, Settings, ThemeChoice, Waiting};
use crate::state::{
    AppState, ClipboardFailure, Column, DropSpot, Effect, Hover, LEFT, Layout, LinkFacts,
    MenuShown, Paging, PathForm, RIGHT, ShellWork, Subfolders, Unkept, dropped_effect,
    preview_width, start_folders,
};
use crate::ui_bridge::{
    self, PaneModels, PickModels, PreviewModels, PropertiesModels, RecentBlock, RecentFolder,
    RowSlot, TreeRows, post,
};
use crate::{
    DialogKind, DragPointer, DropPlace, FolderMenuRow, ItemMenu, JobRowView, MainWindow,
    RecentRowView, TabDrop, open, words,
};

pub mod clipboard;
mod drops;
mod matcher;
mod newest;
mod persist;
mod previewer;
mod properties;
mod sorter;
mod spring;
mod system_menu;
mod thumber;

use clipboard::{Clipper, Done, OsClipboard};
use drops::{DropTarget, WindowDrops};
use matcher::Matcher;
use newest::Outcome;
pub(crate) use persist::theme_number;
pub use persist::{FINAL_SAVE_WAIT, RESTORE_WAIT, RESTORE_WORKERS, SAVE_DELAY};
use persist::{SaveLedger, logical, look_for, theme_of, wait_for};
pub use previewer::Previewer;
use properties::{Hasher, Measurer};
use sorter::Sorter;
use spring::Spring;
use thumber::Thumber;

/// How long the worker waits after a folder's watch ended before it looks for the folder:
/// a folder replaced in one go (`rm -rf dist && mkdir dist`) is back by then, and is listed
/// again instead of being left for its parent.
pub const VANISHED_SETTLE: Duration = Duration::from_millis(300);

/// How often the clipboard's change count is asked while items the window cut are on it.
pub const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

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

/// A second start of the app asked this one to show its window ([`crate::instance`]), from
/// the thread that looks for such requests.
pub fn show_requested() {
    post(|c| c.update(AppState::show_asked));
}

/// The window shows (its first show, its return from the tray): once its winit window is
/// there, which it is not until the event loop runs at the first show and which may be a new
/// one after the tray, the window's drop target takes the system's drags on it (DND-05 받기).
/// Nothing for a window of another backend than winit's (the tests').
pub fn window_shown(window: &MainWindow) {
    let weak = window.as_weak();
    let _ = slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else {
            return;
        };
        // The winit window is let go at once: a window held this way would stay on screen
        // after a hide.
        let made = window.window().winit_window().await.is_ok();
        if made {
            with(Controller::follow_window);
        }
    });
}

/// Hands the window `stop`, which the app's end uses as it begins, so that no second start's
/// request to show the window is taken by an app that is ending (`Controller::quit`).
pub fn stop_requests_at_quit(stop: RequestStop) {
    with(|c| *c.request_stop.borrow_mut() = Some(stop));
}

/// Opens `file` with the system's default app, on a worker thread; a failure shows as a notice.
fn open_file(file: VPath) {
    open::open_file(file, |kind| {
        post(move |c| c.notice(&words::cannot_open(kind)));
    });
}

/// Asks the system for `work` on a worker thread ([`open::shell`]), with the app's window
/// `owner` for its dialogs; what it has for the window comes back to [`Controller::shell_news`].
fn ask_system(work: ShellWork, owner: Owner) {
    open::shell(work, owner, |news| {
        post(move |c| c.shell_news(news));
    });
}

/// What the window starts with, besides the job engine.
#[derive(Debug)]
pub struct Startup {
    /// The folder the app's first argument names: the left pane shows it, in a new tab after
    /// the tabs of the last run (or the one of them that shows it), and has the keyboard.
    pub start: Option<PathBuf>,
    /// The settings of the last run ([`crate::settings::load`]).
    pub settings: Settings,
    /// Writes them back (or, when the file there could not be read, writes nothing).
    pub saver: Saver,
    /// The app's folder for logs, where a job's log goes unless the user chooses another.
    pub log_dir: Option<VPath>,
    /// The app's cache folder, which the thumbnails' disk cache is under.
    pub cache_dir: PathBuf,
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
        log_dir,
        cache_dir,
    } = startup;
    if let Some((width, height)) = settings.window_size() {
        window.set_start_width(logical(width));
        window.set_start_height(logical(height));
    }
    let session = settings.session();
    let started_at = start.is_some();
    window.set_theme_choice(theme_number(settings.theme));
    // The layout of the last run from the start, before the panes' folders are known.
    show_layout(window, session.layout, false);
    ui_bridge::show_shortcuts(window, Os::CURRENT);
    window.set_system_menu_offered(rf_platform::shell::SYSTEM_MENU);
    window
        .global::<ItemMenu<'_>>()
        .set_shares(rf_platform::shell::SHARES);
    let controller = Rc::new(Controller::new(window, jobs, settings, saver));
    *controller.log_dir.borrow_mut() = log_dir;
    *controller.thumber.cache_dir.borrow_mut() = Some(cache_dir);
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&controller)));
    connect(window);
    watch_winit_events(window);

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
        let mut folders = session.folders();
        // `left` is the folder asked for: a gone one adds no tab (`Session::with_start`).
        if started_at && !folders.contains(&left) {
            folders.push(left.clone());
        }
        let count = folders.len();
        let checks = look_for(folders, RESTORE_WORKERS, move |path| loader.look_at(path));
        let found = wait_for(&checks, count, RESTORE_WAIT);
        let (mut session, mut waiting) =
            session.resolve(&fallback, |path| found.get(path).copied());
        let mut left_out = None;
        if started_at {
            let shown;
            (session, shown) =
                session.with_start(left.clone(), found.get(&left).copied(), &mut waiting);
            left_out = (!shown).then(|| left.clone());
        }
        post(move |c| c.open(left, right, session, &waiting, home, &tz));
        if let Some(folder) = left_out {
            post(move |c| c.notice(&words::start_folder_gone(&folder)));
        }
        // The folders not known in time, as they come.
        for (path, what) in checks {
            post(move |c| c.update(|state| state.restored_folder(&path, what)));
        }
    });
    spawn_worker("rf-ui-places", || {
        let mut found = places::favorites();
        found.extend(places::drives());
        post(move |c| c.set_places(&found));
    });
    controller
}

/// The app's end when the event loop ends under the app, called from the loop's `exiting`
/// ([`crate::run`]). On macOS, ⌘Q, the app menu's 종료 and the Dock's 종료 make the system end
/// the process right after that (`terminate:`; winit 0.30 has no `applicationShouldTerminate:`
/// to stop it), so nothing after the loop runs. Waits for the end of the window's close button
/// when it is under way; else does that end without its question (the settings written, the
/// engine shut down, the questions released) and waits for it. The jobs under way are stopped
/// as at any other end, and the next start offers them again. Nothing when there is no
/// controller or the end is done. Blocks the thread of the event loop (on macOS the main
/// thread, while the system waits to end the process) for [`END_WAIT`] at most, about 11.5 s.
pub fn end_at_exit() {
    if let Some(Some(pending)) = with(Controller::end_for_exit) {
        let _ = pending.join();
    }
}

/// How long the app's end waits for the clipboard to be emptied of a cut cancelled with Esc
/// that it may still hold ([`AppState::cut_left_on_clipboard`]).
pub const CLEAR_AT_END: Duration = Duration::from_millis(500);

/// How long the work of the app's end takes at most: the clipboard
/// ([`CLEAR_AT_END`], 0.5 s), the settings ([`FINAL_SAVE_WAIT`], 2 s), the jobs
/// ([`QUIT_TIMEOUT`], 5 s) and the journal's last records after them
/// ([`rf_jobs::JOURNAL_GRACE`], 2 s), and the last notifications ([`NOTICE_WAIT`], 2 s).
pub const END_WAIT: Duration = CLEAR_AT_END
    .saturating_add(FINAL_SAVE_WAIT)
    .saturating_add(QUIT_TIMEOUT)
    .saturating_add(rf_jobs::JOURNAL_GRACE)
    .saturating_add(NOTICE_WAIT);

/// What the app's end does on a worker, after the window's part ([`End`]): the cancelled cut
/// the clipboard may still hold is emptied (waiting [`CLEAR_AT_END`] at most; one that stays is
/// noted in the log), `last`, the text of the settings, is written (waiting
/// [`FINAL_SAVE_WAIT`] at most), then the engine shuts down and the jobs' questions are released
/// ([`Jobs::close_down`]), and the last notification, of the jobs that ended behind the tray
/// icon, is waited for ([`NOTICE_WAIT`] at most). Blocks: a worker's.
fn end_work(end: End) {
    let End {
        jobs,
        saver,
        notifier,
        last,
        cleared,
    } = end;
    if let Some(cleared) = cleared {
        match cleared.recv_timeout(CLEAR_AT_END) {
            Ok(Ok(_)) => {}
            Ok(Err(failure)) => log_cut_left(&format!("{failure:?}")),
            Err(_) => log_cut_left("the clipboard did not answer in time"),
        }
    }
    // First: they do not wait for the engine, and an end forced while it shuts down does not
    // lose them.
    if let Some(text) = last {
        let _ = saver.save_within(text, FINAL_SAVE_WAIT);
    }
    jobs.close_down(QUIT_TIMEOUT);
    // The last notification is not cut off by the end of the app.
    notifier.wait_idle(NOTICE_WAIT);
}

/// The end at the loop's exit apart from the controller ([`Controller::end_for_exit`]): the
/// worker of the end under way, `pending`, to wait for; else, unless the end is done
/// (`closed`), the work of an end, made by `work` only then, on a worker that `spawn` starts.
/// A worker that cannot start leaves the work to this thread, which does it before it returns
/// `None`. Either way the work runs once.
fn end_once<W: FnOnce() + Send + 'static>(
    pending: Option<JoinHandle<()>>,
    closed: bool,
    work: impl FnOnce() -> W,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>,
) -> Option<JoinHandle<()>> {
    if pending.is_some() {
        return pending;
    }
    if closed {
        return None;
    }
    // Shared, so that the work is still here when the worker could not take it.
    let work = Arc::new(Mutex::new(Some(work())));
    let taken = Arc::clone(&work);
    let started = spawn(Box::new(move || {
        if let Some(work) = taken.lock().unwrap_or_else(PoisonError::into_inner).take() {
            work();
        }
    }));
    if let Ok(worker) = started {
        return Some(worker);
    }
    // No worker: the end is done here, the loop being over anyway.
    if let Some(work) = work.lock().unwrap_or_else(PoisonError::into_inner).take() {
        work();
    }
    None
}

/// What the work of the app's end takes along to its worker ([`end_work`]).
struct End {
    jobs: Jobs,
    saver: Saver,
    notifier: OsNotifier,
    /// The last text of the settings, unless an end took it before.
    last: Option<String>,
    /// The answer of the clipboard's worker to the emptying of a cancelled cut, if one was due.
    cleared: Option<Receiver<Result<bool, ClipboardFailure>>>,
}

/// Notes that a cut cancelled with Esc may still be on the clipboard as the app ends, and why:
/// a paste in another app could still move the items. The window is going, so nothing else can
/// tell the user.
#[expect(
    clippy::print_stderr,
    reason = "the app has no log of its own yet; Slint's own messages go to the same place"
)]
fn log_cut_left(why: &str) {
    eprintln!("a cut cancelled with Esc may still be on the clipboard: {why}");
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

/// `value` as a number for the window, at most `i32::MAX`.
fn number(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn connect(window: &MainWindow) {
    window.on_key(|text, ctrl, shift, alt, meta, repeat| {
        let mods = Mods {
            ctrl,
            shift,
            alt,
            meta,
        };
        with(|c| c.key(&text, mods, repeat)).unwrap_or(false)
    });
    window.on_command(|name| {
        with(|c| c.command(&name));
    });
    window.on_tree_key(|text, ctrl, shift, alt, meta| {
        let mods = Mods {
            ctrl,
            shift,
            alt,
            meta,
        };
        with(|c| c.tree_key(&text, mods)).unwrap_or(false)
    });
    window.on_panel_key(|text, ctrl, shift, alt, meta| {
        let mods = Mods {
            ctrl,
            shift,
            alt,
            meta,
        };
        with(|c| c.panel_key(&text, mods)).unwrap_or(false)
    });
    connect_quick_look(window);
    window.on_split_changed(|share| {
        with(|c| {
            c.update(|state| {
                state.set_split_share(f64::from(share));
                Vec::new()
            });
        });
    });
    window.on_theme_chosen(|choice| {
        if let Some(theme) = theme_of(choice) {
            with(|c| c.choose_theme(theme));
        }
    });
    window.on_window_resized(|| {
        with(Controller::window_resized);
    });
    window.on_pane_activated(|s| {
        with(|c| {
            c.update(|state| {
                state.activate(side(s));
                Vec::new()
            });
        });
    });
    window.on_crumb_clicked(|s, i| {
        with(|c| c.update_at(i, |state, i| state.open_crumb(side(s), i)));
    });
    window.on_path_entered(|s, text| {
        with(|c| c.update(|state| state.enter_path(side(s), &text)));
    });
    connect_address(window);
    window.on_filter_edited(|text| {
        with(|c| c.update(|state| state.filter_edited(&text)));
    });
    connect_view(window);
    window.on_row_clicked(|s, i, ctrl, shift| {
        with(|c| c.update_at(i, |state, i| state.press_row(side(s), i, ctrl, shift)));
    });
    window.on_visible_range(|s, first, last| {
        with(|c| c.visible_range(side(s), first, last));
    });
    window.on_row_released(|s, i| {
        with(|c| c.update_at(i, |state, i| state.release_row(side(s), i)));
    });
    window.on_row_activated(|s, i| {
        with(|c| c.update_at(i, |state, i| state.open_row(side(s), i)));
    });
    window.on_retry(|s| {
        with(|c| c.update(|state| state.retry(side(s))));
    });
    connect_sidebar(window);
    connect_drops(window);
    connect_tabs(window);
    connect_jobs(window);
    connect_report(window);
    connect_pick(window);
    connect_properties(window);
}

/// The callbacks of the sidebar: a click on a row or its chevron, and the favorites (the press
/// that readies a drag of one, the items of the menus).
fn connect_sidebar(window: &MainWindow) {
    window.on_tree_clicked(|i| {
        with(|c| c.tree_clicked(i));
    });
    window.on_tree_toggled(|i| {
        with(|c| c.tree_toggled(i));
    });
    window.on_favorite_pressed(|i| {
        with(|c| c.favorite_pressed(i));
    });
    window.on_favorite_command(|i, name| {
        with(|c| c.update_at(i, |state, i| state.favorite_command(i, &name)));
    });
    window.on_row_command(|i, name| {
        with(|c| c.row_command(i, &name));
    });
    window.on_favorites_blank_command(|name| {
        with(|c| c.update(|state| state.favorites_blank_command(&name)));
    });
}

/// The callbacks of a drag of rows: a hover over a place and a drop there, the choice of the
/// menu of 놓을 때 묻기, and 보기 ▸ 설정 ▸ 끌어 놓기: ….
fn connect_drops(window: &MainWindow) {
    window.on_drop_check(|data, place, s, i| {
        with(|c| c.drop_check(&data, place, s, i)).unwrap_or(DragAction::None)
    });
    window.on_drop_items(|data, place, s, i| {
        with(|c| c.drop_items(&data, place, s, i)).unwrap_or(DragAction::None)
    });
    window.on_drop_menu_chosen(|name| {
        if name == "link" {
            with(|c| c.update(AppState::drop_menu_link));
            return;
        }
        let mode = match name.as_str() {
            "move" => Some(TransferMode::Move),
            "copy" => Some(TransferMode::Copy),
            _ => None,
        };
        with(|c| c.update(|state| state.drop_menu_chosen(mode)));
    });
    window.on_drag_default_chosen(|choice| {
        if let Some(default) = ui_bridge::drag_default_of(choice) {
            with(|c| {
                c.update(|state| {
                    state.set_drag_default(default);
                    Vec::new()
                });
            });
        }
    });
    // 머무르면 열기 (DND-08) and the lists that scroll under a drag.
    window.on_drag_ended(|| {
        with(Controller::drag_gone);
    });
    window.on_drag_hovers_nothing(|| {
        with(|c| c.spring.stop());
    });
    window.global::<DragPointer<'_>>().on_moved_under(|| {
        // After the step of the list is drawn, outside Slint's timer.
        slint::Timer::single_shot(Duration::ZERO, || {
            with(Controller::hover_again);
        });
    });
}

/// The callbacks of the properties window: an algorithm chosen, the 중지 of the count and of
/// the checksums, 복사 of a row's checksum, the field 비교 and OS 속성 창 열기; and the menu of
/// the apps that can open a file.
fn connect_properties(window: &MainWindow) {
    window.on_properties_algorithm(|number| {
        with(|c| c.update(|state| state.choose_algorithm(number)));
    });
    window.on_properties_stop(|| {
        with(|c| c.update(AppState::stop_count));
    });
    window.on_properties_stop_hashes(|| {
        with(|c| c.update(AppState::stop_hashes));
    });
    window.on_properties_copy(|row| {
        with(|c| c.update_at(row, AppState::copy_checksum));
    });
    window.on_properties_compare(|text| {
        with(|c| {
            c.update(|state| {
                state.compare_edited(&text);
                Vec::new()
            });
        });
    });
    window.on_properties_system(|| {
        with(|c| c.update(AppState::system_properties_of_window));
    });
    window.on_open_with_chosen(|index| {
        with(|c| c.update_at(index, AppState::open_with_chosen));
    });
}

/// The callbacks of quick look (VIEW-01): its keys, which go nowhere else, and its close button
/// and the click beside it.
fn connect_quick_look(window: &MainWindow) {
    window.on_quick_look_key(|text, ctrl, shift, alt, meta| {
        let mods = Mods {
            ctrl,
            shift,
            alt,
            meta,
        };
        if let Some(key) = keys::quick_look_key(&text, mods, Os::CURRENT) {
            with(|c| c.update(|state| state.quick_look_key(key)));
        }
    });
    window.on_quick_look_closed(|| {
        with(|c| {
            c.update(|state| {
                state.close_quick_look();
                Vec::new()
            });
        });
    });
}

/// The callbacks of the panes' views (NAV-07, NAV-08, NAV-10): the order, the columns of 자세히
/// and the 보기 menu's commands for a folder's view.
fn connect_view(window: &MainWindow) {
    window.on_sort(|s, column, then| {
        if let Some(key) = ui_bridge::sort_key(column) {
            with(|c| c.update(|state| state.sort_by(side(s), key, then)));
        }
    });
    window.on_sort_then(|s, column| {
        let key = ui_bridge::sort_key(column);
        with(|c| c.update(|state| state.sort_then(side(s), key)));
    });
    window.on_column_toggled(|s, column| {
        if let Some(column) = Column::from_number(column) {
            with(|c| c.update(|state| state.toggle_column(side(s), column)));
        }
    });
    window.on_column_moved(|s, from, slot| {
        if let (Some(from), Some(slot)) = (index(from), index(slot)) {
            with(|c| c.update(|state| state.move_column(side(s), from, slot)));
        }
    });
    window.on_column_resized(|s, column, width| {
        let width = whole_width(width);
        if let Some(column) = Column::from_number(column) {
            with(|c| c.update(|state| state.set_column_width(side(s), column, width)));
        }
    });
    window.on_column_texts(|s, column, first, count| {
        let (Some(column), Some(first), Some(count)) =
            (Column::from_number(column), index(first), index(count))
        else {
            return slint::SharedString::new();
        };
        with(|c| c.panes[side(s)].column_texts(column, first, count))
            .unwrap_or_default()
            .into()
    });
    window.on_view_command(|s, name| {
        let side = side(s);
        with(|c| match name.as_str() {
            "folders-first" => c.update(|state| state.toggle_folders_first(side)),
            "apply-below" => c.update(|state| state.apply_view_to_subfolders(side)),
            "reset-view" => c.update(|state| state.reset_view(side)),
            _ => {}
        });
    });
}

/// A width from Slint as whole logical pixels (0 for one that is not a width).
fn whole_width(width: f32) -> u32 {
    let rounded = width.round();
    if rounded.is_finite() && rounded >= 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the cast saturates, and a column is clamped to a few hundred pixels"
        )]
        let whole = rounded as u32;
        whole
    } else {
        0
    }
}

/// The callbacks of the address bar (NAV-03): the `›` of a piece of a path row and its list of
/// folders, and the completion list of a path field.
fn connect_address(window: &MainWindow) {
    window.on_subfolders_clicked(|s, i| {
        with(|c| c.update_at(i, |state, i| state.open_subfolders(side(s), i)));
    });
    window.on_subfolder_chosen(|i| {
        with(|c| c.update_at(i, AppState::choose_subfolder));
    });
    window.on_subfolders_dismissed(|| {
        with(Controller::close_subfolders);
    });
    window.on_path_edited(|s, text| {
        with(|c| c.update(|state| state.path_edited(side(s), &text)));
    });
    window.on_completion_step(|s, delta| {
        let delta = if delta < 0 { -1 } else { 1 };
        with(|c| c.update(|state| state.completion_step(side(s), delta)));
    });
    window.on_path_completed(|s, text| {
        with(|c| c.complete_path(side(s), &text))
            .unwrap_or_default()
            .into()
    });
    window.on_completion_chosen(|s, i| {
        with(|c| c.update_at(i, |state, i| state.choose_completion(side(s), i)));
    });
    window.on_completion_dismissed(|s| {
        with(|c| {
            c.update(|state| {
                state.dismiss_completion(side(s));
                Vec::new()
            });
        });
    });
}

/// The callbacks of the panes' tab bars: a press, the close button, the new tab button, a
/// tab's menu, and a drag of a tab.
fn connect_tabs(window: &MainWindow) {
    window.on_tab_selected(|s, i| {
        with(|c| c.press_tab(side(s), i));
    });
    window.on_tab_closed(|s, i| {
        with(|c| c.update_at(i, |state, i| state.close_tab(side(s), i)));
    });
    window.on_tab_added(|s| {
        with(|c| c.update(|state| state.add_tab(side(s))));
    });
    window.on_tab_command(|s, i, name| {
        with(|c| c.update_at(i, |state, i| tab_command(state, side(s), i, &name)));
    });
    window.on_tab_move_check(|data, s, slot| {
        with(|c| c.tab_move_check(&data, side(s), slot)).unwrap_or(TabDrop::Refused)
    });
    window.on_tab_move_dropped(|data, s, slot| {
        with(|c| c.tab_move_dropped(&data, side(s), slot)).unwrap_or(false)
    });
}

/// Carries out the item `name` of the menu of tab `index` of pane `side` (or of the 보기 menu's
/// 탭 items, for the tab the active pane shows).
fn tab_command(state: &mut AppState, side: usize, index: usize, name: &str) -> Vec<Effect> {
    match name {
        "close" => state.close_tab(side, index),
        "close-others" => state.close_other_tabs(side, index),
        "close-right" => state.close_tabs_right(side, index),
        "duplicate" => state.duplicate_tab(side, index),
        "pin" => state.toggle_pin(side, index),
        "move-other" => state.move_tab_to_other(side, index),
        "move-left" => state.shift_tab(side, index, -1),
        "move-right" => state.shift_tab(side, index, 1),
        "reopen" => state.reopen_tab(side),
        _ => Vec::new(),
    }
}

/// Looks on a worker thread at which of the `folders` (recent ones and favorites) are folders
/// still, then tells the state ([`AppState::folders_checked`]).
fn check_folders(folders: Vec<VPath>) {
    spawn_worker("rf-ui-recent", move || {
        let found: Vec<(VPath, bool)> = folders
            .into_iter()
            .map(|folder| {
                let there = is_folder(&folder);
                (folder, there)
            })
            .collect();
        post(move |c| {
            c.update(|state| {
                state.folders_checked(&found);
                Vec::new()
            });
        });
    });
}

/// Looks on a worker thread at which of the `sources` another app's drag dropped on the list of
/// favorites are folders (a link to one counts), then tells the state
/// ([`AppState::folders_kept`]), which keeps those. The UI thread never looks at them.
fn keep_dropped(generation: u64, sources: Vec<VPath>, index: Option<usize>) {
    spawn_worker("rf-ui-keep", move || {
        let (folders, others): (Vec<VPath>, Vec<VPath>) = sources.into_iter().partition(is_folder);
        post(move |c| {
            c.update(|state| state.folders_kept(generation, folders, !others.is_empty(), index));
        });
    });
}

/// Whether `path` is a local folder, or a link to one (`std::fs::metadata` follows links).
fn is_folder(path: &VPath) -> bool {
    path.as_local()
        .is_some_and(|path| std::fs::metadata(path).is_ok_and(|meta| meta.is_dir()))
}

/// The group of recent folders as the sidebar shows it: each folder by its name, with the
/// name of the folder it is in beside it, dim when a worker found it gone, and marked when the active pane
/// shows it.
fn recent_block(state: &AppState) -> RecentBlock {
    let shown = state.panes()[state.active()].active_tab().location();
    RecentBlock {
        folded: state.recent_folded(),
        folders: state
            .recent_folders()
            .iter()
            .map(|folder| RecentFolder {
                label: folder_label(folder),
                place: folder
                    .parent()
                    .map(|parent| folder_label(&parent))
                    .unwrap_or_default(),
                missing: state.is_missing(folder),
                selected: folder == shown,
            })
            .collect(),
        missing_favorites: state
            .places()
            .iter()
            .filter(|place| place.kind.is_favorite() && state.is_missing(&place.path))
            .map(|place| place.path.clone())
            .collect(),
    }
}

/// The name of the folder `hover` would put the items into, as the words beside the pointer
/// show it; empty where no folder takes them.
fn hover_folder(hover: &Hover) -> String {
    match hover {
        Hover::Takes(offer) => folder_label(&offer.folder),
        Hover::Refused(_)
        | Hover::NoPaths
        | Hover::Keeps { .. }
        | Hover::NoKeep(_)
        | Hover::Nothing => String::new(),
    }
}

/// What shows beside the pointer of a drag over a place that makes `hover` of it, with `folder`
/// for the folder's name: what a drop does, or why it does nothing there (DND-03).
fn hover_words(hover: &Hover, folder: &str) -> String {
    match hover {
        Hover::Takes(offer) => words::drop_badge(offer.choice, folder),
        Hover::Refused(validity) => words::drop_refused(*validity),
        Hover::NoPaths => words::drop_no_paths().to_owned(),
        Hover::Keeps { .. } => words::drop_keeps().to_owned(),
        Hover::NoKeep(Unkept::Files) => words::drop_only_folders().to_owned(),
        // The keys ask for a link, which the list of favorites does not make: it takes folders
        // as additions only.
        Hover::NoKeep(Unkept::Link) => words::drop_favorites_no_links().to_owned(),
        Hover::Nothing => String::new(),
    }
}

/// What the system shows under its drag image for `hover` (Windows' drop description, as File
/// Explorer writes its own): the words beside the pointer with `%1`, the system's placeholder,
/// for the folder, whose name goes as the insert, which the system never reads for `%1`.
fn system_hint(hover: &Hover) -> DropHint {
    DropHint {
        message: hover_words(hover, "%1"),
        insert: hover_folder(hover),
    }
}

/// Tells why a drag of rows that left the window did not go on as the system's drag, where a
/// developer or a report of a user sees it; the drag is over.
#[expect(
    clippy::print_stderr,
    reason = "the app has no log of its own yet; Slint's own messages go to the same place"
)]
fn log_drag_out_failed(error: &std::io::Error) {
    eprintln!("the drag could not go out of the window: {error}");
}

/// What a drop whose `effects` these are did, for the drag it ended: the mode of the transfer
/// they submit, or `none` (a notice, the menu of 놓을 때 묻기, nothing).
fn dropped_action(effects: &[Effect]) -> DragAction {
    match dropped_effect(effects) {
        Some(DropEffect::Copy) => DragAction::Copy,
        Some(DropEffect::Move) => DragAction::Move,
        Some(DropEffect::Link) => DragAction::Link,
        None => DragAction::None,
    }
}

/// Looks at the events of the window's winit window before Slint does, and passes each on: the
/// modifier keys, which Slint's drop events do not tell and a drop's action follows
/// ([`Controller::modifiers_changed`]); the end of a drag over the window, which Slint does
/// not tell when the row the drag started on is gone (a folder that opened under it, DND-08) or
/// when the pointer is past the window's edges ([`Controller::drag_gone`]); the pointer that
/// left the window, which Slint is told of itself when it would not hear it
/// ([`Controller::pointer_left`]); a drag of rows that
/// goes past the window's edges, which goes on as the system's drag (DND-05 보내기,
/// [`Controller::pointer_outside`]), and the left button it needs held; the files another
/// app dropped on the window, where winit's own drop target takes them (X11, macOS's title bar:
/// [`Controller::file_dropped`]; nothing shows while they hover, `HoveredFile`, since winit
/// tells neither where nor the keys); where the pointer was last, for the menu of a drop that
/// asks without telling where it was; and a right press, which Slint then never sees when it
/// ends a drag of rows or comes while the left button is down (no menu shows then), or when it
/// shows Windows' own menu in the place of the app's (OPS-05,
/// [`Controller::right_button_pressed`]); and the wheel with Ctrl (macOS ⌘) and a trackpad's
/// pinch, which step the icon size of the pane under the pointer (COMP-04): the wheel then does
/// not scroll the list.
fn watch_winit_events(window: &MainWindow) {
    window.window().on_winit_window_event(|window, event| {
        match event {
            WinitEvent::HoveredFileCancelled => {
                with(Controller::outside_leave);
            }
            WinitEvent::DroppedFile(path) => {
                let path = path.clone();
                with(|c| c.file_dropped(path));
            }
            WinitEvent::ModifiersChanged(modifiers) => {
                let mods = ui_bridge::drag_mods(modifiers.state());
                // ⌘ with the wheel on macOS, Ctrl elsewhere, changes the icons' size.
                let zoom = if cfg!(target_os = "macos") {
                    modifiers.state().super_key()
                } else {
                    modifiers.state().control_key()
                };
                with(|c| {
                    c.modifiers_changed(mods);
                    c.zoom_key(zoom);
                });
            }
            WinitEvent::MouseWheel { delta, .. } => {
                let (lines, pixels) = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (Some(f64::from(*y)), 0.0),
                    MouseScrollDelta::PixelDelta(at) => {
                        (None, at.y / f64::from(window.scale_factor()))
                    }
                };
                if with(|c| c.zoom_wheel(lines, pixels)).unwrap_or(false) {
                    return EventResult::PreventDefault;
                }
            }
            WinitEvent::PinchGesture { delta, phase, .. } => {
                let (delta, started) = (*delta, *phase == TouchPhase::Started);
                with(|c| c.zoom_pinch(delta, started));
            }
            WinitEvent::CursorLeft { .. } => {
                with(Controller::pointer_left);
            }
            WinitEvent::CursorMoved { position, .. }
                if ui_bridge::outside_window(*position, window.size()) =>
            {
                with(Controller::pointer_outside);
            }
            WinitEvent::CursorMoved { position, .. } => {
                let at = position.to_logical::<f32>(f64::from(window.scale_factor()));
                with(|c| c.pointer.set(Some(WindowPoint { x: at.x, y: at.y })));
            }
            WinitEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                with(|c| c.left_down.set(true));
            }
            WinitEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                if with(Controller::right_button_pressed).unwrap_or(false) {
                    return EventResult::PreventDefault;
                }
            }
            WinitEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                with(|c| c.left_down.set(false));
                // Once Slint has taken the release: its drop, or the end of the drag.
                slint::Timer::single_shot(Duration::ZERO, || {
                    with(Controller::left_button_released);
                });
            }
            _ => {}
        }
        EventResult::Propagate
    });
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

/// The buttons of the job panel's 최근 완료, and the question's 트레이에서 계속.
fn connect_report(window: &MainWindow) {
    window.on_recent_toggled(|| {
        with(|c| {
            c.update(|state| {
                state.recent_toggled();
                Vec::new()
            });
        });
    });
    window.on_recent_cleared(|| {
        with(|c| {
            c.update(|state| {
                state.recent_cleared();
                Vec::new()
            });
        });
    });
    window.on_recent_details(|i| {
        with(|c| c.update_at(i, AppState::recent_details));
    });
    window.on_recent_save(|i| {
        with(|c| c.update_at(i, AppState::save_log_asked));
    });
    window.on_confirm_alternative(|| {
        with(|c| c.update(AppState::confirm_alternative));
    });
}

/// The callbacks of the dialogs, the job panel and the toast, and the close button.
fn connect_jobs(window: &MainWindow) {
    window.on_row_menu(|s, i| {
        with(|c| c.update_at(i, |state, i| state.row_menu(side(s), i)));
    });
    // Each menu asks as it shows, on the UI thread: the look reads nothing and does not wait
    // for an app that holds the clipboard.
    window
        .global::<ItemMenu<'_>>()
        .on_no_files_to_paste(|| !rf_platform::clipboard::has_files());
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
    window.on_pattern_edited(|text| {
        with(|c| c.update(|state| state.pattern_edited(&text)));
    });
    window.on_pattern_syntax(|regex| {
        with(|c| c.update(|state| state.pattern_syntax(regex)));
    });
    window.on_pattern_case(|ignore| {
        with(|c| c.update(|state| state.pattern_case(ignore)));
    });
    window.on_pattern_mode(|add| {
        with(|c| {
            c.update(|state| {
                state.pattern_mode(add);
                Vec::new()
            });
        });
    });
    window.on_pattern_accepted(|text| {
        with(|c| c.update(|state| state.pattern_confirmed(&text)));
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
            // The event loop runs until it is ended (`crate::run`): a hidden window does not.
            let _ = slint::quit_event_loop();
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
    /// The rows of the list of folders under a `›` of a path row.
    subfolder_rows: Rc<VecModel<FolderMenuRow>>,
    panes: [PaneModels; 2],
    /// The tab (its position) and folder each pane showed at the last redraw: a pane that
    /// shows another folder or tab scrolls to its cursor, or its top.
    shown: RefCell<[Option<(usize, VPath)>; 2]>,
    /// The settings as last read or written, with the keys of a newer version: a save writes
    /// them with what the window shows then.
    settings: RefCell<Settings>,
    saver: Saver,
    /// Whether, and what, to save after a change ([`SaveLedger`]).
    ledger: RefCell<SaveLedger>,
    /// Runs [`SAVE_DELAY`] after the first change since the last save.
    save_timer: slint::Timer,
    /// Whether a failed save has been told (once is enough).
    save_failure_told: Cell<bool>,
    /// The id of the drag whose payload the window holds.
    shown_drag: Cell<u64>,
    /// What the item menu's 링크 만들기 ▸ holds: set again only when it changed, so that a menu
    /// that is open is not made anew at every redraw.
    shown_links: Cell<LinkFacts>,
    /// The time of a drag's rest on a place, which then opens (머무르면 열기, DND-08).
    spring: Spring,
    /// The files winit told were dropped on the window, until the next turn of the event loop
    /// takes all of one drop at once ([`Controller::file_dropped`]).
    dropped_files: RefCell<Vec<PathBuf>>,
    /// Where in the window the pointer was last, as winit told.
    pointer: Cell<Option<WindowPoint>>,
    /// The window's drop target for the system's drags, on the window's handle of its last show.
    drop_target: DropTarget<DropRegistration>,
    /// What the window said beside the pointer of the drag of another app at its last hover.
    outside_hint: RefCell<DropHint>,
    /// Whether the window moved its pointer over the tree for the drag of another app
    /// ([`Controller::leave_tree`]).
    moved_over_tree: Cell<bool>,
    /// The left button is down in the window, as winit told last: a drag of rows goes out of the
    /// window only while it is ([`Controller::drag_leaves`]), and no menu shows while it is
    /// ([`Controller::right_button_pressed`], [`Controller::held_key`]). Cleared, too, where
    /// nothing waits for its release any more: a drag of rows ended by Esc or a right press, or
    /// one that went out of the window.
    left_down: Cell<bool>,
    /// The drag of rows that left the window goes out on the next turn of the event loop
    /// ([`Controller::pointer_outside`]).
    leaving: Cell<bool>,
    /// The theme chosen in the 보기 menu.
    theme: Cell<ThemeChoice>,
    /// Stops a worker's look at copies and their originals once a newer look replaces it, and
    /// every look as the app ends.
    checks: RefCell<CopyChecks>,
    /// Puts the rows of long listings in a new order, the newest sort of each tab only.
    sorter: Sorter,
    /// Matches the names of the pattern window's rows, the newest request only.
    matcher: Matcher,
    /// Makes the previews of the preview panel and quick look, the newest request only.
    previewer: Previewer,
    /// The models of the preview's view.
    preview: PreviewModels,
    /// The thumbnails of the icon views, and the icon size of Ctrl+wheel and a pinch.
    thumber: Thumber,
    /// The workers of the properties window: what the listing does not tell and the count of
    /// what its folders hold, and the checksums of its files.
    measurer: Measurer,
    hasher: Hasher,
    /// The models of the properties window's view.
    properties: PropertiesModels,
    /// The worker of the system clipboard.
    clipper: Clipper,
    /// Asks the clipboard's change count while items the window cut are on it.
    clipboard_timer: slint::Timer,
    /// The titles of the menu of the apps that can open a file (macOS).
    open_with_apps: Rc<VecModel<slint::SharedString>>,
    /// The lines of the job panel's 최근 완료.
    recent_rows: Rc<VecModel<RecentRowView>>,
    /// The app's folder for logs, until the state has it.
    log_dir: RefCell<Option<VPath>>,
    /// Shows the notifications of the system.
    notifier: OsNotifier,
    /// The tray icon while the window is hidden.
    tray: Tray,
    /// Stops the look for a second start's requests to show the window, as the app ends.
    request_stop: RefCell<Option<RequestStop>>,
    /// The worker of the app's end, once [`Controller::quit`] started it.
    quitting: RefCell<Option<JoinHandle<()>>>,
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
        let open_with_apps = Rc::new(VecModel::default());
        window.set_open_with_apps(ModelRc::from(Rc::clone(&open_with_apps)));
        let recent_rows = Rc::new(VecModel::default());
        window.set_recent_rows(ModelRc::from(Rc::clone(&recent_rows)));
        // A payload from the start: a drag only begins on a press whose area has one, and the
        // press readies the drag's own.
        window.set_drag_data(ui_bridge::drag_payload(0));
        // The same for a drag of a tab: the press that shows the tab readies its own (id 0 is
        // no tab's).
        window.set_tab_drag_data(ui_bridge::tab_payload(0));
        // A drag starts only with a payload, and the press on a favorite's row readies its own
        // after the drag area has looked: this one belongs to no favorite.
        window.set_favorite_drag_data(ui_bridge::favorite_payload(usize::MAX, ""));
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
            subfolder_rows: Rc::new(VecModel::default()),
            panes: [PaneModels::default(), PaneModels::default()],
            shown: RefCell::new([None, None]),
            theme: Cell::new(settings.theme),
            ledger: RefCell::new(SaveLedger::new(
                settings.to_text().unwrap_or_default(),
                saver.writable(),
            )),
            settings: RefCell::new(settings),
            saver,
            save_timer: slint::Timer::default(),
            save_failure_told: Cell::new(false),
            shown_drag: Cell::new(0),
            shown_links: Cell::new(LinkFacts::default()),
            spring: Spring::new(|spot| {
                with(|c| c.spring_due(spot));
            }),
            dropped_files: RefCell::new(Vec::new()),
            pointer: Cell::new(None),
            drop_target: DropTarget::new(|owner| {
                dnd::register_drop_target(owner, Rc::new(WindowDrops))
            }),
            outside_hint: RefCell::new(DropHint::default()),
            moved_over_tree: Cell::new(false),
            left_down: Cell::new(false),
            leaving: Cell::new(false),
            checks: RefCell::new(CopyChecks::default()),
            sorter: sorter::sorter(|outcome| {
                post(move |c| {
                    c.update(|state| match outcome {
                        Outcome::Done(sorted) => state.sorted(sorted),
                        Outcome::Failed((generation, serial)) => {
                            state.sort_failed(generation, serial)
                        }
                    });
                });
            }),
            matcher: matcher::matcher(|outcome| {
                post(move |c| {
                    c.update(|state| match outcome {
                        Outcome::Done(matched) => state.pattern_matched(matched),
                        Outcome::Failed(generation) => state.pattern_failed(generation),
                    });
                });
            }),
            previewer: {
                let vfs = rf_vfs::Vfs::with_local();
                Previewer::new(
                    PREVIEW_DELAY,
                    move |request, cancel| preview::make_preview(request, &vfs, cancel),
                    |previewed| post(move |c| c.update(|state| state.previewed(previewed))),
                )
            },
            preview: PreviewModels::default(),
            thumber: Thumber::new(),
            measurer: measurer(),
            hasher: hasher(),
            properties: PropertiesModels::default(),
            clipper: Clipper::start(Arc::new(OsClipboard), Os::CURRENT, |done| {
                post(move |c| c.clipboard_done(done));
            }),
            clipboard_timer: slint::Timer::default(),
            open_with_apps,
            recent_rows,
            log_dir: RefCell::new(None),
            notifier: OsNotifier::system(),
            tray: Tray::new(Box::new(SlintTray::new(|command| {
                with(|c| c.update(|state| state.tray_command(command)));
            }))),
            request_stop: RefCell::new(None),
            quitting: RefCell::new(None),
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
        // The favorites the user keeps, the folders visited last and whether their group is
        // folded.
        effects.extend(state.restore_sidebar(&self.settings.borrow()));
        // The tabs show the views remembered for their folders, and ask for their listings in
        // their orders.
        state.set_views(self.settings.borrow().views());
        {
            let settings = self.settings.borrow();
            state.set_drag_default(settings.drag_default);
            state.set_move_undo_toast(settings.move_undo_toast);
            state.set_system_menu_first(settings.system_menu_first);
            state.set_show_thumbnails(settings.show_thumbnails);
        }
        state.offer_system_menu(rf_platform::shell::SYSTEM_MENU);
        state.settle_views(&mut effects);
        if !self.saver.writable() {
            effects.push(Effect::Notice(words::settings_unreadable().to_owned()));
        }
        let places = self.places.borrow().clone();
        if !places.is_empty() {
            effects.extend(state.set_places(&places));
        }
        effects.extend(state.follow_active());
        state.note_visits();
        state.set_history(self.jobs.history());
        state.set_lanes(self.jobs.lanes());
        if let Some(dir) = self.log_dir.borrow_mut().take() {
            state.set_log_dir(dir);
        }
        state.set_tray(rf_platform::desktop::TRAY);
        for early in self.early.take() {
            effects.extend(match early {
                Early::Event(event) => state.job_event(event),
                Early::Question(question) => state.question(question),
            });
        }
        *self.state.borrow_mut() = Some(state);
        self.apply(effects);
        self.fit_preview();
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
        // to date now. Whether the window is in front decides whether the end is notified.
        let history = self.jobs.history();
        let active = self.window_active();
        self.update(|state| {
            state.set_history(history);
            state.set_window_active(active);
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

    /// Ends the app on a worker thread: the settings are written a last time (waiting
    /// [`FINAL_SAVE_WAIT`] at most), then the engine shuts down and the questions are released
    /// ([`Jobs::close_down`]), and then the event loop ends. The window shows the closing note
    /// meanwhile. A worker that cannot start leaves the settings and the shutdown to
    /// [`crate::run`], after the event loop. From the start, no second start's request to show
    /// the window is taken ([`stop_requests_at_quit`]).
    fn quit(&self) {
        let end = self.end_parts();
        let taken = end.last.is_some();
        let started = std::thread::Builder::new()
            .name("rf-ui-quit".to_owned())
            .spawn(move || {
                end_work(end);
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            });
        if let Ok(worker) = started {
            *self.quitting.borrow_mut() = Some(worker);
            return;
        }
        if taken {
            self.reopen_settings();
        }
        let _ = slint::quit_event_loop();
    }

    /// What the work of an end takes along ([`end_work`]): the engine, the saver, the notifier,
    /// the last text of the settings ([`Controller::final_settings`]), and the emptying of a
    /// cancelled cut the clipboard may still hold, asked of the clipboard's worker now. From now
    /// on no second start's request to show the window is taken (the window is about to go, and
    /// that start then tries the lock again, `instance::take_or_ask`), and the looks at copies
    /// stop (the state takes no answer while the app ends).
    fn end_parts(&self) -> End {
        if let Some(stop) = self.request_stop.borrow().as_ref() {
            stop.stop();
        }
        self.checks.borrow_mut().stop();
        let left = self
            .state
            .borrow()
            .as_ref()
            .and_then(AppState::cut_left_on_clipboard);
        End {
            jobs: self.jobs.clone(),
            saver: self.saver.clone(),
            notifier: self.notifier.clone(),
            last: self.final_settings(),
            cleared: left.and_then(|change| self.clipper.clear_now(change)),
        }
    }

    /// The end of the app when the event loop ends under it ([`end_at_exit`]): the worker of
    /// the end under way, to wait for; else a worker that does the end of [`Controller::quit`]
    /// without its question. `None` when the end is done, or when no worker could start and
    /// the end was done here.
    fn end_for_exit(&self) -> Option<JoinHandle<()>> {
        let pending = self.quitting.borrow_mut().take();
        end_once(
            pending,
            self.jobs.is_closed(),
            || {
                let end = self.end_parts();
                move || end_work(end)
            },
            |work| {
                std::thread::Builder::new()
                    .name("rf-ui-exit".to_owned())
                    .spawn(work)
            },
        )
    }

    /// Makes `found` the places of the sidebar: the roots of the tree, now or once the panes'
    /// folders are known.
    fn set_places(&self, found: &[Place]) {
        found.clone_into(&mut self.places.borrow_mut());
        self.update(|state| state.set_places(found));
    }

    /// The free space of the drive at `path`, read again after a job: the sidebar's place
    /// shows it from now on.
    fn drive_space(&self, path: &VPath, space: rf_platform::DiskSpace) {
        for place in self.places.borrow_mut().iter_mut() {
            if place.kind == places::PlaceKind::Drive && place.path == *path {
                place.space = Some(space);
            }
        }
        self.tree_rows.invalidate();
        self.update(|state| {
            state.drive_space(path, space);
            Vec::new()
        });
    }

    /// A key (`repeat`: the system repeats a held one); `true` when the window used it. While
    /// the left button is down in the window, Esc ends a drag of rows and no key opens a menu
    /// ([`Controller::held_key`]). While the list of subfolders under a `›` shows, its keys
    /// (`keys::list_key`) go to it; any other command closes it as it does its work.
    fn key(&self, text: &str, mods: Mods, repeat: bool) -> bool {
        if let Some(used) = self.held_key(text, mods) {
            return used;
        }
        let listing = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.subfolders().is_some());
        if listing && let Some(key) = keys::list_key(text, mods, Os::CURRENT) {
            let page = self
                .window
                .upgrade()
                .and_then(|window| usize::try_from(window.get_subfolders_page()).ok())
                .unwrap_or(1);
            self.update(|state| state.subfolders_key(key, page));
            return true;
        }
        // A Space while a name is being typed types into it.
        let now = Instant::now();
        let typing = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.typing(now));
        let Some(command) = keys::pane_command(text, mods, Os::CURRENT, typing) else {
            return false;
        };
        // A held key starts one terminal, not one for each repeat; the repeats go nowhere.
        if repeat && command.launches() {
            return true;
        }
        // How the active pane's view lays its rows out.
        let paging = self
            .window
            .upgrade()
            .map_or(Paging::rows(1), |window| Paging {
                line: usize::try_from(window.get_line_items()).unwrap_or(1),
                page: usize::try_from(window.get_page_rows()).unwrap_or(1),
            });
        // Esc alone may cancel a cut too; the other keys to clear the selection do not.
        // Only while the table still makes Esc its key to clear the selection.
        let escape = keys::cancels_cut(text, mods) && command == Command::ClearSelection;
        let mut used = false;
        self.update(|state| {
            let effects = if escape {
                state.escape(paging, now)
            } else {
                state.key(command, mods, paging, now)
            };
            used = effects.is_some();
            effects.unwrap_or_default()
        });
        used
    }

    /// A key while the sidebar's tree has the keys; `true` when it is used there. While the left
    /// button is down, as from the panes ([`Controller::held_key`]). A key of the tree moves in
    /// it; one of the panes' rows is used, and does nothing; any other goes on to the window's
    /// keys (`false`), as from the panes.
    fn tree_key(&self, text: &str, mods: Mods) -> bool {
        if let Some(used) = self.held_key(text, mods) {
            return used;
        }
        if let Some(key) = keys::tree_key(text, mods, Os::CURRENT) {
            self.update(|state| state.tree_key(key));
            return true;
        }
        keys::command_for(text, mods, Os::CURRENT).is_some_and(|command| command.acts_on_rows())
    }

    /// A key while the job panel has the keys; `true` when it is used there, as with the tree
    /// ([`Controller::tree_key`]).
    fn panel_key(&self, text: &str, mods: Mods) -> bool {
        if let Some(used) = self.held_key(text, mods) {
            return used;
        }
        if let Some(key) = keys::panel_key(text, mods, Os::CURRENT) {
            self.update(|state| state.panel_key(key));
            return true;
        }
        keys::command_for(text, mods, Os::CURRENT).is_some_and(|command| command.acts_on_rows())
    }

    /// Where a favorite dragged by its row would go, dropped at `place` (the gap `i` of the list
    /// of favorites, -1 for its end): the favorite's place and its place after the move; `None`
    /// anywhere else, and where it would stay.
    fn reorder_target(
        &self,
        (from, folder): &(usize, String),
        place: DropPlace,
        i: i32,
    ) -> Option<(usize, usize)> {
        if place != DropPlace::Favorites {
            return None;
        }
        let state = self.state.borrow();
        let to = state
            .as_ref()?
            .favorite_reorder_to(*from, folder, index(i))?;
        Some((*from, to))
    }

    /// The left button went down on favorite `i`: a drag of it, should one follow, carries it.
    fn favorite_pressed(&self, i: i32) {
        let Some(i) = index(i) else {
            return;
        };
        let folder = self
            .state
            .borrow()
            .as_ref()
            .and_then(|state| state.favorite_folder_text(i));
        if let (Some(folder), Some(window)) = (folder, self.window.upgrade()) {
            window.set_favorite_drag_data(ui_bridge::favorite_payload(i, &folder));
        }
    }

    /// A click on row `i` of the sidebar's list: a folder of the tree shows in the active pane,
    /// the head of the group of recent folders folds it or opens it, a recent folder shows in the
    /// active pane (a notice says so when it is gone).
    fn tree_clicked(&self, i: i32) {
        let Some(i) = index(i) else {
            return;
        };
        match self.tree_rows.slot_at(i) {
            RowSlot::Tree(row) => self.update(|state| state.open_tree_row(row)),
            RowSlot::RecentHead => self.update(AppState::toggle_recent_fold),
            RowSlot::Recent(n) => self.update(|state| state.open_recent(n)),
        }
    }

    /// An item of the menu of row `i` of the sidebar's list, which is no favorite
    /// (`add-this`: its folder joins the favorites).
    fn row_command(&self, i: i32, name: &str) {
        let Some(i) = index(i) else {
            return;
        };
        if name != "add-this" {
            return;
        }
        match self.tree_rows.slot_at(i) {
            RowSlot::Tree(row) => self.update(|state| state.favorite_add_tree_row(row)),
            RowSlot::Recent(n) => self.update(|state| state.favorite_add_recent(n)),
            RowSlot::RecentHead => {}
        }
    }

    /// A click on the chevron of row `i` of the sidebar's list.
    fn tree_toggled(&self, i: i32) {
        let Some(i) = index(i) else {
            return;
        };
        if let RowSlot::Tree(row) = self.tree_rows.slot_at(i) {
            self.update(|state| state.toggle_tree_row(row));
        }
    }

    /// A drag with the payload `data` hovers over `place` (pane `side`, row or tab `index`):
    /// what a drop there does with the keys held now and 보기 ▸ 설정, which the pointer shows, or
    /// `none`. What it does, or why it does nothing there, shows beside the pointer. Called at
    /// every move of the pointer (and as the keys change), it changes nothing else and redraws
    /// nothing.
    fn drop_check(&self, data: &DataTransfer, place: DropPlace, side: i32, i: i32) -> DragAction {
        // A favorite dragged by its row goes only to another place of the list.
        if let Some(dragged) = ui_bridge::favorite_dragged(data) {
            if let Some(window) = self.window.upgrade() {
                window.invoke_drop_hint("".into(), false);
            }
            self.spring.hover(None);
            return match self.reorder_target(&dragged, place, i) {
                Some(_) => DragAction::Move,
                None => DragAction::None,
            };
        }
        let Some(spot) = ui_bridge::spot_of(place, side, i) else {
            return DragAction::None;
        };
        let id = ui_bridge::drag_id(data);
        let hover = match self.state.borrow_mut().as_mut() {
            Some(state) => state.drop_hover(id, spot),
            None => Hover::Nothing,
        };
        let action = match &hover {
            Hover::Takes(offer) => ui_bridge::drop_action(offer.choice),
            // The folders join the favorites: a copy cursor, a plus, for an addition.
            Hover::Keeps { .. } => DragAction::Copy,
            Hover::Refused(_) | Hover::NoPaths | Hover::NoKeep(_) | Hover::Nothing => {
                DragAction::None
            }
        };
        if let Some(window) = self.window.upgrade() {
            self.show_hover(&window, spot, &hover, action != DragAction::None);
        }
        action
    }

    /// What the window shows of a drag over `spot`, of rows or of another app, the place making
    /// `hover` of it, which a drop there takes or not (`takes`): what a drop does, or why it does
    /// nothing there, beside the pointer (DND-03); and the time of a rest, which runs while the
    /// drag stays on a place that takes it (DND-08). The place's frame is Slint's own for a drag
    /// of rows; the caller shows it for a drag of another app.
    fn show_hover(&self, window: &MainWindow, spot: DropSpot, hover: &Hover, takes: bool) {
        let hint = hover_words(hover, &hover_folder(hover));
        window.invoke_drop_hint(hint.into(), !takes);
        let rest = match hover {
            Hover::Takes(offer) if takes => Some((spot, offer.folder.clone())),
            _ => None,
        };
        self.spring.hover(rest);
    }

    /// A drag of another app (or of this app's rows, through the system's drag) carrying `items`,
    /// with the keys `mods` held and allowing `allowed`, is over the point `at` of the window: what
    /// a drop there would do, for the system's pointer, or `None`. The window shows it as it shows
    /// a drag of rows: what a drop does or why not beside the pointer, the frame of the place
    /// that takes it, the time of a rest there; and the list or the tree under it scrolls when
    /// it is held near their ends (DND-08: `outside-over`, a step at each of their timer's turns
    /// while the system tells the same point, which it does every few tens of milliseconds).
    /// `None` too, at once, while the state is borrowed (the system calls this from a loop of its
    /// own inside the app's code).
    pub fn outside_over(
        &self,
        at: WindowPoint,
        items: &DroppedItems,
        mods: DragMods,
        allowed: Effects,
    ) -> Option<DropEffect> {
        self.outside_hint.take();
        if self.state.try_borrow_mut().is_err() {
            return None;
        }
        let window = self.window.upgrade()?;
        let place = ui_bridge::drop_place(&window, at);
        if place.found && ui_bridge::in_sidebar(place.place) {
            self.moved_over_tree.set(true);
        } else {
            self.leave_tree(&window);
        }
        let spot = ui_bridge::spot_found(&place);
        let hover = match self.state.try_borrow_mut().ok()?.as_mut() {
            Some(state) => state.outside_hover(spot, items, mods, allowed),
            None => Hover::Nothing,
        };
        let effect = hover.effect(allowed);
        let pointer = window.global::<DragPointer<'_>>();
        pointer.set_x(at.x);
        pointer.set_y(at.y);
        let shown = spot.filter(|_| effect.is_some());
        window.set_outside_drop(ui_bridge::drop_at(shown));
        window.set_outside_over(place);
        if let Some(spot) = spot {
            self.show_hover(&window, spot, &hover, effect.is_some());
            *self.outside_hint.borrow_mut() = system_hint(&hover);
        } else {
            window.invoke_drop_hint("".into(), false);
            self.spring.stop();
        }
        effect
    }

    /// What the window said beside the pointer of the drag of another app at its last hover
    /// ([`Controller::outside_over`]), for the system to show under its own (`system_hint`);
    /// empty for nothing.
    pub fn outside_hint(&self) -> DropHint {
        self.outside_hint.borrow().clone()
    }

    /// The drag of another app was dropped at the point `at` of the window, with the right
    /// button (`right_button`) or not: what was done, for the system (`None` for nothing, and for
    /// the menu of 놓을 때 묻기, which shows where the drop was); see
    /// [`AppState::outside_drop`]. What it does is carried out on the next turn of the event
    /// loop, once the system has its answer: a menu or a question shown now would run inside the
    /// system's drop. `None`, and nothing done, while the state is borrowed; what showed of the
    /// drag goes either way (`Controller::outside_gone`).
    pub fn outside_drop(
        &self,
        at: WindowPoint,
        items: DroppedItems,
        mods: DragMods,
        allowed: Effects,
        right_button: bool,
    ) -> Option<DropEffect> {
        // The place under the drop, while nothing is borrowed: over the tree, finding it moves
        // the window's pointer.
        let found = self
            .window
            .upgrade()
            .filter(|_| self.state.try_borrow_mut().is_ok())
            .map(|window| {
                let place = ui_bridge::drop_place(&window, at);
                if place.found && ui_bridge::in_sidebar(place.place) {
                    self.moved_over_tree.set(true);
                }
                (window, place)
            });
        // The drag is over, whether the drop can be made or not.
        self.outside_gone();
        let (window, place) = found?;
        let pointer = window.global::<DragPointer<'_>>();
        pointer.set_x(at.x);
        pointer.set_y(at.y);
        self.drop_outside(
            ui_bridge::spot_found(&place),
            items,
            mods,
            allowed,
            right_button,
        )
    }

    /// The drag of another app left the window without a drop: nothing shows of it any more,
    /// and nothing it rested on opens.
    pub fn outside_leave(&self) {
        self.outside_gone();
        if let Ok(mut state) = self.state.try_borrow_mut()
            && let Some(state) = state.as_mut()
        {
            state.outside_leave();
        }
    }

    /// The drag of another app is over (it left the window or was dropped, the drop made or
    /// not): nothing of it shows any more (the badge, the frame, a tree row under the pointer,
    /// the words under the system's pointer), and nothing of it runs on (the lists that scroll
    /// under it, the time of a rest). It needs nothing of the state, so it is done even while
    /// the state is borrowed.
    fn outside_gone(&self) {
        if let Some(window) = self.window.upgrade() {
            self.leave_tree(&window);
        }
        self.drag_gone();
        self.outside_hint.take();
    }

    /// The pointer that the window moved over the tree to find the row under a drag of another
    /// app ([`ui_bridge::drop_place`]) leaves it, once the drag has moved on, left or dropped: no
    /// row of the tree stays shown as under the mouse, which the system's drag does not move.
    fn leave_tree(&self, window: &MainWindow) {
        if self.moved_over_tree.replace(false) {
            ui_bridge::leave_tree(window);
        }
    }

    /// The drop of another app's drag on `spot`, as [`Controller::outside_drop`] makes it.
    fn drop_outside(
        &self,
        spot: Option<DropSpot>,
        items: DroppedItems,
        mods: DragMods,
        allowed: Effects,
        right_button: bool,
    ) -> Option<DropEffect> {
        let (done, effects) = {
            let mut state = self.state.try_borrow_mut().ok()?;
            state
                .as_mut()?
                .outside_drop(spot, items, mods, allowed, right_button)
        };
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(|c| c.update(move |_| effects));
        });
        done
    }

    /// winit told that the file at `path` was dropped on the window (X11, and wherever winit's
    /// own drop target takes drops): the files of one drop come one after another, so the next
    /// turn of the event loop takes all of them at once ([`Controller::files_dropped`]).
    fn file_dropped(&self, path: PathBuf) {
        let first = {
            let mut dropped = self.dropped_files.borrow_mut();
            dropped.push(path);
            dropped.len() == 1
        };
        if first {
            slint::Timer::single_shot(Duration::ZERO, || {
                with(Controller::files_dropped);
            });
        }
    }

    /// The files winit told were dropped go into the folder the active pane shows, as the rule
    /// of a drop's action chooses without a key: winit tells neither where the pointer was nor
    /// the keys held, nor whether the app they came from allows a move, so only a copy is
    /// allowed. The menu of 놓을 때 묻기 (the setting 매번 묻기) shows where the pointer was last.
    /// Nothing while a dialog, quick look or a list of the window shows over the panes
    /// (`drops-blocked`), as a drag of another app finds no place then.
    fn files_dropped(&self) {
        let paths = self.dropped_files.take();
        if self
            .window
            .upgrade()
            .is_some_and(|window| window.invoke_drops_blocked())
        {
            return;
        }
        let Some(active) = self.state.borrow().as_ref().map(AppState::active) else {
            return;
        };
        if let Some(window) = self.window.upgrade() {
            let at = self.pointer.get().unwrap_or_else(|| {
                let size = window
                    .window()
                    .size()
                    .to_logical(window.window().scale_factor());
                WindowPoint {
                    x: size.width / 2.0,
                    y: size.height / 2.0,
                }
            });
            let pointer = window.global::<DragPointer<'_>>();
            pointer.set_x(at.x);
            pointer.set_y(at.y);
        }
        let spot = DropSpot::Pane {
            side: active,
            row: None,
        };
        let items = DroppedItems {
            paths,
            ..DroppedItems::default()
        };
        self.drop_outside(Some(spot), items, DragMods::default(), Effects::COPY, false);
    }

    /// The drag with the payload `data` was dropped on `place`: the place's folder gets the
    /// transfer of its items, or the menu of 놓을 때 묻기 shows, as the state chooses; what the
    /// drop did (a copy, a move), or `none`.
    fn drop_items(&self, data: &DataTransfer, place: DropPlace, side: i32, i: i32) -> DragAction {
        self.spring.stop();
        // A favorite dragged by its row moves to the gap it was dropped in.
        if let Some(dragged) = ui_bridge::favorite_dragged(data) {
            let Some((from, to)) = self.reorder_target(&dragged, place, i) else {
                return DragAction::None;
            };
            self.update(|state| state.favorite_move(from, to));
            return DragAction::Move;
        }
        let Some(spot) = ui_bridge::spot_of(place, side, i) else {
            return DragAction::None;
        };
        let id = ui_bridge::drag_id(data);
        let mut done = DragAction::None;
        self.update(|state| {
            let effects = state.drop_items(id, spot);
            done = dropped_action(&effects);
            effects
        });
        done
    }

    /// The drag has rested on `spot` for [`SPRING_DELAY`](crate::state::SPRING_DELAY): the
    /// place opens, as the state says (a folder, a tab, a folder of the tree). What is under the
    /// still pointer may be another place then, which the window asks about again.
    fn spring_due(&self, spot: DropSpot) {
        self.update(|state| state.drag_rests(spot));
        slint::Timer::single_shot(Duration::ZERO, || {
            with(Controller::hover_again);
        });
    }

    /// The drag over the window is over: it ended, dropped or not (`drag-finished` of a drag of
    /// rows, the left button let go), or it left the window; or a drag of another app left it or
    /// was dropped. Nothing it rested on opens, nothing shows beside its pointer or around a
    /// place, and the lists stop scrolling under it.
    fn drag_gone(&self) {
        self.spring.stop();
        if let Some(window) = self.window.upgrade() {
            window.invoke_drag_gone();
        }
    }

    /// [`Controller::drag_gone`] when a drag is over the window (`DragPointer.over`): the left
    /// button was let go (and Slint has taken the release), or the pointer is past the window's
    /// edges, where nothing tells the window of it. Nor does Slint tell the end of a drag whose
    /// row is gone (a folder opened under it, a list scrolled it away).
    fn drag_maybe_gone(&self) {
        let over = self
            .window
            .upgrade()
            .is_some_and(|window| window.global::<DragPointer<'_>>().get_over());
        if over {
            self.drag_gone();
        }
    }

    /// The pointer left the window, as winit told: a drag over the window is gone from it
    /// ([`Controller::drag_maybe_gone`]). With the left button up, Slint hears it too
    /// (`PointerExited`): its adapter passes no `CursorLeft` on while it thinks a button is
    /// pressed, as after a drag of rows that went on as the system's drag, whose release it never
    /// heard, and a row would stay shown as under the pointer. The hook runs before the adapter,
    /// so this exit is the first: the adapter's own, when it passes one on, comes second and
    /// finds nothing under the pointer. With the button down, the pointer that left is a drag's,
    /// which goes on.
    fn pointer_left(&self) {
        if !self.left_down.get()
            && let Some(window) = self.window.upgrade()
        {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerExited);
        }
        self.drag_maybe_gone();
    }

    /// The right button went down in the window: `true` when the press is kept from Slint.
    ///
    /// While the left button is down in the window, no menu shows, the app's or the system's:
    /// the menu's loop would take the left button's release, which winit then never tells, and a
    /// drag under way would go on with no button held, to drop at the next click, or to go out
    /// of the window as the system's drag, which drops at once where no button is held. A drag
    /// of rows ends there with no drop, as File Explorer ends its own at a right press
    /// ([`Controller::cancel_drag_of_rows`]). Slint, which passes a right press on to the place
    /// under the pointer during a drag, never sees it. With the left button up, the system's own
    /// menu may show in the place of the app's ([`Controller::right_pressed`]).
    fn right_button_pressed(&self) -> bool {
        if self.left_down.get() {
            self.cancel_drag_of_rows();
            return true;
        }
        self.right_pressed()
    }

    /// A key while the left button is down in the window ([`keys::held_key`]): Esc ends a drag of
    /// rows under way, and nothing else (the selection stays); a key that opens a menu opens
    /// none, as a right press shows none then ([`Controller::right_button_pressed`]). `Some`,
    /// whether the key is used, when it is one of those; `None` leaves the key to do what it
    /// always does (Esc too, when no drag of rows is under way).
    fn held_key(&self, text: &str, mods: Mods) -> Option<bool> {
        if !self.left_down.get() {
            return None;
        }
        match keys::held_key(text, mods, Os::CURRENT)? {
            keys::HeldKey::CancelDrag => self.cancel_drag_of_rows().then_some(true),
            keys::HeldKey::NoMenu => Some(true),
        }
    }

    /// Ends the drag of rows under way (the left button is down in the window, and a row's drag
    /// area drags: `DragPointer.rows`) with no drop, as Esc or a right press does; `true` when
    /// there was one. Slint's drag ends as when it goes out of the window as the system's drag
    /// ([`Controller::end_drag_of_rows`]), and the left button counts as up from now on: the
    /// release of a menu that showed anyway may never come, and the next exit from the window
    /// starts no drag out ([`Controller::drag_leaves`]). Its release, when it comes, drops
    /// nothing. The drag the press readied stays the state's until the next press: it never went
    /// out, and no drag of Slint carries it any more.
    fn cancel_drag_of_rows(&self) -> bool {
        let Some(window) = self.window.upgrade() else {
            return false;
        };
        if !self.left_down.get() || !window.global::<DragPointer<'_>>().get_rows() {
            return false;
        }
        self.end_drag_of_rows(&window);
        self.left_down.set(false);
        true
    }

    /// Slint's drag of rows ends with no drop: the pointer leaves the window as far as Slint
    /// knows (a `PointerExited`, after which `drag-finished` comes with `none`, never a drop,
    /// which a release would make where the last move was taken), `DragPointer` forgets it, and
    /// what the drag started stops: the time of a rest and the lists that scroll under it
    /// ([`Controller::drag_gone`]). From then on the window may name a place under the system's
    /// drag (`ui_bridge::drop_spot`), which moves Slint's pointer, since no drag of Slint's is
    /// under way.
    fn end_drag_of_rows(&self, window: &MainWindow) {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerExited);
        window.global::<DragPointer<'_>>().set_rows(false);
        self.drag_gone();
    }

    /// The left button was let go, and Slint has taken the release: a drag of rows is over (its
    /// row, gone, may tell no end), and so is any drag over the window.
    fn left_button_released(&self) {
        if let Some(window) = self.window.upgrade() {
            window.global::<DragPointer<'_>>().set_rows(false);
        }
        self.drag_maybe_gone();
    }

    /// The pointer is past the window's edges, where a drag holds it: a drag of rows goes on as
    /// the system's drag, on the next turn of the event loop (DND-05 보내기,
    /// [`Controller::drag_leaves`]), where the system takes one ([`dnd::DRAGS_OUT`]; elsewhere it
    /// stays Slint's, and a release out there drops nothing); and any drag over the window is
    /// gone from it.
    fn pointer_outside(&self) {
        let rows = self
            .window
            .upgrade()
            .is_some_and(|window| window.global::<DragPointer<'_>>().get_rows());
        if dnd::DRAGS_OUT && rows && !self.leaving.replace(true) {
            slint::Timer::single_shot(Duration::ZERO, || {
                with(Controller::drag_leaves);
            });
        }
        self.drag_maybe_gone();
    }

    /// The drag of rows under way left the window ([`Controller::pointer_outside`]), and goes on
    /// as the system's drag, which other apps take (DND-05 보내기), while the left button is
    /// still down and the state lets it go ([`AppState::drag_leaves_window`]; else it stays
    /// Slint's):
    ///
    /// 1. Slint's drag ends with no drop, and what it started stops
    ///    ([`Controller::end_drag_of_rows`]).
    /// 2. The system's drag begins, with no borrow of the state held, if the left button is
    ///    still down as the system has it (`rf_platform::dnd::drag_out` checks): on Windows it
    ///    runs inside the call, and the window's own drop target answers it as it passes over the
    ///    window (module `drops`), the drag of rows that came back. Its end comes to
    ///    [`AppState::drag_out_ended`], which does nothing with what the taker did: the app
    ///    deletes nothing.
    fn drag_leaves(&self) {
        self.leaving.set(false);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if !window.global::<DragPointer<'_>>().get_rows() || !self.left_down.get() {
            return;
        }
        let out = self
            .state
            .borrow_mut()
            .as_mut()
            .and_then(AppState::drag_leaves_window);
        let Some(out) = out else {
            return;
        };
        self.end_drag_of_rows(&window);
        let ours = out.ours;
        let done: Box<dyn FnOnce(Option<DropEffect>)> = Box::new(move |taken| {
            // Out of the system's call (macOS: its callback) first.
            slint::Timer::single_shot(Duration::ZERO, move || {
                with(|c| c.update(|state| state.drag_out_ended(ours, taken)));
            });
        });
        let begun = dnd::drag_out(self.owner(), &out.paths, Effects::ALL, out.ours, done);
        // Nothing waits for the button's release any more: the system's drag has it (winit hears
        // no release of it), or it is up already (a loop of the system took its release), or
        // Slint's drag is over anyway.
        self.left_down.set(false);
        if let Err(error) = begun {
            log_drag_out_failed(&error);
            self.update(|state| state.drag_out_ended(ours, None));
        }
    }

    /// The modifier keys changed: the state chooses a drop's action with them from now on.
    /// While a drag hovers over the window, the window asks again where the pointer is, on the
    /// next turn of the event loop, so that the pointer and what shows beside it follow the keys
    /// without a move of the mouse. Slint asks again at the press of a key itself, but winit may
    /// tell the keys held after that press.
    fn modifiers_changed(&self, mods: DragMods) {
        if let Some(state) = self.state.borrow_mut().as_mut() {
            state.modifiers_changed(mods);
        }
        if self
            .window
            .upgrade()
            .is_some_and(|window| window.global::<DragPointer<'_>>().get_over())
        {
            slint::Timer::single_shot(Duration::ZERO, || {
                with(Controller::hover_again);
            });
        }
    }

    /// The pointer of the drag that hovers over the window moves to where it is, so that the
    /// place under it answers again ([`Controller::drop_check`]): the keys changed, a list
    /// scrolled under it, a place opened under it.
    fn hover_again(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let pointer = window.global::<DragPointer<'_>>();
        if !pointer.get_over() {
            return;
        }
        let position = slint::LogicalPosition::new(pointer.get_x(), pointer.get_y());
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
    }

    /// A left press on tab `i` of pane `side`: the tab shows, and the payload of a drag of it is
    /// ready for the drag that may follow.
    fn press_tab(&self, side: usize, i: i32) {
        self.update_at(i, |state, i| state.select_tab(side, i));
        let id = self
            .state
            .borrow()
            .as_ref()
            .map(|state| state.panes()[side].active_tab().id());
        if let (Some(id), Some(window)) = (id, self.window.upgrade()) {
            window.set_tab_drag_data(ui_bridge::tab_payload(id));
        }
    }

    /// What a drag with the payload `data` would do at `slot` of pane `side`'s strip: nothing
    /// for a payload that is no tab's, or a tab that cannot go into that strip; leave the tab
    /// where it is; or move it there. Called at every move of the pointer, and at every step
    /// the strip scrolls under it; it changes nothing.
    fn tab_move_check(&self, data: &DataTransfer, side: usize, slot: i32) -> TabDrop {
        let Some(id) = ui_bridge::tab_id(data) else {
            return TabDrop::Refused;
        };
        let state = self.state.borrow();
        let Some(state) = state.as_ref().filter(|state| state.tab_may_enter(id, side)) else {
            return TabDrop::Refused;
        };
        match index(slot) {
            Some(slot) if state.tab_drop_offer(id, side, slot) => TabDrop::Moves,
            _ => TabDrop::Stays,
        }
    }

    /// The drag with the payload `data` was dropped at `slot` of pane `side`'s strip: the tab it
    /// carries moves there, when it may; whether it did.
    fn tab_move_dropped(&self, data: &DataTransfer, side: usize, slot: i32) -> bool {
        let (Some(id), Some(slot)) = (ui_bridge::tab_id(data), index(slot)) else {
            return false;
        };
        let mut moved = false;
        self.update(|state| {
            moved = state.tab_drop_offer(id, side, slot);
            state.drop_tab(id, side, slot)
        });
        moved
    }

    /// The list of subfolders under a `›` is to close (a press elsewhere, the keys leaving the
    /// panes); nothing to do when none shows.
    fn close_subfolders(&self) {
        let listing = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.subfolders().is_some());
        if listing {
            self.update(|state| {
                state.close_subfolders();
                Vec::new()
            });
        }
    }

    /// The window's size changed: the settings keep it, the list of subfolders, which stays
    /// where its `›` was, closes, and the preview panel fits the new width.
    fn window_resized(&self) {
        self.close_subfolders();
        self.fit_preview();
        self.plan_save();
    }

    /// Gives the preview panel its width for the window's width, and hides it while the window
    /// is too narrow for it ([`preview_width`]); the setting stays.
    fn fit_preview(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let shown = window.window();
        let width = shown.size().to_logical(shown.scale_factor()).width;
        // Not laid out yet.
        if width <= 0.0 {
            return;
        }
        let fitted = preview_width(width);
        if let Some(fitted) = fitted {
            window.set_preview_width(fitted);
        }
        let cramped = fitted.is_none();
        let changed = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.preview_cramped() != cramped);
        if changed {
            self.update(|state| state.set_preview_cramped(cramped));
        }
    }

    /// Tab in the path field of pane `side`, which holds `text`: the completed text, empty when
    /// there is nothing to complete with.
    fn complete_path(&self, side: usize, text: &str) -> String {
        let mut completed = String::new();
        self.update(|state| match state.complete_path(side, text) {
            Some((text, effects)) => {
                completed = text;
                effects
            }
            None => Vec::new(),
        });
        completed
    }

    /// Entry `index` of the item menu's 링크 만들기 ▸ was chosen: links in the folder of the pane,
    /// or, with Shift held as it was chosen, in a folder the picker finds (OPS-08). The keys are
    /// read as the system has them now (`rf_platform::dnd::held_mods`): the menu of the system's
    /// own ran a loop of its own, in which the window heard no key change.
    fn make_links(&self, index: usize) {
        let held = dnd::held_mods().shift;
        self.update(|state| {
            let pick = held || state.shift_held();
            state.make_links(index, pick)
        });
    }

    /// A toolbar button or a menu item: the same command as its key. A row menu's 붙여넣기 and
    /// 여기로 이동 paste into the folder of its row, and 경로 복사 has a form for each item.
    fn command(&self, name: &str) {
        match name {
            "paste-into" => return self.update(|state| state.paste_from_menu(false)),
            "folder-properties" => return self.update(AppState::folder_properties),
            "folder-terminal" => return self.update(AppState::folder_terminal),
            "more-options" => return self.update(|state| state.more_options(false)),
            "folder-more-options" => return self.update(|state| state.more_options(true)),
            "share" => return self.update(AppState::share),
            "add-favorite" => return self.update(AppState::favorite_add_selected),
            "paste-move-into" => return self.update(|state| state.paste_from_menu(true)),
            "toggle-move-undo-toast" => {
                return self.update(|state| {
                    state.set_move_undo_toast(!state.move_undo_toast());
                    Vec::new()
                });
            }
            "toggle-system-menu-first" => {
                return self.update(|state| {
                    state.set_system_menu_first(!state.system_menu_first());
                    Vec::new()
                });
            }
            "toggle-thumbnails" => {
                return self.update(|state| {
                    state.set_show_thumbnails(!state.show_thumbnails());
                    Vec::new()
                });
            }
            _ => {}
        }
        if let Some(index) = name
            .strip_prefix("link-")
            .and_then(|number| number.parse().ok())
        {
            return self.make_links(index);
        }
        if let Some(form) = name
            .strip_prefix("copy-path-")
            .and_then(|number| number.parse().ok())
            .and_then(PathForm::from_number)
        {
            return self.update(|state| state.copy_paths_as(form));
        }
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
            "toggle-extensions" => Command::ToggleExtensions,
            "select-all" => Command::SelectAll,
            "clear-selection" => Command::ClearSelection,
            "invert-selection" => Command::InvertSelection,
            "select-pattern" => Command::SelectPattern,
            "select-same-extension" => Command::SelectSameExtension,
            "refresh" => Command::Refresh,
            "toggle-tree" => Command::ToggleTree,
            "focus-tree" => Command::FocusTree,
            "toggle-preview" => Command::TogglePreview,
            "toggle-dual" => Command::ToggleDualPane,
            "toggle-split" => Command::ToggleSplit,
            "swap-panes" => Command::SwapPanes,
            "same-folder" => Command::SameFolderInOther,
            "toggle-sync" => Command::SyncNavigation,
            "view-details" => Command::ViewDetails,
            "view-list" => Command::ViewList,
            "view-small-icons" => Command::ViewSmallIcons,
            "view-medium-icons" => Command::ViewMediumIcons,
            "view-large-icons" => Command::ViewLargeIcons,
            "view-extra-large-icons" => Command::ViewExtraLargeIcons,
            "fit-columns" => Command::FitColumns,
            "quick-look" => Command::QuickLook,
            "cut" => Command::Cut,
            "copy-files" => Command::Copy,
            "paste" => Command::Paste,
            "paste-move" => Command::PasteMove,
            "properties" => Command::Properties,
            "open-with" => Command::OpenWith,
            "run-as-admin" => Command::RunAsAdmin,
            "open-terminal" => Command::OpenTerminal,
            "system-properties" => Command::SystemProperties,
            _ => return,
        };
        self.update(|state| {
            state
                .key(command, Mods::default(), Paging::rows(1), Instant::now())
                .unwrap_or_default()
        });
    }

    /// Changes the state with `f`, drops the address bar's lists that the change left behind,
    /// lets the tree follow the active pane, gives each tab that came to another folder the view
    /// remembered for that folder and hands the sorts too long for this thread to a worker
    /// ([`AppState::settle_views`]), then carries out what all of them return and redraws.
    fn update(&self, f: impl FnOnce(&mut AppState) -> Vec<Effect>) {
        let effects = {
            let mut state = self.state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            let mut effects = f(state);
            state.drop_stale_address();
            effects.extend(state.follow_active());
            state.note_visits();
            state.settle_views(&mut effects);
            effects.extend(state.follow_preview());
            effects.extend(state.follow_properties());
            // Nothing that asks the user stays behind a hidden window.
            effects.extend(state.follow_background());
            effects.extend(state.check_folders_due(Instant::now()));
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
                Effect::CheckTrash { .. }
                | Effect::CheckCopies { .. }
                | Effect::CheckFolders(_)
                | Effect::KeepDropped { .. }
                | Effect::FindExisting { .. }
                | Effect::InspectTarget { .. }
                | Effect::ReadSpace { .. } => self.on_worker(effect),
                Effect::SaveLog { report, folder } => self.save_log(report, folder),
                Effect::Quit => self.quit(),
                Effect::Notify(_) | Effect::ToTray | Effect::FromTray => self.desktop(effect),
                Effect::Sort(_)
                | Effect::MatchNames(_)
                | Effect::Preview(_)
                | Effect::StopPreview
                | Effect::Measure(_)
                | Effect::StopMeasure
                | Effect::Hash(_)
                | Effect::StopHashes => self.to_newest(effect),
                Effect::Clipboard(work) => {
                    if !self.clipper.submit(work) {
                        on_screen.push(Effect::Notice(words::background_failed().to_owned()));
                    }
                }
                Effect::Load(_) | Effect::LoadChildren(_) | Effect::ListNames(_) => {
                    self.list(effect);
                }
                Effect::Follow { generation, link } => {
                    self.loader.follow(generation, link, |followed| {
                        post(move |c| c.update(|state| state.followed(followed)));
                    });
                }
                Effect::OpenFile(file) => open_file(file),
                Effect::Shell(work) => ask_system(work, self.owner()),
                Effect::ReloadLater {
                    generation,
                    path,
                    after,
                } => slint::Timer::single_shot(after, move || {
                    with(|c| c.update(|state| state.reload_due(generation, &path, Instant::now())));
                }),
                Effect::Notice(_)
                | Effect::EditPath(_)
                | Effect::FillPath { .. }
                | Effect::FocusFilter
                | Effect::ShowViewMenu
                | Effect::FocusTree
                | Effect::FocusPanes
                | Effect::FocusPanel
                | Effect::ScrollTo { .. }
                | Effect::ScrollSideways { .. }
                | Effect::FitColumns(_)
                | Effect::ScrollTree(_)
                | Effect::ScrollRecent(_)
                | Effect::ScrollPickTree(_)
                | Effect::ShowMenu(_)
                | Effect::SystemMenu(_)
                | Effect::Share(_)
                | Effect::Toast { .. }
                | Effect::Busy { .. }
                | Effect::NotBusy(_) => {
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
        self.watch_clipboard();
        // After the redraw, so that the rows to scroll to are there.
        for effect in on_screen {
            self.show(effect);
        }
        // Any change may be one the settings keep (a tab, the hidden items, a recent target).
        self.plan_save();
    }

    /// What a worker that asked the system something has for the window: the apps for the menu
    /// of 연결 프로그램, or a notice of what was not done.
    fn shell_news(&self, news: ShellNews) {
        match news {
            ShellNews::Apps { token, found } => {
                self.update(|state| state.apps_found(token, found));
            }
            ShellNews::Failed(action, kind) => {
                self.notice(&words::shell_failed(action, kind, Os::CURRENT));
            }
            ShellNews::LinkElsewhere { folder } => self.notice(words::link_elsewhere(folder)),
        }
    }

    /// What the clipboard's worker did, for the state.
    fn clipboard_done(&self, done: Done) {
        match done {
            Done::Put { token, result } => self.update(|state| state.clipboard_put(token, result)),
            Done::Read { token, result } => {
                self.update(|state| state.clipboard_read(token, result));
            }
            Done::Paths(result) => self.update(|state| state.paths_copied(result)),
            Done::Text { token, result } => {
                self.update(|state| state.checksum_copied(token, result));
            }
            Done::Cleared { change, result } => {
                self.update(|state| state.clipboard_cleared(change, result));
            }
            // Most polls find the clipboard as it was: the window is drawn again only for one
            // that ends the dimming or tries again to empty it.
            Done::Polled(change) => {
                let effects = self
                    .state
                    .borrow_mut()
                    .as_mut()
                    .and_then(|state| state.clipboard_polled(change));
                if let Some(effects) = effects {
                    self.apply(effects);
                }
            }
        }
    }

    /// Asks the clipboard's change count every [`CLIPBOARD_POLL`] while the state watches it
    /// ([`AppState::clipboard_watched`]), and not otherwise.
    fn watch_clipboard(&self) {
        let watched = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(AppState::clipboard_watched);
        if watched == self.clipboard_timer.running() {
            return;
        }
        if watched {
            self.clipboard_timer
                .start(slint::TimerMode::Repeated, CLIPBOARD_POLL, || {
                    with(|c| c.clipper.poll());
                });
        } else {
            self.clipboard_timer.stop();
        }
    }

    /// Hands `request` to the worker of the previews ([`Previewer`]), in the place of the one
    /// before.
    fn preview_on_worker(&self, request: preview::PreviewRequest) {
        if self.previewer.preview(request).is_err() {
            self.notice(words::background_failed());
        }
    }

    /// Carries out an effect whose work blocks, on a worker thread of its own: its result comes
    /// back to the state on the UI thread.
    fn on_worker(&self, effect: Effect) {
        match effect {
            Effect::CheckTrash { folder, paths } => {
                spawn_worker("rf-ui-trash", move || {
                    let supported = rf_platform::trash_supported(&folder);
                    post(move |c| c.update(|state| state.trash_checked(paths, supported)));
                });
            }
            Effect::CheckCopies { token, copies, at } => {
                let loader = self.loader.clone();
                let cancel = self.checks.borrow_mut().start();
                spawn_worker("rf-ui-originals", move || {
                    let found: Vec<_> = copies
                        .iter()
                        .map(|(made, original)| loader.check_copy(made, at, original, &cancel))
                        .collect();
                    post(move |c| c.update(|state| state.copies_checked(token, &found)));
                });
            }
            Effect::ReadSpace { tabs, drives } => self.read_space(tabs, drives),
            Effect::CheckFolders(folders) => check_folders(folders),
            Effect::KeepDropped {
                generation,
                sources,
                index,
            } => keep_dropped(generation, sources, index),
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
            // `apply` carries these out itself: it hands only the ones above to this method.
            Effect::Load(_)
            | Effect::Sort(_)
            | Effect::MatchNames(_)
            | Effect::Preview(_)
            | Effect::StopPreview
            | Effect::Measure(_)
            | Effect::StopMeasure
            | Effect::Hash(_)
            | Effect::StopHashes
            | Effect::Clipboard(_)
            | Effect::LoadChildren(_)
            | Effect::ListNames(_)
            | Effect::Follow { .. }
            | Effect::OpenFile(_)
            | Effect::Shell(_)
            | Effect::ShowMenu(_)
            | Effect::SystemMenu(_)
            | Effect::Share(_)
            | Effect::ReloadLater { .. }
            | Effect::Notice(_)
            | Effect::EditPath(_)
            | Effect::FillPath { .. }
            | Effect::FocusFilter
            | Effect::ShowViewMenu
            | Effect::FocusTree
            | Effect::FocusPanes
            | Effect::FocusPanel
            | Effect::ScrollTo { .. }
            | Effect::ScrollSideways { .. }
            | Effect::FitColumns(_)
            | Effect::ScrollTree(_)
            | Effect::ScrollRecent(_)
            | Effect::ScrollPickTree(_)
            | Effect::Job(_)
            | Effect::Toast { .. }
            | Effect::Busy { .. }
            | Effect::NotBusy(_)
            | Effect::SaveLog { .. }
            | Effect::Quit
            | Effect::Notify(_)
            | Effect::ToTray
            | Effect::FromTray => {}
        }
    }

    /// Gives the keys to the part of the window `effect` names: the filter box, the tree, the
    /// panes or the job panel.
    fn give_keys(&self, effect: &Effect) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        match effect {
            Effect::FocusFilter => window.invoke_focus_filter(),
            Effect::FocusTree => window.invoke_focus_tree(),
            Effect::FocusPanel => window.invoke_focus_panel(),
            _ => window.invoke_focus_panes(),
        }
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
            Effect::FillPath { side, text } => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_fill_path(side_number(side), text.into());
                }
            }
            Effect::FocusFilter | Effect::FocusTree | Effect::FocusPanes | Effect::FocusPanel => {
                self.give_keys(&effect);
            }
            Effect::ShowViewMenu => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_show_view_menu();
                }
            }
            Effect::ScrollTo { side, row } => self.scroll_to(side, row),
            Effect::ScrollSideways { side, right } => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_scroll_sideways(side_number(side), right);
                }
            }
            Effect::FitColumns(side) => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_fit_columns(side_number(side));
                }
            }
            Effect::ScrollTree(row) => self.scroll_sidebar(self.tree_rows.display_of(row)),
            Effect::ScrollRecent(at) => self.scroll_sidebar(self.tree_rows.recent_display(at)),
            Effect::ScrollPickTree(row) => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_pick_tree_scroll_to(i32::try_from(row).unwrap_or(i32::MAX));
                }
            }
            Effect::Toast {
                token,
                text,
                action,
                time,
            } => {
                if let Some(window) = self.window.upgrade() {
                    // A token past the range of a Slint int has no button to click.
                    let token = i32::try_from(token).unwrap_or(0);
                    match time {
                        Some(time) => {
                            let ms = i64::try_from(time.as_millis()).unwrap_or(i64::MAX);
                            window.invoke_show_toast_for(text.into(), action.into(), token, ms);
                        }
                        None => window.invoke_show_toast(text.into(), action.into(), token),
                    }
                }
            }
            Effect::Busy { token, text } => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_show_busy(text.into(), i32::try_from(token).unwrap_or(0));
                }
            }
            Effect::NotBusy(token) => {
                if let Some(window) = self.window.upgrade() {
                    window.invoke_end_busy(i32::try_from(token).unwrap_or(0));
                }
            }
            Effect::ShowMenu(menu) => self.show_menu(menu),
            Effect::SystemMenu(ask) => self.ask_system_menu(ask),
            Effect::Share(paths) => self.ask_share(paths),
            // Carried out by `apply`.
            Effect::Load(_)
            | Effect::Sort(_)
            | Effect::MatchNames(_)
            | Effect::Preview(_)
            | Effect::StopPreview
            | Effect::Measure(_)
            | Effect::StopMeasure
            | Effect::Hash(_)
            | Effect::StopHashes
            | Effect::Clipboard(_)
            | Effect::LoadChildren(_)
            | Effect::ListNames(_)
            | Effect::FindExisting { .. }
            | Effect::Follow { .. }
            | Effect::ReloadLater { .. }
            | Effect::OpenFile(_)
            | Effect::Shell(_)
            | Effect::Job(_)
            | Effect::CheckTrash { .. }
            | Effect::CheckCopies { .. }
            | Effect::CheckFolders(_)
            | Effect::KeepDropped { .. }
            | Effect::InspectTarget { .. }
            | Effect::ReadSpace { .. }
            | Effect::SaveLog { .. }
            | Effect::Quit
            | Effect::Notify(_)
            | Effect::ToTray
            | Effect::FromTray => {}
        }
    }

    /// Shows a menu that no click opened: the item menu or the folder's from the keys, the
    /// menu of the apps that can open a file, or the menu of 놓을 때 묻기 where a drop was.
    fn show_menu(&self, menu: MenuShown) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        match menu {
            MenuShown::Item { side, row } => window
                .invoke_show_item_menu(side_number(side), i32::try_from(row).unwrap_or(i32::MAX)),
            MenuShown::Folder(side) => window.invoke_show_blank_menu(side_number(side)),
            MenuShown::Apps(titles) => {
                ui_bridge::set_rows(
                    &self.open_with_apps,
                    titles.into_iter().map(Into::into).collect(),
                );
                window.invoke_show_open_with();
            }
            MenuShown::AskDrop {
                moves,
                copies,
                links,
            } => window.invoke_show_drop_menu(moves, copies, links),
            MenuShown::Favorite { index } => {
                let row = self
                    .state
                    .borrow()
                    .as_ref()
                    .and_then(AppState::tree_cursor_row)
                    .map_or(0, |row| self.tree_rows.display_of(row));
                window.invoke_show_favorite_menu(number(index), number(row));
            }
            MenuShown::FavoritesBlank => window.invoke_show_favorites_blank_menu(),
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

    /// Scrolls the sidebar so that its row `display` (its place in the list) shows.
    fn scroll_sidebar(&self, display: usize) {
        if let Some(window) = self.window.upgrade() {
            window.invoke_tree_scroll_to(number(display));
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
                self.panes[side].set_show_extensions(state.show_extensions());
                self.panes[side].set_cut(state.cut_names(pane.active_tab().location()));
                let mut view = self.panes[side].view(pane, side == state.active());
                view.own_view = state.views().own(pane.active_tab().location()).is_some();
                let chevron = state
                    .subfolders()
                    .filter(|list| list.side() == side)
                    .map(Subfolders::index);
                self.panes[side].show_address(&mut view, state.completion(side), chevron);
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
            window.set_show_extensions(state.show_extensions());
            window.set_drag_default(ui_bridge::drag_default_number(state.drag_default()));
            window.set_move_undo_toast(state.move_undo_toast());
            window.set_show_thumbnails(state.show_thumbnails());
            window.set_system_menu_first(state.system_menu_first());
            show_layout(&window, state.layout(), state.syncing());
            window.set_preview_fits(!state.preview_cramped());
            let list = state.subfolders().map(|list| (list.names(), list.cursor()));
            window.set_subfolders(ui_bridge::folder_menu(&self.subfolder_rows, list, true));
            // The active pane's own filter (another pane's, or none once the tab moved on).
            window.invoke_show_filter(state.filter_text().into());
            // The next drag carries the items the last press readied.
            let drag = state.drag_id();
            if self.shown_drag.replace(drag) != drag {
                window.set_drag_data(ui_bridge::drag_payload(drag));
            }
            self.tree_rows.set_recent(recent_block(state));
            self.tree_rows.sync(state.tree(), state.places());
            window.set_tree_cursor(self.sidebar_cursor(state));
            window.set_preview(self.preview.view(
                state.preview(),
                state.show_extensions(),
                &self.tz.borrow(),
            ));
            window.set_quick_look(state.quick_look_open());
            ui_bridge::show_item_menu(&window, state.item_menu());
            let links = state.item_links();
            if self.shown_links.replace(links) != links {
                ui_bridge::show_item_links(&window, links, Os::CURRENT);
            }
            self.show_jobs(&window, state);
            self.show_dialog(&window, state);
        }
        for (side, row) in scrolls {
            self.scroll_to(side, row);
        }
        self.schedule_thumbs();
    }

    /// The place in the sidebar's list of the keyboard cursor, while it lies on a row of the
    /// tree or in the group of recent folders; -1 for none.
    fn sidebar_cursor(&self, state: &AppState) -> i32 {
        let display = match (state.tree_cursor_row(), state.recent_cursor()) {
            (Some(row), _) => self.tree_rows.display_of(row),
            (None, Some(at)) if !state.recent_folders().is_empty() => {
                self.tree_rows.recent_display(at)
            }
            _ => return -1,
        };
        i32::try_from(display).unwrap_or(-1)
    }

    /// What the measure worker found for the properties window: the window alone is drawn
    /// again, when it took it.
    fn measure_news(&self, news: MeasureNews) {
        self.properties_changed(|state| state.measure_news(news));
    }

    /// The measure worker ended the request with `generation` (`None`: it failed).
    fn measured(&self, generation: u64, tally: Option<Tally>) {
        self.properties_changed(|state| state.measured(generation, tally));
    }

    /// What the worker of the checksums told.
    fn hash_news(&self, news: HashNews) {
        self.properties_changed(|state| state.hash_news(news));
    }

    /// The worker of the checksums failed the request with `generation`.
    fn hashes_failed(&self, generation: u64) {
        self.properties_changed(|state| state.hashes_failed(generation));
    }

    /// Changes the state with `f`, a change of the properties window alone, and draws the
    /// window again when `f` says it changed.
    fn properties_changed(&self, f: impl FnOnce(&mut AppState) -> bool) {
        let changed = self.state.borrow_mut().as_mut().is_some_and(f);
        if changed
            && let Some(window) = self.window.upgrade()
            && let Some(state) = self.state.borrow().as_ref()
        {
            self.show_properties(&window, state);
        }
    }

    /// Hands a request to the newest-request worker it is for (the sorts, the pattern window's
    /// matches, the previews, the properties window's measure and checksums), or stops that
    /// worker's request.
    fn to_newest(&self, effect: Effect) {
        match effect {
            // A sort no thread can be started for comes back failed, and the state says so.
            Effect::Sort(request) => {
                let _ = self.sorter.submit(request);
            }
            // As a sort; the pattern window says so.
            Effect::MatchNames(request) => {
                let _ = self.matcher.submit(request);
            }
            Effect::Preview(request) => self.preview_on_worker(request),
            Effect::StopPreview => self.previewer.stop(),
            // As a sort; the properties window says so.
            Effect::Measure(request) => {
                let _ = self.measurer.submit(request);
            }
            Effect::StopMeasure => self.measurer.stop(),
            Effect::Hash(request) => {
                let _ = self.hasher.submit(request);
            }
            Effect::StopHashes => self.hasher.stop(),
            // `apply` sends only the requests above here.
            other => unreachable!("not a request for a newest-request worker: {other:?}"),
        }
    }

    /// Sets the view of the properties window, while there is one, when it changed since the
    /// last one set.
    fn show_properties(&self, window: &MainWindow, state: &AppState) {
        if let Some(properties) = state.properties()
            && let Some(view) = self.properties.view(properties, &self.tz.borrow())
        {
            window.set_properties_view(view);
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
        let menu = window.global::<ItemMenu<'_>>();
        menu.set_can_undo(state.can_undo());
        menu.set_can_redo(state.can_redo());
        window.set_notice_action_live(state.toast_button_live());
        ui_bridge::set_rows(
            &self.recent_rows,
            state
                .recent_rows()
                .iter()
                .map(ui_bridge::recent_row_view)
                .collect(),
        );
        window.set_recent_open(state.recent_open());
        let (line, button) = state
            .panel_cursor()
            .map_or((-1, -1), |(line, button)| (number(line), number(button)));
        window.set_panel_line(line);
        window.set_panel_button(button);
        self.tray
            .follow(state.tray_view(rf_core::identity::DISPLAY_NAME));
    }

    /// The window's drop target goes on the handle the window has now, unless it is there
    /// already ([`DropTarget::follow`]).
    fn follow_window(&self) {
        self.drop_target.follow(self.owner());
    }

    /// The app's window, for the dialogs of the system to belong to ([`ui_bridge::owner_of`]).
    fn owner(&self) -> Owner {
        self.window
            .upgrade()
            .map_or(Owner::NONE, |window| ui_bridge::owner_of(&window))
    }

    /// Whether the window is the one in front, with the keyboard: not while it is hidden or
    /// minimized, or another app's window is in front. A window of another backend than
    /// winit's (the tests') counts as in front.
    fn window_active(&self) -> bool {
        self.window.upgrade().is_none_or(|window| {
            window
                .window()
                .with_winit_window(slint::winit_030::winit::window::Window::has_focus)
                .unwrap_or(true)
        })
    }

    /// Carries out what the window asks of the desktop outside it: a notification of the
    /// system; the window hiding while the jobs go on behind the tray icon, once it shows; or
    /// the window showing again, in front, the redraw taking the icon away.
    fn desktop(&self, effect: Effect) {
        let window = self.window.upgrade();
        match (effect, window) {
            (Effect::Notify(notice), _) => self.notifier.notify(notice),
            (Effect::ToTray, Some(window)) => {
                // The window hides only behind an icon that shows: one that could not be made
                // leaves it shown, and the tray is not offered again.
                let view = self
                    .state
                    .borrow()
                    .as_ref()
                    .and_then(|state| state.tray_view(rf_core::identity::DISPLAY_NAME));
                if view.is_some_and(|view| self.tray.follow(Some(view))) {
                    let _ = window.hide();
                } else {
                    post(|c| c.update(AppState::tray_unavailable));
                }
            }
            (Effect::FromTray, Some(window)) => {
                let _ = window.show();
                window.window().with_winit_window(|winit| {
                    winit.set_minimized(false);
                    winit.focus_window();
                });
                window_shown(&window);
            }
            _ => {}
        }
    }

    /// Reads again on a worker thread the free space of the folders of `tabs`, for the tabs
    /// that still have the generation each comes with, and of the drives `drives`.
    fn read_space(&self, tabs: Vec<(u64, VPath)>, drives: Vec<VPath>) {
        let loader = self.loader.clone();
        spawn_worker("rf-ui-space", move || {
            for (generation, path) in tabs {
                if let Some(available) = loader.free_space(&path) {
                    let event = LoadEvent::Space {
                        generation,
                        available,
                    };
                    post(move |c| c.loaded(event));
                }
            }
            for drive in drives {
                if let Ok(space) = rf_platform::disk_space(&drive) {
                    post(move |c| c.drive_space(&drive, space));
                }
            }
        });
    }

    /// Starts the listing `effect` asks for (a tab's folder, the folders of a folder of a tree,
    /// or the names of a list of the address bar) on the loader; it comes back to
    /// [`Controller::loaded`].
    fn list(&self, effect: Effect) {
        let loaded = |event: LoadEvent| post(move |c| c.loaded(event));
        let (generation, ticket) = match effect {
            Effect::Load(request) => (request.generation, self.loader.request(request, loaded)),
            Effect::LoadChildren(request) => (
                request.generation,
                self.loader.list_folders(request, loaded),
            ),
            Effect::ListNames(request) => {
                (request.generation, self.loader.list_names(request, loaded))
            }
            _ => return,
        };
        self.tickets.borrow_mut().insert(generation, ticket);
    }

    /// Writes the log of the job that left `report` into `folder` on a worker thread, named by
    /// the job and the time it started here ([`jobs::log_name`]), and hands the name it was
    /// written under, or why not, to the state.
    fn save_log(&self, report: jobs::SharedReport, folder: VPath) {
        let tz = self.tz.borrow().clone();
        spawn_worker("rf-ui-log", move || {
            let name = jobs::log_name(&report.0, &tz);
            let saved = match folder.as_local() {
                Some(folder) => jobs::save_log(&report.0, folder, &name)
                    .map(|path| {
                        path.file_name()
                            .map_or(name, |name| name.to_string_lossy().into_owned())
                    })
                    .map_err(|error| jobs::log_error(folder, error)),
                None => Err(rf_core::ErrorKind::Unsupported),
            };
            post(move |c| c.update(|state| state.log_saved(saved)));
        });
    }

    /// Shows the dialog of the state, setting its view before its kind (the dialog reads the
    /// view when it opens), and only when it changed. The destination picker's view is set at
    /// every redraw while it is on screen, under another dialog too: its tree and what a
    /// worker found at its target change without a change of the dialog.
    fn show_dialog(&self, window: &MainWindow, state: &AppState) {
        let kind = ui_bridge::dialog_kind(state.modal(), state.quitting());
        // Its view changes without a change of the dialog, and is set before it shows.
        self.show_properties(window, state);
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
            Some(Modal::Pattern(dialog)) => {
                window.set_pattern_view(ui_bridge::pattern_view(dialog));
            }
            // Set above.
            Some(Modal::Pick(_) | Modal::Properties) | None => {}
        }
        window.set_dialog(kind);
        if !behind {
            window.set_pick_behind(false);
        }
        *self.shown_modal.borrow_mut() = Some(now);
    }
}

/// The measure worker of the properties window: it reads through the local provider and hands
/// what it finds to the controller, from its thread.
fn measurer() -> Measurer {
    let vfs = rf_vfs::Vfs::with_local();
    properties::measurer(
        move |request, cancel| {
            crate::properties::measure(request, &vfs, cancel, PROGRESS_GAP, &mut |news| {
                post(move |c| c.measure_news(news));
            })
        },
        |outcome| {
            post(move |c| match outcome {
                Outcome::Done((generation, tally)) => c.measured(generation, Some(tally)),
                Outcome::Failed(generation) => c.measured(generation, None),
            });
        },
    )
}

/// The worker of the checksums of the properties window, as [`measurer`].
fn hasher() -> Hasher {
    let vfs = rf_vfs::Vfs::with_local();
    properties::hasher(
        move |request, cancel| {
            hashing::hash_files(
                request,
                &vfs,
                cancel,
                HASH_CHUNK,
                PROGRESS_GAP,
                &mut |news| {
                    post(move |c| c.hash_news(news));
                },
            )
        },
        |outcome| {
            // A request that ended has told of every file already.
            if let Outcome::Failed(generation) = outcome {
                post(move |c| c.hashes_failed(generation));
            }
        },
    )
}

/// Shows `layout` in the window, and whether navigation sync is on (`sync`, for the 보기
/// menu's mark).
fn show_layout(window: &MainWindow, layout: Layout, sync: bool) {
    window.set_dual_pane(layout.dual);
    window.set_stacked(layout.stacked);
    window.set_split_share(layout.share());
    window.set_show_tree(layout.tree);
    window.set_show_preview(layout.preview);
    window.set_sync_navigation(sync);
}

/// The number Slint uses for pane `side`.
fn side_number(side: usize) -> i32 {
    i32::from(side == RIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings;

    #[test]
    fn an_end_writes_the_settings_then_shuts_the_engine_down_and_waits_for_the_notice() {
        // The end of the window's close button and the end when the system ends the event
        // loop under the app (macOS ⌘Q, the Dock's 종료) do the same work.
        let dir = tempfile::tempdir().expect("a temporary folder");
        let jobs = Jobs::start(&dir.path().join("journal"), |_| {}, |_| false).expect("the engine");
        let config = dir.path().join("config");
        let saver = Saver::new(config.clone(), true);
        // A notification on its way to the system.
        let (sent, shown) = std::sync::mpsc::channel();
        let notifier = OsNotifier::new(move |title, _| {
            let _ = sent.send(title.to_owned());
            Ok(())
        });
        notifier.notify(crate::notify::Notice {
            title: "복사를 마쳤습니다".to_owned(),
            body: String::new(),
        });
        assert!(!jobs.is_closed());
        // The clipboard's worker answers the emptying of a cancelled cut late, but within the
        // wait.
        let (answer, cleared) = std::sync::mpsc::channel();
        let answering = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = answer.send(Ok(true));
        });
        end_work(End {
            jobs: jobs.clone(),
            saver: saver.clone(),
            notifier: notifier.clone(),
            last: Some("theme = \"dark\"\n".to_owned()),
            cleared: Some(cleared),
        });
        answering.join().expect("the answer");
        assert!(jobs.is_closed());
        // Shown, however long the system took (the end waits for it a while, `NOTICE_WAIT`).
        assert_eq!(
            shown.recv_timeout(Duration::from_secs(30)).as_deref(),
            Ok("복사를 마쳤습니다")
        );
        assert_eq!(
            std::fs::read_to_string(config.join(settings::FILE_NAME)).expect("the settings"),
            "theme = \"dark\"\n"
        );
        // Without a text to write (taken by an end before), and with a clipboard that never
        // answers, it waits a moment for the clipboard and ends the engine.
        let (_never, unanswered) = std::sync::mpsc::channel();
        let started = Instant::now();
        end_work(End {
            jobs: jobs.clone(),
            saver,
            notifier,
            last: None,
            cleared: Some(unanswered),
        });
        assert!(started.elapsed() >= CLEAR_AT_END);
        assert!(jobs.is_closed());
    }

    #[test]
    fn the_end_at_the_loop_s_exit_waits_for_the_end_under_way_or_does_it_once() {
        use std::cell::Cell;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let runs = Arc::new(AtomicUsize::new(0));
        let work = || {
            let runs = Arc::clone(&runs);
            move || {
                runs.fetch_add(1, Ordering::SeqCst);
            }
        };
        let made = Cell::new(false);
        let making = || {
            made.set(true);
            work()
        };

        // The close button's end is under way: its worker is the one to wait for, and no other
        // end is made.
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let under_way = std::thread::Builder::new()
            .name("under way".to_owned())
            .spawn(move || {
                let _ = gate.recv();
            })
            .expect("a thread");
        let handed = end_once(Some(under_way), false, making, |_| {
            panic!("no second worker")
        })
        .expect("the worker under way");
        assert_eq!(handed.thread().name(), Some("under way"));
        go.send(()).expect("the worker");
        handed.join().expect("the worker");
        assert!(!made.get());

        // The end is done (the engine is closed): nothing.
        let making = || {
            made.set(true);
            work()
        };
        assert!(end_once(None, true, making, |_| panic!("no worker")).is_none());
        assert!(!made.get());
        assert_eq!(runs.load(Ordering::SeqCst), 0);

        // Else the end runs once on a worker of its own.
        let worker = end_once(None, false, work, |job| {
            std::thread::Builder::new().spawn(job)
        })
        .expect("a worker");
        worker.join().expect("the worker");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // No worker can start: it runs once here, before the call returns.
        let refused = end_once(None, false, work, |_job| {
            Err(std::io::Error::other("no threads left"))
        });
        assert!(refused.is_none());
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_pointer_that_leaves_with_the_button_up_leaves_no_hover_behind() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the window");
        window
            .window()
            .set_size(slint::LogicalSize::new(1280.0, 800.0));
        window.show().expect("it shows");
        let dir = tempfile::tempdir().expect("a temporary folder");
        let jobs = Jobs::start(&dir.path().join("journal"), |_| {}, |_| false).expect("the engine");
        let saver = Saver::new(dir.path().join("config"), true);
        let controller = Controller::new(&window, jobs.clone(), Settings::default(), saver);
        // A toast keeps its time while the pointer is over it: what Slint thinks is under the
        // pointer shows there.
        let text = "항목 1개를 복사했습니다";
        let shown = || {
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, text).next()
        };
        window.invoke_show_toast(text.into(), "되돌리기".into(), 3);
        slint::platform::update_timers_and_animations();
        let toast = shown().expect("the toast shows");
        let (at, size) = (toast.absolute_position(), toast.size());
        let position = slint::LogicalPosition::new(at.x + 8.0, at.y + size.height / 2.0);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        slint::platform::update_timers_and_animations();
        // With the button down (a drag that went out of the window), the pointer that left is
        // the drag's: nothing is told.
        controller.left_down.set(true);
        controller.pointer_left();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(20));
        assert!(shown().is_some(), "still under the pointer");
        // With it up, Slint hears that the pointer left (its adapter would not, while it thinks a
        // button is pressed): nothing stays under it, and the toast's time runs again.
        controller.left_down.set(false);
        controller.pointer_left();
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(9));
        assert!(shown().is_none(), "no hover left behind");
        drop(controller);
        assert!(jobs.close_down(Duration::from_secs(10)));
    }

    #[test]
    fn a_drop_refused_while_the_state_is_borrowed_ends_what_the_drag_showed() {
        use rf_platform::dnd::DroppedItems;

        use crate::{DropAt, DropPlace};

        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().expect("the window");
        window.show().expect("it shows");
        let dir = tempfile::tempdir().expect("a temporary folder");
        let jobs = Jobs::start(&dir.path().join("journal"), |_| {}, |_| false).expect("the engine");
        let saver = Saver::new(dir.path().join("config"), true);
        let controller = Controller::new(&window, jobs.clone(), Settings::default(), saver);
        // What the last hover of a drag of another app left: the place it took, the list that
        // scrolls under it, what the system shows under its pointer, the time of a rest.
        let place = DropAt {
            found: true,
            place: DropPlace::List,
            side: 0,
            index: -1,
        };
        window.set_outside_over(place.clone());
        window.set_outside_drop(place);
        controller.outside_hint.borrow_mut().message = "이동 → %1".to_owned();
        let folder = VPath::local(dir.path()).expect("a local path");
        controller
            .spring
            .hover(Some((DropSpot::Pane { side: 0, row: None }, folder)));
        assert!(controller.spring.running());

        // The system drops while the state is borrowed (a loop of its own inside the app's
        // code): nothing is done, and nothing of the drag shows or runs on.
        let held = controller.state.borrow_mut();
        let items = DroppedItems {
            paths: vec![dir.path().join("a.txt")],
            ..DroppedItems::default()
        };
        let at = WindowPoint { x: 400.0, y: 300.0 };
        let done = controller.outside_drop(at, items, DragMods::default(), Effects::COPY, false);
        drop(held);
        assert_eq!(done, None);
        assert!(!window.get_outside_over().found);
        assert!(!window.get_outside_drop().found);
        assert_eq!(controller.outside_hint(), DropHint::default());
        assert!(!controller.spring.running());
        drop(controller);
        assert!(jobs.close_down(Duration::from_secs(10)));
    }

    /// A window with the app's controller, as [`begin`] connects them, whose left pane lists the
    /// file `x.txt` of a folder and whose right pane an empty folder, both real; the drops Slint
    /// makes are counted. What the winit hook would do is done by hand: the tests have no winit.
    struct Dragging {
        window: MainWindow,
        controller: Rc<Controller>,
        jobs: Jobs,
        /// How many drops Slint made (`drop-items`).
        drops: Rc<Cell<usize>>,
        /// The middle of the row of `x.txt`, and a place of the right pane's list.
        row: slint::LogicalPosition,
        there: slint::LogicalPosition,
        _dir: tempfile::TempDir,
    }

    impl Dragging {
        fn new() -> Dragging {
            use i_slint_backend_testing::{AccessibleRole, ElementHandle};
            use rf_core::{Entry, EntryKind};

            i_slint_backend_testing::init_no_event_loop();
            let window = MainWindow::new().expect("the window");
            window
                .window()
                .set_size(slint::LogicalSize::new(1280.0, 800.0));
            window.show().expect("it shows");
            let dir = tempfile::tempdir().expect("a temporary folder");
            let (left, right) = (dir.path().join("left"), dir.path().join("right"));
            std::fs::create_dir(&left).expect("a folder");
            std::fs::create_dir(&right).expect("a folder");
            std::fs::write(left.join("x.txt"), b"x").expect("a file");
            let jobs =
                Jobs::start(&dir.path().join("journal"), |_| {}, |_| false).expect("the engine");
            let saver = Saver::new(dir.path().join("config"), true);
            let controller = Rc::new(Controller::new(
                &window,
                jobs.clone(),
                Settings::default(),
                saver,
            ));
            CONTROLLER.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&controller)));
            connect(&window);
            let drops = Rc::new(Cell::new(0));
            let counted = Rc::clone(&drops);
            window.on_drop_items(move |data, place, s, i| {
                counted.set(counted.get() + 1);
                with(|c| c.drop_items(&data, place, s, i)).unwrap_or(DragAction::None)
            });
            let left = VPath::local(&left).expect("a local path");
            let (mut state, effects) = AppState::new(
                left.clone(),
                VPath::local(&right).expect("a local path"),
                None,
            );
            for effect in effects {
                if let Effect::Load(request) = effect {
                    let entries = if request.path == left {
                        vec![Entry::new("x.txt", EntryKind::File)]
                    } else {
                        Vec::new()
                    };
                    state.loaded(LoadEvent::Listed {
                        generation: request.generation,
                        entries,
                    });
                }
            }
            *controller.state.borrow_mut() = Some(state);
            controller.redraw();
            slint::platform::update_timers_and_animations();
            let row = ElementHandle::find_by_accessible_label(&window, "x.txt")
                .find(|found| found.accessible_role() == Some(AccessibleRole::ListItem))
                .expect("the row of x.txt");
            let (at, size) = (row.absolute_position(), row.size());
            let row =
                slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
            let there = slint::LogicalPosition::new(row.x + 640.0, row.y + 100.0);
            let spot = window.invoke_spot_at(there.x, there.y);
            assert!(spot.found && spot.place == DropPlace::List && spot.side == 1);
            Dragging {
                window,
                controller,
                jobs,
                drops,
                row,
                there,
                _dir: dir,
            }
        }

        /// The left button at `at`, pressed (`down`) or let go, as winit tells it to the hook
        /// and then to Slint.
        fn left(&self, down: bool, at: slint::LogicalPosition) {
            use slint::platform::{PointerEventButton, WindowEvent};

            let button = PointerEventButton::Left;
            self.controller.left_down.set(down);
            self.window.window().dispatch_event(if down {
                WindowEvent::PointerPressed {
                    position: at,
                    button,
                }
            } else {
                WindowEvent::PointerReleased {
                    position: at,
                    button,
                }
            });
            if !down {
                self.controller.left_button_released();
            }
            slint::platform::update_timers_and_animations();
        }

        fn move_to(&self, at: slint::LogicalPosition) {
            self.window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at });
            slint::platform::update_timers_and_animations();
        }

        /// A drag of the row of `x.txt` (its press selects it) over the right pane's list, which
        /// takes it.
        fn drag_row(&self) {
            self.move_to(self.row);
            self.left(true, self.row);
            for step in [10.0, 20.0, 30.0] {
                self.move_to(slint::LogicalPosition::new(self.row.x + step, self.row.y));
            }
            self.move_to(self.there);
            assert!(self.rows(), "a drag of rows is under way");
            assert!(self.over());
        }

        /// `DragPointer.rows`, as at the top of a turn of the event loop.
        fn rows(&self) -> bool {
            slint::platform::update_timers_and_animations();
            self.window.global::<DragPointer<'_>>().get_rows()
        }

        fn over(&self) -> bool {
            self.window.global::<DragPointer<'_>>().get_over()
        }

        /// Whether the drag the press readied is still the window's, never gone out of it
        /// (`AppState::drag_leaves_window` would hand it out now).
        fn never_went_out(&self) -> bool {
            self.controller
                .state
                .borrow_mut()
                .as_mut()
                .is_some_and(|state| state.drag_leaves_window().is_some())
        }

        fn end(self) {
            CONTROLLER.with(|cell| cell.borrow_mut().take());
            drop(self.controller);
            assert!(self.jobs.close_down(Duration::from_secs(10)));
        }
    }

    #[test]
    fn a_drag_of_rows_goes_out_only_while_the_button_is_down() {
        let dragging = Dragging::new();
        // The key of the 보기 menu here (F10, macOS ⌘J).
        let (chord, _) = keys::bindings(Os::CURRENT)
            .find(|(_, command)| **command == Command::ViewMenu)
            .expect("a key of the 보기 menu");
        let view_key = match chord.press {
            keys::Press::Key(key) => char::from(key).to_string(),
            keys::Press::Char(c) => c.to_string(),
        };
        let menu_shows = || {
            slint::platform::update_timers_and_animations();
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &dragging.window,
                "보기 방식",
            )
            .next()
            .is_some()
        };
        dragging.drag_row();
        // During the drag no key opens a menu, whose loop would take the left button's
        // release: the drag goes on.
        assert!(dragging.controller.key(&view_key, chord.mods, false));
        assert!(!menu_shows());
        assert!(dragging.rows());
        // A right press during the drag (with Shift, or with Windows' menu first, a menu of the
        // system would take the left button's release) ends it with no drop, as File Explorer
        // ends its own, and shows no menu: Slint never sees the press.
        assert!(
            dragging.controller.right_button_pressed(),
            "kept from Slint"
        );
        assert!(!dragging.rows());
        assert!(!dragging.over());
        assert!(!dragging.controller.left_down.get());
        // The pointer that leaves the window then carries nothing out of it (on Windows and
        // macOS the system's drag would drop at once, with no button held).
        dragging.controller.pointer_outside();
        dragging.controller.drag_leaves();
        assert!(dragging.never_went_out());
        // Neither the release of the left button nor a later click on the other pane drops it.
        dragging.move_to(dragging.there);
        dragging.left(false, dragging.there);
        dragging.left(true, dragging.there);
        dragging.left(false, dragging.there);
        assert_eq!(dragging.drops.get(), 0);
        assert!(!dragging.rows());
        // With the left button up, the key opens its menu as before.
        assert!(dragging.controller.key(&view_key, chord.mods, false));
        assert!(menu_shows());
        dragging.end();
    }

    #[test]
    fn esc_ends_a_drag_of_rows_with_no_drop_and_keeps_the_selection() {
        let escape = char::from(slint::platform::Key::Escape).to_string();
        let dragging = Dragging::new();
        let selected = || {
            dragging.controller.state.borrow().as_ref().map(|state| {
                let tab = state.panes()[LEFT].active_tab();
                tab.selection().indices(tab.visible())
            })
        };
        dragging.drag_row();
        assert_eq!(selected(), Some(vec![0]));
        assert!(dragging.controller.key(&escape, Mods::NONE, false));
        assert!(!dragging.rows());
        assert!(!dragging.over());
        assert_eq!(selected(), Some(vec![0]), "the selection stays");
        // The release where the drag was drops nothing, and nothing goes out of the window.
        dragging.left(false, dragging.there);
        assert_eq!(dragging.drops.get(), 0);
        dragging.controller.pointer_outside();
        dragging.controller.drag_leaves();
        assert!(dragging.never_went_out());
        // With no drag, Esc clears the selection as before.
        assert!(dragging.controller.key(&escape, Mods::NONE, false));
        assert_eq!(selected(), Some(Vec::new()));
        dragging.end();
    }
}
