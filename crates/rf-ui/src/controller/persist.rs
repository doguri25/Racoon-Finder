//! How the [`Controller`] keeps the [`Settings`] while the app runs: the look at the folders of
//! the last run's tabs at the start, the theme of the 보기 menu, the window's size, and the
//! saves after a change and as the app ends ([`SaveLedger`] decides when).

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rf_core::VPath;
use slint::{ComponentHandle, TimerMode};

use super::{Controller, with};
use crate::settings::{Found, Settings, ThemeChoice};
use crate::ui_bridge::post;
use crate::words;

/// How long after a change of what the settings keep they are written: the changes in that
/// time are written together.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

/// How long the start waits to know whether the folders of the last run's tabs are still there.
/// A tab whose folder is not known by then (a slow or sleeping drive) shows home meanwhile.
pub const RESTORE_WAIT: Duration = Duration::from_millis(300);

/// How many folders of the last run's tabs are looked at together at the start: a file with
/// hundreds of tabs does not start hundreds of threads.
pub const RESTORE_WORKERS: usize = 4;

/// How long the app's end waits for the last save of the settings before it goes on: a disk
/// that hangs (a network home that stopped answering) does not keep the window from closing.
pub const FINAL_SAVE_WAIT: Duration = Duration::from_secs(2);

/// Looks at `folders` with `look` on at most `workers` worker threads, which take the folders
/// in turn; the answers come on the receiver as they are found. A folder on a drive that hangs
/// holds its worker, not the others. When no worker can start, every folder counts as there:
/// its tab's listing tells.
pub(super) fn look_for(
    folders: Vec<VPath>,
    workers: usize,
    look: impl Fn(&VPath) -> Found + Send + Sync + 'static,
) -> Receiver<(VPath, Found)> {
    let (sent, answers) = mpsc::channel();
    let count = workers.min(folders.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(folders)));
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
                    let Some(folder) = next else {
                        break;
                    };
                    let found = look(&folder);
                    if sent.send((folder, found)).is_err() {
                        break;
                    }
                }
            });
        started += usize::from(worker.is_ok());
    }
    if started == 0 {
        let left = std::mem::take(&mut *queue.lock().unwrap_or_else(PoisonError::into_inner));
        for folder in left {
            let _ = sent.send((folder, Found::Folder));
        }
    }
    answers
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

/// The number the 보기 menu uses for `theme` (`theme-choice` of the window).
pub(super) fn theme_number(theme: ThemeChoice) -> i32 {
    match theme {
        ThemeChoice::System => 0,
        ThemeChoice::Light => 1,
        ThemeChoice::Dark => 2,
    }
}

/// The theme of the 보기 menu's number `choice`.
pub(super) fn theme_of(choice: i32) -> Option<ThemeChoice> {
    match choice {
        0 => Some(ThemeChoice::System),
        1 => Some(ThemeChoice::Light),
        2 => Some(ThemeChoice::Dark),
        _ => None,
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

    /// The last write [`Controller::final_settings`] gave could not be handed on: it is to be
    /// taken again.
    pub(super) fn reopen_settings(&self) {
        self.ledger.borrow_mut().reopen();
    }

    /// The settings with what the window shows now, and their text: the tabs, the active pane,
    /// the hidden items, the recent targets, the theme and the window's size (kept as it was
    /// while the window is maximized, full screen or minimized). `None` before the panes show.
    fn snapshot(&self) -> Option<(Settings, String)> {
        let mut settings = self.settings.borrow().clone();
        {
            let state = self.state.borrow();
            let state = state.as_ref()?;
            settings.set_session(&state.session());
            settings.set_recent(state.recent_targets());
        }
        settings.theme = self.theme.get();
        if let Some(size) = self.window_size() {
            settings.window = Some(size);
        }
        let text = settings.to_text().ok()?;
        Some((settings, text))
    }

    /// The window's size in logical pixels, while it has one of its own.
    fn window_size(&self) -> Option<(u32, u32)> {
        let window = self.window.upgrade()?;
        let window = window.window();
        if window.is_maximized() || window.is_fullscreen() || window.is_minimized() {
            return None;
        }
        let size = window.size().to_logical(window.scale_factor());
        Some((whole_pixels(size.width)?, whole_pixels(size.height)?))
    }

    /// After a change: a save [`SAVE_DELAY`] from now, when the settings differ from those
    /// last written and none is planned yet.
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

    /// The 보기 menu chose `theme`: the window shows it at once (its colors follow
    /// `theme-choice`).
    pub(super) fn choose_theme(&self, theme: ThemeChoice) {
        self.theme.set(theme);
        if let Some(window) = self.window.upgrade() {
            window.set_theme_choice(theme_number(theme));
        }
        self.plan_save();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
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

    #[test]
    fn window_sizes_convert_both_ways() {
        assert_eq!(whole_pixels(1279.6), Some(1280));
        assert_eq!(whole_pixels(0.2), None);
        assert_eq!(whole_pixels(-5.0), None);
        assert_eq!(whole_pixels(1e9), None);
        assert!((logical(900) - 900.0).abs() < f32::EPSILON);
        assert!((logical(u32::MAX) - 65535.0).abs() < f32::EPSILON);
        for theme in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            assert_eq!(theme_of(theme_number(theme)), Some(theme));
        }
        assert_eq!(theme_of(3), None);
    }
}
