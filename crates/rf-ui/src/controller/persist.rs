//! How the [`Controller`] keeps the [`Settings`] while the app runs: the look at the folders of
//! the last run's tabs at the start, the theme of the 보기 menu, the window's size, place and
//! maximized state (LAY-07: put back at the start with [`placement`]), and the saves after a
//! change and as the app ends ([`SaveLedger`] decides when).

use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::path::{Component, Prefix};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rf_core::VPath;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, TimerMode};

use super::{Controller, with};
use crate::pane::ancestors;
use crate::settings::{Found, Settings, ThemeChoice};
use crate::state::{Placement, Screen, placement};
use crate::ui_bridge::post;
use crate::words;

/// How long after a change of what the settings keep they are written: the changes in that
/// time are written together.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

/// How long the start waits to know whether the folders of the last run's tabs are still there.
/// A tab whose folder is not known by then (a slow or sleeping drive) shows home meanwhile.
pub const RESTORE_WAIT: Duration = Duration::from_millis(300);

/// How many volumes the folders of the last run's tabs are looked at on together at the start
/// (one worker each): a file with hundreds of tabs does not start hundreds of threads.
pub const RESTORE_WORKERS: usize = 4;

/// How long the app's end waits for the last save of the settings before it goes on: a disk
/// that hangs (a network home that stopped answering) does not keep the window from closing.
pub const FINAL_SAVE_WAIT: Duration = Duration::from_secs(2);

/// Looks at `folders` with `look` on at most `workers` worker threads; the answers come on the
/// receiver as they are found. A worker takes the folders of one volume ([`volume_of`]) in
/// turn, and the volumes of the system's own disks go first: a volume that hangs holds one
/// worker, and neither the others nor the folders of other volumes behind it. When no worker
/// can start, every folder counts as there: its tab's listing tells.
pub(super) fn look_for(
    folders: Vec<VPath>,
    workers: usize,
    look: impl Fn(&VPath) -> Found + Send + Sync + 'static,
) -> Receiver<(VPath, Found)> {
    let (sent, answers) = mpsc::channel();
    let volumes = by_volume(folders);
    let count = workers.min(volumes.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(volumes)));
    let look = Arc::new(look);
    let mut started = 0;
    for _ in 0..count {
        let (queue, look, sent) = (Arc::clone(&queue), Arc::clone(&look), sent.clone());
        let worker = std::thread::Builder::new()
            .name("rf-ui-restore".to_owned())
            .spawn(move || {
                loop {
                    let next = queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop_front();
                    let Some(volume) = next else {
                        break;
                    };
                    for folder in volume {
                        let found = look(&folder);
                        if sent.send((folder, found)).is_err() {
                            return;
                        }
                    }
                }
            });
        started += usize::from(worker.is_ok());
    }
    if started == 0 {
        let left = std::mem::take(&mut *queue.lock().unwrap_or_else(PoisonError::into_inner));
        for folder in left.into_iter().flatten() {
            let _ = sent.send((folder, Found::Folder));
        }
    }
    answers
}

/// `folders` by their volume ([`volume_of`]), each volume's in the order they come, and the
/// volumes in the order their first folder comes, the system's own disks first.
fn by_volume(folders: Vec<VPath>) -> Vec<Vec<VPath>> {
    let mut volumes: Vec<((bool, VPath), Vec<VPath>)> = Vec::new();
    for folder in folders {
        let volume = volume_of(&folder);
        match volumes.iter_mut().find(|(known, _)| *known == volume) {
            Some((_, on_it)) => on_it.push(folder),
            None => volumes.push((volume, vec![folder])),
        }
    }
    volumes.sort_by_key(|((removable, _), _)| *removable);
    volumes.into_iter().map(|(_, on_it)| on_it).collect()
}

/// The volume `folder` is on, as far as its path tells, as the folder of its root, and whether
/// it is a removable or network one (`true`) rather than one of the system's own disks
/// ([`volume_in`], for the user and the system's drive this process runs with).
fn volume_of(folder: &VPath) -> (bool, VPath) {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok();
    let system_drive = std::env::var("SystemDrive").ok();
    volume_in(folder, user.as_deref(), system_drive.as_deref())
}

