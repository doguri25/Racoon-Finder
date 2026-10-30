//! The system clipboard for files and text, in the forms File Explorer and Finder use (PRD
//! DND-10), and the network share of a mapped drive (for 경로 복사, OPS-07).
//!
//! - **Windows:** files go on the clipboard as `CF_HDROP` (a `DROPFILES` header, then the
//!   paths as a UTF-16 list, each path ended by a NUL and the list by one more), with the
//!   registered format `Preferred DropEffect` (a `DWORD`, `DROPEFFECT_COPY` or
//!   `DROPEFFECT_MOVE`), which tells a paste whether they were copied or cut, and with the
//!   paths as text (`CF_UNICODETEXT`, one a line). Text alone goes as `CF_UNICODETEXT`.
//! - **macOS:** one pasteboard item for each file with its file URL (`public.file-url`), the
//!   first one with the paths as text too (`public.utf8-plain-text`, one a line). The
//!   pasteboard has no way to say "move": Finder moves what was copied with ⌥⌘V, and a cut is
//!   the app's own mark. So [`get_files`] gives no [`DropEffect`] there.
//! - **Elsewhere** (Linux, which is for development): the files stay in this process, where
//!   [`get_files`] finds them again; text gives `io::ErrorKind::Unsupported`.
//!
//! The clipboard's *change count* tells whether another app has put something on it since:
//! `GetClipboardSequenceNumber` on Windows, `NSPasteboard.changeCount` on macOS, a counter of
//! this process elsewhere. [`put_files`] and [`put_text`] give the count of what they put, and
//! [`get_files`] the count of what it read.
//!
//! Every function blocks, if only for a short time: on Windows another app may hold the
//! clipboard open, and it is tried up to ten times, 10 ms apart, before the call fails with
//! `io::ErrorKind::ResourceBusy` (about 100 ms at most); another app's delayed data may take
//! longer to come. On macOS the pasteboard server answers. Call them from a worker thread, one
//! at a time, rather than from the UI thread; [`change_count`] alone is cheap. Nothing here
//! changes a file, though the system may look at one: macOS's `fileURLWithPath` asks whether
//! a path is a folder, and reading Finder's file reference URLs finds their items.
//!
//! The buffers of the Windows formats are made and read by pure functions of this module,
//! which the tests check on every system.

use std::io;
use std::path::{Component, Path, PathBuf, Prefix};

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(not(any(windows, target_os = "macos")))]
use self::other as imp;
#[cfg(windows)]
use self::windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(windows)]
mod windows;

/// What a paste of files does with them, as the clipboard says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DropEffect {
    /// Copy them: they were copied (Ctrl+C, ⌘C).
    Copy,
    /// Move them: they were cut (Ctrl+X).
    Move,
}

/// The files on the clipboard, as [`get_files`] read them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardFiles {
    /// Their paths, in the clipboard's order.
    pub paths: Vec<PathBuf>,
    /// Whether they are to be copied or moved, when the clipboard says (Windows, and the files
    /// of this process elsewhere); `None` on macOS, whose pasteboard does not say.
    pub effect: Option<DropEffect>,
    /// The clipboard's change count when they were read.
    pub change: u64,
}

/// Puts `paths` on the clipboard in the place of what it held, to be copied or moved
/// (`effect`) by a paste, and gives the clipboard's change count after it. Fails with
/// `io::ErrorKind::InvalidInput` for no paths, an empty path or a path with a NUL (or, on
/// macOS, one that is not UTF-8), and with `ResourceBusy` while another app keeps the
/// clipboard open (Windows). On macOS `effect` is not kept: see the module.
pub fn put_files(paths: &[PathBuf], effect: DropEffect) -> io::Result<u64> {
    if paths.is_empty() || paths.iter().any(|path| path.as_os_str().is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no files to put on the clipboard",
        ));
    }
    imp::put_files(paths, effect)
}

/// The files on the clipboard; `None` when it holds none (text, an image, nothing).
pub fn get_files() -> io::Result<Option<ClipboardFiles>> {
    imp::get_files()
}

/// Puts `text` on the clipboard in the place of what it held, and gives the clipboard's
/// change count after it. `io::ErrorKind::Unsupported` on Linux.
pub fn put_text(text: &str) -> io::Result<u64> {
    imp::put_text(text)
}

/// The text on the clipboard; `None` when it holds none. `io::ErrorKind::Unsupported` on Linux.
pub fn get_text() -> io::Result<Option<String>> {
    imp::get_text()
}

