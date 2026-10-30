#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

use std::ffi::{OsStr, OsString};

use rf_core::{Os, Scheme, VPath};
use rf_jobs::preflight::{
    FAT32_MAX_FILE, MACOS_PATH_LIMIT, NAME_LIMIT, OTHER_PATH_LIMIT, WINDOWS_PATH_LIMIT,
};
use rf_jobs::{
    CaseIndex, FsKind, Issue, IssueKind, Severity, check_name, check_path_len, check_size,
};
use rf_vfs::NameFolding;

#[test]
fn each_file_system_folds_names_by_its_own_rule() {
    use NameFolding::{Case, CaseAndForm, Exact};
    let rules = [
        (FsKind::Ntfs, Case),
        (FsKind::ReFs, Case),
        (FsKind::Fat32, Case),
        (FsKind::ExFat, Case),
        (FsKind::Apfs, CaseAndForm),
        (FsKind::HfsPlus, CaseAndForm),
        (FsKind::Smb, Case),
        (FsKind::Other, Exact),
    ];
    assert_eq!(rules.map(|(fs, _)| fs), ALL_KINDS, "every kind, once");
    for (fs, folding) in rules {
        assert_eq!(fs.folding(), folding, "{fs:?}");
        // Case folds exactly where the file system ignores case.
        assert_eq!(folding.ignores_case(), fs.case_insensitive(), "{fs:?}");
    }
}

const ALL_KINDS: [FsKind; 8] = [
    FsKind::Ntfs,
    FsKind::ReFs,
    FsKind::Fat32,
    FsKind::ExFat,
    FsKind::Apfs,
    FsKind::HfsPlus,
    FsKind::Smb,
    FsKind::Other,
];

/// The file systems whose names are counted in UTF-16 units.
const UTF16_KINDS: [FsKind; 6] = [
    FsKind::Ntfs,
    FsKind::ReFs,
    FsKind::Fat32,
    FsKind::ExFat,
    FsKind::Smb,
    FsKind::HfsPlus,
];

fn names(name: &str, fs: FsKind) -> Vec<IssueKind> {
    check_name(OsStr::new(name), fs)
}

/// Asserts that `name` raises no issue on `fs`.
fn ok(name: &str, fs: FsKind) {
    assert_eq!(
        names(name, fs),
        Vec::<IssueKind>::new(),
        "{name:?} on {fs:?}"
    );
}

fn too_long(len: usize) -> Vec<IssueKind> {
    vec![IssueKind::NameTooLong {
        len,
        max: NAME_LIMIT,
    }]
}

