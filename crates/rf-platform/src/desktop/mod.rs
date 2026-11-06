//! What the app shows of itself outside its window: the system's notifications (PRD XFER-14,
//! "끝나면 OS 알림을 보낸다"), and whether the app can keep working behind an icon of the system's
//! tray while its window is hidden (XFER-15).
//!
//! - **Windows:** a toast (the Windows Runtime's `ToastNotificationManager`, through `notify-rust`). Windows shows
//!   a toast only for an app id that a shortcut in the Start menu names. The installer's
//!   shortcut carries the app's `AppUserModelID` (`identity::AUMID`), and an installed app sends
//!   its toasts under it ([`notification_app_id`]), so they show its name and icon. A portable
//!   app has no shortcut, so its toasts go under the id of Windows PowerShell, which every
//!   Windows has: they show "Windows PowerShell" as their app. The tray icon is the
//!   notification area's (`Shell_NotifyIconW`).
//! - **macOS:** the User Notifications framework (`UNUserNotificationCenter`), which asks the user
//!   once whether the app may notify. A delegate of the app's has each notification shown as a
//!   banner and in the list even while the app is in front, as it still is after its window hid
//!   behind the menu bar icon. Only an app bundle may use it (outside one it raises an
//!   Objective-C exception, which would end the app), so a program run outside a bundle, as by
//!   `cargo run`, gets `io::ErrorKind::Unsupported` and nothing is asked. The tray icon is an icon
//!   of the menu bar (`NSStatusItem`).
//! - **Elsewhere** (Linux, which is for development): the desktop's notification service over
//!   D-Bus (`org.freedesktop.Notifications`, through `notify-rust` and its pure-Rust `zbus`).
//!   There is no [`TRAY`]: a tray icon there needs a desktop that hosts `StatusNotifierItem`s,
//!   and an app cannot tell whether its icon showed, so a hidden window could not be found again.
//!
//! [`notify()`] blocks while the system takes the notification (on macOS also while the user
//! answers the first question, [`ASK_WAIT`] at most): call it from a worker thread, never from
//! the UI thread. A failure only means that nothing showed; callers log it and go on.

use std::io;
use std::time::Duration;

use rf_core::InstallMode;
use rf_core::identity::AUMID;

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(not(target_os = "macos"))]
use self::xdg_or_toast as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod xdg_or_toast;

/// Whether the app offers to keep working behind a tray icon while its window is hidden:
/// Windows (the notification area) and macOS (the menu bar), not Linux (see the module).
pub const TRAY: bool = imp::TRAY;

/// How long [`notify()`] waits on macOS for the user's answer to whether the app may notify, the
/// first time it asks, and then for the system to take the notification. Past it nothing shows.
pub const ASK_WAIT: Duration = Duration::from_secs(60);

/// The app id the notifications go out under for an app installed as `mode` (Windows reads
/// it; the other systems name the app by its bundle or its D-Bus name): the app's
/// `AppUserModelID` for an installed app, whose Start menu shortcut carries it; none for a
/// portable one, which has no shortcut and so no id Windows would show a toast for.
pub fn notification_app_id(mode: InstallMode) -> Option<&'static str> {
    match mode {
        InstallMode::Installed => Some(AUMID),
        InstallMode::Portable => None,
    }
}

/// Shows a notification with `title` and `body`, in the system's own place for them, under
/// `app_id` on Windows ([`notification_app_id`]; the default one when `None`, and ignored
/// elsewhere). Blocks; `Unsupported` where the system offers none to this program (macOS
/// outside an app bundle), `PermissionDenied` when the user said no to them, and any other error
/// the system gave.
pub fn notify(app_id: Option<&str>, title: &str, body: &str) -> io::Result<()> {
    imp::notify(app_id, title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installed_app_notifies_under_its_own_id_and_a_portable_one_under_none() {
        assert_eq!(notification_app_id(InstallMode::Installed), Some(AUMID));
        assert_eq!(notification_app_id(InstallMode::Portable), None);
    }

    #[test]
    fn the_tray_is_offered_on_windows_and_macos_only() {
        assert_eq!(TRAY, cfg!(any(windows, target_os = "macos")));
    }
}
