use std::cmp::Ordering::{self, Equal, Greater, Less};
use std::ffi::OsStr;

use rf_platform::{NaturalKey, natural_cmp, portable_natural_cmp};

const AGREED: &[(&str, &str, Ordering)] = &[
    ("file2", "file10", Less),
    ("file10", "file2", Greater),
    ("file9.txt", "file10.txt", Less),
    ("abc", "ABD", Less),
    ("x", "x1", Less),
    ("1", "a", Less),
    ("사진2", "사진10", Less),
    ("img 9.png", "img 10.png", Less),
    ("same", "same", Equal),
];

#[test]
fn os_rule_agrees_on_common_cases() {
    for &(a, b, want) in AGREED {
        assert_eq!(
            natural_cmp(OsStr::new(a), OsStr::new(b)),
            want,
            "{a} vs {b}"
        );
    }
}

#[test]
fn natural_key_matches_natural_cmp() {
    for &(a, b, want) in AGREED {
        let (ka, kb) = (
            NaturalKey::new(OsStr::new(a)),
            NaturalKey::new(OsStr::new(b)),
        );
        assert_eq!(ka.compare(&kb), want, "{a} vs {b}");
    }
}

#[test]
fn portable_rule_agrees_on_common_cases() {
    for &(a, b, want) in AGREED {
        assert_eq!(
            portable_natural_cmp(OsStr::new(a), OsStr::new(b)),
            want,
            "{a} vs {b}"
        );
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn other_oses_use_the_portable_rule() {
    assert_eq!(natural_cmp(OsStr::new("File1"), OsStr::new("file1")), Equal);
    assert_eq!(natural_cmp(OsStr::new("a01"), OsStr::new("a1")), Equal);
}