/// The clipboard's change count now; `None` when the system does not tell (Windows gives none
/// to a process without access to the clipboard).
pub fn change_count() -> Option<u64> {
    imp::change_count()
}

/// Empties the clipboard if its change count is still `change`, that is, if nothing was put
/// on it since; `true` when it did. On Windows the count is looked at while the clipboard is
/// held, so nothing another app puts on it in between is lost; on macOS another app could
/// write between the look and the emptying.
pub fn clear_if_unchanged(change: u64) -> io::Result<bool> {
    imp::clear_if_unchanged(change)
}

/// The path of `path` through the network share its drive is mapped to (Windows:
/// `Z:\docs\a.txt` on `\\server\share` is `\\server\share\docs\a.txt`); `None` for a path on
/// a drive that is not a network drive, for a path without a drive letter, and on other
/// systems. Asking may wait for the network: call it from a worker thread.
pub fn universal_path(path: &Path) -> Option<PathBuf> {
    imp::universal_path(path)
}

/// Whether `path` is a Windows path in the verbatim (`\\?\…`) or device (`\\.\…`)
/// namespace. A local path normalized as `rf_core::VPath` stores it keeps such a form only when
/// it names no folder of a drive or a share (`\\?\GLOBALROOT\Device\…`, `\\?\Volume{…}\…`):
/// a device or a volume that another app may put on the clipboard and that is no place for a
/// paste to read from. Always `false` on other systems, whose paths have no such prefix.
pub fn is_device_path(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => {
            prefix.kind().is_verbatim() || matches!(prefix.kind(), Prefix::DeviceNS(_))
        }
        _ => false,
    }
}

/// `CF_UNICODETEXT`, from `Win32::System::Ole`, whose feature the build leaves out.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
const CF_UNICODETEXT: u32 = 13;
/// `CF_HDROP`.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
const CF_HDROP: u32 = 15;
/// `DROPEFFECT_COPY` and `DROPEFFECT_MOVE`.
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// The size of `DROPFILES`: `pFiles` (a `DWORD`), `pt` (two `LONG`s), `fNC` and `fWide`
/// (`BOOL`s). The list of paths follows it.
const DROPFILES_SIZE: usize = 20;

/// The `CF_HDROP` buffer for `paths`, each as UTF-16 units: a `DROPFILES` header that says the
/// list starts right after it and is wide (UTF-16), then each path ended by a NUL, then one
/// more NUL. `None` for no paths, an empty path, or one with a NUL unit inside.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn drop_files(paths: &[Vec<u16>]) -> Option<Vec<u8>> {
    if paths.is_empty()
        || paths
            .iter()
            .any(|path| path.is_empty() || path.contains(&0))
    {
        return None;
    }
    let units: usize = paths.iter().map(|path| path.len() + 1).sum::<usize>() + 1;
    let mut buffer = Vec::with_capacity(DROPFILES_SIZE + units * 2);
    let header = u32::try_from(DROPFILES_SIZE).ok()?;
    buffer.extend_from_slice(&header.to_le_bytes());
    // `pt` (0, 0) and `fNC` (false): no point of a drop.
    buffer.extend_from_slice(&[0; 12]);
    // `fWide`: the paths are UTF-16.
    buffer.extend_from_slice(&1u32.to_le_bytes());
    for unit in paths
        .iter()
        .flat_map(|path| path.iter().copied().chain([0]))
    {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    buffer.extend_from_slice(&0u16.to_le_bytes());
    Some(buffer)
}

/// The paths of a `CF_HDROP` buffer, as [`read_drop_files`] finds them.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DropList {
    /// A wide list: each path as UTF-16 units.
    Wide(Vec<Vec<u16>>),
    /// A list in the system's ANSI code page (`fWide` false), which only the system reads.
    Narrow,
}

