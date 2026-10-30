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
use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use rf_core::{Os, VPath};
use rf_jobs::{JobEvent, TransferMode};
use rf_platform::KnownFolder;
use slint::language::DragAction;
use slint::winit_030::WinitWindowAccessor;
use slint::{CloseRequestResponse, ComponentHandle, DataTransfer, ModelRc, VecModel};

use crate::dialogs::{ConflictChoice, Modal};
use crate::format::folder_label;
use crate::hashing::{self, HASH_CHUNK, HashNews, PROGRESS_GAP};
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
    AppState, Column, DropSpot, Effect, LEFT, Layout, MenuShown, Paging, PathForm, RIGHT,
    ShellWork, Subfolders, preview_width, start_folders,
};
use crate::ui_bridge::{
    self, PaneModels, PickModels, PreviewModels, PropertiesModels, TreeRows, post,
};
use crate::{
    DialogKind, DropPlace, FolderMenuRow, JobRowView, MainWindow, RecentRowView, TabDrop, open,
    words,
};

pub mod clipboard;
mod matcher;
mod newest;
mod persist;
mod previewer;
mod properties;
mod sorter;

use clipboard::{Clipper, Done, OsClipboard};
use matcher::Matcher;
use newest::Outcome;
pub(crate) use persist::theme_number;
pub use persist::{FINAL_SAVE_WAIT, RESTORE_WAIT, RESTORE_WORKERS, SAVE_DELAY};
use persist::{SaveLedger, logical, look_for, theme_of, wait_for};
pub use previewer::Previewer;
use properties::{Hasher, Measurer};
use sorter::Sorter;

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

/// Opens `file` with the system's default app, on a worker thread; a failure shows as a notice.
fn open_file(file: VPath) {
    open::open_file(file, |kind| {
        post(move |c| c.notice(&words::cannot_open(kind)));
    });
}

