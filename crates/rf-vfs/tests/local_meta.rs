use std::io::Write;
use std::time::{Duration, SystemTime};

use rf_core::{Attributes, Entry, EntryKind, ErrorKind, Scheme, VPath};
use rf_vfs::{LinkTarget, LocalProvider, MetadataPatch, Provider, WriteMode, testkit};

fn run(check: fn(&dyn Provider, &VPath)) {
    let t = tempfile::tempdir().unwrap();
    check(&LocalProvider::new(), &VPath::local(t.path()).unwrap());
}

fn t(s: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(s)
}

#[test]
fn links() {
    run(testkit::check_links);
}

#[test]
fn metadata() {
    run(testkit::check_metadata);
}

#[test]
fn remove_dir() {
    run(testkit::check_remove_dir);
}

#[test]
fn patch_helpers_take_times_and_portable_attributes() {
    let mut e = Entry::new("a", EntryKind::File);
    (e.modified, e.accessed, e.created) = (Some(t(1)), Some(t(2)), Some(t(3)));
    e.attributes = Attributes::READONLY
        | Attributes::HIDDEN
        | Attributes::COMPRESSED
        | Attributes::REPARSE_POINT;
    let times = MetadataPatch::times_of(&e);
    assert_eq!(
        (
            times.modified,
            times.accessed,
            times.created,
            times.attributes
        ),
        (Some(t(1)), Some(t(2)), Some(t(3)), None)
    );
    let attrs = MetadataPatch::attributes_of(&e);
    assert_eq!(
        (attrs.attributes, attrs.modified),
        (Some(Attributes::READONLY | Attributes::HIDDEN), None)
    );
    assert!(MetadataPatch::default().is_empty() && !attrs.is_empty());
}

/// Off Windows a leading dot hides an item by its name, so a `HIDDEN` that may come only from
/// the dot is never carried over as an attribute (it would set `UF_HIDDEN` on macOS).
#[test]
fn attributes_of_leaves_out_hidden_for_a_dot_name_off_windows() {
    let mut e = Entry::new(".dot", EntryKind::File);
    e.attributes = Attributes::READONLY | Attributes::HIDDEN;
    let want = if cfg!(windows) {
        Attributes::READONLY | Attributes::HIDDEN
    } else {
        Attributes::READONLY
    };
    assert_eq!(MetadataPatch::attributes_of(&e).attributes, Some(want));
}

#[test]
fn fs_name_matches_volume_info() {
    let d = tempfile::tempdir().unwrap();
    let v = VPath::local(d.path()).unwrap();
    assert_eq!(
        LocalProvider::new().fs_name(&v),
        rf_platform::volume_info(&v).unwrap().fs_name
    );
}

#[test]
fn restore_trashed_moves_the_item_back() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("in-trash"), b"x").unwrap();
    let (loc, orig) = (
        VPath::local(d.path().join("in-trash")).unwrap(),
        VPath::local(d.path().join("orig")).unwrap(),
    );
    LocalProvider::new().restore_trashed(&loc, &orig).unwrap();
    assert_eq!(std::fs::read(d.path().join("orig")).unwrap(), b"x");
    assert!(!d.path().join("in-trash").exists());
}

/// A link to a socket leads to an `EntryKind::Other` item, which is never opened for data.
#[cfg(unix)]
#[test]
fn stat_target_of_a_link_to_a_socket_is_other() {
    let d = tempfile::tempdir().unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(d.path().join("sock")).unwrap();
    let p = LocalProvider::new();
    let link = VPath::local(d.path().join("link")).unwrap();
    let to_socket = LinkTarget {
        target: "sock".into(),
        is_dir: false,
    };
    p.create_symlink(&link, &to_socket).unwrap();
    assert_eq!(p.stat(&link).unwrap().kind, EntryKind::Symlink);
    assert_eq!(p.stat_target(&link).unwrap().kind, EntryKind::Other);
}

#[test]
fn new_methods_refuse_non_local_paths() {
    let r = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    let p = LocalProvider::new();
    let link = LinkTarget {
        target: "t".into(),
        is_dir: false,
    };
    let kinds = [
        p.stat_target(&r).map(drop),
        p.read_link(&r).map(drop),
        p.create_symlink(&r, &link),
        p.set_metadata(&r, &MetadataPatch::default()),
        p.copy_extras(&r, &r),
        p.remove_dir(&r),
        p.restore_trashed(&r, &r),
        p.sync_dir(&r),
    ]
    .map(|result| result.unwrap_err().kind());
    assert_eq!(kinds, [ErrorKind::Unsupported; 8]);
    assert_eq!(p.fs_name(&r), None);
}

/// A folder whose entries changed can be synced. Off Windows a missing folder is an error, so
/// the call really reaches the file system; on Windows it does nothing.
#[test]
fn sync_dir_syncs_an_existing_folder() {
    let t = tempfile::tempdir().unwrap();
    let p = LocalProvider::new();
    let dir = VPath::local(t.path()).unwrap();
    p.create_file(&dir.join("a").unwrap()).unwrap();
    p.sync_dir(&dir).unwrap();
    let missing = p.sync_dir(&dir.join("missing").unwrap());
    #[cfg(unix)]
    assert_eq!(missing.unwrap_err().kind(), ErrorKind::NotFound);
    #[cfg(windows)]
    missing.unwrap();
}

#[test]
fn write_streams_can_leave_holes() {
    let d = tempfile::tempdir().unwrap();
    let p = LocalProvider::new();
    let f = VPath::local(d.path().join("h.bin")).unwrap();
    let mut w = p.open_write(&f, WriteMode::CreateNew).unwrap();
    w.write_all(b"a").unwrap();
    assert!(w.write_hole(1 << 20).unwrap());
    w.write_all(b"b").unwrap();
    assert!(w.write_hole(1 << 20).unwrap());
    w.finish().unwrap();
    let bytes = std::fs::read(d.path().join("h.bin")).unwrap();
    assert_eq!(bytes.len(), 2 + (2 << 20));
    assert_eq!((bytes[0], bytes[1 + (1 << 20)]), (b'a', b'b'));
    assert!(
        bytes[1..=(1 << 20)]
            .iter()
            .chain(&bytes[2 + (1 << 20)..])
            .all(|&b| b == 0)
    );
    assert!(
        !p.open_write(&f, WriteMode::Append)
            .unwrap()
            .write_hole(1)
            .unwrap()
    );
}
