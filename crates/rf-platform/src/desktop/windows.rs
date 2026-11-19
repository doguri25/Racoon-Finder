#![allow(
    unsafe_code,
    reason = "SystemParametersInfoW, which reads the high contrast setting"
)]

//! The system's high contrast setting (설정 ▸ 접근성 ▸ 대비 테마; 고대비 of older Windows).

use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

/// Whether a contrast theme is on; `false` when the setting cannot be read.
pub fn high_contrast() -> bool {
    let mut setting = HIGHCONTRASTW {
        cbSize: u32::try_from(size_of::<HIGHCONTRASTW>()).unwrap_or(u32::MAX),
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: `SPI_GETHIGHCONTRAST` writes one `HIGHCONTRASTW` to `pvparam`, whose size
    // `uiparam` and `cbSize` give; `setting` is one, alive and writable for the whole call, and
    // nothing else refers to it. The scheme name it may leave `lpszDefaultScheme` pointing at is
    // the system's and is never read.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            setting.cbSize,
            Some((&raw mut setting).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && setting.dwFlags.contains(HCF_HIGHCONTRASTON)
}
