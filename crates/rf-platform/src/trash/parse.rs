//! The records trashes keep about their items, read without touching any file: the Windows
//! recycle bin's `$I…` files and the freedesktop `.trashinfo` files. Both readers take any input
//! without panicking and give `None` for anything malformed; they are tested on every OS.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The length of the header every `$I…` record starts with: the version, the size of the item
/// and the time it was deleted, 8 bytes each, little-endian.
const RECYCLE_HEADER: usize = 24;

/// The length of a version 1 record (Windows Vista to 8.1): the header and a path of a fixed
/// 260 UTF-16 units, padded with NULs.
const RECYCLE_V1_LEN: usize = RECYCLE_HEADER + 520;

/// Where the path of a version 2 record (Windows 10 and later) starts: after the header and the
/// 4-byte number of UTF-16 units the path takes, its terminating NUL included.
const RECYCLE_V2_PATH: usize = RECYCLE_HEADER + 4;

/// The most UTF-16 units a version 2 path may take, its NUL included: the 32,767-unit limit of
/// an extended-length path and one more.
const RECYCLE_V2_MAX_UNITS: usize = 32_768;

/// The number of 100 ns steps of a `FILETIME` from 1601-01-01 to 1970-01-01.
const FILETIME_AT_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// The number of 100 ns steps of a `FILETIME` in one second.
const FILETIME_PER_SECOND: u64 = 10_000_000;

/// Reads a Windows recycle bin record, the `$I…` file next to each `$R…` item: the item's
/// original path, when it was deleted, and its size in bytes (a folder's whole size).
///
/// Both layouts are read. Each starts with an 8-byte version, an 8-byte size and an 8-byte
/// `FILETIME`, all little-endian. Version 1 then holds the path in a fixed 520 bytes (260 UTF-16
/// units, ended and padded with NULs), so the record is exactly 544 bytes long. Version 2 holds a
/// 4-byte number of UTF-16 units and then that many units, the path's NUL included, and nothing
/// after them.
///
/// `None` for anything else: another version, another length, a length field that does not
/// match the bytes there, a path with no NUL, an empty path, or one that is not an absolute
/// Windows path (`X:\…` or `\\…`). Elsewhere than on Windows the path must also be valid UTF-16.
/// A time of 0 or before 1970 is no time (`None`).
pub fn parse_recycle_index(bytes: &[u8]) -> Option<(PathBuf, Option<SystemTime>, u64)> {
    let word = |at: usize| -> Option<u64> {
        let field: [u8; 8] = bytes.get(at..at + 8)?.try_into().ok()?;
        Some(u64::from_le_bytes(field))
    };
    let (version, size, filetime) = (word(0)?, word(8)?, word(16)?);
    let path_bytes = match version {
        1 if bytes.len() == RECYCLE_V1_LEN => &bytes[RECYCLE_HEADER..],
        2 => {
            let count: [u8; 4] = bytes
                .get(RECYCLE_HEADER..RECYCLE_V2_PATH)?
                .try_into()
                .ok()?;
            let units = usize::try_from(u32::from_le_bytes(count)).ok()?;
            if units == 0 || units > RECYCLE_V2_MAX_UNITS {
                return None;
            }
            let end = RECYCLE_V2_PATH.checked_add(units.checked_mul(2)?)?;
            if bytes.len() != end {
                return None;
            }
            &bytes[RECYCLE_V2_PATH..end]
        }
        _ => return None,
    };
    let units: Vec<u16> = path_bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    let end = units.iter().position(|&unit| unit == 0)?;
    let path = &units[..end];
    if !is_absolute_windows_path(path) {
        return None;
    }
    Some((path_from_utf16(path)?, filetime_to_system(filetime), size))
}

/// Whether the UTF-16 `path` starts as an absolute Windows path does: a drive (`X:\`) or two
/// backslashes (`\\server\share`, `\\?\…`).
fn is_absolute_windows_path(path: &[u16]) -> bool {
    let is = |unit: u16, c: u8| unit == u16::from(c);
    match path {
        [letter, colon, slash, ..] if is(*colon, b':') && is(*slash, b'\\') => {
            u8::try_from(*letter).is_ok_and(|letter| letter.is_ascii_alphabetic())
        }
        [first, second, _, ..] => is(*first, b'\\') && is(*second, b'\\'),
        _ => false,
    }
}

