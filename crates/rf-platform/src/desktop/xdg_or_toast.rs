//! Notifications through `notify-rust`: a toast of the Windows Runtime on Windows, the desktop's D-Bus
//! notification service elsewhere (see the module above).

use std::io;

use rf_core::identity::DISPLAY_NAME;

/// The notification area on Windows; none elsewhere.
pub const TRAY: bool = cfg!(windows);

pub fn notify(title: &str, body: &str) -> io::Result<()> {
    // On Windows the toast goes under the app id `notify-rust` gives by default, Windows
    // PowerShell's; the name is the D-Bus service's app name elsewhere.
    notify_rust::Notification::new()
        .appname(DISPLAY_NAME)
        .summary(title)
        .body(body)
        .show()
        .map(drop)
        .map_err(io::Error::other)
}