/// [`volume_of`] for the user named `user` and the system's drive `system_drive` (`C:` when
/// not known):
///
/// - on Windows a drive is a volume, removable unless it is the system's drive (a USB stick
///   at `E:`, a share mapped at `Z:`), and a server's share is one, on the network, however
///   the path spells them ([`prefix_volume`]);
/// - below where the system mounts removable and network volumes, the folder of the volume:
///   `/Volumes/<name>`, `/mnt/<name>`, `/run/media/<user>/<label>`, and `/media/<user>/<label>`
///   for `user`'s own folder there, `/media/<label>` otherwise;
/// - a location of another kind, its root;
/// - anywhere else on the system's own disk, the folder right below the root: a path cannot
///   tell a network share mounted at a folder (`/srv/nas`) from a folder, so each counts as a
///   volume of its own.
fn volume_in(folder: &VPath, user: Option<&str>, system_drive: Option<&str>) -> (bool, VPath) {
    let chain = ancestors(folder);
    let root = chain.first().unwrap_or(folder);
    let Some(path) = folder.as_local() else {
        return (true, root.clone());
    };
    let mut parts = path.components();
    match parts.next() {
        Some(Component::Prefix(prefix)) => {
            let (removable, volume) = prefix_volume(prefix.kind(), system_drive);
            return (
                removable,
                VPath::local(volume).unwrap_or_else(|_| root.clone()),
            );
        }
        Some(Component::RootDir) => {}
        _ => return (true, root.clone()),
    }
    let names: Vec<Option<&str>> = parts.map(|part| part.as_os_str().to_str()).collect();
    let depth = match names.as_slice() {
        [Some("run"), Some("media"), Some(_), Some(_), ..] => Some(4),
        [Some("media"), Some(owner), Some(_), ..] if Some(*owner) == user => Some(3),
        [Some("Volumes" | "mnt" | "media"), Some(_), ..] => Some(2),
        _ => None,
    };
    match depth.and_then(|depth| chain.get(depth)) {
        Some(volume) => (true, volume.clone()),
        None => (false, chain.get(1).unwrap_or(root).clone()),
    }
}

/// The Windows volume a path that starts with `prefix` is on, as the text of its root, and
/// whether it is a removable or network one (`true`), for the system's drive `system_drive`
/// (`C:` when not known). One volume has one root however a path spells it: a drive
/// (`C:\`, `\\?\c:\`) by its letter in upper case, and a server's share (`\\NAS\Share\`,
/// `\\?\UNC\nas\share\`) by its names in lower case, as Windows does not tell their cases
/// apart.
fn prefix_volume(prefix: Prefix<'_>, system_drive: Option<&str>) -> (bool, String) {
    let lower = |name: &OsStr| name.to_string_lossy().to_lowercase();
    match prefix {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            let system = system_drive
                .and_then(|drive| drive.bytes().next())
                .unwrap_or(b'C');
            let drive = char::from(letter.to_ascii_uppercase());
            (!letter.eq_ignore_ascii_case(&system), format!("{drive}:\\"))
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            (true, format!(r"\\{}\{}\", lower(server), lower(share)))
        }
        Prefix::Verbatim(name) => (true, format!(r"\\?\{}\", name.to_string_lossy())),
        Prefix::DeviceNS(name) => (true, format!(r"\\.\{}\", name.to_string_lossy())),
    }
}

/// The answers of `checks` that come within `wait`, until there are `count`.
pub(super) fn wait_for(
    checks: &Receiver<(VPath, Found)>,
    count: usize,
    wait: Duration,
) -> HashMap<VPath, Found> {
    let deadline = Instant::now() + wait;
    let mut found = HashMap::new();
    while found.len() < count {
        let left = deadline.saturating_duration_since(Instant::now());
        match checks.recv_timeout(left) {
            Ok((path, what)) => {
                found.insert(path, what);
            }
            Err(_) => break,
        }
    }
    found
}

/// A size in logical pixels from the settings (at most `settings::MAX_WINDOW`) as a Slint
/// length.
pub(super) fn logical(pixels: u32) -> f32 {
    f32::from(u16::try_from(pixels).unwrap_or(u16::MAX))
}

/// A length in logical pixels as a size for the settings; `None` for one below a pixel or past
/// what a window can be.
fn whole_pixels(length: f32) -> Option<u32> {
    let rounded = length.round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the cast saturates; the value is kept only for a whole 1..=u16::MAX"
    )]
    (1.0..=f32::from(u16::MAX))
        .contains(&rounded)
        .then_some(rounded as u32)
}

/// The number the 보기 menu uses for `theme` (`theme-choice` of the window, and of the notice
/// window).
pub(crate) fn theme_number(theme: ThemeChoice) -> i32 {
    match theme {
        ThemeChoice::System => 0,
        ThemeChoice::Light => 1,
        ThemeChoice::Dark => 2,
        ThemeChoice::HighContrast => 3,
    }
}

/// The theme of the 보기 menu's number `choice`.
pub(crate) fn theme_of(choice: i32) -> Option<ThemeChoice> {
    match choice {
        0 => Some(ThemeChoice::System),
        1 => Some(ThemeChoice::Light),
        2 => Some(ThemeChoice::Dark),
        3 => Some(ThemeChoice::HighContrast),
        _ => None,
    }
}

/// What the window is, as the window system tells it at a move or a resize of it: shown or
/// not, and maximized, minimized or full screen, by Slint's word or the system's own (a window
/// being maximized may not have told Slint yet).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact the window system tells"
)]
pub(super) struct WindowWord {
    pub(super) visible: bool,
    pub(super) maximized: bool,
    pub(super) minimized: bool,
    pub(super) fullscreen: bool,
}

impl WindowWord {
    /// The window shows with a size and a place of its own.
    fn own(self) -> bool {
        self.visible && !self.maximized && !self.minimized && !self.fullscreen
    }
}

/// What the settings keep of the window (LAY-07), from what the window system tells as the
/// window moves and is resized, so that a save asks it nothing: the last size (logical) and
/// outer corner (physical) the window had of its own, and whether it is maximized. A hidden,
/// minimized or full-screen window tells nothing of either: a minimized one on Windows sits at
/// (-32000, -32000). A maximize the start asked for is kept until the window has shown
/// maximized: the words before it (the window made at its normal size, then maximized) do not
/// undo it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WindowKept {
    size: Option<(u32, u32)>,
    position: Option<(i32, i32)>,
    maximized: bool,
    /// The start asked for a maximized window, which has not shown maximized yet.
    maximizing: bool,
}

