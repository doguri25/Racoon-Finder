//! Sizes, times, kinds and folder names as the file list and the other screens show them.

use std::ffi::OsStr;
use std::path::Path;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rf_core::{Attributes, Entry, EntryKind, VPath};

/// The units above bytes, each 1024 times the one before.
const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

/// A byte count for the size column and the status line: `"{n} 바이트"` below 1 KB, else the
/// largest unit (KB, MB, GB, TB, 1024 apart) that keeps the number below 1024. A number below
/// 10 keeps one decimal (`1.4 MB`, but `1 KB` rather than `1.0 KB`); a larger one is rounded to
/// a whole number (`214 KB`). It rounds to the nearest; see [`format_size_rounded`].
pub fn format_size(bytes: u64) -> String {
    format_size_rounded(bytes, Rounding::Nearest)
}

/// How [`format_size_rounded`] rounds to the digits it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rounding {
    /// To the nearest, halves up: for a size on its own.
    Nearest,
    /// Never below the value: for what something needs, or how large it is, next to a limit.
    Up,
    /// Never above the value: for a limit or what is free, next to what is needed.
    Down,
}

impl Rounding {
    /// `value / unit`, rounded this way.
    fn divide(self, value: u128, unit: u128) -> u128 {
        match self {
            Rounding::Nearest => (value + unit / 2) / unit,
            Rounding::Up => value.div_ceil(unit),
            Rounding::Down => value / unit,
        }
    }
}

/// A byte count in the form of [`format_size`], rounded as `rounding` says. A need rounded
/// [`Rounding::Up`] next to a limit rounded [`Rounding::Down`] never reads as the same
/// number when the need is the larger: `4 GB, 최대 3.9 GB` for a 4 GiB file on FAT32.
pub fn format_size_rounded(bytes: u64, rounding: Rounding) -> String {
    if bytes < 1024 {
        return format!("{bytes} 바이트");
    }
    let bytes = u128::from(bytes);
    let mut unit: u128 = 1024;
    let mut index = 0;
    loop {
        let name = UNITS[index];
        let tenths = rounding.divide(bytes * 10, unit);
        if tenths < 100 {
            return match tenths % 10 {
                0 => format!("{} {name}", tenths / 10),
                frac => format!("{}.{frac} {name}", tenths / 10),
            };
        }
        let whole = rounding.divide(bytes, unit);
        if whole < 1024 || index == UNITS.len() - 1 {
            return format!("{whole} {name}");
        }
        unit *= 1024;
        index += 1;
    }
}

/// A time in the system's time zone (UTC when it cannot be told), `YYYY-MM-DD HH:MM`.
///
/// This looks the system time zone up on every call; a list that formats many times can look
/// it up once with `TimeZone::try_system` and call [`format_time_in`].
pub fn format_time(t: SystemTime) -> String {
    format_time_in(t, &TimeZone::try_system().unwrap_or(TimeZone::UTC))
}

/// A time in `tz`, `YYYY-MM-DD HH:MM`, or an empty string for a time too far from today to
/// show (beyond the years -9999 to 9999).
pub fn format_time_in(t: SystemTime, tz: &TimeZone) -> String {
    match Timestamp::try_from(t) {
        Ok(timestamp) => tz
            .to_datetime(timestamp)
            .strftime("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// `path`, as the screen shows it, shortened in the middle to about `max` characters: its root
/// (`/`, the drive, or the server and share of a network path) and as many of its last parts as
/// fit, with `…` between them (`C:\…\문서\백업`, `/…/작업/백업`, `\\서버\공유\…\백업`). The end
/// of the path, where it differs from its neighbours, stays in view. A path that fits is
/// returned as it is; the root and the last part are kept whole, even when they alone are
/// longer.
pub fn short_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_owned();
    }
    let sep = if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let parts: Vec<&str> = path.split(sep).collect();
    // `\\server\share` splits into two empty parts, the server and the share.
    let root_parts = if sep == '\\' && path.starts_with(r"\\") {
        4
    } else {
        1
    };
    if parts.len() <= root_parts {
        return path.to_owned();
    }
    let (root, rest) = parts.split_at(root_parts);
    let head = format!("{}{sep}…", root.join(&sep.to_string()));
    let mut length = head.chars().count();
    let mut kept: Vec<&str> = Vec::new();
    for part in rest.iter().rev() {
        let more = part.chars().count() + 1;
        if !kept.is_empty() && length + more > max {
            break;
        }
        kept.push(part);
        length += more;
    }
    if kept.len() == rest.len() {
        return path.to_owned();
    }
    kept.reverse();
    let sep = sep.to_string();
    format!("{head}{sep}{}", kept.join(&sep))
}

/// The name of a folder in a tab or the path row: its own name, or for a root the path
/// without its last separator (`/`, `C:`, `\\server\share`).
pub fn folder_label(path: &VPath) -> String {
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }
    let text = path.to_string();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        text
    } else {
        trimmed.to_owned()
    }
}