/// The UTF-16 `units` as a path: as they are on Windows, and only when they are valid UTF-16
/// elsewhere.
#[cfg_attr(
    windows,
    allow(
        clippy::unnecessary_wraps,
        reason = "elsewhere units can fail to be a path"
    )
)]
fn path_from_utf16(units: &[u16]) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_wide(units)))
    }
    #[cfg(not(windows))]
    {
        String::from_utf16(units).ok().map(PathBuf::from)
    }
}

/// A `FILETIME` (100 ns steps since 1601) as a time; `None` for 0 and for any time before 1970,
/// which no record of a deletion has and a job record cannot store.
pub(crate) fn filetime_to_system(filetime: u64) -> Option<SystemTime> {
    let since_epoch = filetime.checked_sub(FILETIME_AT_UNIX_EPOCH)?;
    let seconds = since_epoch / FILETIME_PER_SECOND;
    let nanos = u32::try_from((since_epoch % FILETIME_PER_SECOND) * 100).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

/// A date and time on a wall clock, in no time zone: what `DeletionDate=` holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Civil {
    pub year: i32,
    /// 1 to 12.
    pub month: i32,
    /// 1 to the length of the month.
    pub day: i32,
    /// 0 to 23.
    pub hour: i32,
    /// 0 to 59.
    pub minute: i32,
    /// 0 to 59.
    pub second: i32,
}

/// Reads a freedesktop `.trashinfo` record: the original path of the item and when it was
/// deleted.
///
/// The first line that is neither blank nor a comment (`#`) must be the group header
/// `[Trash Info]`. In that group, `Path=` is the original path with its bytes escaped as in URLs
/// (`%20` for a space; any `%XX` is read, of either case), and `DeletionDate=` the local time of
/// the deletion as `YYYY-MM-DDThh:mm:ss`. Keys of other groups and other keys are passed over;
/// spaces around `=` and around a line are allowed.
///
/// A relative path is given as it is, relative: the specification makes it relative to the
/// folder the trash is in (the top of its volume for a `.Trash-<uid>` folder, `$XDG_DATA_HOME`
/// for the home trash), which the caller knows.
///
/// `None` for a malformed record: no header first, a second `[Trash Info]` group, a line in it
/// that is no comment and has no `=`, a key given twice, no `Path`, an empty path, an escape that
/// is not `%` and two hex digits, a NUL byte, or a `..` among the names of the path. Elsewhere
/// than on Unix the path must also be valid UTF-8. A `DeletionDate` that is missing or not a real
/// date in that form only makes the time unknown (`None`), and so does a time before 1970: the
/// item can still be put back.
///
/// The local time is read with the system's time zone on Unix (`mktime`), and as UTC elsewhere,
/// where no freedesktop trash is kept.
pub fn parse_trashinfo(text: &str) -> Option<(PathBuf, Option<SystemTime>)> {
    let mut seen_header = false;
    let mut in_group = false;
    let (mut path, mut date) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            match (seen_header, line == "[Trash Info]") {
                (false, true) => (seen_header, in_group) = (true, true),
                (true, true) | (false, false) => return None,
                (true, false) => in_group = false,
            }
            continue;
        }
        if !seen_header {
            return None;
        }
        let Some((key, value)) = line.split_once('=') else {
            if in_group {
                return None;
            }
            continue;
        };
        if !in_group {
            continue;
        }
        let slot = match key.trim() {
            "Path" => &mut path,
            "DeletionDate" => &mut date,
            _ => continue,
        };
        if slot.replace(value.trim()).is_some() {
            return None;
        }
    }
    let path = decode_path(path?)?;
    let deleted = date.and_then(parse_deletion_date).and_then(local_to_system);
    Some((path, deleted))
}

/// The path a `Path=` value names, with every `%XX` read back as its byte; see
/// [`parse_trashinfo`] for what is refused.
fn decode_path(value: &str) -> Option<PathBuf> {
    let bytes = unescape(value)?;
    if bytes.is_empty() || bytes.contains(&0) {
        return None;
    }
    let path = path_from_bytes(bytes)?;
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return None;
    }
    Some(path)
}