impl WindowKept {
    /// What the settings say of the window at the start.
    pub(super) fn new(settings: &Settings) -> WindowKept {
        WindowKept {
            size: settings.window,
            position: settings.window_at,
            maximized: settings.maximized,
            maximizing: settings.maximized,
        }
    }

    /// The start puts the window at `placed` ([`placement`]).
    pub(super) fn placed(&mut self, placed: Placement) {
        self.size = placed.size.or(self.size);
        self.position = placed.position;
        self.maximized = placed.maximized;
        self.maximizing = placed.maximized;
    }

    /// The window moved to `corner` (outer, physical), being as `word` says.
    pub(super) fn moved(&mut self, corner: (i32, i32), word: WindowWord) {
        self.told(word);
        if word.own() {
            self.position = Some(corner);
        }
    }

    /// The window has `size` (logical) now, being as `word` says.
    pub(super) fn resized(&mut self, size: Option<(u32, u32)>, word: WindowWord) {
        self.told(word);
        if word.own()
            && let Some(size) = size
        {
            self.size = Some(size);
        }
    }

    fn told(&mut self, word: WindowWord) {
        if !word.visible || word.minimized || word.fullscreen {
            return;
        }
        if word.maximized {
            self.maximizing = false;
            self.maximized = true;
        } else if !self.maximizing {
            self.maximized = false;
        }
    }

    /// The window's last size of its own, as the settings keep it.
    pub(super) fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// The window's last outer corner of its own; `None` before the first one is known.
    pub(super) fn position(&self) -> Option<(i32, i32)> {
        self.position
    }

    /// Whether the window is maximized, as the settings keep it.
    pub(super) fn maximized(&self) -> bool {
        self.maximized
    }
}

/// What has been written of the settings, and whether to write more: from the text of the
/// settings after a change ([`Settings::to_text`]), it decides when a save is due. Texts, not
/// values, are compared: a newer version's `nan` is never equal to itself as a value, and would
/// make every change look new.
#[derive(Debug)]
pub(super) struct SaveLedger {
    /// The text last written, or read at the start.
    written: String,
    /// A save is planned (its timer runs).
    planned: bool,
    /// No save any more: the app ends, or the settings may not be written.
    closed: bool,
}

impl SaveLedger {
    /// The ledger of settings whose file read `written` at the start; one that may not be
    /// written (`Loaded::writable`) never plans a save.
    pub(super) fn new(written: String, writable: bool) -> SaveLedger {
        SaveLedger {
            written,
            planned: false,
            closed: !writable,
        }
    }

    /// Whether a change can lead to a save: none is planned yet, and saves go on.
    pub(super) fn takes_changes(&self) -> bool {
        !self.planned && !self.closed
    }

    /// A change left the settings reading `now`: `true` when a save is to be planned (it differs
    /// from what was written, and none is planned yet), which it then is.
    pub(super) fn plan(&mut self, now: &str) -> bool {
        if !self.takes_changes() || now == self.written {
            return false;
        }
        self.planned = true;
        true
    }

    /// The planned save is due, the settings reading `now` (`None`: not known): the text to
    /// write, unless it is the one written.
    pub(super) fn due(&mut self, now: Option<String>) -> Option<String> {
        self.planned = false;
        let now = now.filter(|now| !self.closed && *now != self.written)?;
        now.clone_into(&mut self.written);
        Some(now)
    }

    /// The app ends, the settings reading `now` (`None`: not known): the text to write a last
    /// time, the first time only, and also when it is the one written, so that the last write
    /// comes after any still running. No save is planned after this.
    pub(super) fn close(&mut self, now: Option<String>) -> Option<String> {
        if self.closed {
            return None;
        }
        self.closed = true;
        self.planned = false;
        let now = now?;
        now.clone_into(&mut self.written);
        Some(now)
    }