/// The paths of the `CF_HDROP` buffer `bytes`; `None` for a buffer that is not one: too short
/// for its header, a list that starts outside it, a path that does not end with a NUL inside
/// it (it may be cut short, and a path cut short may name another item), or no path at all.
/// A wide list ends at its empty path, or at the end of the buffer right after a path. A
/// narrow list, which the system reads, must end with its double NUL inside the buffer, so
/// that the system's reading stays inside it too.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn read_drop_files(bytes: &[u8]) -> Option<DropList> {
    let word = |at: usize| -> Option<u32> {
        let bytes: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(bytes))
    };
    let start = usize::try_from(word(0)?).ok()?;
    if start < DROPFILES_SIZE || start > bytes.len() {
        return None;
    }
    if word(16)? == 0 {
        // A code page's second byte of a character is never 0: the first two NULs in a row end
        // the list. At the start there is no path at all.
        let end = bytes[start..].windows(2).position(|pair| pair == [0, 0])?;
        return (end > 0).then_some(DropList::Narrow);
    }
    let units: Vec<u16> = bytes[start..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let mut paths = Vec::new();
    let mut rest = units.as_slice();
    loop {
        match rest.iter().position(|&unit| unit == 0) {
            // The empty path that ends the list.
            Some(0) => break,
            Some(end) => {
                paths.push(rest[..end].to_vec());
                rest = &rest[end + 1..];
            }
            // The end of the buffer right after a path ends the list too; anything else there
            // is a path without its NUL.
            None if rest.is_empty() => break,
            None => return None,
        }
    }
    (!paths.is_empty()).then_some(DropList::Wide(paths))
}

/// The `Preferred DropEffect` value for `effect`, as the clipboard holds it.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn drop_effect_bytes(effect: DropEffect) -> [u8; 4] {
    match effect {
        DropEffect::Copy => DROPEFFECT_COPY,
        DropEffect::Move => DROPEFFECT_MOVE,
    }
    .to_le_bytes()
}

/// What the `Preferred DropEffect` value `bytes` asks of a paste. Only a move alone is a move:
/// File Explorer's cut says `DROPEFFECT_MOVE`, its copy `DROPEFFECT_COPY | DROPEFFECT_LINK`;
/// a value that allows a copy too, or none that can be read, copies, which leaves the
/// originals where they are.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn read_drop_effect(bytes: &[u8]) -> DropEffect {
    let Some(value) = bytes
        .get(..4)
        .and_then(|word| <[u8; 4]>::try_from(word).ok())
        .map(u32::from_le_bytes)
    else {
        return DropEffect::Copy;
    };
    if value & DROPEFFECT_MOVE != 0 && value & DROPEFFECT_COPY == 0 {
        DropEffect::Move
    } else {
        DropEffect::Copy
    }
}

/// The `CF_UNICODETEXT` buffer of the UTF-16 text `units`: the units, then a NUL. A NUL inside
/// would end the text there.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn unicode_text(units: impl IntoIterator<Item = u16>) -> Vec<u8> {
    units
        .into_iter()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The `CF_UNICODETEXT` buffer of the paths `paths` as text, one a line (`\r\n` between them,
/// as File Explorer's 경로로 복사 writes them).
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn path_lines(paths: &[Vec<u16>]) -> Vec<u8> {
    let newline = [u16::from(b'\r'), u16::from(b'\n')];
    let mut units = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            units.extend_from_slice(&newline);
        }
        units.extend_from_slice(path);
    }
    unicode_text(units)
}

