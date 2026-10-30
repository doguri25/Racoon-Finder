//! The system's notifications and the tray icon, as the window uses them (XFER-14, XFER-15).
//!
//! The state decides what to tell and when ([`Effect::Notify`](crate::state::Effect::Notify),
//! and the tray icon's view while the window is hidden); this module carries it out behind two
//! traits, so that the code that does can be tested with fakes:
//!
//! - [`Notifier`] shows a [`Notice`]. [`OsNotifier`] hands each one to a worker thread of its
//!   own, which calls the system ([`rf_platform::desktop::notify`]); a notification the system
//!   does not show is only logged, and the app goes on. The app's end waits a little for the
//!   ones on their way ([`Notifier::wait_idle`]), so that the last one is not cut off.
//! - [`TrayHost`] shows the tray icon with a [`TrayView`], or takes it away; [`Tray`] tells it
//!   only of a change, so that a redraw does not rebuild the system's menu each time.
//!   [`SlintTray`] is the window's own: Slint's `SystemTrayIcon` (`ui/tray.slint`).

use std::cell::RefCell;
use std::fmt;
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::JobTray;
use crate::state::TrayCommand;

/// A job that took longer than this is told of by a notification when it ends while the window
/// is not in front, or hidden behind the tray icon.
pub const NOTIFY_AFTER: Duration = Duration::from_secs(10);

/// How long the app's end waits for the notifications still on their way to the system.
pub const NOTICE_WAIT: Duration = Duration::from_secs(2);

/// What a notification says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// What shows notifications: the system ([`OsNotifier`]), or a fake in tests.
pub trait Notifier {
    /// Shows `notice` without waiting for it.
    fn notify(&self, notice: Notice);

    /// Waits up to `timeout` for the notifications on their way; `true` once none is left.
    fn wait_idle(&self, timeout: Duration) -> bool;
}

/// The system's own call: shows a notification with a title and a body, blocking.
type SystemCall = dyn Fn(&str, &str) -> io::Result<()> + Send + Sync;

/// Shows notifications through `send` ([`rf_platform::desktop::notify`] in the app), each on a
/// worker thread of its own. Clones share the count of those on their way.
#[derive(Clone)]
pub struct OsNotifier {
    send: Arc<SystemCall>,
    /// The notifications on their way, and the signal of one that is done.
    pending: Arc<(Mutex<usize>, Condvar)>,
}

impl fmt::Debug for OsNotifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OsNotifier").finish_non_exhaustive()
    }
}

impl OsNotifier {
    pub fn new(send: impl Fn(&str, &str) -> io::Result<()> + Send + Sync + 'static) -> OsNotifier {
        OsNotifier {
            send: Arc::new(send),
            pending: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    /// The system's notifications ([`rf_platform::desktop::notify`]).
    pub fn system() -> OsNotifier {
        OsNotifier::new(rf_platform::desktop::notify)
    }

    /// One more notification on its way (`1`), or one less (`-1`).
    fn count(&self, step: isize) {
        let (count, done) = &*self.pending;
        let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
        *count = count.saturating_add_signed(step);
        if *count == 0 {
            done.notify_all();
        }
    }
}

impl Notifier for OsNotifier {
    fn notify(&self, notice: Notice) {
        self.count(1);
        let worker = self.clone();
        let started = std::thread::Builder::new()
            .name("rf-ui-notice".to_owned())
            .spawn(move || {
                if let Err(error) = (worker.send)(&notice.title, &notice.body) {
                    log_failure(&error);
                }
                worker.count(-1);
            });
        if let Err(error) = started {
            log_failure(&error);
            self.count(-1);
        }
    }

    fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (count, done) = &*self.pending;
        let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
        while *count > 0 {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            count = done
                .wait_timeout(count, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// Notes that a notification did not show. Nothing else is to be done about it: the job panel's
/// 최근 완료 tells the same.
#[expect(
    clippy::print_stderr,
    reason = "the app has no log of its own yet; Slint's own messages go to the same place"
)]
fn log_failure(error: &io::Error) {
    if error.kind() != io::ErrorKind::Unsupported {
        eprintln!("notification not shown: {error}");
    }
}

/// What the tray icon shows while the window is hidden and its jobs go on.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayView {
    /// The tooltip: the app, and how far the jobs have got ([`crate::words::tray_status`]).
    pub status: String,
    /// The text beside the icon in the macOS menu bar: the first job's percent.
    pub label: String,
    /// Every job is paused: the menu offers 모두 계속 instead of 모두 일시정지.
    pub all_paused: bool,
}

/// What shows the tray icon: the window's tray component, or a fake in tests.
pub trait TrayHost {
    /// Shows the icon with `view`, or shows `view` on the icon that shows; `false` when it
    /// could not be made, so that the window does not hide behind no icon.
    fn show(&self, view: &TrayView) -> bool;

    /// Takes the icon away.
    fn hide(&self);
}

/// Keeps the tray icon as the state wants it ([`Tray::follow`]), telling its host only of what
/// changed.
pub struct Tray {
    host: Box<dyn TrayHost>,
    shown: RefCell<Option<TrayView>>,
}

impl fmt::Debug for Tray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tray")
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl Tray {
    pub fn new(host: Box<dyn TrayHost>) -> Tray {
        Tray {
            host,
            shown: RefCell::new(None),
        }
    }

    /// Shows the icon with `view`, or with `None` takes it away; nothing when that is what
    /// shows already. `false` when the icon could not be shown: nothing is noted then, and the
    /// next call tries again.
    pub fn follow(&self, view: Option<TrayView>) -> bool {
        let mut shown = self.shown.borrow_mut();
        if *shown == view {
            return true;
        }
        let done = if let Some(view) = &view {
            self.host.show(view)
        } else {
            self.host.hide();
            true
        };
        if done {
            *shown = view;
        }
        done
    }

    /// Whether the icon shows.
    pub fn showing(&self) -> bool {
        self.shown.borrow().is_some()
    }
}

/// The tray icon of the window's own component ([`JobTray`]): made as the window hides, and
/// dropped, which takes the icon away, as it shows again. Its menu's choices go to `on_command`,
/// later in the event loop: never while the menu is being handled, during which the icon must
/// stay.
pub struct SlintTray {
    tray: RefCell<Option<JobTray>>,
    on_command: Rc<dyn Fn(TrayCommand)>,
}

impl fmt::Debug for SlintTray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlintTray").finish_non_exhaustive()
    }
}

