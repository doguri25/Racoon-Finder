//! The tray icon for real (XFER-15). It needs a desktop session with a tray (the notification
//! area on Windows, the menu bar on macOS), which CI's runners may not have: run it by hand with
//! `cargo test -p rf-ui --test tray -- --ignored` and look at the tray while it runs.

use std::time::Duration;

use rf_ui::JobTray;
use rf_ui::notify::tray_icon;

#[test]
#[ignore = "needs an interactive session"]
fn the_tray_icon_shows_its_status_and_menu() {
    let tray = JobTray::new().expect("the tray builds");
    tray.set_picture(tray_icon());
    tray.set_status("복사 중 25%\n3 / 12 항목 · 48 MB/s".into());
    tray.set_label("25%".into());
    tray.on_pause_all(|| println!("모두 일시정지"));
    tray.on_resume_all(|| println!("모두 계속"));
    tray.on_open_window(|| println!("창 열기"));
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    tray.show().expect("the tray shows");
    // Look for the logo in the tray, its tooltip, and its menu (끝내기 ends the test early).
    slint::Timer::single_shot(Duration::from_secs(20), || {
        let _ = slint::quit_event_loop();
    });
    slint::run_event_loop_until_quit().expect("the event loop runs");
}