/// The text of the `CF_UNICODETEXT` buffer `bytes`: its units up to the first NUL (or the end),
/// with any unit that is not text replaced (U+FFFD).
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn read_unicode_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// The path `local` (UTF-16 units, `X:` and what follows) through `remote`, the network share
/// its drive is mapped to (`\\server\share`): the share in the place of the drive, so
/// `Z:\docs\a.txt` becomes `\\server\share\docs\a.txt` and the drive's root `Z:\` (or `Z:`) the
/// share's root `\\server\share\`. `None` for a path without a drive letter, or no share.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn unc_path(local: &[u16], remote: &[u16]) -> Option<Vec<u16>> {
    let backslash = u16::from(b'\\');
    let [letter, colon, rest @ ..] = local else {
        return None;
    };
    let is_letter = u8::try_from(*letter).is_ok_and(|letter| letter.is_ascii_alphabetic());
    if !is_letter || *colon != u16::from(b':') {
        return None;
    }
    let last = remote.iter().rposition(|&unit| unit != backslash)?;
    let share = &remote[..=last];
    let mut path = share.to_vec();
    if rest.first() == Some(&backslash) {
        path.extend_from_slice(rest);
    } else {
        path.push(backslash);
        path.extend_from_slice(rest);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn wides(texts: &[&str]) -> Vec<Vec<u16>> {
        texts.iter().map(|text| wide(text)).collect()
    }

    #[test]
    fn a_file_list_has_the_dropfiles_header_and_a_double_nul() {
        let buffer = drop_files(&wides(&["C:\\a", "D:\\b c"])).expect("a list");
        // `pFiles` 20, `pt` (0, 0), `fNC` false, `fWide` true.
        assert_eq!(&buffer[..4], &20u32.to_le_bytes());
        assert_eq!(&buffer[4..16], &[0; 12]);
        assert_eq!(&buffer[16..20], &1u32.to_le_bytes());
        let units: Vec<u16> = buffer[20..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let mut wanted = wide("C:\\a");
        wanted.push(0);
        wanted.extend(wide("D:\\b c"));
        wanted.extend([0, 0]);
        assert_eq!(units, wanted);
    }

    #[test]
    fn a_file_list_reads_back_as_it_was_made() {
        let long = format!("C:\\{}\\끝.txt", "아주 긴 폴더 이름\\".repeat(40));
        assert!(long.encode_utf16().count() > 260, "longer than MAX_PATH");
        let cases: Vec<Vec<Vec<u16>>> = vec![
            wides(&["C:\\Users\\me\\a.txt"]),
            wides(&[
                "C:\\문서\\보고서 (최종).hwp",
                "\\\\server\\share\\x y",
                "D:\\",
            ]),
            wides(&[long.as_str(), "E:\\%20 #?.txt"]),
            // A unit that is no text on its own (half of a pair) is kept as it is.
            vec![vec![u16::from(b'C'), u16::from(b':'), 0xD800]],
        ];
        for paths in cases {
            let buffer = drop_files(&paths).expect("a list");
            assert_eq!(read_drop_files(&buffer), Some(DropList::Wide(paths)));
        }
    }

    #[test]
    fn no_list_is_made_of_nothing_or_of_a_path_that_is_not_one() {
        assert_eq!(drop_files(&[]), None);
        assert_eq!(drop_files(&wides(&["C:\\a", ""])), None);
        assert_eq!(
            drop_files(&[vec![u16::from(b'a'), 0, u16::from(b'b')]]),
            None
        );
    }

    #[test]
    fn a_broken_file_list_is_refused() {
        let good = drop_files(&wides(&["C:\\a", "C:\\b"])).expect("a list");
        // Too short for the header.
        assert_eq!(read_drop_files(&good[..19]), None);
        // The list starts inside the header, or past the end.
        let mut inside = good.clone();
        inside[..4].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(read_drop_files(&inside), None);
        let mut past = good.clone();
        let beyond = u32::try_from(good.len() + 2).expect("small");
        past[..4].copy_from_slice(&beyond.to_le_bytes());
        assert_eq!(read_drop_files(&past), None);
        // A path cut short, without its NUL, may name another item: nothing is read.
        let cut = &good[..good.len() - 8];
        assert_eq!(read_drop_files(cut), None);
        // No path at all.
        let mut empty = good[..20].to_vec();
        empty.extend_from_slice(&[0, 0]);
        assert_eq!(read_drop_files(&empty), None);
    }

    #[test]
    fn a_list_without_its_last_nul_ends_after_its_last_path() {
        // The buffer ends right after the NUL of the last path: the system's block may be cut
        // there by another app, and every path in it is whole.
        let good = drop_files(&wides(&["C:\\a", "C:\\b"])).expect("a list");
        let short = &good[..good.len() - 2];
        assert_eq!(
            read_drop_files(short),
            Some(DropList::Wide(wides(&["C:\\a", "C:\\b"])))
        );
        // Bytes past the double NUL (the system's block is rounded up) are not read.
        let mut padded = good.clone();
        padded.extend_from_slice(&[0x41, 0, 0x42, 0, 0x43]);
        assert_eq!(
            read_drop_files(&padded),
            Some(DropList::Wide(wides(&["C:\\a", "C:\\b"])))
        );
    }

    #[test]
    fn a_narrow_list_is_left_to_the_system_once_it_ends_inside_its_buffer() {
        let narrow = |list: &[u8]| {
            let mut buffer = drop_files(&wides(&["C:\\a"])).expect("a list");
            buffer.truncate(20);
            buffer[16..20].copy_from_slice(&0u32.to_le_bytes());
            buffer.extend_from_slice(list);
            read_drop_files(&buffer)
        };
        assert_eq!(narrow(b"C:\\a\0D:\\b c\0\0"), Some(DropList::Narrow));
        // Bytes after the end are not the list's.
        assert_eq!(narrow(b"C:\\a\0\0\x41\x42"), Some(DropList::Narrow));
        // No double NUL inside the buffer: the system would read past it.
        assert_eq!(narrow(b"C:\\a\0D:\\b"), None);
        assert_eq!(narrow(b"C:\\a\0"), None);
        assert_eq!(narrow(b""), None);
        // No path at all.
        assert_eq!(narrow(b"\0\0"), None);
    }

    #[test]
    fn only_device_and_verbatim_paths_are_device_paths() {
        for path in ["/home/a", "C:\\a", "\\\\server\\share\\a", "relative"] {
            assert!(!is_device_path(Path::new(path)), "{path}");
        }
        // Windows alone parses these prefixes; elsewhere they are names like any other.
        for path in [
            "\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy1\\a.txt",
            "\\\\?\\Volume{12345678-0000-0000-0000-000000000000}\\a.txt",
            "\\\\?\\C:\\a.txt",
            "\\\\.\\COM1",
        ] {
            assert_eq!(is_device_path(Path::new(path)), cfg!(windows), "{path}");
        }
    }

    #[test]
    fn only_a_move_alone_moves() {
        assert_eq!(
            read_drop_effect(&drop_effect_bytes(DropEffect::Move)),
            DropEffect::Move
        );
        assert_eq!(
            read_drop_effect(&drop_effect_bytes(DropEffect::Copy)),
            DropEffect::Copy
        );
        // File Explorer's copy: DROPEFFECT_COPY | DROPEFFECT_LINK.
        assert_eq!(read_drop_effect(&5u32.to_le_bytes()), DropEffect::Copy);
        // Move with link is still a move; with copy too it is not.
        assert_eq!(read_drop_effect(&6u32.to_le_bytes()), DropEffect::Move);
        assert_eq!(read_drop_effect(&3u32.to_le_bytes()), DropEffect::Copy);
        assert_eq!(read_drop_effect(&7u32.to_le_bytes()), DropEffect::Copy);
        // Nothing to read, or too little: copy.
        assert_eq!(read_drop_effect(&[]), DropEffect::Copy);
        assert_eq!(read_drop_effect(&[2, 0]), DropEffect::Copy);
        assert_eq!(read_drop_effect(&0u32.to_le_bytes()), DropEffect::Copy);
        // A longer block reads its first DWORD.
        assert_eq!(read_drop_effect(&[2, 0, 0, 0, 9, 9]), DropEffect::Move);
    }

    #[test]
    fn text_is_utf16_ended_by_a_nul() {
        let buffer = unicode_text(wide("경로 a"));
        assert_eq!(buffer.len(), (4 + 1) * 2);
        assert_eq!(&buffer[buffer.len() - 2..], &[0, 0]);
        assert_eq!(read_unicode_text(&buffer), "경로 a");
        // It stops at the first NUL, and at the end of a block without one.
        let mut padded = buffer.clone();
        padded.extend(unicode_text(wide("뒤")));
        assert_eq!(read_unicode_text(&padded), "경로 a");
        assert_eq!(read_unicode_text(&buffer[..4]), "경로");
        // An odd last byte is not a unit.
        assert_eq!(read_unicode_text(&buffer[..3]), "경");
        assert_eq!(read_unicode_text(&[]), "");
    }

    #[test]
    fn the_paths_as_text_take_a_line_each() {
        let buffer = path_lines(&wides(&["C:\\a b", "D:\\문서"]));
        assert_eq!(read_unicode_text(&buffer), "C:\\a b\r\nD:\\문서");
        assert_eq!(read_unicode_text(&path_lines(&wides(&["C:\\a"]))), "C:\\a");
    }

    #[test]
    fn a_mapped_drive_s_path_goes_through_its_share() {
        let unc = |local: &str, remote: &str| {
            unc_path(&wide(local), &wide(remote)).map(|units| String::from_utf16_lossy(&units))
        };
        let share = "\\\\server\\share";
        assert_eq!(
            unc("Z:\\docs\\a.txt", share).as_deref(),
            Some("\\\\server\\share\\docs\\a.txt")
        );
        assert_eq!(
            unc("z:\\보고서", share).as_deref(),
            Some("\\\\server\\share\\보고서")
        );
        // The drive's root is the share's.
        assert_eq!(unc("Z:\\", share).as_deref(), Some("\\\\server\\share\\"));
        assert_eq!(unc("Z:", share).as_deref(), Some("\\\\server\\share\\"));
        // A share written with a trailing backslash has one between it and the path.
        assert_eq!(
            unc("Z:\\a", "\\\\server\\share\\").as_deref(),
            Some("\\\\server\\share\\a")
        );
        // No drive letter, or no share: nothing.
        assert_eq!(unc("\\\\other\\x\\a", share), None);
        assert_eq!(unc("/home/a", share), None);
        assert_eq!(unc("1:\\a", share), None);
        assert_eq!(unc("Z:\\a", ""), None);
        assert_eq!(unc("Z:\\a", "\\\\"), None);
    }
}