/// `text` with each `%XX` turned into its byte; `None` for a `%` without two hex digits after it.
fn unescape(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let [high, low, tail @ ..] = tail else {
                return None;
            };
            let digit = |c: u8| {
                char::from(c)
                    .to_digit(16)
                    .and_then(|d| u8::try_from(d).ok())
            };
            bytes.push(digit(*high)? << 4 | digit(*low)?);
            rest = tail;
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    Some(bytes)
}

/// The raw bytes of a path as a path: as they are on Unix, and only when they are valid UTF-8
/// elsewhere.
#[cfg_attr(
    unix,
    allow(
        clippy::unnecessary_wraps,
        reason = "elsewhere bytes can fail to be a path"
    )
)]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// The bytes that a `Path=` value writes as they are: the unreserved characters of RFC 2396
/// (letters, digits and `-_.!~*'()`) and the separator `/`. Every other byte is `%XX`.
fn kept_in_path(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_.!~*'()/".contains(&byte)
}

/// The `Path=` value of `path`: its bytes, each one [`kept_in_path`] as it is and every other one
/// as `%XX` with uppercase hex digits.
#[cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "used by the freedesktop trash; tested on every OS"
    )
)]
pub(crate) fn escape_path(path: &Path) -> String {
    use std::fmt::Write;

    let mut text = String::new();
    for &byte in path.as_os_str().as_encoded_bytes() {
        if kept_in_path(byte) {
            text.push(char::from(byte));
        } else {
            // Writing to a `String` cannot fail.
            let _ = write!(text, "%{byte:02X}");
        }
    }
    text
}

/// Reads `YYYY-MM-DDThh:mm:ss`, exactly that, into its fields; `None` for anything else or for a
/// date that does not exist (February 30th, hour 24).
pub(crate) fn parse_deletion_date(text: &str) -> Option<Civil> {
    let bytes = text.as_bytes();
    if bytes.len() != 19
        || [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')]
            .iter()
            .any(|&(at, sep)| bytes[at] != sep)
    {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i32> {
        let digits = &bytes[from..to];
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse().ok()
    };
    let civil = Civil {
        year: number(0, 4)?,
        month: number(5, 7)?,
        day: number(8, 10)?,
        hour: number(11, 13)?,
        minute: number(14, 16)?,
        second: number(17, 19)?,
    };
    let valid = (1..=12).contains(&civil.month)
        && (1..=days_in_month(civil.year, civil.month)).contains(&civil.day)
        && (0..24).contains(&civil.hour)
        && (0..60).contains(&civil.minute)
        && (0..60).contains(&civil.second);
    valid.then_some(civil)
}

/// `civil` as `YYYY-MM-DDThh:mm:ss`, the form `DeletionDate=` takes.
#[cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "used by the freedesktop trash; tested on every OS"
    )
)]
pub(crate) fn format_deletion_date(civil: Civil) -> String {
    let Civil {
        year,
        month,
        day,
        hour,
        minute,
        second,
    } = civil;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
}

/// The number of days of `month` (1 to 12) in `year` of the Gregorian calendar.
fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

/// The number of days from 1970-01-01 to the date `year`-`month`-`day` of the proleptic
/// Gregorian calendar (negative before it), by Howard Hinnant's `days_from_civil`.
#[cfg_attr(
    unix,
    allow(
        dead_code,
        reason = "used where the system has no local time; tested on every OS"
    )
)]
fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The seconds from 1970-01-01T00:00:00 UTC to `civil` read as a UTC time.
#[cfg_attr(
    unix,
    allow(
        dead_code,
        reason = "used where the system has no local time; tested on every OS"
    )
)]
pub(crate) fn utc_seconds(civil: Civil) -> i64 {
    days_from_civil(civil.year, civil.month, civil.day) * 86_400
        + i64::from(civil.hour) * 3_600
        + i64::from(civil.minute) * 60
        + i64::from(civil.second)
}

