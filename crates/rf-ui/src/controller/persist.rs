//! How the [`Controller`] keeps the [`Settings`] while the app runs: the look at the folders of
//! the last run's tabs at the start, the theme of the 보기 menu, the window's size, and the
//! saves after a change and as the app ends.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use rf_core::VPath;
use slint::{ComponentHandle, TimerMode};

use super::{Controller, with};
use crate::loader::Loader;
use crate::settings::{Settings, ThemeChoice};
use crate::ui_bridge::post;
use crate::words;

/// How long after a change of what the settings keep they are written: the changes in that
/// time are written together.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

/// How long the start waits to know whether the folders of the last run's tabs are still there.
/// A tab whose folder is not known by then (a slow or sleeping drive) shows home meanwhile.
pub const RESTORE_WAIT: Duration = Duration::from_millis(300);

/// Looks on worker threads, one for each of `folders`, whether they are still there; the
/// answers come on the receiver as they are found. A folder whose thread cannot start counts as
/// there: its tab's listing tells.
pub(super) fn look_for(loader: &Loader, folders: Vec<VPath>) -> Receiver<(VPath, bool)> {
    let (sent, checks) = mpsc::channel();
    for folder in folders {
        let (loader, answer, asked) = (loader.clone(), sent.clone(), folder.clone());
        let started = std::thread::Builder::new()
            .name("rf-ui-restore".to_owned())
            .spawn(move || {
                let exists = loader.is_folder(&asked);
                let _ = answer.send((asked, exists));
            });
        if started.is_err() {
            let _ = sent.send((folder, true));
        }
    }
    checks
}

/// The answers of `checks` that come within `wait`, until there are `count`.
pub(super) fn wait_for(
    checks: &Receiver<(VPath, bool)>,
    count: usize,
    wait: Duration,
) -> HashMap<VPath, bool> {
    let deadline = Instant::now() + wait;
    let mut found = HashMap::new();
    while found.len() < count {
        let left = deadline.saturating_duration_since(Instant::now());
        match checks.recv_timeout(left) {
            Ok((path, exists)) => {
                found.insert(path, exists);
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
        reason = "the cast saturates, and its value is kept only for a whole number from 1 to u16::MAX"
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

impl Controller {
    /// The settings to write as the app ends: what the window shows now. No save is planned
    /// after this. `None` before the window shows the panes, or once taken.
    pub fn final_settings(&self) -> Option<Settings> {
        if self.settings_closed.replace(true) {
            return None;
        }
        self.save_timer.stop();
        let now = self.snapshot()?;
        now.clone_into(&mut self.settings.borrow_mut());
        Some(now)
    }

    /// The settings with what the window shows now: the tabs, the active pane, the hidden
    /// items, the recent targets, the theme and the window's size (kept as it was while the
    /// window is maximized, full screen or minimized). `None` before the panes show.
    fn snapshot(&self) -> Option<Settings> {
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
        Some(settings)
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

    /// After a change: unless one is planned already, a save [`SAVE_DELAY`] from now, when
    /// what the window shows differs from the settings last written.
    pub(super) fn plan_save(&self) {
        if self.settings_closed.get() || self.save_timer.running() {
            return;
        }
        if self
            .snapshot()
            .is_none_or(|now| now == *self.settings.borrow())
        {
            return;
        }
        self.save_timer
            .start(TimerMode::SingleShot, SAVE_DELAY, || {
                with(Controller::save_due);
            });
    }

    /// The planned save: what the window shows now, on a worker thread.
    fn save_due(&self) {
        if self.settings_closed.get() {
            return;
        }
        let Some(now) = self.snapshot() else {
            return;
        };
        if now == *self.settings.borrow() {
            return;
        }
        now.clone_into(&mut self.settings.borrow_mut());
        let started = self.saver.save_in_background(now, |result| {
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
    use super::*;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    #[test]
    fn the_start_waits_only_a_moment_for_the_folders() {
        let (sent, checks) = mpsc::channel();
        sent.send((folder("fast"), true)).expect("sent");
        // "slow" never answers in time: its sender stays.
        let started = Instant::now();
        let found = wait_for(&checks, 2, Duration::from_millis(100));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(found, HashMap::from([(folder("fast"), true)]));

        // With every answer in, it does not wait.
        sent.send((folder("slow"), false)).expect("sent");
        let started = Instant::now();
        let found = wait_for(&checks, 1, Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(found, HashMap::from([(folder("slow"), false)]));
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
