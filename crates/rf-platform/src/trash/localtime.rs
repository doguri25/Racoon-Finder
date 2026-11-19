#![allow(unsafe_code, reason = "localtime_r and mktime FFI")]

//! The system's own conversions between UTC and its local time, which a freedesktop
//! `.trashinfo` record keeps its `DeletionDate` in. Both functions read the time zone as the C
//! library does (`TZ`, else `/etc/localtime`).

use super::parse::Civil;

/// The seconds since 1970-01-01T00:00:00 UTC of `civil`, a local time, by `mktime`, which also
/// decides whether daylight saving time was in force then. `None` when the system cannot place
/// it (`mktime` fails; the one second before 1970 that `-1` also names is lost with it).
pub(super) fn local_to_utc(civil: Civil) -> Option<i64> {
    // SAFETY: `tm` holds only integers and, on some systems, a pointer to a zone name, for which
    // all zero bytes (a null pointer) are a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = civil.year.checked_sub(1900)?;
    tm.tm_mon = civil.month - 1;
    tm.tm_mday = civil.day;
    tm.tm_hour = civil.hour;
    tm.tm_min = civil.minute;
    tm.tm_sec = civil.second;
    // Unknown: `mktime` looks it up.
    tm.tm_isdst = -1;
    // SAFETY: `tm` is a valid, writable `tm` that lives through the call.
    let seconds = unsafe { libc::mktime(&raw mut tm) };
    (seconds != -1).then_some(seconds)
}

/// `seconds` since 1970-01-01T00:00:00 UTC as the local wall-clock time, by `localtime_r`.
/// `None` when the system cannot convert it.
pub(super) fn utc_to_local(seconds: i64) -> Option<Civil> {
    let time: libc::time_t = seconds;
    // SAFETY: as in `local_to_utc`, all zero bytes are a valid `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `time` and `tm` are valid and live through the call; `localtime_r` writes only to
    // `tm` and keeps no pointer to either.
    let filled = unsafe { libc::localtime_r(&raw const time, &raw mut tm) };
    if filled.is_null() {
        return None;
    }
    Some(Civil {
        year: tm.tm_year.checked_add(1900)?,
        month: tm.tm_mon + 1,
        day: tm.tm_mday,
        hour: tm.tm_hour,
        minute: tm.tm_min,
        // A leap second shows as :60; the record has no room for it.
        second: tm.tm_sec.min(59),
    })
}