/// `civil`, a local time, as a time: by the system's time zone on Unix, as UTC elsewhere. `None`
/// for a time the system cannot place or one before 1970.
fn local_to_system(civil: Civil) -> Option<SystemTime> {
    #[cfg(unix)]
    let seconds = super::localtime::local_to_utc(civil)?;
    #[cfg(not(unix))]
    let seconds = utc_seconds(civil);
    let seconds = u64::try_from(seconds).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// A version 2 record of `path`, deleted at `filetime`, of `size` bytes; `units` overrides
    /// the unit count it states.
    fn v2(path: &str, filetime: u64, size: u64, units: Option<u32>) -> Vec<u8> {
        let mut wide: Vec<u16> = path.encode_utf16().collect();
        wide.push(0);
        let count = units.unwrap_or_else(|| u32::try_from(wide.len()).unwrap());
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2_u64.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&filetime.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        for unit in wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    /// A version 1 record of `path`: the path padded with NULs to 260 units.
    fn v1(path: &str, filetime: u64, size: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&filetime.to_le_bytes());
        let mut wide: Vec<u16> = path.encode_utf16().collect();
        wide.resize(260, 0);
        for unit in wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    /// 2004-08-31T22:32:08 UTC, as a `FILETIME` and as a time.
    const FILETIME: u64 = FILETIME_AT_UNIX_EPOCH + 1_093_991_528 * FILETIME_PER_SECOND + 5_000;

    fn at(seconds: u64, nanos: u32) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos)
    }

    #[test]
    fn both_recycle_bin_layouts_are_read() {
        let path = r"C:\Users\kim\문서\report v2.txt";
        let expected = (PathBuf::from(path), Some(at(1_093_991_528, 500_000)), 1234);
        assert_eq!(
            parse_recycle_index(&v2(path, FILETIME, 1234, None)),
            Some(expected.clone())
        );
        assert_eq!(
            parse_recycle_index(&v1(path, FILETIME, 1234)),
            Some(expected)
        );
        // A share and an extended-length path are absolute too.
        for path in [r"\\server\share\a.txt", r"\\?\D:\deep\a"] {
            let (read, _, _) = parse_recycle_index(&v2(path, FILETIME, 0, None)).unwrap();
            assert_eq!(read, PathBuf::from(path));
        }
    }

    #[test]
    fn a_recycle_bin_time_of_zero_or_before_1970_is_unknown() {
        let path = r"C:\a.txt";
        for filetime in [0, 1, FILETIME_AT_UNIX_EPOCH - 1] {
            let (_, deleted, _) = parse_recycle_index(&v2(path, filetime, 0, None)).unwrap();
            assert_eq!(deleted, None, "{filetime}");
        }
        let (_, deleted, _) =
            parse_recycle_index(&v2(path, FILETIME_AT_UNIX_EPOCH, 0, None)).unwrap();
        assert_eq!(deleted, Some(SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn malformed_recycle_bin_records_are_refused() {
        let good = v2(r"C:\a.txt", FILETIME, 3, None);
        let mut other_version = good.clone();
        other_version[0] = 3;
        let mut v1_short = v1(r"C:\a.txt", FILETIME, 3);
        v1_short.pop();
        let mut v1_long = v1(r"C:\a.txt", FILETIME, 3);
        v1_long.push(0);
        let mut trailing = good.clone();
        trailing.extend_from_slice(&[0, 0]);
        let hostile: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("header only", good[..RECYCLE_HEADER].to_vec()),
            ("no length", good[..RECYCLE_HEADER + 2].to_vec()),
            ("version 3", other_version),
            ("version 1 one byte short", v1_short),
            ("version 1 one byte long", v1_long),
            ("bytes after the path", trailing),
            ("odd length", good[..good.len() - 1].to_vec()),
            (
                "count larger than the bytes",
                v2(r"C:\a.txt", FILETIME, 3, Some(10)),
            ),
            (
                "count smaller than the bytes",
                v2(r"C:\a.txt", FILETIME, 3, Some(2)),
            ),
            ("count of zero", v2("", FILETIME, 3, Some(0))),
            ("huge count", v2(r"C:\a.txt", FILETIME, 3, Some(u32::MAX))),
            ("empty path", v2("", FILETIME, 3, None)),
            ("relative path", v2(r"a\b.txt", FILETIME, 3, None)),
            ("unix path", v2("/etc/passwd", FILETIME, 3, None)),
            (
                "drive without a backslash",
                v2("C:a.txt", FILETIME, 3, None),
            ),
        ];
        for (what, bytes) in hostile {
            assert_eq!(parse_recycle_index(&bytes), None, "{what}");
        }
        // A path with no NUL at all.
        let mut no_nul = v2(r"C:\a.txt", FILETIME, 3, None);
        let len = no_nul.len();
        no_nul[len - 2..].copy_from_slice(&u16::from(b'x').to_le_bytes());
        assert_eq!(parse_recycle_index(&no_nul), None);
    }

    /// An unpaired surrogate is a name Windows can hold; elsewhere it cannot be a path.
    #[test]
    fn an_unpaired_surrogate_is_kept_only_on_windows() {
        let mut bytes = v2(r"C:\ab", FILETIME, 0, None);
        // The unit of `b`, just before the NUL.
        let at = bytes.len() - 4;
        bytes[at..at + 2].copy_from_slice(&0xD800_u16.to_le_bytes());
        assert_eq!(parse_recycle_index(&bytes).is_some(), cfg!(windows));
    }

    #[test]
    fn a_trashinfo_record_is_read() {
        let text = "[Trash Info]\nPath=/home/kim/%EB%AC%B8%EC%84%9C/a%20b%25.txt\nDeletionDate=2004-08-31T22:32:08\n";
        let (path, deleted) = parse_trashinfo(text).unwrap();
        assert_eq!(path, PathBuf::from("/home/kim/문서/a b%.txt"));
        let expected = local_to_system(parse_deletion_date("2004-08-31T22:32:08").unwrap());
        assert!(expected.is_some());
        assert_eq!(deleted, expected);
        // Read as a local time, it is within a day of the same time in UTC.
        let utc = at(1_093_991_528, 0);
        let off = deleted
            .unwrap()
            .duration_since(utc)
            .unwrap_or_else(|e| e.duration());
        assert!(off <= Duration::from_hours(26), "{off:?}");
    }

    #[test]
    fn a_trashinfo_record_may_be_laid_out_loosely() {
        let text = "\r\n# made by hand\r\n[Trash Info]\r\n  DeletionDate = 2020-02-29T00:00:00  \r\nX-Other=1\r\nPath = relative/a.txt\r\n[Other Group]\r\nPath=/elsewhere\r\nNoEquals\r\n";
        let (path, deleted) = parse_trashinfo(text).unwrap();
        assert_eq!(path, PathBuf::from("relative/a.txt"));
        assert!(deleted.is_some());
        // Lowercase escapes are read too.
        let (path, _) = parse_trashinfo("[Trash Info]\nPath=/a%2fb%c3%a9").unwrap();
        assert_eq!(path, PathBuf::from("/a/bé"));
    }

    #[test]
    fn an_unreadable_date_only_makes_the_time_unknown() {
        for date in [
            "",
            "2004-08-31 22:32:08",
            "20040831T22:32:08",
            "2004-08-31T22:32",
            "2004-02-30T00:00:00",
            "2003-02-29T00:00:00",
            "2004-13-01T00:00:00",
            "2004-08-31T24:00:00",
            "2004-08-31T23:60:00",
            "2004-08-31T23:59:60",
            "+004-08-31T23:59:59",
            "1969-12-31T00:00:00",
        ] {
            let text = format!("[Trash Info]\nPath=/a\nDeletionDate={date}\n");
            assert_eq!(
                parse_trashinfo(&text),
                Some((PathBuf::from("/a"), None)),
                "{date:?}"
            );
        }
        assert_eq!(
            parse_trashinfo("[Trash Info]\nPath=/a\n"),
            Some((PathBuf::from("/a"), None))
        );
    }

    #[test]
    fn malformed_trashinfo_records_are_refused() {
        for text in [
            "",
            "Path=/a\n",
            "Path=/a\n[Trash Info]\n",
            "[Trash]\nPath=/a\n",
            "[Trash Info]\n",
            "[Trash Info]\nDeletionDate=2004-08-31T22:32:08\n",
            "[Trash Info]\nPath=\n",
            "[Trash Info]\nPath=/a\nPath=/b\n",
            "[Trash Info]\nPath=/a\nDeletionDate=2004-08-31T22:32:08\nDeletionDate=2004-08-31T22:32:08\n",
            "[Trash Info]\nPath=/a\n[Trash Info]\nPath=/b\n",
            "[Trash Info]\nPath=/a\ngarbage\n",
            "[Trash Info]\nPath=/a%\n",
            "[Trash Info]\nPath=/a%4\n",
            "[Trash Info]\nPath=/a%zz\n",
            "[Trash Info]\nPath=/a%00b\n",
            "[Trash Info]\nPath=/a\0b\n",
            "[Trash Info]\nPath=/a/../etc/passwd\n",
            "[Trash Info]\nPath=../a\n",
            "[Trash Info]\nPath=a/%2E%2E/b\n",
        ] {
            assert_eq!(parse_trashinfo(text), None, "{text:?}");
        }
    }

    /// A path that is not UTF-8 is kept byte for byte on Unix.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_keeps_its_bytes() {
        use std::os::unix::ffi::OsStrExt;

        let (path, _) = parse_trashinfo("[Trash Info]\nPath=/a%FFb\n").unwrap();
        assert_eq!(path.as_os_str().as_bytes(), b"/a\xFFb");
        assert_eq!(escape_path(&path), "/a%FFb");
    }

    #[test]
    fn an_escaped_path_reads_back_as_it_was() {
        for path in [
            "/home/kim/a b.txt",
            "/x/문서/%25 100%.txt",
            "/a/(1)~!*'.-_",
            "/한",
        ] {
            let escaped = escape_path(Path::new(path));
            assert!(
                escaped.bytes().all(|b| kept_in_path(b) || b == b'%'),
                "{escaped}"
            );
            let text = format!("[Trash Info]\nPath={escaped}\n");
            assert_eq!(parse_trashinfo(&text).unwrap().0, PathBuf::from(path));
        }
        assert_eq!(escape_path(Path::new("/a b")), "/a%20b");
    }

    #[test]
    fn a_deletion_date_is_written_as_the_specification_shows() {
        let civil = Civil {
            year: 4,
            month: 8,
            day: 1,
            hour: 2,
            minute: 3,
            second: 9,
        };
        assert_eq!(format_deletion_date(civil), "0004-08-01T02:03:09");
        let text = "2004-08-31T22:32:08";
        assert_eq!(
            format_deletion_date(parse_deletion_date(text).unwrap()),
            text
        );
    }

    #[test]
    fn civil_dates_count_from_1970() {
        let utc = |text| utc_seconds(parse_deletion_date(text).unwrap());
        assert_eq!(utc("1970-01-01T00:00:00"), 0);
        assert_eq!(utc("2004-08-31T22:32:08"), 1_093_991_528);
        assert_eq!(utc("2000-03-01T00:00:00"), 951_868_800);
        assert_eq!(utc("1969-12-31T23:59:59"), -1);
        assert_eq!(utc("2400-02-29T12:00:00"), 13_574_606_400);
    }

    /// The system's own conversion agrees with the one here, to the time-zone offset.
    #[cfg(unix)]
    #[test]
    fn local_times_round_trip_through_the_system() {
        for seconds in [0_i64, 1_093_991_528, 1_700_000_000, 4_102_444_800] {
            let local = super::super::localtime::utc_to_local(seconds).unwrap();
            assert_eq!(super::super::localtime::local_to_utc(local), Some(seconds));
            let offset = utc_seconds(local) - seconds;
            assert!(offset.abs() <= 26 * 3_600, "{offset}");
        }
    }

    proptest! {
        #[test]
        fn no_record_bytes_make_the_reader_panic(bytes in proptest::collection::vec(any::<u8>(), 0..700)) {
            let _ = parse_recycle_index(&bytes);
        }

        #[test]
        fn records_with_a_real_header_never_make_the_reader_panic(
            version in 0_u64..4,
            count in any::<u32>(),
            tail in proptest::collection::vec(any::<u8>(), 0..600),
        ) {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&version.to_le_bytes());
            bytes.extend_from_slice(&[0; 16]);
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&tail);
            let _ = parse_recycle_index(&bytes);
        }

        #[test]
        fn no_text_makes_the_trashinfo_reader_panic(text in "\\PC*") {
            let _ = parse_trashinfo(&text);
        }

        #[test]
        fn no_value_makes_the_trashinfo_reader_panic(path in "[%/a-z0-9.]{0,40}", date in "[-0-9T:]{0,22}") {
            let text = format!("[Trash Info]\nPath={path}\nDeletionDate={date}\n");
            let _ = parse_trashinfo(&text);
        }
    }
}
