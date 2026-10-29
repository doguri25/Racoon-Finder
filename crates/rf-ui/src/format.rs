//! Sizes, times and kinds as the file list shows them.

use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rf_core::{Entry, EntryKind};

/// The units above bytes, each 1024 times the one before.
const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

/// A byte count for the size column and the status line: `"{n} 바이트"` below 1 KB, else the
/// largest unit (KB, MB, GB, TB, 1024 apart) that keeps the number below 1024. A number below
/// 10 keeps one decimal (`1.4 MB`, but `1 KB` rather than `1.0 KB`); a larger one is rounded to
/// a whole number (`214 KB`).
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} 바이트");
    }
    let bytes = u128::from(bytes);
    let mut unit: u128 = 1024;
    let mut index = 0;
    loop {
        let name = UNITS[index];
        let tenths = (bytes * 10 + unit / 2) / unit;
        if tenths < 100 {
            return match tenths % 10 {
                0 => format!("{} {name}", tenths / 10),
                frac => format!("{}.{frac} {name}", tenths / 10),
            };
        }
        let whole = (bytes + unit / 2) / unit;
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

/// The kind column of `entry`: what a folder, a link and a special file are, and for a file
/// the kind its extension names (`PDF 문서`, `PNG 이미지`, `CSV 파일`), or `파일` without one.
pub fn kind_label(entry: &Entry) -> String {
    match entry.kind {
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

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
}
