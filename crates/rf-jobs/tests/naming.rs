#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

use std::ffi::{OsStr, OsString};

use rf_core::Os;
use rf_jobs::{JobId, MAX_NUMBERED, TEMP_SUFFIX, numbered_name, temp_job, temp_name};

fn n(name: &str, k: u32, dir: bool, os: Os) -> OsString {
    numbered_name(OsStr::new(name), k, dir, os)
}

#[test]
fn windows_rule_puts_the_number_before_the_last_extension() {
    assert_eq!(n("report.txt", 2, false, Os::Windows), "report (2).txt");
    assert_eq!(n("a.tar.gz", 3, false, Os::Windows), "a.tar (3).gz");
    assert_eq!(n("사진.jpg", 2, false, Os::Other), "사진 (2).jpg");
}

#[test]
fn macos_rule_uses_a_bare_number() {
    assert_eq!(n("report.txt", 2, false, Os::MacOs), "report 2.txt");
    assert_eq!(n("Folder", 2, true, Os::MacOs), "Folder 2");
}

#[test]
fn folders_dotfiles_and_trailing_dots_keep_the_whole_name() {
    assert_eq!(n("photos.2026", 2, true, Os::Windows), "photos.2026 (2)");
    assert_eq!(n(".bashrc", 2, false, Os::Windows), ".bashrc (2)");
    assert_eq!(n("a.", 2, false, Os::Windows), "a. (2)");
    assert_eq!(n("noext", 2, false, Os::Windows), "noext (2)");
}

#[test]
fn a_name_that_already_carries_a_number_gets_another_one() {
    assert_eq!(
        n("report (2).txt", 3, false, Os::Windows),
        "report (2) (3).txt"
    );
    assert_eq!(n("report 2.txt", 3, false, Os::MacOs), "report 2 3.txt");
    assert_eq!(n("dir (2)", 3, true, Os::Other), "dir (2) (3)");
}

#[test]
fn a_dot_file_with_an_extension_keeps_its_leading_dot_in_the_stem() {
    assert_eq!(n(".config.json", 2, false, Os::Windows), ".config (2).json");
    assert_eq!(n(".config.json", 2, false, Os::MacOs), ".config 2.json");
}

#[test]
fn numbers_up_to_the_limit_are_written_in_full() {
    assert_eq!(MAX_NUMBERED, 9_999);
    assert_eq!(n("a.txt", MAX_NUMBERED, false, Os::Windows), "a (9999).txt");
    assert_eq!(n("a.txt", MAX_NUMBERED, false, Os::MacOs), "a 9999.txt");
}

#[test]
fn a_name_that_is_not_one_path_component_keeps_every_character() {
    assert_eq!(n("dir/a.txt", 2, false, Os::Windows), "dir/a.txt (2)");
    assert_eq!(n("..", 2, false, Os::Windows), ".. (2)");
}

#[test]
fn temp_names_carry_the_job_id() {
    let name = temp_name(JobId(0xab), 7);
    assert_eq!(name, ".rf-00000000000000ab-7.rfpart");
    assert_eq!(temp_job(&name), Some(JobId(0xab)));
    for other in [
        "x.rfpart",
        ".rf-zz-1.rfpart",
        ".rf-00000000000000ab-7.txt",
        ".rf-00000000000000ab-.rfpart",
        "a.txt",
    ] {
        assert_eq!(temp_job(OsStr::new(other)), None, "{other}");
    }
}

#[test]
fn the_temp_suffix_is_part_of_the_name() {
    assert_eq!(TEMP_SUFFIX, ".rfpart");
    assert!(
        temp_name(JobId(1), 0)
            .to_string_lossy()
            .ends_with(TEMP_SUFFIX)
    );
}