impl SlintTray {
    pub fn new(on_command: impl Fn(TrayCommand) + 'static) -> SlintTray {
        SlintTray {
            tray: RefCell::new(None),
            on_command: Rc::new(on_command),
        }
    }

    /// The component, made with its picture and its menu's callbacks.
    fn make(&self) -> Result<JobTray, slint::PlatformError> {
        let tray = JobTray::new()?;
        tray.set_picture(tray_icon());
        let later = |command: TrayCommand| {
            let on_command = Rc::clone(&self.on_command);
            move || {
                let on_command = Rc::clone(&on_command);
                slint::Timer::single_shot(Duration::ZERO, move || on_command(command));
            }
        };
        tray.on_open_window(later(TrayCommand::Open));
        tray.on_pause_all(later(TrayCommand::PauseAll));
        tray.on_resume_all(later(TrayCommand::ResumeAll));
        tray.on_quit(later(TrayCommand::Quit));
        Ok(tray)
    }
}

impl TrayHost for SlintTray {
    fn show(&self, view: &TrayView) -> bool {
        let mut tray = self.tray.borrow_mut();
        if tray.is_none() {
            match self.make() {
                Ok(made) => *tray = Some(made),
                Err(error) => {
                    log_failure(&io::Error::other(error.to_string()));
                    return false;
                }
            }
        }
        let Some(shown) = tray.as_ref() else {
            return false;
        };
        shown.set_status(view.status.as_str().into());
        shown.set_label(view.label.as_str().into());
        shown.set_all_paused(view.all_paused);
        if let Err(error) = shown.show() {
            log_failure(&io::Error::other(error.to_string()));
            // Made again at the next try.
            *tray = None;
            return false;
        }
        true
    }

    fn hide(&self) {
        // Dropping it takes the icon away.
        if let Some(tray) = self.tray.borrow_mut().take() {
            let _ = tray.hide();
        }
    }
}

/// The picture of the tray icon: the app's logo on a square of the accent color, 64 pixels a
/// side, which the system scales to its tray's size. An empty image, which shows no icon, if
/// it could not be read.
pub fn tray_icon() -> Image {
    image::load_from_memory_with_format(TRAY_ICON, image::ImageFormat::Png)
        .map(|picture| {
            let rgba = picture.to_rgba8();
            let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
            );
            Image::from_rgba8(buffer)
        })
        .unwrap_or_default()
}

