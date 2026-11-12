use std::path::Path;
use std::time::{Duration, SystemTime};

use rf_core::{Attributes, ErrorKind, Scheme, VPath};
use rf_platform::{copy_extras, entry_attributes, restore_from_trash, set_attributes, set_times};

const T1: u64 = 1_000_000_000;
const T2: u64 = 1_100_000_000;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn tmp_file(name: &str, body: &[u8]) -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join(name), body).unwrap();
    let v = VPath::local(t.path().join(name)).unwrap();
    (t, v)
}

fn attrs(v: &VPath) -> Attributes {
    let p = v.as_local().unwrap();
    entry_attributes(
        p.file_name().unwrap(),
        &std::fs::symlink_metadata(p).unwrap(),
    )
}

#[test]
fn set_times_sets_modified_and_accessed() {
    let (_t, f) = tmp_file("f.txt", b"x");
    set_times(&f, Some(at(T1)), Some(at(T2)), None).unwrap();
    let m = std::fs::metadata(f.as_local().unwrap()).unwrap();
    assert_eq!(
        (m.modified().unwrap(), m.accessed().unwrap()),
        (at(T1), at(T2))
    );
}

#[test]
fn set_times_works_on_directories() {
    let t = tempfile::tempdir().unwrap();
    set_times(&VPath::local(t.path()).unwrap(), Some(at(T1)), None, None).unwrap();
    assert_eq!(
        std::fs::metadata(t.path()).unwrap().modified().unwrap(),
        at(T1)
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn set_times_sets_creation_time() {
    let (_t, f) = tmp_file("f.txt", b"x");
    set_times(&f, None, None, Some(at(T1))).unwrap();
    assert_eq!(
        std::fs::metadata(f.as_local().unwrap())
            .unwrap()
            .created()
            .unwrap(),
        at(T1)
    );
}

#[test]
fn readonly_attribute_is_exact() {
    let (_t, f) = tmp_file("ro.txt", b"x");
    set_attributes(&f, Attributes::READONLY).unwrap();
    assert!(attrs(&f).contains(Attributes::READONLY));
    set_attributes(&f, Attributes::empty()).unwrap();
    assert!(!attrs(&f).contains(Attributes::READONLY));
}

#[cfg(windows)]
#[test]
fn windows_hidden_and_system_are_set_and_cleared() {
    let (_t, f) = tmp_file("h.txt", b"x");
    set_attributes(&f, Attributes::HIDDEN | Attributes::SYSTEM).unwrap();
    assert!(attrs(&f).contains(Attributes::HIDDEN | Attributes::SYSTEM));
    set_attributes(&f, Attributes::empty()).unwrap();
    assert!(!attrs(&f).intersects(Attributes::HIDDEN | Attributes::SYSTEM));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_hidden_flag_is_set_and_cleared() {
    let (_t, f) = tmp_file("visible.txt", b"x");
    set_attributes(&f, Attributes::HIDDEN).unwrap();
    assert!(attrs(&f).contains(Attributes::HIDDEN));
    set_attributes(&f, Attributes::empty()).unwrap();
    assert!(!attrs(&f).contains(Attributes::HIDDEN));
}

#[cfg(unix)]
#[test]
fn copy_extras_copies_mode_and_xattrs() {
    use std::os::unix::fs::PermissionsExt;
    let (t, from) = tmp_file("from", b"x");
    let to_path = t.path().join("to");
    std::fs::write(&to_path, b"x").unwrap();
    std::fs::set_permissions(
        from.as_local().unwrap(),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    let name = if cfg!(target_os = "macos") {
        "com.example.rf-test"
    } else {
        "user.rf-test"
    };
    let xattr_ok = rustix::fs::lsetxattr(
        from.as_local().unwrap(),
        name,
        b"value",
        rustix::fs::XattrFlags::empty(),
    )
    .is_ok();
    copy_extras(&from, &VPath::local(&to_path).unwrap()).unwrap();
    assert_eq!(
        std::fs::metadata(&to_path).unwrap().permissions().mode() & 0o777,
        0o750
    );
    if xattr_ok {
        let mut buf = [0u8; 16];
        let n = rustix::fs::lgetxattr(&to_path, name, &mut buf[..]).unwrap();
        assert_eq!(&buf[..n], b"value");
    } else {
        eprintln!("skipped the xattr check: this temp file system has no user xattrs");
    }
}

/// Writing an extended attribute needs write access, so the mode of a read-only source must not
/// reach the copy before its attributes do.
#[cfg(unix)]
#[test]
fn copy_extras_copies_xattrs_of_a_read_only_file() {
    use std::os::unix::fs::PermissionsExt;
    let (t, from) = tmp_file("from", b"x");
    let to_path = t.path().join("to");
    std::fs::write(&to_path, b"x").unwrap();
    let name = if cfg!(target_os = "macos") {
        "com.example.rf-test"
    } else {
        "user.rf-test"
    };
    let flags = rustix::fs::XattrFlags::empty();
    if rustix::fs::lsetxattr(from.as_local().unwrap(), name, b"value", flags).is_err() {
        eprintln!("skipped: this temp file system has no user xattrs");
        return;
    }
    let read_only = std::fs::Permissions::from_mode(0o444);
    std::fs::set_permissions(from.as_local().unwrap(), read_only).unwrap();
    copy_extras(&from, &VPath::local(&to_path).unwrap()).unwrap();
    let mode = std::fs::metadata(&to_path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o444);
    let mut buf = [0u8; 16];
    let n = rustix::fs::lgetxattr(&to_path, name, &mut buf[..]).unwrap();
    assert_eq!(&buf[..n], b"value");
}

#[cfg(windows)]
#[test]
fn copy_extras_copies_alternate_streams() {
    let (t, from) = tmp_file("from.txt", b"main");
    let zone = b"[ZoneTransfer]\r\nZoneId=3\r\n";
    std::fs::write(t.path().join("from.txt:Zone.Identifier"), zone).unwrap();
    std::fs::write(t.path().join("from.txt:custom"), b"side").unwrap();
    std::fs::write(t.path().join("to.txt"), b"main").unwrap();
    copy_extras(&from, &VPath::local(t.path().join("to.txt")).unwrap()).unwrap();
    assert_eq!(
        std::fs::read(t.path().join("to.txt:custom")).unwrap(),
        b"side"
    );
    assert_eq!(
        std::fs::read(t.path().join("to.txt:Zone.Identifier")).unwrap(),
        zone
    );
    assert_eq!(std::fs::read(t.path().join("to.txt")).unwrap(), b"main");
}

/// A `to` that is a link keeps its streams: writing them would change the file it points to.
#[cfg(windows)]
#[test]
fn copy_extras_writes_no_stream_through_a_link() {
    let t = tempfile::tempdir().unwrap();
    let (pointed, link, from) = (
        t.path().join("pointed.txt"),
        t.path().join("link.txt"),
        t.path().join("from.txt"),
    );
    std::fs::write(&pointed, b"main").unwrap();
    std::fs::write(t.path().join("pointed.txt:Zone.Identifier"), b"keep").unwrap();
    std::fs::write(&from, b"x").unwrap();
    std::fs::write(t.path().join("from.txt:Zone.Identifier"), b"new").unwrap();
    if let Err(e) = std::os::windows::fs::symlink_file(&pointed, &link) {
        // CI runners may create symbolic links: a failure there is a broken test, not a skip.
        assert!(
            std::env::var_os("GITHUB_ACTIONS").is_none(),
            "cannot create symbolic links on CI: {e}"
        );
        eprintln!("skipped: cannot create symbolic links here: {e}");
        return;
    }
    copy_extras(&VPath::local(&from).unwrap(), &VPath::local(&link).unwrap()).unwrap();
    assert_eq!(
        std::fs::read(t.path().join("pointed.txt:Zone.Identifier")).unwrap(),
        b"keep"
    );
    assert_eq!(std::fs::read(&pointed).unwrap(), b"main");
}

/// Onto itself (the same path, or a hard link to the same file) nothing is touched; on Windows
/// a stream would otherwise be emptied before it is read.
#[test]
fn copy_extras_onto_the_same_item_changes_nothing() {
    let (t, f) = tmp_file("self.txt", b"main");
    #[cfg(windows)]
    std::fs::write(t.path().join("self.txt:side"), b"side").unwrap();
    let hard = t.path().join("hard.txt");
    std::fs::hard_link(f.as_local().unwrap(), &hard).unwrap();
    copy_extras(&f, &f).unwrap();
    copy_extras(&f, &VPath::local(&hard).unwrap()).unwrap();
    assert_eq!(std::fs::read(f.as_local().unwrap()).unwrap(), b"main");
    #[cfg(windows)]
    assert_eq!(
        std::fs::read(t.path().join("self.txt:side")).unwrap(),
        b"side"
    );
}

/// A final link is changed itself, or left alone, but what it points to never changes.
#[cfg(unix)]
#[test]
fn a_final_link_is_never_followed() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let (pointed, link, from) = (
        t.path().join("pointed"),
        t.path().join("link"),
        t.path().join("from"),
    );
    std::fs::write(&pointed, b"keep").unwrap();
    std::fs::set_permissions(&pointed, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::write(&from, b"x").unwrap();
    std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o700)).unwrap();
    let name = if cfg!(target_os = "macos") {
        "com.example.rf-test"
    } else {
        "user.rf-test"
    };
    let flags = rustix::fs::XattrFlags::empty();
    let xattr_ok = rustix::fs::lsetxattr(&from, name, b"value", flags).is_ok();
    std::os::unix::fs::symlink(&pointed, &link).unwrap();
    let before = std::fs::metadata(&pointed).unwrap().modified().unwrap();

    let at_link = VPath::local(&link).unwrap();
    set_times(&at_link, Some(at(T1)), Some(at(T2)), None).unwrap();
    set_attributes(&at_link, Attributes::READONLY).unwrap();
    copy_extras(&VPath::local(&from).unwrap(), &at_link).unwrap();

    let meta = std::fs::metadata(&pointed).unwrap();
    assert_eq!(meta.modified().unwrap(), before);
    assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    if xattr_ok {
        let mut buf = [0u8; 16];
        assert!(rustix::fs::lgetxattr(&pointed, name, &mut buf[..]).is_err());
    }
    let link_meta = std::fs::symlink_metadata(&link).unwrap();
    assert_eq!(link_meta.modified().unwrap(), at(T1));
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn restore_from_trash_puts_the_item_back() {
    let t = tempfile::tempdir().unwrap();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!(
        "{}restore-{}-{nanos}.txt",
        rf_core::identity::TEMP_PREFIX,
        std::process::id()
    );
    let original = VPath::local(t.path().join(&name)).unwrap();
    std::fs::write(original.as_local().unwrap(), b"back").unwrap();
    let location = rf_platform::move_to_trash(&original)
        .unwrap()
        .expect("trash location");
    restore_from_trash(&VPath::local(&location).unwrap(), &original).unwrap();
    assert_eq!(
        std::fs::read(original.as_local().unwrap()).unwrap(),
        b"back"
    );
    assert!(!location.exists());
    #[cfg(windows)]
    {
        let n = location.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!location.with_file_name(n.replacen("$R", "$I", 1)).exists());
    }
}

#[test]
fn restore_never_overwrites() {
    let (t, location) = tmp_file("trashed.txt", b"old copy");
    std::fs::write(t.path().join("original.txt"), b"new").unwrap();
    let original = VPath::local(t.path().join("original.txt")).unwrap();
    assert_eq!(
        restore_from_trash(&location, &original).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        std::fs::read(t.path().join("original.txt")).unwrap(),
        b"new"
    );
    assert_eq!(
        std::fs::read(location.as_local().unwrap()).unwrap(),
        b"old copy"
    );
}

/// The bin keeps its items in `<root>\$Recycle.Bin\<user SID>\$R…`. Anywhere else a `$R…`
/// name is an ordinary file, and the `$I…` file next to it is someone's data, not a bin record.
#[test]
fn restore_deletes_a_bin_record_only_next_to_a_bin_item() {
    let t = tempfile::tempdir().unwrap();
    // Restores `dir/$R{stem}.txt` and says whether `dir/$I{stem}.txt` is still there.
    let restore_keeps_record = |dir: &Path, stem: &str| {
        std::fs::create_dir_all(dir).unwrap();
        let (item, record) = (
            dir.join(format!("$R{stem}.txt")),
            dir.join(format!("$I{stem}.txt")),
        );
        std::fs::write(&item, b"item").unwrap();
        std::fs::write(&record, b"record").unwrap();
        let back = t.path().join(format!("back-{stem}.txt"));
        let original = VPath::local(&back).unwrap();
        restore_from_trash(&VPath::local(&item).unwrap(), &original).unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"item");
        record.exists()
    };
    let user = t.path().join("$Recycle.Bin").join("S-1-5-21-1");
    assert!(restore_keeps_record(t.path(), "plain"));
    assert!(restore_keeps_record(&user.join("deeper"), "deep"));
    assert_eq!(restore_keeps_record(&user, "bin"), cfg!(not(windows)));
}

#[cfg(unix)]
#[test]
fn set_times_needs_no_read_access() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, f) = tmp_file("locked.txt", b"x");
    let p = f.as_local().unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o000)).unwrap();
    set_times(&f, Some(at(T1)), Some(at(T2)), None).unwrap();
    let m = std::fs::symlink_metadata(p).unwrap();
    assert_eq!(
        (m.modified().unwrap(), m.accessed().unwrap()),
        (at(T1), at(T2))
    );
}