/// The extension of `entry` as the rows show it, in the 확장자 column and for 같은 확장자 선택:
/// a file's alone ([`Entry::file_extension`], the one the sort by 확장자 uses). A folder has
/// none, and neither has a link, which may lead to a folder (`앨범.2025`): the listing does not
/// tell.
pub fn file_extension(entry: &Entry) -> Option<&OsStr> {
    entry.file_extension()
}

/// The name the rows show for `entry`: the whole name, or, with the extensions hidden
/// (`show_extensions` off, NAV-11), a file's name without the dot and the extension the 확장자
/// column shows ([`file_extension`]). A folder, a link, a name without an extension and a dot
/// file (`.bashrc`) show whole.
pub fn shown_name(entry: &Entry, show_extensions: bool) -> String {
    let name = entry.name.to_string_lossy();
    let extension = file_extension(entry).filter(|_| !show_extensions);
    let stem = extension.and_then(|extension| {
        name.strip_suffix(&*extension.to_string_lossy())?
            .strip_suffix('.')
    });
    stem.map_or_else(|| name.clone().into_owned(), str::to_owned)
}

/// The kind column of `entry`: what a folder, a link and a special file are, and for a file
/// the kind its extension names (`PDF 문서`, `PNG 이미지`, `CSV 파일`), or `파일` without one.
/// A folder named `….app` is a macOS app bundle and reads `응용 프로그램`.
pub fn kind_label(entry: &Entry) -> String {
    match entry.kind {
        EntryKind::Dir if is_app_bundle(&entry.name) => return "응용 프로그램".to_owned(),
        EntryKind::Dir => return "파일 폴더".to_owned(),
        EntryKind::Symlink => return "바로 가기".to_owned(),
        EntryKind::Other => return "특수 파일".to_owned(),
        EntryKind::File => {}
    }
    let Some(ext) = entry.extension() else {
        return "파일".to_owned();
    };
    let ext = ext.to_string_lossy();
    let upper = ext.to_uppercase();
    match ext.to_lowercase().as_str() {
        "pdf" => "PDF 문서".to_owned(),
        "txt" => "텍스트 문서".to_owned(),
        "md" => "Markdown 문서".to_owned(),
        "doc" | "docx" => "Word 문서".to_owned(),
        "xls" | "xlsx" => "Excel 통합 문서".to_owned(),
        "ppt" | "pptx" => "PowerPoint 프레젠테이션".to_owned(),
        "hwp" | "hwpx" => "한글 문서".to_owned(),
        "exe" | "app" => "응용 프로그램".to_owned(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "tiff" => {
            format!("{upper} 이미지")
        }
        "mp3" | "wav" | "flac" | "m4a" => format!("{upper} 오디오"),
        "mp4" | "mov" | "mkv" | "avi" => format!("{upper} 동영상"),
        "zip" | "7z" | "rar" | "tar" | "gz" => format!("{upper} 압축 파일"),
        _ => format!("{upper} 파일"),
    }
}

/// The line icon of `entry` in the icon views and the list (`Icon.name`): a folder, a link, an
/// app, or a file by what its extension says it holds (the groups of [`kind_label`]); thumbnails
/// are for a later version.
pub fn file_icon(entry: &Entry) -> &'static str {
    match entry.kind {
        EntryKind::Dir if is_app_bundle(&entry.name) => return "app",
        EntryKind::Dir => return "folder",
        EntryKind::Symlink => return "link",
        EntryKind::Other => return "file",
        EntryKind::File => {}
    }
    let Some(ext) = entry.extension() else {
        return "file";
    };
    match ext.to_string_lossy().to_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "tiff" | "ico" | "svg" => {
            "image"
        }
        "mp3" | "wav" | "flac" | "m4a" | "ogg" | "aac" => "audio",
        "mp4" | "mov" | "mkv" | "avi" | "webm" => "video",
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" => "archive",
        "pdf" | "txt" | "md" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "hwp"
        | "hwpx" | "rtf" | "csv" => "document",
        "exe" | "app" | "msi" | "dmg" | "sh" | "bat" | "cmd" => "app",
        _ => "file",
    }
}