/// The PNG of [`tray_icon`].
const TRAY_ICON: &[u8] = include_bytes!("../ui/tray-icon.png");

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;

    use super::*;

    fn notice(title: &str) -> Notice {
        Notice {
            title: title.to_owned(),
            body: "assets → 백업".to_owned(),
        }
    }

    #[test]
    fn notifications_go_to_the_system_off_the_calling_thread() {
        let (shown, seen) = mpsc::channel();
        let shown = Mutex::new(shown);
        let caller = std::thread::current().id();
        let notifier = OsNotifier::new(move |title, body| {
            let on_caller = std::thread::current().id() == caller;
            let _ = shown.lock().unwrap_or_else(PoisonError::into_inner).send((
                title.to_owned(),
                body.to_owned(),
                on_caller,
            ));
            Ok(())
        });
        notifier.notify(notice("복사를 마쳤습니다 · 항목 3개"));
        assert!(notifier.wait_idle(Duration::from_secs(10)));
        let (title, body, on_caller) = seen.recv().expect("sent");
        assert_eq!(title, "복사를 마쳤습니다 · 항목 3개");
        assert_eq!(body, "assets → 백업");
        assert!(!on_caller);
    }

    #[test]
    fn a_notification_the_system_refuses_is_only_logged() {
        let notifier = OsNotifier::new(|_, _| Err(io::Error::other("no notification service")));
        notifier.notify(notice("이동을 마쳤습니다 · 항목 1개"));
        notifier.notify(notice("이동을 마쳤습니다 · 항목 2개"));
        // Nothing waits for an answer, and nothing is left on its way.
        assert!(notifier.wait_idle(Duration::from_secs(10)));
    }

    #[test]
    fn the_end_waits_a_little_for_a_notification_on_its_way() {
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let notifier = OsNotifier::new(move |_, _| {
            let _ = released
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recv_timeout(Duration::from_secs(10));
            Ok(())
        });
        notifier.notify(notice("복사를 마쳤습니다 · 항목 3개"));
        // The system takes its time: the wait gives up.
        assert!(!notifier.wait_idle(Duration::from_millis(50)));
        let _ = release.send(());
        assert!(notifier.wait_idle(Duration::from_secs(10)));
        // Nothing on its way: no wait at all.
        assert!(notifier.wait_idle(Duration::ZERO));
    }

    /// Records what the tray was told; its icon can be made while `.1` says so.
    struct Recorder(Rc<RefCell<Vec<String>>>, Rc<Cell<bool>>);

    impl TrayHost for Recorder {
        fn show(&self, view: &TrayView) -> bool {
            let made = self.1.get();
            let call = if made { "show" } else { "failed" };
            self.0.borrow_mut().push(format!("{call} {}", view.label));
            made
        }

        fn hide(&self) {
            self.0.borrow_mut().push("hide".to_owned());
        }
    }

    #[test]
    fn the_tray_icon_has_its_picture() {
        let icon = tray_icon();
        assert_eq!((icon.size().width, icon.size().height), (64, 64));
        assert!(icon.to_rgba8().is_some());
    }

    #[test]
    fn the_tray_host_hears_only_of_changes() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let tray = Tray::new(Box::new(Recorder(
            Rc::clone(&calls),
            Rc::new(Cell::new(true)),
        )));
        let view = |label: &str| TrayView {
            status: "앱\n복사 중 25%".to_owned(),
            label: label.to_owned(),
            all_paused: false,
        };
        tray.follow(None);
        assert!(!tray.showing());
        tray.follow(Some(view("25%")));
        tray.follow(Some(view("25%")));
        tray.follow(Some(view("26%")));
        assert!(tray.showing());
        tray.follow(None);
        tray.follow(None);
        assert_eq!(calls.take(), ["show 25%", "show 26%", "hide"]);
    }

    #[test]
    fn an_icon_that_could_not_be_made_is_tried_again() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let made = Rc::new(Cell::new(false));
        let tray = Tray::new(Box::new(Recorder(Rc::clone(&calls), Rc::clone(&made))));
        let view = TrayView {
            status: "앱\n복사 중 25%".to_owned(),
            label: "25%".to_owned(),
            all_paused: false,
        };
        // Not made: the caller hears it, nothing is noted, and the next call tries again.
        assert!(!tray.follow(Some(view.clone())));
        assert!(!tray.showing());
        made.set(true);
        assert!(tray.follow(Some(view.clone())));
        assert!(tray.showing());
        // What shows is not told again.
        assert!(tray.follow(Some(view)));
        assert_eq!(calls.take(), ["failed 25%", "show 25%"]);
    }
}