    /// The last write [`SaveLedger::close`] gave could not be handed on: it is to be taken
    /// again.
    pub(super) fn reopen(&mut self) {
        self.closed = false;
    }
}

impl Controller {
    /// The text of the settings to write as the app ends: what the window shows now. No save is
    /// planned after this. `None` before the window shows the panes, once taken, or when the
    /// settings may not be written.
    pub fn final_settings(&self) -> Option<String> {
        self.save_timer.stop();
        let snapshot = self.snapshot();
        let text = self
            .ledger
            .borrow_mut()
            .close(snapshot.as_ref().map(|(_, text)| text.clone()));
        if text.is_some()
            && let Some((settings, _)) = snapshot
        {
            *self.settings.borrow_mut() = settings;
        }
        text
    }

    /// The text of the settings as the window shows them now, without ending the saves: for the
    /// backup 완전 삭제… makes, which may still be given up. `None` before the panes show.
    pub(super) fn settings_text(&self) -> Option<String> {
        self.snapshot().map(|(_, text)| text)
    }

    /// The last write [`Controller::final_settings`] gave could not be handed on: it is to be
    /// taken again.
    pub(super) fn reopen_settings(&self) {
        self.ledger.borrow_mut().reopen();
    }

    /// The settings with what the window shows now, and their text: the tabs, the active pane,
    /// the hidden items, the recent targets, the choices of 보기 ▸ 설정, the theme, the
    /// window's size and place (both kept as they were while the window is maximized, full
    /// screen or minimized) and whether it is maximized. `None` before the panes show.
    fn snapshot(&self) -> Option<(Settings, String)> {
        let mut settings = self.settings.borrow().clone();
        {
            let state = self.state.borrow();
            let state = state.as_ref()?;
            settings.set_session(&state.session());
            settings.set_recent(state.recent_targets());
            state.save_sidebar(&mut settings);
            settings.set_views(state.views());
            settings.drag_default = state.drag_default();
            settings.move_undo_toast = state.move_undo_toast();
            settings.on_error = state.on_error();
            settings.pick_start = state.pick_start();
            settings.system_menu_first = state.system_menu_first();
            settings.show_thumbnails = state.show_thumbnails();
            settings.long_press = state.long_press();
            settings.long_press_ms = state.long_press_ms();
            settings.checkbox_select = state.checkbox_select();
        }
        settings.theme = self.theme.get();
        let kept = *self.window_kept.borrow();
        if let Some(size) = kept.size() {
            settings.window = Some(size);
        }
        settings.window_at = kept.position();
        settings.maximized = kept.maximized();
        let text = settings.to_text().ok()?;
        Some((settings, text))
    }

    /// Puts the window where the settings say it was (LAY-07), checked against `screens`
    /// ([`placement`]: off every screen, where the system puts a new window; partly off, moved
    /// onto the screen; larger than it, made to fit), before the window system makes it: its
    /// size, its place and whether it is maximized. Once; the window keeps whatever place it
    /// takes from then on.
    ///
    /// The size is the window's own (`set_size`), not only its preferred one: Slint's first
    /// show then leaves the window as the system made it, maximized when asked. Given only as
    /// the preferred size, the first show asks for that size, and on Windows and macOS a window
    /// asked for a size is maximized no more (winit's `request_inner_size`). The window is made
    /// at this size, which is the size a maximized window goes back to.
    pub(super) fn place_window(&self, screens: &[Screen]) {
        let Some(saved) = self.saved_place.take() else {
            return;
        };
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let placed = placement(saved, screens);
        if let Some((width, height)) = placed.size {
            window.set_start_width(logical(width));
            window.set_start_height(logical(height));
            window
                .window()
                .set_size(slint::LogicalSize::new(logical(width), logical(height)));
        }
        if let Some((x, y)) = placed.position {
            window
                .window()
                .set_position(slint::PhysicalPosition::new(x, y));
        }
        window.window().set_maximized(placed.maximized);
        self.window_kept.borrow_mut().placed(placed);
        self.maximize_after_show.set(placed.maximized);
    }

    /// The window that the start asked to be maximized is there and shows: it is maximized
    /// once more, by the window system's own call. X11 window managers do not take a maximize
    /// asked for before the window is mapped, and Slint makes it unmapped; on Windows and macOS
    /// the window is maximized already, and this does nothing. Once, at the start.
    pub(super) fn maximize_as_placed(&self) {
        if !self.maximize_after_show.take() {
            return;
        }
        if let Some(window) = self.window.upgrade() {
            window
                .window()
                .with_winit_window(|winit| winit.set_maximized(true));
        }
    }

    /// The window moved to `corner` (outer, physical, as winit's `Moved` tells): the settings
    /// keep it while the window has a place of its own ([`WindowKept`]).
    pub(super) fn window_moved(&self, corner: (i32, i32)) {
        let word = self.window_word();
        self.window_kept.borrow_mut().moved(corner, word);
        self.plan_save();
    }