/// The attribute column (속성): a letter for each attribute set, in File Explorer's letters and
/// order — R read-only, H hidden, S system, A archive, C compressed, E encrypted, P sparse, O
/// offline, L a link or reparse point. Windows sets any of them; on macOS and Linux an item
/// has R (no one may write it), H (a dot file, or Finder's hidden flag), and on macOS C and O
/// (iCloud's files not on the disk). The Unix permission bits are not in an `Entry`, so the
/// column does not show them.
pub fn attribute_letters(attributes: Attributes) -> String {
    const LETTERS: [(Attributes, char); 9] = [
        (Attributes::READONLY, 'R'),
        (Attributes::HIDDEN, 'H'),
        (Attributes::SYSTEM, 'S'),
        (Attributes::ARCHIVE, 'A'),
        (Attributes::COMPRESSED, 'C'),
        (Attributes::ENCRYPTED, 'E'),
        (Attributes::SPARSE, 'P'),
        (Attributes::OFFLINE, 'O'),
        (Attributes::REPARSE_POINT, 'L'),
    ];
    LETTERS
        .into_iter()
        .filter(|(attribute, _)| attributes.contains(*attribute))
        .map(|(_, letter)| letter)
        .collect()
}

/// Whether a folder called `name` is a macOS app bundle: its extension is `app`, in any case.
fn is_app_bundle(name: &OsStr) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn hidden_extensions_leave_the_name_before_them() {
        let shown =
            |name: &str, kind: EntryKind, show: bool| shown_name(&Entry::new(name, kind), show);
        assert_eq!(shown("설계안.pdf", EntryKind::File, true), "설계안.pdf");
        assert_eq!(shown("설계안.pdf", EntryKind::File, false), "설계안");
        assert_eq!(shown("a.tar.gz", EntryKind::File, false), "a.tar");
        assert_eq!(shown("PHOTO.JPG", EntryKind::File, false), "PHOTO");
        // A folder's name has no extension, and a link's shows whole: it may lead to a folder
        // (the listing does not follow it). Windows' shortcuts (.lnk) are files.
        assert_eq!(shown("photos.2026", EntryKind::Dir, false), "photos.2026");
        assert_eq!(
            shown("photos.2026", EntryKind::Symlink, false),
            "photos.2026"
        );
        assert_eq!(shown("문서.lnk", EntryKind::File, false), "문서");
        assert_eq!(shown("장치", EntryKind::Other, false), "장치");
        // Nothing after the dot, or nothing before it: no extension.
        assert_eq!(shown(".bashrc", EntryKind::File, false), ".bashrc");
        assert_eq!(shown("README", EntryKind::File, false), "README");
        assert_eq!(shown("끝.", EntryKind::File, false), "끝.");
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_no_text_loses_its_extension_as_it_shows() {
        use std::os::unix::ffi::OsStrExt;
        let mut entry = Entry::new("x", EntryKind::File);
        entry.name = OsStr::from_bytes(b"ph\xffoto.jpg").to_os_string();
        assert_eq!(shown_name(&entry, false), "ph\u{FFFD}oto");
    }

    #[test]
    fn long_paths_lose_their_middle() {
        assert_eq!(short_path("/home/me/작업/백업", 40), "/home/me/작업/백업");
        assert_eq!(
            short_path("/tmp/claude-0/scratchpad/t6/home/작업/백업", 24),
            "/…/t6/home/작업/백업"
        );
        assert_eq!(
            short_path("C:\\Users\\me\\Documents\\프로젝트\\백업", 20),
            "C:\\…\\프로젝트\\백업"
        );
        // The last part stays whole, however long.
        let long = format!("/a/{}", "x".repeat(50));
        assert_eq!(short_path(&long, 10), format!("/…/{}", "x".repeat(50)));
        // Nothing to leave out: as it is.
        assert_eq!(
            short_path("/abcdefghijklmnopqrstuvwxyz", 5),
            "/abcdefghijklmnopqrstuvwxyz"
        );
    }

    #[test]
    fn a_network_path_keeps_its_server_and_share() {
        assert_eq!(
            short_path(r"\\server\share\팀\2026\보고서\최종", 24),
            r"\\server\share\…\보고서\최종"
        );
        // However short the room, the root and the last part stay.
        assert_eq!(
            short_path(r"\\server\share\팀\2026\보고서\최종", 5),
            r"\\server\share\…\최종"
        );
        // A root alone, or with one part, has nothing to leave out.
        assert_eq!(short_path(r"\\server\share", 5), r"\\server\share");
        assert_eq!(short_path(r"\\server\share\팀", 5), r"\\server\share\팀");
    }

    #[test]
    fn sizes_round_like_the_mockup() {
        assert_eq!(format_size(0), "0 바이트");
        assert_eq!(format_size(1023), "1023 바이트");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(1_477_427), "1.4 MB");
        assert_eq!(format_size(219_136), "214 KB");
        assert_eq!(format_size(13_002_342), "12 MB");
        // 9.95 KB rounds to 10, which shows without a decimal.
        assert_eq!(format_size(10_189), "10 KB");
        assert_eq!(format_size(10_188), "9.9 KB");
        // Just below 1 MB rounds up into the next unit, not to "1024 KB".
        assert_eq!(format_size(1024 * 1024 - 1), "1 MB");
        assert_eq!(format_size(212 * 1024 * 1024 * 1024), "212 GB");
        assert_eq!(format_size(3 * 1024_u64.pow(4) / 2), "1.5 TB");
        // TB is the largest unit.
        assert_eq!(format_size(2048 * 1024_u64.pow(4)), "2048 TB");
        assert_eq!(format_size(u64::MAX), "16777216 TB");
    }

    #[test]
    fn sizes_round_up_or_down_on_request() {
        // Nearest is what `format_size` does.
        assert_eq!(format_size_rounded(1_477_427, Rounding::Nearest), "1.4 MB");
        assert_eq!(format_size_rounded(1_477_427, Rounding::Up), "1.5 MB");
        assert_eq!(format_size_rounded(1_477_427, Rounding::Down), "1.4 MB");
        assert_eq!(format_size_rounded(219_137, Rounding::Up), "215 KB");
        assert_eq!(format_size_rounded(219_137, Rounding::Down), "214 KB");
        // An exact value stays as it is.
        assert_eq!(format_size_rounded(219_136, Rounding::Up), "214 KB");
        // Bytes are exact either way.
        assert_eq!(format_size_rounded(1000, Rounding::Up), "1000 바이트");
        // Up never shows less than the size, and moves to the next unit at 1024.
        assert_eq!(format_size_rounded(1024 * 1024 - 1, Rounding::Up), "1 MB");
        assert_eq!(
            format_size_rounded(1024 * 1024 - 1, Rounding::Down),
            "1023 KB"
        );
        assert_eq!(format_size_rounded(1024 * 1024 + 1, Rounding::Up), "1.1 MB");
        assert_eq!(format_size_rounded(1024 * 1024 + 1, Rounding::Down), "1 MB");
        // The largest file FAT32 holds is a byte short of 4 GiB.
        let fat32_max = 4 * 1024_u64.pow(3) - 1;
        assert_eq!(format_size_rounded(fat32_max, Rounding::Down), "3.9 GB");
        assert_eq!(format_size_rounded(fat32_max + 1, Rounding::Up), "4 GB");
        assert_eq!(format_size_rounded(u64::MAX, Rounding::Up), "16777216 TB");
        assert_eq!(format_size_rounded(u64::MAX, Rounding::Down), "16777215 TB");
    }

    #[test]
    fn times_use_the_given_zone() {
        // 2026-10-30 03:04:59 UTC
        let t = UNIX_EPOCH + Duration::from_secs(1_793_329_499);
        assert_eq!(format_time_in(t, &TimeZone::UTC), "2026-10-30 03:04");
        let seoul = TimeZone::get("Asia/Seoul").expect("Asia/Seoul is in the time zone database");
        assert_eq!(format_time_in(t, &seoul), "2026-10-30 12:04");
        // Before 1970 works too.
        let early = UNIX_EPOCH - Duration::from_hours(24);
        assert_eq!(format_time_in(early, &TimeZone::UTC), "1969-12-31 00:00");
        // The local form has the same shape whatever the zone is.
        assert_eq!(format_time(t).len(), "YYYY-MM-DD HH:MM".len());
    }

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File)
    }

    #[test]
    fn kinds_cover_common_extensions() {
        assert_eq!(kind_label(&Entry::new("사진", EntryKind::Dir)), "파일 폴더");
        assert_eq!(
            kind_label(&Entry::new("a.pdf", EntryKind::Dir)),
            "파일 폴더"
        );
        // A macOS app is a folder whose name ends in `.app`.
        assert_eq!(
            kind_label(&Entry::new("Safari.app", EntryKind::Dir)),
            "응용 프로그램"
        );
        assert_eq!(
            kind_label(&Entry::new("Tool.APP", EntryKind::Dir)),
            "응용 프로그램"
        );
        assert_eq!(
            kind_label(&Entry::new("링크.pdf", EntryKind::Symlink)),
            "바로 가기"
        );
        assert_eq!(
            kind_label(&Entry::new("fifo", EntryKind::Other)),
            "특수 파일"
        );

        let cases = [
            ("설계안.pdf", "PDF 문서"),
            ("메모.TXT", "텍스트 문서"),
            ("README.md", "Markdown 문서"),
            ("a.png", "PNG 이미지"),
            ("a.JPG", "JPG 이미지"),
            ("a.jpeg", "JPEG 이미지"),
            ("a.gif", "GIF 이미지"),
            ("a.webp", "WEBP 이미지"),
            ("a.heic", "HEIC 이미지"),
            ("a.bmp", "BMP 이미지"),
            ("a.tiff", "TIFF 이미지"),
            ("a.mp3", "MP3 오디오"),
            ("a.wav", "WAV 오디오"),
            ("a.flac", "FLAC 오디오"),
            ("a.m4a", "M4A 오디오"),
            ("a.mp4", "MP4 동영상"),
            ("a.mov", "MOV 동영상"),
            ("a.mkv", "MKV 동영상"),
            ("a.avi", "AVI 동영상"),
            ("a.zip", "ZIP 압축 파일"),
            ("a.7z", "7Z 압축 파일"),
            ("a.rar", "RAR 압축 파일"),
            ("a.tar", "TAR 압축 파일"),
            ("a.tar.gz", "GZ 압축 파일"),
            ("a.doc", "Word 문서"),
            ("a.docx", "Word 문서"),
            ("a.xls", "Excel 통합 문서"),
            ("a.xlsx", "Excel 통합 문서"),
            ("a.ppt", "PowerPoint 프레젠테이션"),
            ("a.pptx", "PowerPoint 프레젠테이션"),
            ("보고서.hwp", "한글 문서"),
            ("보고서.HWPX", "한글 문서"),
            ("setup.exe", "응용 프로그램"),
            ("Tool.app", "응용 프로그램"),
            ("data.csv", "CSV 파일"),
            ("main.rs", "RS 파일"),
            ("Makefile", "파일"),
            (".bashrc", "파일"),
            ("끝.", "파일"),
        ];
        for (name, label) in cases {
            assert_eq!(kind_label(&file(name)), label, "{name}");
        }
    }

    #[test]
    fn icons_follow_the_kind() {
        let cases = [
            (Entry::new("사진", EntryKind::Dir), "folder"),
            (Entry::new("Safari.app", EntryKind::Dir), "app"),
            (Entry::new("링크.pdf", EntryKind::Symlink), "link"),
            (Entry::new("fifo", EntryKind::Other), "file"),
            (file("a.JPG"), "image"),
            (file("a.flac"), "audio"),
            (file("a.mkv"), "video"),
            (file("a.tar.gz"), "archive"),
            (file("보고서.hwp"), "document"),
            (file("setup.exe"), "app"),
            (file("main.rs"), "file"),
            (file("Makefile"), "file"),
        ];
        for (entry, icon) in cases {
            assert_eq!(file_icon(&entry), icon, "{:?}", entry.name);
        }
    }

    #[test]
    fn attributes_read_as_explorer_letters() {
        assert_eq!(attribute_letters(Attributes::empty()), "");
        assert_eq!(attribute_letters(Attributes::ARCHIVE), "A");
        assert_eq!(
            attribute_letters(Attributes::ARCHIVE | Attributes::HIDDEN | Attributes::SYSTEM),
            "HSA"
        );
        assert_eq!(
            attribute_letters(
                Attributes::READONLY | Attributes::OFFLINE | Attributes::REPARSE_POINT
            ),
            "ROL"
        );
        assert_eq!(attribute_letters(Attributes::all()), "RHSACEPOL");
    }
}
