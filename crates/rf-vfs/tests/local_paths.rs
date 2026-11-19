use std::collections::BTreeSet;
use std::ffi::OsString;

use rf_core::VPath;
use rf_vfs::{DeleteMode, LocalProvider, Provider, RenameMode, WriteMode, testkit};

fn tmp() -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path()).unwrap();
    (t, v)
}

fn roundtrip(p: &dyn Provider, dir: &VPath, name: &OsString) {
    let path = dir.join(name).unwrap();
    testkit::write_all(p, &path, WriteMode::CreateNew, b"x");
    assert_eq!(
        testkit::names(p, dir),
        BTreeSet::from([name.clone()]),
        "{}",
        name.display()
    );
    assert_eq!(&p.stat(&path).unwrap().name, name);
    let other = dir.join("renamed").unwrap();
    p.rename(&path, &other, RenameMode::NoReplace).unwrap();
    p.rename(&other, &path, RenameMode::NoReplace).unwrap();
    assert_eq!(testkit::read_all(p, &path), b"x");
    p.delete(&path, DeleteMode::Permanent).unwrap();
    assert_eq!(testkit::names(p, dir), BTreeSet::new());
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[test]
    fn reserved_and_trailing_names_roundtrip() {
        let (_t, d) = tmp();
        for name in ["CON", "aux.txt", "foo.", "bar "] {
            roundtrip(&LocalProvider::new(), &d, &OsString::from(name));
        }
    }

    #[test]
    fn very_long_path_over_1000_units() {
        let (_t, d) = tmp();
        let p = LocalProvider::new();
        let segment = |i: usize| format!("{i}-{}", "가나다라마바사아자차".repeat(24));
        let mut deep = d.clone();
        for i in 0..5 {
            deep = deep.join(segment(i)).unwrap();
            p.create_dir(&deep).unwrap();
        }
        assert!(deep.as_local().unwrap().as_os_str().encode_wide().count() > 1_000);
        roundtrip(&p, &deep, &OsString::from("끝.txt"));
        p.delete(&d.join(segment(0)).unwrap(), DeleteMode::Permanent)
            .unwrap();
    }

    #[test]
    fn lone_surrogate_name_roundtrips() {
        let (_t, d) = tmp();
        roundtrip(
            &LocalProvider::new(),
            &d,
            &OsString::from_wide(&[0xD800, u16::from(b'x')]),
        );
    }
}

#[cfg(unix)]
#[test]
fn unix_only_characters_roundtrip() {
    let (_t, d) = tmp();
    for name in [r"back\slash", "co:lon", "star*?", "<angle>|pipe\"quote"] {
        roundtrip(&LocalProvider::new(), &d, &OsString::from(name));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_name_roundtrips() {
    use std::os::unix::ffi::OsStrExt;
    let (_t, d) = tmp();
    roundtrip(
        &LocalProvider::new(),
        &d,
        &std::ffi::OsStr::from_bytes(b"\xff\xfe.bin").to_owned(),
    );
}