/// A leading dot hides an item without `UF_HIDDEN`, so `HIDDEN` on a dot name never sets the
/// flag; `copy_extras` carries the flag itself.
#[cfg(target_os = "macos")]
#[test]
fn macos_hidden_flag_is_not_set_for_a_dot_name_but_is_copied() {
    use std::os::macos::fs::MetadataExt;
    let flag = |v: &VPath| {
        std::fs::symlink_metadata(v.as_local().unwrap())
            .unwrap()
            .st_flags()
            & 0x8000
    };
    let (t, dot) = tmp_file(".dot", b"x");
    set_attributes(&dot, Attributes::HIDDEN).unwrap();
    assert_eq!(flag(&dot), 0);
    let (from, to) = (
        VPath::local(t.path().join("from")).unwrap(),
        VPath::local(t.path().join("to")).unwrap(),
    );
    std::fs::write(from.as_local().unwrap(), b"x").unwrap();
    std::fs::write(to.as_local().unwrap(), b"x").unwrap();
    set_attributes(&from, Attributes::HIDDEN).unwrap();
    copy_extras(&from, &to).unwrap();
    assert_eq!(flag(&to), 0x8000);
}

#[test]
fn non_local_paths_are_unsupported() {
    let r = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(
        set_times(&r, Some(at(T1)), None, None).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        set_attributes(&r, Attributes::empty()).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        copy_extras(&r, &r).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        restore_from_trash(&r, &r).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}
