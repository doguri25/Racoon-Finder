//! Notifications through `notify-rust`: a toast of the Windows Runtime on Windows, the desktop's D-Bus
//! notification service elsewhere (see the module above).

use std::io;

use rf_core::identity::DISPLAY_NAME;

/// The notification area on Windows; none elsewhere.
pub const TRAY: bool = cfg!(windows);

pub fn notify(app_id: Option<&str>, title: &str, body: &str) -> io::Result<()> {
    // On Windows the toast goes under `app_id` when there is one (the Start menu shortcut of an
    // installed app has it), else under the app id `notify-rust` gives by default, Windows
    // PowerShell's; the name is the D-Bus service's app name elsewhere.
    let mut notification = notify_rust::Notification::new();
    notification.appname(DISPLAY_NAME).summary(title).body(body);
    #[cfg(windows)]
    if let Some(id) = app_id {
        notification.app_id(id);
    }
    #[cfg(not(windows))]
    let _ = app_id;
    notification.show().map(drop).map_err(io::Error::other)
}