/// Asks the system for `work` on a worker thread ([`open::shell`]); what it has for the window
/// comes back to [`Controller::shell_news`].
fn ask_system(work: ShellWork) {
    open::shell(work, |news| {
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
    let controller = Rc::new(Controller::new(window, jobs, settings, saver));
    *controller.log_dir.borrow_mut() = log_dir;
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
    window.on_key(|text, ctrl, shift, alt, meta| {
        let mods = Mods {
            ctrl,
            shift,
            alt,
            meta,
        };
        with(|c| c.key(&text, mods)).unwrap_or(false)
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
    window.on_row_released(|s, i| {
        with(|c| c.update_at(i, |state, i| state.release_row(side(s), i)));
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
    window.on_drop_check(|data, place, s, i, action| {
        with(|c| c.drop_check(&data, place, s, i, action)).unwrap_or(DragAction::None)
    });
    window.on_drop_items(|data, place, s, i, action| {
        with(|c| c.drop_items(&data, place, s, i, action)).unwrap_or(DragAction::None)
    });
    connect_tabs(window);
    connect_jobs(window);
    connect_report(window);
    connect_pick(window);
    connect_properties(window);
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

/// The place of the state that a drag over `place` names, with the pane `side` and the row or
/// tab `index` of the callback; `None` for a negative tab or tree row.
fn drop_spot(place: DropPlace, side_number: i32, i: i32) -> Option<DropSpot> {
    Some(match place {
        DropPlace::List => DropSpot::Pane {
            side: side(side_number),
            row: index(i),
        },
        DropPlace::Tab => DropSpot::Tab {
            side: side(side_number),
            index: index(i)?,
        },
        DropPlace::Tree => DropSpot::Tree(index(i)?),
    })
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
        // The tabs show the views remembered for their folders, and ask for their listings in
        // their orders.
        state.set_views(self.settings.borrow().views());
        state.settle_views(&mut effects);
        if !self.saver.writable() {
            effects.push(Effect::Notice(words::settings_unreadable().to_owned()));
        }
        let places = self.places.borrow().clone();
        if !places.is_empty() {
            effects.extend(state.set_places(&places));
        }
        effects.extend(state.follow_active());
        state.set_history(self.jobs.history());
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
    /// [`crate::run`], after the event loop.
    fn quit(&self) {
        // The looks at copies stop: the state takes no answer while the app ends.
        self.checks.borrow_mut().stop();
        let jobs = self.jobs.clone();
        let saver = self.saver.clone();
        let notifier = self.notifier.clone();
        let last = self.final_settings();
        let taken = last.is_some();
        let started = std::thread::Builder::new()
            .name("rf-ui-quit".to_owned())
            .spawn(move || {
                // First: they do not wait for the engine, and an end forced while it shuts
                // down does not lose them.
                if let Some(text) = last {
                    let _ = saver.save_within(text, FINAL_SAVE_WAIT);
                }
                jobs.close_down(QUIT_TIMEOUT);
                // The last notification, of the jobs that ended behind the tray icon, is not
                // cut off by the end of the app.
                notifier.wait_idle(NOTICE_WAIT);
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            });
        if started.is_err() {
            if taken {
                self.reopen_settings();
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

    /// A key; `true` when the window used it. While the list of subfolders under a `›` shows,
    /// its keys (`keys::list_key`) go to it; any other command closes it as it does its work.
    fn key(&self, text: &str, mods: Mods) -> bool {
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

    /// A key while the sidebar's tree has the keys; `true` when it is used there. A key of the
    /// tree moves in it; one of the panes' rows is used, and does nothing; any other goes on to
    /// the window's keys (`false`), as from the panes.
    fn tree_key(&self, text: &str, mods: Mods) -> bool {
        if let Some(key) = keys::tree_key(text, mods, Os::CURRENT) {
            self.update(|state| state.tree_key(key));
            return true;
        }
        keys::command_for(text, mods, Os::CURRENT).is_some_and(|command| command.acts_on_rows())
    }

    /// A key while the job panel has the keys; `true` when it is used there, as with the tree
    /// ([`Controller::tree_key`]).
    fn panel_key(&self, text: &str, mods: Mods) -> bool {
        if let Some(key) = keys::panel_key(text, mods, Os::CURRENT) {
            self.update(|state| state.panel_key(key));
            return true;
        }
        keys::command_for(text, mods, Os::CURRENT).is_some_and(|command| command.acts_on_rows())
    }

    /// A drag with the payload `data` hovers over `place` (pane `side`, row or tab `index`)
    /// and asks for `action`: the action the place takes, or `none`. What a drop there would
    /// do goes to the label beside it. Called at every move of the pointer, it changes nothing
    /// else and redraws nothing.
    fn drop_check(
        &self,
        data: &DataTransfer,
        place: DropPlace,
        side: i32,
        i: i32,
        action: DragAction,
    ) -> DragAction {
        let (Some(spot), Some(mode)) = (drop_spot(place, side, i), ui_bridge::drag_mode(action))
        else {
            return DragAction::None;
        };
        let id = ui_bridge::drag_id(data);
        let (offer, refused) = match self.state.borrow_mut().as_mut() {
            Some(state) => match state.drop_offer(id, spot) {
                Some(offer) => (Some(offer), None),
                None => (None, state.drop_refused(id, spot)),
            },
            None => (None, None),
        };
        let Some(window) = self.window.upgrade() else {
            return DragAction::None;
        };
        // Why the place does not take them, beside it; nothing where it does.
        let why = refused.map(words::drop_refused).unwrap_or_default();
        window.invoke_drop_refused(place, side, why.into());
        let Some(offer) = offer else {
            return DragAction::None;
        };
        let label = words::drag_label(offer.count, mode, &folder_label(&offer.folder));
        if window.get_drag_label() != label.as_str() {
            window.set_drag_label(label.into());
        }
        action
    }

    /// The drag with the payload `data` was dropped on `place` with `action`: the place's
    /// folder gets the transfer of its items, when it takes them; the action done, or `none`.
    fn drop_items(
        &self,
        data: &DataTransfer,
        place: DropPlace,
        side: i32,
        i: i32,
        action: DragAction,
    ) -> DragAction {
        let (Some(spot), Some(mode)) = (drop_spot(place, side, i), ui_bridge::drag_mode(action))
        else {
            return DragAction::None;
        };
        let id = ui_bridge::drag_id(data);
        let mut taken = false;
        self.update(|state| {
            let effects = state.drop_items(id, spot, mode);
            // A notice (the drop was not made) takes nothing.
            taken = effects
                .iter()
                .any(|effect| matches!(effect, Effect::Job(_)));
            effects
        });
        if taken { action } else { DragAction::None }
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

    /// A toolbar button or a menu item: the same command as its key. A row menu's 붙여넣기 and
    /// 여기로 이동 paste into the folder of its row, and 경로 복사 has a form for each item.
    fn command(&self, name: &str) {
        match name {
            "paste-into" => return self.update(|state| state.paste_from_menu(false)),
            "folder-properties" => return self.update(AppState::folder_properties),
            "folder-terminal" => return self.update(AppState::folder_terminal),
            "paste-move-into" => return self.update(|state| state.paste_from_menu(true)),
            _ => {}
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
            state.settle_views(&mut effects);
            effects.extend(state.follow_preview());
            effects.extend(state.follow_properties());
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
                Effect::Shell(work) => ask_system(work),
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
                | Effect::ScrollPickTree(_)
                | Effect::ShowMenu(_)
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

    /// Does an effect that shows on the screen.
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
            | Effect::InspectTarget { .. }
            | Effect::ReadSpace { .. }
            | Effect::SaveLog { .. }
            | Effect::Quit
            | Effect::Notify(_)
            | Effect::ToTray
            | Effect::FromTray => {}
        }
    }

    /// Shows a menu that no click opened: the item menu or the folder's from the keys, or the
    /// menu of the apps that can open a file.
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
            self.tree_rows.sync(state.tree(), &self.places.borrow());
            window.set_tree_cursor(
                state
                    .tree_cursor_row()
                    .and_then(|row| i32::try_from(row).ok())
                    .unwrap_or(-1),
            );
            window.set_preview(self.preview.view(
                state.preview(),
                state.show_extensions(),
                &self.tz.borrow(),
            ));
            window.set_quick_look(state.quick_look_open());
            ui_bridge::show_item_menu(&window, state.item_menu());
            self.show_jobs(&window, state);
            self.show_dialog(&window, state);
        }
        for (side, row) in scrolls {
            self.scroll_to(side, row);
        }
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
        window.set_can_undo(state.can_undo());
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
    /// system; the window hiding while the jobs go on behind the tray icon, which the redraw
    /// shows; or the window showing again, in front, the redraw taking the icon away.
    fn desktop(&self, effect: Effect) {
        let window = self.window.upgrade();
        match (effect, window) {
            (Effect::Notify(notice), _) => self.notifier.notify(notice),
            (Effect::ToTray, Some(window)) => {
                let _ = window.hide();
            }
            (Effect::FromTray, Some(window)) => {
                let _ = window.show();
                window.window().with_winit_window(|winit| {
                    winit.set_minimized(false);
                    winit.focus_window();
                });
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
