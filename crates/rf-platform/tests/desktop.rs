//! The system's notifications for real. They need a desktop session (and on macOS an app
//! bundle), which CI's runners may not have: run them by hand with
//! `cargo test -p rf-platform --test desktop -- --ignored` and look for the notification.

use rf_platform::desktop;

#[test]
#[ignore = "needs an interactive session"]
fn a_notification_shows() {
    let shown = desktop::notify("복사를 마쳤습니다 · 항목 3개", "assets → 백업");
    if cfg!(target_os = "macos") {
        // A test is no app bundle: the demo's .app shows one, the test only checks that
        // nothing breaks.
        assert_eq!(
            shown.expect_err("no bundle").kind(),
            std::io::ErrorKind::Unsupported
        );
    } else {
        shown.expect("the notification shows");
    }
}