fn local_root() -> VPath {
    VPath::local(if cfg!(windows) { r"C:\" } else { "/" }).unwrap()
}

/// A local path whose `Display` text is exactly `len` characters of ASCII.
fn path_of_len(len: usize) -> VPath {
    let root = local_root();
    let mut left = len + 1 - root.to_string().len(); // every segment costs its length + 1
    let mut path = root;
    while left > 0 {
        let cost = if left > 103 { 101 } else { left };
        path = path.join("a".repeat(cost - 1)).unwrap();
        left -= cost;
    }
    assert_eq!(path.to_string().len(), len);
    path
}

#[test]
fn fs_names_map_to_kinds() {
    let cases = [
        (Some("NTFS"), FsKind::Ntfs),
        (Some("ReFS"), FsKind::ReFs),
        (Some("FAT32"), FsKind::Fat32),
        (Some("msdos"), FsKind::Fat32),
        (Some("exFAT"), FsKind::ExFat),
        (Some("exfat"), FsKind::ExFat),
        (Some("apfs"), FsKind::Apfs),
        (Some("hfs"), FsKind::HfsPlus),
        (Some("smbfs"), FsKind::Smb),
        (Some("ext4"), FsKind::Other),
        (None, FsKind::Other),
    ];
    for (name, kind) in cases {
        assert_eq!(FsKind::from_fs_name(name), kind, "{name:?}");
    }
    assert!(
        FsKind::ExFat.windows_names()
            && FsKind::Smb.windows_names()
            && !FsKind::Apfs.windows_names()
    );
    assert!(FsKind::Apfs.case_insensitive() && !FsKind::Other.case_insensitive());
    assert_eq!(
        (FsKind::Fat32.max_file_size(), FsKind::ExFat.max_file_size()),
        (Some(4_294_967_295), None)
    );
}

#[test]
fn windows_family_rejects_characters_once_each() {
    assert_eq!(
        names("a?b*c?.txt", FsKind::ExFat),
        [
            IssueKind::InvalidCharacter('?'),
            IssueKind::InvalidCharacter('*')
        ]
    );
    assert_eq!(
        names("tab\there", FsKind::Ntfs),
        [IssueKind::InvalidCharacter('\t')]
    );
    assert_eq!(names(r#"<>:"\|"#, FsKind::Smb).len(), 6);
    ok("a?b.txt", FsKind::Apfs);
    ok("a?b.txt", FsKind::Other);
}

#[test]
fn reserved_names_and_trailing_dots_are_warnings() {
    for name in [
        "CON",
        "con.txt",
        "Nul.tar.gz",
        "COM1",
        "lpt9.log",
        "AUX .txt",
    ] {
        assert_eq!(
            names(name, FsKind::Ntfs),
            [IssueKind::ReservedName],
            "{name}"
        );
    }
    for name in ["CONSOLE", "COM10", "xcon", "COM0"] {
        ok(name, FsKind::Ntfs);
    }
    assert_eq!(names("foo.", FsKind::Ntfs), [IssueKind::TrailingDotOrSpace]);
    assert_eq!(
        names("bar ", FsKind::Fat32),
        [IssueKind::TrailingDotOrSpace]
    );
    ok("CON", FsKind::Apfs);
    assert_eq!(IssueKind::ReservedName.severity(), Severity::Warning);
    assert_eq!(IssueKind::TrailingDotOrSpace.severity(), Severity::Warning);
    assert_eq!(
        IssueKind::InvalidCharacter('?').severity(),
        Severity::Blocking
    );
}

#[test]
fn name_length_counts_units_by_file_system() {
    let hangul = "가".repeat(86); // 258 UTF-8 bytes, 86 UTF-16 units
    assert_eq!(
        names(&hangul, FsKind::Apfs),
        [IssueKind::NameTooLong { len: 258, max: 255 }]
    );
    ok(&hangul, FsKind::Ntfs);
    ok(&hangul, FsKind::HfsPlus);
    assert_eq!(
        names(&"a".repeat(256), FsKind::ExFat),
        [IssueKind::NameTooLong { len: 256, max: 255 }]
    );
    ok(&"a".repeat(255), FsKind::Other);
}

#[test]
fn fat32_limits_file_size() {
    assert_eq!(check_size(FAT32_MAX_FILE, FsKind::Fat32), None);
    let big = FAT32_MAX_FILE + 1;
    assert_eq!(
        check_size(big, FsKind::Fat32),
        Some(IssueKind::FileTooLarge {
            size: big,
            max: FAT32_MAX_FILE
        })
    );
    assert_eq!(check_size(u64::MAX, FsKind::ExFat), None);
    assert_eq!(
        IssueKind::FileTooLarge { size: 1, max: 0 }.severity(),
        Severity::Blocking
    );
}

#[test]
fn path_length_limits_follow_the_os() {
    let root = VPath::local(if cfg!(windows) { r"C:\" } else { "/" }).unwrap();
    let deep = |units: usize| {
        (0..=units / 101).fold(root.clone(), |p, _| p.join("a".repeat(100)).unwrap())
    };
    let mac = deep(MACOS_PATH_LIMIT + 1);
    assert!(matches!(
        check_path_len(&mac, Os::MacOs),
        Some(IssueKind::PathTooLong {
            max: MACOS_PATH_LIMIT,
            ..
        })
    ));
    assert_eq!(check_path_len(&mac, Os::Windows), None);
    assert!(matches!(
        check_path_len(&deep(OTHER_PATH_LIMIT + 1), Os::Other),
        Some(IssueKind::PathTooLong {
            max: OTHER_PATH_LIMIT,
            ..
        })
    ));
    assert!(matches!(
        check_path_len(&deep(WINDOWS_PATH_LIMIT + 1), Os::Windows),
        Some(IssueKind::PathTooLong {
            max: WINDOWS_PATH_LIMIT,
            ..
        })
    ));
    assert_eq!(
        check_path_len(
            &VPath::new(Scheme::Remote, "p", &["x"]).unwrap(),
            Os::Windows
        ),
        None
    );
}

#[test]
fn case_collisions_follow_the_file_system() {
    let dir = VPath::local(if cfg!(windows) { r"C:\d" } else { "/d" }).unwrap();
    let nfc = OsString::from("한글.txt");
    let nfd = OsString::from("\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}.txt");
    let mut ntfs = CaseIndex::new(FsKind::Ntfs);
    assert_eq!(ntfs.insert(&dir, OsStr::new("Readme")), None);
    assert_eq!(
        ntfs.insert(&dir, OsStr::new("README")),
        Some(OsString::from("Readme"))
    );
    assert_eq!(
        ntfs.insert(&dir.join("sub").unwrap(), OsStr::new("README")),
        None
    );
    assert_eq!(
        (ntfs.insert(&dir, &nfc), ntfs.insert(&dir, &nfd)),
        (None, None)
    );
    let mut apfs = CaseIndex::new(FsKind::Apfs);
    assert_eq!(apfs.insert(&dir, &nfc), None);
    assert_eq!(apfs.insert(&dir, &nfd), Some(nfc.clone()));
    let mut ext4 = CaseIndex::new(FsKind::Other);
    assert_eq!(
        (
            ext4.insert(&dir, OsStr::new("a")),
            ext4.insert(&dir, OsStr::new("A"))
        ),
        (None, None)
    );
    assert_eq!(
        IssueKind::CaseCollision { other: nfc }.severity(),
        Severity::Blocking
    );
}

#[test]
fn issues_have_log_messages() {
    assert_eq!(
        IssueKind::InvalidCharacter('?').to_string(),
        "name contains '?', which the target file system does not allow"
    );
    assert_eq!(
        IssueKind::FileTooLarge { size: 5, max: 4 }.to_string(),
        "file is 5 bytes; the target file system allows 4"
    );
    assert_eq!(
        IssueKind::NotEnoughSpace {
            needed: 10,
            available: 3
        }
        .to_string(),
        "needs 10 bytes; 3 available"
    );
}

// The tests below pin the edges of every rule: the limit itself and the value just past it.

#[test]
fn fs_name_matching_is_ascii_case_insensitive_and_exact() {
    let cases = [
        ("ntfs", FsKind::Ntfs),
        ("Ntfs", FsKind::Ntfs),
        ("refs", FsKind::ReFs),
        ("REFS", FsKind::ReFs),
        ("FAT", FsKind::Fat32),
        ("fat", FsKind::Fat32),
        ("FAT12", FsKind::Fat32),
        ("FAT16", FsKind::Fat32),
        ("fat32", FsKind::Fat32),
        ("MSDOS", FsKind::Fat32),
        ("EXFAT", FsKind::ExFat),
        ("APFS", FsKind::Apfs),
        ("HFS", FsKind::HfsPlus),
        ("SMBFS", FsKind::Smb),
        ("", FsKind::Other),
        (" NTFS", FsKind::Other),
        ("NTFS ", FsKind::Other),
        ("FAT64", FsKind::Other),
        ("hfsx", FsKind::Other),
        ("ＮＴＦＳ", FsKind::Other),
    ];
    for (name, kind) in cases {
        assert_eq!(FsKind::from_fs_name(Some(name)), kind, "{name:?}");
    }
}

#[test]
fn every_kind_answers_the_capability_questions() {
    for kind in ALL_KINDS {
        let windows = matches!(
            kind,
            FsKind::Ntfs | FsKind::ReFs | FsKind::Fat32 | FsKind::ExFat | FsKind::Smb
        );
        assert_eq!(kind.windows_names(), windows, "{kind:?}");
        assert_eq!(kind.case_insensitive(), kind != FsKind::Other, "{kind:?}");
        let max = (kind == FsKind::Fat32).then_some(FAT32_MAX_FILE);
        assert_eq!(kind.max_file_size(), max, "{kind:?}");
    }
    assert_eq!(FAT32_MAX_FILE, u64::from(u32::MAX));
}

#[test]
fn control_characters_are_blocked_from_u0001_to_u001f_only() {
    for c in ['\u{1}', '\u{8}', '\u{a}', '\u{1f}'] {
        let name = format!("a{c}b");
        assert_eq!(
            names(&name, FsKind::Ntfs),
            [IssueKind::InvalidCharacter(c)],
            "U+{:04X}",
            u32::from(c)
        );
    }
    for c in [' ', '\u{7f}', '\u{80}', '\u{a0}', '가'] {
        ok(&format!("a{c}b"), FsKind::Ntfs);
    }
}

#[test]
fn each_forbidden_character_is_reported_alone() {
    for c in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
        let name = format!("a{c}b");
        for kind in UTF16_KINDS.into_iter().filter(|k| k.windows_names()) {
            assert_eq!(
                names(&name, kind),
                [IssueKind::InvalidCharacter(c)],
                "{c} {kind:?}"
            );
        }
        ok(&name, FsKind::Apfs);
        ok(&name, FsKind::HfsPlus);
        ok(&name, FsKind::Other);
    }
}

#[test]
fn characters_are_reported_in_the_order_they_first_appear() {
    assert_eq!(
        names("*a|b*c|d<", FsKind::Ntfs),
        [
            IssueKind::InvalidCharacter('*'),
            IssueKind::InvalidCharacter('|'),
            IssueKind::InvalidCharacter('<'),
        ]
    );
}

#[test]
fn reserved_names_cover_exactly_the_documented_devices() {
    let mut reserved = vec![
        "CON".to_owned(),
        "PRN".into(),
        "AUX".into(),
        "NUL".into(),
        "CONIN$".into(),
        "CONOUT$".into(),
    ];
    for n in 1..=9 {
        reserved.push(format!("COM{n}"));
        reserved.push(format!("LPT{n}"));
    }
    // The superscript digits one, two and three, which Windows takes for 1, 2 and 3.
    for digit in ['\u{b9}', '\u{b2}', '\u{b3}'] {
        reserved.push(format!("COM{digit}"));
        reserved.push(format!("LPT{digit}"));
    }
    for name in &reserved {
        for spelling in [name.clone(), name.to_lowercase(), format!("{name}.txt")] {
            assert_eq!(
                names(&spelling, FsKind::ReFs),
                [IssueKind::ReservedName],
                "{spelling}"
            );
        }
    }
    assert_eq!(
        names("cOm9", FsKind::Smb),
        [IssueKind::ReservedName],
        "mixed case"
    );
    for name in [
        "COM0",
        "LPT0",
        "COM10",
        "LPT10",
        "CO",
        "COM",
        "LPT",
        "CONN",
        "PR",
        "NU",
        "AU",
        "CON1",
        ".con",
        " CON",
        "ＣＯＮ",
        "COM１",
        "CÓN",
        "CONIN",
        "CONOUT",
        "CONIN$$",
        "COM\u{2074}",
        "LPT\u{2070}",
        "COM\u{b9}\u{b9}",
    ] {
        ok(name, FsKind::Ntfs);
    }
    // Only the part before the first dot counts, with trailing spaces removed.
    assert_eq!(names("con .txt", FsKind::Ntfs), [IssueKind::ReservedName]);
    ok("con.", FsKind::Apfs);
    ok("x.con", FsKind::Ntfs);
    ok("con x.txt", FsKind::Ntfs);
}

#[test]
fn trailing_dot_or_space_only_looks_at_the_last_character() {
    for name in ["foo.", "foo ", "foo. ", "foo ."] {
        assert_eq!(
            names(name, FsKind::ExFat),
            [IssueKind::TrailingDotOrSpace],
            "{name:?}"
        );
        ok(name, FsKind::Other);
        ok(name, FsKind::HfsPlus);
    }
    for name in [".foo", " foo", "foo.bar", "foo\u{a0}", "foo\u{3000}"] {
        ok(name, FsKind::ExFat);
    }
    // A reserved name that also ends in a dot or space raises both warnings, reserved first.
    assert_eq!(
        names("con.", FsKind::Ntfs),
        [IssueKind::ReservedName, IssueKind::TrailingDotOrSpace]
    );
    assert_eq!(
        names("con ", FsKind::Ntfs),
        [IssueKind::ReservedName, IssueKind::TrailingDotOrSpace]
    );
}

#[test]
fn issues_come_in_the_documented_order() {
    let name = format!("CON .:{}.", "x".repeat(260));
    assert_eq!(
        names(&name, FsKind::Ntfs),
        [
            IssueKind::InvalidCharacter(':'),
            IssueKind::ReservedName,
            IssueKind::TrailingDotOrSpace,
            IssueKind::NameTooLong { len: 267, max: 255 },
        ]
    );
    // Off the Windows family only the length is looked at.
    assert_eq!(names(&name, FsKind::Apfs), too_long(267));
}

#[test]
fn name_length_edges_in_utf16_units() {
    let smile = "😀"; // 2 UTF-16 units, 4 UTF-8 bytes
    for kind in UTF16_KINDS {
        ok(&"a".repeat(NAME_LIMIT), kind);
        assert_eq!(
            names(&"a".repeat(NAME_LIMIT + 1), kind),
            too_long(256),
            "{kind:?}"
        );
        // 127 pairs and one letter make exactly 255 units, one more pair makes 257.
        ok(&format!("{}a", smile.repeat(127)), kind);
        assert_eq!(names(&smile.repeat(128), kind), too_long(256), "{kind:?}");
        assert_eq!(
            names(&format!("{}a", smile.repeat(128)), kind),
            too_long(257)
        );
        ok(&"가".repeat(NAME_LIMIT), kind);
        assert_eq!(names(&"가".repeat(NAME_LIMIT + 1), kind), too_long(256));
    }
}

#[test]
fn name_length_edges_in_utf8_bytes() {
    let smile = "😀";
    for kind in [FsKind::Apfs, FsKind::Other] {
        ok(&"a".repeat(NAME_LIMIT), kind);
        assert_eq!(
            names(&"a".repeat(NAME_LIMIT + 1), kind),
            too_long(256),
            "{kind:?}"
        );
        // 63 emoji and three letters are exactly 255 bytes; 64 emoji are 256.
        ok(&format!("{}abc", smile.repeat(63)), kind);
        assert_eq!(names(&smile.repeat(64), kind), too_long(256), "{kind:?}");
        // 85 Hangul syllables are 255 bytes, 86 are 258.
        ok(&"가".repeat(85), kind);
        assert_eq!(names(&"가".repeat(86), kind), too_long(258), "{kind:?}");
        // Two-byte letters: 127 are 254 bytes plus one letter makes 255, 128 make 256.
        ok(&format!("{}a", "é".repeat(127)), kind);
        assert_eq!(names(&"é".repeat(128), kind), too_long(256), "{kind:?}");
    }
}

#[cfg(unix)]
#[test]
fn bytes_that_are_not_text_count_once_each() {
    use std::os::unix::ffi::OsStrExt;

    // The kernel counts raw bytes, so 255 invalid bytes fit and 256 do not, whatever a lossy
    // conversion would make of them.
    let fits = vec![0xFF; NAME_LIMIT];
    let over = vec![0xFF; NAME_LIMIT + 1];
    assert_eq!(
        check_name(OsStr::from_bytes(&fits), FsKind::Other),
        Vec::<IssueKind>::new()
    );
    assert_eq!(
        check_name(OsStr::from_bytes(&over), FsKind::Other),
        too_long(256)
    );
}

#[test]
fn file_size_edges() {
    for size in [0, 1, FAT32_MAX_FILE - 1, FAT32_MAX_FILE] {
        assert_eq!(check_size(size, FsKind::Fat32), None, "{size}");
    }
    for size in [FAT32_MAX_FILE + 1, FAT32_MAX_FILE * 2, u64::MAX] {
        assert_eq!(
            check_size(size, FsKind::Fat32),
            Some(IssueKind::FileTooLarge {
                size,
                max: FAT32_MAX_FILE
            }),
            "{size}"
        );
    }
    for kind in ALL_KINDS.into_iter().filter(|&k| k != FsKind::Fat32) {
        assert_eq!(check_size(u64::MAX, kind), None, "{kind:?}");
    }
}

#[test]
fn path_length_edges_per_os() {
    for (os, limit) in [
        (Os::Windows, WINDOWS_PATH_LIMIT),
        (Os::MacOs, MACOS_PATH_LIMIT),
        (Os::Other, OTHER_PATH_LIMIT),
    ] {
        assert_eq!(
            check_path_len(&path_of_len(limit), os),
            None,
            "{os:?} at the limit"
        );
        assert_eq!(
            check_path_len(&path_of_len(limit + 1), os),
            Some(IssueKind::PathTooLong {
                len: limit + 1,
                max: limit
            }),
            "{os:?} past the limit"
        );
    }
    assert_eq!(MACOS_PATH_LIMIT, 1_023);
    assert_eq!(OTHER_PATH_LIMIT, 4_095);
    assert_eq!(WINDOWS_PATH_LIMIT, 32_000);
}

#[test]
fn path_length_counts_units_by_os() {
    let root_len = local_root().to_string().len();
    // 80 folders of 100 emoji (2 UTF-16 units, 4 UTF-8 bytes each) with 79 separators: far
    // over 32,000 bytes, yet only about half that many units, which is what Windows counts.
    let mut emoji = local_root();
    for _ in 0..80 {
        emoji = emoji.join("😀".repeat(100)).unwrap();
    }
    let shown = emoji.to_string();
    let (units, bytes) = (shown.encode_utf16().count(), shown.len());
    assert_eq!((units, bytes), (root_len + 16_079, root_len + 32_079));
    assert_eq!(check_path_len(&emoji, Os::Windows), None);
    for (os, limit) in [(Os::MacOs, MACOS_PATH_LIMIT), (Os::Other, OTHER_PATH_LIMIT)] {
        let issue = IssueKind::PathTooLong {
            len: bytes,
            max: limit,
        };
        assert_eq!(check_path_len(&emoji, os), Some(issue), "{os:?}");
    }
    // Three folders of 85 Hangul syllables (255 bytes, 85 units each) and a last name of
    // letters make exactly 1,023 bytes, the macOS limit, in only 513 units.
    let mut hangul = local_root();
    for _ in 0..3 {
        hangul = hangul.join("가".repeat(85)).unwrap();
    }
    let last = 1_023 - root_len - 3 * 255 - 3; // three separators
    let fits = hangul.join("a".repeat(last)).unwrap();
    assert_eq!(fits.to_string().len(), MACOS_PATH_LIMIT);
    assert_eq!(check_path_len(&fits, Os::MacOs), None);
    let over = hangul.join("a".repeat(last + 1)).unwrap();
    assert_eq!(
        check_path_len(&over, Os::MacOs),
        Some(IssueKind::PathTooLong {
            len: 1_024,
            max: MACOS_PATH_LIMIT
        })
    );
    assert_eq!(check_path_len(&over, Os::Windows), None);
    assert_eq!(check_path_len(&over, Os::Other), None);
}

#[test]
fn only_local_paths_have_a_length_to_check() {
    let long = "x".repeat(40_000);
    let remote = VPath::new(Scheme::Remote, &long, &[long.as_str()]).unwrap();
    for os in [Os::Windows, Os::MacOs, Os::Other] {
        assert_eq!(check_path_len(&remote, os), None, "{os:?}");
    }
}

#[test]
fn case_index_edges() {
    let dir = local_root().join("d").unwrap();
    let other_dir = local_root().join("e").unwrap();
    for kind in ALL_KINDS.into_iter().filter(|&k| k != FsKind::Other) {
        let mut index = CaseIndex::new(kind);
        assert_eq!(index.insert(&dir, OsStr::new("Report")), None, "{kind:?}");
        // The very same name is no collision; it is the conflict policy's business.
        assert_eq!(index.insert(&dir, OsStr::new("Report")), None, "{kind:?}");
        let first = Some(OsString::from("Report"));
        assert_eq!(index.insert(&dir, OsStr::new("report")), first, "{kind:?}");
        // Every later spelling points at the first one.
        assert_eq!(index.insert(&dir, OsStr::new("REPORT")), first, "{kind:?}");
        assert_eq!(index.insert(&dir, OsStr::new("Reports")), None, "{kind:?}");
        // Another folder starts empty.
        assert_eq!(
            index.insert(&other_dir, OsStr::new("REPORT")),
            None,
            "{kind:?}"
        );
    }
    let mut ext4 = CaseIndex::new(FsKind::Other);
    for name in ["Report", "report", "REPORT", "Report"] {
        assert_eq!(ext4.insert(&dir, OsStr::new(name)), None, "{name}");
    }
}

#[test]
fn case_index_folds_non_ascii_letters_and_only_normalizes_on_apple_systems() {
    let dir = local_root().join("d").unwrap();
    let composed = OsString::from("Caf\u{c9}"); // É
    let lower_decomposed = OsString::from("cafe\u{301}"); // e + combining acute
    for kind in [FsKind::Ntfs, FsKind::ExFat, FsKind::Smb] {
        let mut index = CaseIndex::new(kind);
        assert_eq!(index.insert(&dir, &composed), None);
        assert_eq!(
            index.insert(&dir, OsStr::new("caf\u{e9}")),
            Some(composed.clone())
        );
        // Without normalization the decomposed spelling is a different name.
        assert_eq!(index.insert(&dir, &lower_decomposed), None, "{kind:?}");
        // Lower-casing is not full case folding: the sharp s stays apart from `ss`.
        assert_eq!(
            index.insert(&dir, OsStr::new("stra\u{df}e")),
            None,
            "{kind:?}"
        );
        assert_eq!(index.insert(&dir, OsStr::new("STRASSE")), None, "{kind:?}");
    }
    for kind in [FsKind::Apfs, FsKind::HfsPlus] {
        let mut index = CaseIndex::new(kind);
        assert_eq!(index.insert(&dir, &composed), None);
        assert_eq!(
            index.insert(&dir, &lower_decomposed),
            Some(composed.clone()),
            "{kind:?}"
        );
    }
}

#[test]
fn every_issue_has_a_severity_and_a_log_message() {
    let other = OsString::from("Readme");
    let rows = [
        (
            IssueKind::InvalidCharacter('*'),
            Severity::Blocking,
            "name contains '*', which the target file system does not allow",
        ),
        (
            IssueKind::ReservedName,
            Severity::Warning,
            "name is reserved on Windows",
        ),
        (
            IssueKind::TrailingDotOrSpace,
            Severity::Warning,
            "name ends with a dot or space",
        ),
        (
            IssueKind::NameTooLong { len: 258, max: 255 },
            Severity::Blocking,
            "name is 258 units long; the limit is 255",
        ),
        (
            IssueKind::PathTooLong {
                len: 1_024,
                max: 1_023,
            },
            Severity::Blocking,
            "path is 1024 units long; the limit is 1023",
        ),
        (
            IssueKind::FileTooLarge {
                size: 4_294_967_296,
                max: 4_294_967_295,
            },
            Severity::Blocking,
            "file is 4294967296 bytes; the target file system allows 4294967295",
        ),
        (
            IssueKind::CaseCollision {
                other: other.clone(),
            },
            Severity::Blocking,
            "name differs only in case from Readme",
        ),
        (
            IssueKind::NotEnoughSpace {
                needed: 10,
                available: 3,
            },
            Severity::Blocking,
            "needs 10 bytes; 3 available",
        ),
    ];
    for (kind, severity, message) in rows {
        assert_eq!(kind.severity(), severity, "{kind:?}");
        assert_eq!(kind.to_string(), message, "{kind:?}");
    }
}

#[test]
fn an_issue_pairs_a_target_with_what_is_wrong() {
    let target = local_root().join("a?").unwrap();
    let issue = Issue {
        target: target.clone(),
        kind: IssueKind::InvalidCharacter('?'),
    };
    assert_eq!(issue.clone(), issue);
    assert_eq!(issue.target, target);
    assert_ne!(
        issue,
        Issue {
            target,
            kind: IssueKind::ReservedName
        }
    );
}

/// A Unix path that is not text is counted by its raw bytes, each byte once, as the kernel
/// counts it: never by the lossy text, which shows each invalid byte as a `U+FFFD` of three.
#[cfg(unix)]
#[test]
fn a_path_that_is_not_text_counts_its_raw_bytes() {
    use std::os::unix::ffi::OsStrExt;

    let name = |len: usize| OsStr::from_bytes(&vec![0xff; len]).to_owned();
    let mut folder = local_root();
    for _ in 0..3 {
        folder = folder.join(name(255)).unwrap();
    }
    // The root, three names of 255 bytes and one of 254, and three separators.
    let at_limit = folder.join(name(254)).unwrap();
    assert_eq!(check_path_len(&at_limit, Os::MacOs), None);
    let past = folder.join(name(255)).unwrap();
    assert_eq!(
        check_path_len(&past, Os::MacOs),
        Some(IssueKind::PathTooLong {
            len: MACOS_PATH_LIMIT + 1,
            max: MACOS_PATH_LIMIT
        })
    );
}
