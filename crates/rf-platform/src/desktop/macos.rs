//! Notifications through the User Notifications framework (see the module above).

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use block2::RcBlock;
use objc2::rc::autoreleasepool;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};

use super::ASK_WAIT;

/// An icon of the menu bar.
pub const TRAY: bool = true;

/// The number of the next notification, for its identifier: the system replaces a
/// notification that has the identifier of one it shows.
static NEXT: AtomicU64 = AtomicU64::new(1);

pub fn notify(title: &str, body: &str) -> io::Result<()> {
    autoreleasepool(|_| {
        if !in_app_bundle() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "notifications need an app bundle",
            ));
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        // The first time, the system asks the user; later it answers at once.
        let (sender, answer) = mpsc::channel();
        let granted = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _ = sender.send(granted.as_bool());
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &granted,
        );
        match answer.recv_timeout(ASK_WAIT) {
            Ok(true) => {}
            Ok(false) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the user does not allow notifications",
                ));
            }
            Err(_) => return Err(io::Error::from(io::ErrorKind::TimedOut)),
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        let id = format!(
            "job-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&id),
            &content,
            None,
        );
        let (sender, added) = mpsc::channel();
        let done = RcBlock::new(move |error: *mut NSError| {
            let _ = sender.send(error.is_null());
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&done));
        match added.recv_timeout(ASK_WAIT) {
            Ok(true) => Ok(()),
            Ok(false) => Err(io::Error::other("the system did not take the notification")),
            Err(_) => Err(io::Error::from(io::ErrorKind::TimedOut)),
        }
    })
}

/// Whether the app runs from an app bundle, which alone may use the notification center: a
/// main bundle with an identifier, whose folder is an `.app`.
fn in_app_bundle() -> bool {
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier().is_some()
        && std::path::Path::new(&bundle.bundlePath().to_string())
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_an_app_bundle_nothing_is_asked() {
        // A test runs from a plain program, which is no app bundle: the framework is not
        // touched (it would raise), and the call says why nothing showed.
        assert!(!in_app_bundle());
        let error = notify("제목", "본문").expect_err("no bundle");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}