    /// The window's size changed: the settings keep it while the window has a size of its own
    /// ([`WindowKept`]).
    pub(super) fn note_window_size(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let shown = window.window();
        let size = shown.size().to_logical(shown.scale_factor());
        let size = whole_pixels(size.width).zip(whole_pixels(size.height));
        let word = self.window_word();
        self.window_kept.borrow_mut().resized(size, word);
    }

    /// What the window is now, by Slint's word and the window system's own: asked at a move or
    /// a resize of the window, not at every change of the state.
    fn window_word(&self) -> WindowWord {
        let Some(window) = self.window.upgrade() else {
            return WindowWord::default();
        };
        let shown = window.window();
        let (maximized, minimized, fullscreen) = shown
            .with_winit_window(|winit| {
                (
                    winit.is_maximized(),
                    winit.is_minimized() == Some(true),
                    winit.fullscreen().is_some(),
                )
            })
            .unwrap_or_default();
        WindowWord {
            visible: shown.is_visible(),
            maximized: maximized || shown.is_maximized(),
            minimized: minimized || shown.is_minimized(),
            fullscreen: fullscreen || shown.is_fullscreen(),
        }
    }

    /// After a change (the window moved, too): a save [`SAVE_DELAY`] from now, when the
    /// settings differ from those last written and none is planned yet.
    pub(super) fn plan_save(&self) {
        if !self.ledger.borrow().takes_changes() {
            return;
        }
        let Some((_, now)) = self.snapshot() else {
            return;
        };
        if self.ledger.borrow_mut().plan(&now) {
            self.save_timer
                .start(TimerMode::SingleShot, SAVE_DELAY, || {
                    with(Controller::save_due);
                });
        }
    }

    /// The planned save: what the window shows now, on a worker thread.
    fn save_due(&self) {
        let snapshot = self.snapshot();
        let text = self
            .ledger
            .borrow_mut()
            .due(snapshot.as_ref().map(|(_, text)| text.clone()));
        let (Some(text), Some((settings, _))) = (text, snapshot) else {
            return;
        };
        *self.settings.borrow_mut() = settings;
        let started = self.saver.save_in_background(text, |result| {
            if result.is_err() {
                post(Controller::save_failed);
            }
        });
        if started.is_err() {
            self.save_failed();
        }
    }

    /// A save failed: a notice says so, the first time.
    fn save_failed(&self) {
        if !self.save_failure_told.replace(true) {
            self.notice(words::settings_not_saved());
        }
    }

    /// The 보기 menu chose `theme`: the window shows it at once (as its `theme-choice` changes
    /// it asks for the theme's tokens), and so does the job panel's own window, while there is
    /// one.
    pub(super) fn choose_theme(&self, theme: ThemeChoice) {
        self.theme.set(theme);
        if let Some(window) = self.window.upgrade() {
            window.set_theme_choice(theme_number(theme));
        }
        if let Some(own) = self.panel.window.borrow().as_ref() {
            own.set_theme_choice(theme_number(theme));
        }
        self.plan_save();
    }

