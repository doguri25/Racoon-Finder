//! Helpers for turning Windows API errors into `std::io::Error`.
//!
//! `std::io::Error::from(windows::core::Error)` stores the whole HRESULT (for example
//! `0x800700B7`) as the raw OS error, which std cannot classify (`ErrorKind::Uncategorized`).
//! [`to_io`] unwraps the Win32 code first so that `rf_core::Error::kind` can classify it.

/// The Win32 error code inside an `HRESULT_FROM_WIN32` value (`0x8007_xxxx`), or `None` for
/// any other HRESULT (success, `E_FAIL`, COM facility codes).
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
pub(crate) fn win32_code_from_hresult(hr: u32) -> Option<u16> {
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        u16::try_from(hr & 0xFFFF).ok()
    } else {
        None
    }
}

/// Converts a Windows API error to an `io::Error` that std and `rf_core::Error::kind` can
/// classify: a Win32 code becomes the raw OS error, anything else keeps its message.
#[cfg(windows)]
pub(crate) fn to_io(e: &windows::core::Error) -> std::io::Error {
    let hr = u32::from_ne_bytes(e.code().0.to_ne_bytes());
    match win32_code_from_hresult(hr) {
        Some(code) => std::io::Error::from_raw_os_error(i32::from(code)),
        None => std::io::Error::other(e.message()),
    }
}

/// `s` as NUL-terminated UTF-16, ready to hand to a Win32 `PCWSTR` parameter.
#[cfg(windows)]
#[allow(
    dead_code,
    reason = "first used by the Windows modules added in later tasks"
)]
pub(crate) fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    s.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_codes_unwrap_from_hresult() {
        assert_eq!(win32_code_from_hresult(0x8007_00B7), Some(183));
        assert_eq!(win32_code_from_hresult(0x8000_4004), None);
        assert_eq!(win32_code_from_hresult(0), None);
    }
}
