//! Headless checks of the Slint screens (no window system needed).

use rf_ui::{MainWindow, Theme};
use slint::{Color, ComponentHandle};

#[test]
fn main_window_builds_with_theme() {
    i_slint_backend_testing::init_no_event_loop();
    let window = MainWindow::new().expect("MainWindow builds");
    let theme = window.global::<Theme<'_>>();

    assert!(!theme.get_dark());
    assert_eq!(theme.get_bg(), Color::from_rgb_u8(0xF6, 0xF7, 0xF9));
    assert_eq!(theme.get_accent(), Color::from_rgb_u8(0x2F, 0x6F, 0xEB));
    assert_eq!(theme.get_selection(), Color::from_rgb_u8(0xE6, 0xEE, 0xFD));

    theme.set_dark(true);
    assert_eq!(theme.get_bg(), Color::from_rgb_u8(0x15, 0x17, 0x1A));
    assert_eq!(theme.get_accent(), Color::from_rgb_u8(0x78, 0xA1, 0xF2));
    assert_eq!(theme.get_selection(), Color::from_rgb_u8(0x31, 0x3C, 0x51));
}