    /// The window came to the front: while the theme follows the system's setting, the
    /// windows take their tokens again, so that a high contrast the system turned on or off
    /// meanwhile shows (Windows and macOS tell the window of a change between light and dark
    /// alone). The same tokens change nothing on the screen.
    pub(super) fn look_at_system_theme(&self) {
        if self.theme.get() != ThemeChoice::System {
            return;
        }
        if let Some(window) = self.window.upgrade() {
            window.invoke_show_theme();
        }
        if let Some(own) = self.panel.window.borrow().as_ref() {
            own.invoke_show_theme();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    /// A folder on a network volume: on a server's share on Windows, below `/Volumes`
    /// elsewhere.
    fn on_the_nas(name: &str) -> VPath {
        let path = if cfg!(windows) {
            format!(r"\\nas\share\{name}")
        } else {
            format!("/Volumes/NAS/{name}")
        };
        VPath::local(path).expect("a local path")
    }

    #[test]
    fn a_volume_that_hangs_holds_one_worker() {
        // The network volume hangs: a look at one of its folders ends only with the test.
        let (release, hang) = mpsc::channel::<()>();
        let hang = Mutex::new(hang);
        let on_nas = Arc::new(AtomicUsize::new(0));
        let looks = Arc::clone(&on_nas);
        let mut folders: Vec<VPath> = (0..6).map(|i| on_the_nas(&format!("t{i}"))).collect();
        folders.extend([folder("here"), folder("there")]);
        let local = [folder("here"), folder("there")];
        let answers = look_for(folders, RESTORE_WORKERS, move |path| {
            if local.contains(path) {
                return Found::Folder;
            }
            looks.fetch_add(1, Ordering::SeqCst);
            let _ = hang.lock().unwrap_or_else(PoisonError::into_inner).recv();
            Found::Offline
        });

        // The folders of the system's own disk are answered, though they come after the
        // volume's, which takes one worker and holds it.
        let mut found = HashMap::new();
        while found.len() < 2 {
            let (path, what) = answers
                .recv_timeout(Duration::from_secs(10))
                .expect("the local folders are answered");
            found.insert(path, what);
        }
        assert_eq!(
            found,
            HashMap::from([
                (folder("here"), Found::Folder),
                (folder("there"), Found::Folder)
            ])
        );
        let started = Instant::now();
        while on_nas.load(Ordering::SeqCst) == 0 && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(on_nas.load(Ordering::SeqCst), 1);
        drop(release);
    }

    #[test]
    fn folders_go_by_their_volume() {
        assert_eq!(volume_of(&folder("a")), volume_of(&folder("b")));
        let nas = volume_of(&on_the_nas("a"));
        assert!(nas.0);
        assert_eq!(nas, volume_of(&on_the_nas("b")));
        assert_ne!(nas, volume_of(&folder("a")));
    }

    /// The local folder at `path`.
    fn at(path: &str) -> VPath {
        VPath::local(path).expect("a local path")
    }

    #[test]
    fn a_drive_or_a_share_is_one_volume_however_its_path_spells_it() {
        use std::ffi::OsStr;

        let volume = |prefix| prefix_volume(prefix, Some("C:"));
        let (nas, share) = (OsStr::new("nas"), OsStr::new("share"));
        // A drive, with `\\?\` or without, and its letter in either case.
        assert_eq!(volume(Prefix::Disk(b'c')), (false, r"C:\".to_owned()));
        assert_eq!(
            volume(Prefix::VerbatimDisk(b'C')),
            volume(Prefix::Disk(b'C'))
        );
        assert_eq!(
            volume(Prefix::VerbatimDisk(b'e')),
            (true, r"E:\".to_owned())
        );
        assert_eq!(
            volume(Prefix::Disk(b'E')),
            volume(Prefix::VerbatimDisk(b'e'))
        );
        // A server's share, with `\\?\UNC\` or without, its names in any case.
        let on_nas = volume(Prefix::UNC(nas, share));
        assert_eq!(on_nas, (true, r"\\nas\share\".to_owned()));
        assert_eq!(volume(Prefix::VerbatimUNC(nas, share)), on_nas);
        assert_eq!(
            volume(Prefix::UNC(OsStr::new("NAS"), OsStr::new("Share"))),
            on_nas
        );
        assert_ne!(volume(Prefix::UNC(nas, OsStr::new("other"))), on_nas);
        // A volume known only by its name is one of its own.
        assert_eq!(
            volume(Prefix::Verbatim(OsStr::new("Volume{1}"))),
            (true, r"\\?\Volume{1}\".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_volume_is_where_the_system_mounts_it() {
        let volume = |path: &str| volume_in(&at(path), Some("alice"), None);
        // A removable or network volume is one, however deep the folder and wherever in it.
        for (path, mounted) in [
            ("/media/USB/a/b", "/media/USB"),
            ("/media/USB/c", "/media/USB"),
            ("/media/USB", "/media/USB"),
            ("/media/alice/USB/a/b", "/media/alice/USB"),
            ("/media/alice/Other/a", "/media/alice/Other"),
            ("/media/bob/x/y", "/media/bob"),
            ("/run/media/alice/USB/a", "/run/media/alice/USB"),
            ("/mnt/nas/a/b", "/mnt/nas"),
            ("/Volumes/Stick/a/b", "/Volumes/Stick"),
        ] {
            assert_eq!(volume(path), (true, at(mounted)), "{path}");
        }
        // Without a known user, `/media/<name>` is the volume.
        assert_eq!(
            volume_in(&at("/media/alice/USB/a"), None, None),
            (true, at("/media/alice"))
        );
        // On the system's own disk, each folder right below the root counts as one (a share
        // mounted at `/srv/nas` looks like a folder).
        assert_eq!(volume("/home/alice/a"), (false, at("/home")));
        assert_eq!(volume("/home"), (false, at("/home")));
        assert_eq!(volume("/srv/nas/a"), (false, at("/srv")));
        assert_eq!(volume("/"), (false, at("/")));
    }

    #[cfg(windows)]
    #[test]
    fn a_volume_is_a_drive_or_a_share() {
        let volume = |path: &str| volume_in(&at(path), None, Some("C:"));
        // A drive is one volume, the system's own first, whatever its folders.
        assert_eq!(volume(r"C:\Users\a\b"), (false, at(r"C:\")));
        assert_eq!(volume(r"C:\Windows"), (false, at(r"C:\")));
        assert_eq!(volume(r"C:\"), (false, at(r"C:\")));
        assert_eq!(volume(r"E:\photos\2024"), (true, at(r"E:\")));
        assert_eq!(volume(r"E:\photos"), volume(r"E:\music\a"));
        assert_eq!(volume(r"Z:\a"), (true, at(r"Z:\")));
        assert_ne!(volume(r"Z:\a"), volume(r"E:\a"));
        // The system's drive may have another letter.
        assert_eq!(
            volume_in(&at(r"D:\a"), None, Some("d:")),
            (false, at(r"D:\"))
        );
        assert!(volume_in(&at(r"C:\a"), None, Some("D:")).0);
        // A share is one volume, on the network.
        let share = volume(r"\\nas\share\a\b");
        assert!(share.0);
        assert_eq!(share, volume(r"\\nas\share\c"));
        assert_eq!(share, volume(r"\\NAS\Share\d"));
        assert_ne!(share, volume(r"\\nas\other\a"));
        // However the path is spelled.
        assert_eq!(volume(r"\\?\C:\Users\a"), volume(r"C:\Users\a"));
        assert_eq!(volume(r"\\?\UNC\nas\share\e"), share);
    }

    #[test]
    fn the_start_waits_only_a_moment_for_the_folders() {
        let (sent, checks) = mpsc::channel();
        sent.send((folder("fast"), Found::Folder)).expect("sent");
        // "slow" never answers in time: its sender stays.
        let started = Instant::now();
        let found = wait_for(&checks, 2, Duration::from_millis(100));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(found, HashMap::from([(folder("fast"), Found::Folder)]));

        // With every answer in, it does not wait.
        sent.send((folder("slow"), Found::Gone)).expect("sent");
        let started = Instant::now();
        let found = wait_for(&checks, 1, Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(found, HashMap::from([(folder("slow"), Found::Gone)]));
    }

    #[test]
    fn the_folders_are_looked_at_by_a_few_threads() {
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let (now, top) = (Arc::clone(&running), Arc::clone(&most));
        let folders: Vec<VPath> = (0..40).map(|i| folder(&format!("f{i}"))).collect();

        let answers = look_for(folders.clone(), RESTORE_WORKERS, move |path| {
            let at_once = now.fetch_add(1, Ordering::SeqCst) + 1;
            top.fetch_max(at_once, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            now.fetch_sub(1, Ordering::SeqCst);
            if path.to_uri().ends_with('0') {
                Found::Gone
            } else {
                Found::Folder
            }
        });

        let found: HashMap<VPath, Found> = answers.iter().collect();
        assert_eq!(found.len(), folders.len(), "every folder is answered");
        assert_eq!(found[&folder("f10")], Found::Gone);
        assert_eq!(found[&folder("f11")], Found::Folder);
        assert!(most.load(Ordering::SeqCst) <= RESTORE_WORKERS);
        assert!(
            look_for(Vec::new(), RESTORE_WORKERS, |_| Found::Gone)
                .recv()
                .is_err()
        );
    }

    #[test]
    fn a_save_is_planned_once_for_the_changes_until_it_is_due() {
        let mut ledger = SaveLedger::new("a".to_owned(), true);
        assert!(!ledger.plan("a"), "nothing changed");
        assert!(ledger.plan("b"));
        assert!(!ledger.plan("c"), "one is planned already");
        assert!(!ledger.takes_changes());
        // It writes what the settings read when it is due.
        assert_eq!(ledger.due(Some("c".to_owned())), Some("c".to_owned()));
        assert!(!ledger.plan("c"), "that is written");
        assert!(ledger.plan("d"));
        // Changed back in the meantime: nothing to write.
        assert_eq!(ledger.due(Some("c".to_owned())), None);
        assert!(ledger.takes_changes());
    }

    #[test]
    fn the_last_save_comes_once_and_ends_the_plans() {
        let mut ledger = SaveLedger::new("a".to_owned(), true);
        assert!(ledger.plan("b"));
        // Also unchanged: it comes after any write still running.
        assert_eq!(ledger.close(Some("a".to_owned())), Some("a".to_owned()));
        assert_eq!(ledger.close(Some("b".to_owned())), None, "once");
        assert!(!ledger.plan("c"));
        assert_eq!(ledger.due(Some("c".to_owned())), None);
        // A last write that could not be handed on is taken again.
        ledger.reopen();
        assert_eq!(ledger.close(Some("c".to_owned())), Some("c".to_owned()));
        // Before the panes show there is nothing to write.
        let mut early = SaveLedger::new("a".to_owned(), true);
        assert_eq!(early.close(None), None);
    }

    #[test]
    fn settings_that_may_not_be_written_never_are() {
        let mut ledger = SaveLedger::new(String::new(), false);
        assert!(!ledger.takes_changes());
        assert!(!ledger.plan("b"));
        assert_eq!(ledger.due(Some("b".to_owned())), None);
        assert_eq!(ledger.close(Some("b".to_owned())), None);
    }

    #[test]
    fn a_nan_of_a_newer_version_plans_no_save() {
        let settings: Settings = toml::from_str("zoom = nan\n").expect("parsed");
        let read = settings.to_text().expect("text");
        let mut ledger = SaveLedger::new(read, true);
        let again = settings.clone().to_text().expect("text");
        assert!(!ledger.plan(&again), "the same text is no change");
    }

    /// A window that shows, maximized or not.
    fn shown(maximized: bool) -> WindowWord {
        WindowWord {
            visible: true,
            maximized,
            ..WindowWord::default()
        }
    }

    #[test]
    fn a_maximize_the_start_asks_for_holds_until_the_window_shows_it() {
        let saved = Settings {
            window: Some((1400, 900)),
            window_at: Some((100, 80)),
            maximized: true,
            ..Settings::default()
        };
        let mut kept = WindowKept::new(&saved);
        kept.placed(Placement {
            position: Some((100, 80)),
            size: Some((1400, 900)),
            maximized: true,
        });
        // Made at its normal size and place first: that is what it goes back to, and the
        // maximize stays asked for.
        kept.moved((100, 80), shown(false));
        kept.resized(Some((1400, 900)), shown(false));
        assert!(kept.maximized());
        assert_eq!(kept.size(), Some((1400, 900)));
        // Maximized, its corner and size are no place of its own.
        kept.moved((-8, -8), shown(true));
        kept.resized(Some((1920, 1040)), shown(true));
        assert!(kept.maximized());
        assert_eq!(kept.position(), Some((100, 80)));
        assert_eq!(kept.size(), Some((1400, 900)));
        // The user restores it: no longer maximized, back at its own place.
        kept.resized(Some((1400, 900)), shown(false));
        assert!(!kept.maximized());
        kept.moved((300, 200), shown(false));
        assert_eq!(kept.position(), Some((300, 200)));
        // Without a maximize asked for, the first word is the window's.
        let mut normal = WindowKept::new(&Settings::default());
        normal.resized(Some((1280, 800)), shown(false));
        assert!(!normal.maximized());
        normal.resized(Some((1920, 1040)), shown(true));
        assert!(normal.maximized());
        assert_eq!(normal.size(), Some((1280, 800)));
    }

    #[test]
    fn a_minimized_hidden_or_full_screen_window_tells_nothing_of_its_place() {
        let mut kept = WindowKept::new(&Settings {
            window: Some((1200, 800)),
            window_at: Some((40, 30)),
            ..Settings::default()
        });
        // Windows moves a minimized window to (-32000, -32000) before it tells its new size.
        for word in [
            WindowWord {
                visible: true,
                minimized: true,
                ..WindowWord::default()
            },
            WindowWord::default(),
            WindowWord {
                visible: true,
                fullscreen: true,
                ..WindowWord::default()
            },
        ] {
            kept.moved((-32000, -32000), word);
            kept.resized(Some((160, 28)), word);
            assert_eq!(kept.position(), Some((40, 30)), "{word:?}");
            assert_eq!(kept.size(), Some((1200, 800)), "{word:?}");
            assert!(!kept.maximized(), "{word:?}");
        }
        // A maximized window that is minimized stays maximized for the settings.
        kept.resized(Some((1920, 1040)), shown(true));
        kept.resized(
            Some((160, 28)),
            WindowWord {
                visible: true,
                minimized: true,
                ..WindowWord::default()
            },
        );
        assert!(kept.maximized());
        // A size the window cannot say (below a pixel) changes nothing.
        kept.resized(None, shown(false));
        assert_eq!(kept.size(), Some((1200, 800)));
        // The place the start puts it at is the one kept until it moves; off every screen,
        // none.
        kept.placed(Placement {
            position: None,
            size: Some((1000, 700)),
            maximized: false,
        });
        assert_eq!(kept.position(), None);
        assert_eq!(kept.size(), Some((1000, 700)));
    }

    #[test]
    fn window_sizes_convert_both_ways() {
        assert_eq!(whole_pixels(1279.6), Some(1280));
        assert_eq!(whole_pixels(0.2), None);
        assert_eq!(whole_pixels(-5.0), None);
        assert_eq!(whole_pixels(1e9), None);
        assert!((logical(900) - 900.0).abs() < f32::EPSILON);
        assert!((logical(u32::MAX) - 65535.0).abs() < f32::EPSILON);
        for theme in [
            ThemeChoice::System,
            ThemeChoice::Light,
            ThemeChoice::Dark,
            ThemeChoice::HighContrast,
        ] {
            assert_eq!(theme_of(theme_number(theme)), Some(theme));
        }
        assert_eq!(theme_of(4), None);
        assert_eq!(theme_of(-1), None);
    }
}