#[test]
fn temp_job_only_matches_names_temp_name_wrote() {
    for other in [
        // Upper-case or mixed-case spellings of the id, prefix or suffix.
        ".rf-00000000000000AB-7.rfpart",
        ".rf-00000000000000aB-7.rfpart",
        ".RF-00000000000000ab-7.rfpart",
        ".rf-00000000000000ab-7.RFPART",
        ".rf-00000000000000ab-7.RfPart",
        // The id is exactly 16 digits.
        ".rf-0000000000000ab-7.rfpart",
        ".rf-000000000000000ab-7.rfpart",
        ".rf-+0000000000000ab-7.rfpart",
        ".rf- 0000000000000ab-7.rfpart",
        // The sequence is a plain decimal number: no sign, no leading zeros, fits in 64 bits.
        ".rf-00000000000000ab-.rfpart",
        ".rf-00000000000000ab-+7.rfpart",
        ".rf-00000000000000ab--7.rfpart",
        ".rf-00000000000000ab-07.rfpart",
        ".rf-00000000000000ab-00.rfpart",
        ".rf-00000000000000ab-7a.rfpart",
        ".rf-00000000000000ab-٧.rfpart",
        ".rf-00000000000000ab-18446744073709551616.rfpart",
        ".rf-00000000000000ab-99999999999999999999999.rfpart",
        // Nothing before, after or between.
        "x.rf-00000000000000ab-7.rfpart",
        " .rf-00000000000000ab-7.rfpart",
        ".rf-00000000000000ab-7.rfpart ",
        ".rf-00000000000000ab-7.rfpart.rfpart",
        ".rf-00000000000000ab-7-8.rfpart",
        ".rf-00000000000000ab7.rfpart",
        "rf-00000000000000ab-7.rfpart",
        ".rf-00000000000000ab-7",
        ".rf-.rfpart",
        ".rfpart",
        "",
    ] {
        assert_eq!(temp_job(OsStr::new(other)), None, "{other:?}");
    }
    // The largest sequence is still a temp name.
    let last = format!(".rf-00000000000000ab-{}.rfpart", u64::MAX);
    assert_eq!(temp_job(OsStr::new(&last)), Some(JobId(0xab)));
    assert_eq!(
        temp_job(OsStr::new(".rf-00000000000000ab-0.rfpart")),
        Some(JobId(0xab))
    );
}

#[test]
fn temp_names_fit_every_file_system() {
    let longest = temp_name(JobId(u64::MAX), u64::MAX);
    assert!(longest.len() <= 255, "{} bytes", longest.len());
    assert!(longest.to_str().is_some_and(str::is_ascii));
}

proptest::proptest! {
    #[test]
    fn temp_names_roundtrip(id in proptest::prelude::any::<u64>(), seq in proptest::prelude::any::<u64>()) {
        proptest::prop_assert_eq!(temp_job(&temp_name(JobId(id), seq)), Some(JobId(id)));
    }

    #[test]
    fn temp_job_accepts_exactly_the_canonical_sequence(
        id in proptest::prelude::any::<u64>(),
        seq in "[0-9+]{0,22}",
    ) {
        let name = format!(".rf-{id:016x}-{seq}{TEMP_SUFFIX}");
        let canonical = seq.parse::<u64>().is_ok_and(|value| value.to_string() == seq);
        let expected = canonical.then_some(JobId(id));
        proptest::prop_assert_eq!(temp_job(OsStr::new(&name)), expected, "{}", name);
    }

    #[test]
    fn temp_job_never_matches_other_spellings_of_the_id(
        id in proptest::prelude::any::<u64>(),
        seq in proptest::prelude::any::<u64>(),
    ) {
        let lower = format!("{id:016x}");
        let upper = lower.to_uppercase();
        proptest::prop_assume!(upper != lower);
        let name = format!(".rf-{upper}-{seq}{TEMP_SUFFIX}");
        proptest::prop_assert_eq!(temp_job(OsStr::new(&name)), None, "{}", name);
    }

    #[test]
    fn numbered_names_keep_the_stem_and_the_extension(
        stem in "[a-z0-9 _()-]{1,20}",
        ext in "[a-z0-9]{1,8}",
        k in 2..=MAX_NUMBERED,
    ) {
        let name = format!("{stem}.{ext}");
        let windows = n(&name, k, false, Os::Windows);
        let mac = n(&name, k, false, Os::MacOs);
        proptest::prop_assert_eq!(windows, OsString::from(format!("{stem} ({k}).{ext}")));
        proptest::prop_assert_eq!(mac, OsString::from(format!("{stem} {k}.{ext}")));
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_kept() {
    use std::os::unix::ffi::OsStrExt;
    let got = numbered_name(OsStr::from_bytes(b"\xff.bin"), 2, false, Os::Windows);
    assert_eq!(got.as_bytes(), b"\xff (2).bin");
    let got = numbered_name(OsStr::from_bytes(b"a.\xfe"), 2, false, Os::MacOs);
    assert_eq!(got.as_bytes(), b"a 2.\xfe");
    let got = numbered_name(OsStr::from_bytes(b"\xff"), 2, true, Os::Other);
    assert_eq!(got.as_bytes(), b"\xff (2)");
}

#[cfg(unix)]
#[test]
fn temp_job_ignores_names_that_are_not_text() {
    use std::os::unix::ffi::OsStrExt;
    assert_eq!(
        temp_job(OsStr::from_bytes(b".rf-00000000000000ab-7\xff.rfpart")),
        None
    );
    assert_eq!(
        temp_job(OsStr::from_bytes(b".rf-00000000000000a\xff-7.rfpart")),
        None
    );
}
