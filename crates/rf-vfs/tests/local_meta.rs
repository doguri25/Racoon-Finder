use std::io::{Read, Write};
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

/// A folder whose entries changed can be synced. A missing folder is an error on every system,
/// so the call really reaches the file system, on Windows too; so is a file, which is no folder.
#[test]
fn sync_dir_syncs_an_existing_folder() {
    let t = tempfile::tempdir().unwrap();
    let p = LocalProvider::new();
    let dir = VPath::local(t.path()).unwrap();
    let file = dir.join("a").unwrap();
    p.create_file(&file).unwrap();
    p.sync_dir(&dir).unwrap();
    let missing = p.sync_dir(&dir.join("missing").unwrap()).unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
    assert!(
        missing.to_string().starts_with("sync_dir failed for "),
        "{missing}"
    );
    assert!(p.sync_dir(&file).is_err());
}

/// A FIFO put where a file or a folder was found never blocks the provider: reading it and
/// syncing it as a folder are refused at once. A plain open of its read end would wait for a
/// writer forever.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_blocking() {
    use std::sync::mpsc;

    let t = tempfile::tempdir().unwrap();
    let fifo = t.path().join("fifo");
    // `mkfifo(1)` is on Linux and macOS alike.
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    assert!(made.unwrap().success(), "mkfifo failed");
    let path = VPath::local(&fifo).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let p = LocalProvider::new();
        let read = p.open_read(&path).map(drop);
        let synced = p.sync_dir(&path);
        let _ = tx.send((read, synced));
    });
    let Ok((read, synced)) = rx.recv_timeout(Duration::from_secs(10)) else {
        // Opening the other end lets the blocked open go on, so the thread ends.
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        panic!("the provider blocked on a FIFO");
    };
    let read = read.unwrap_err();
    assert_eq!(read.kind(), ErrorKind::Unsupported, "{read}");
    assert!(read.to_string().ends_with("not a regular file"), "{read}");
    assert!(synced.is_err());
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

/// A read stream reports the ranges of the file it reads, also after an app saved over its
/// path by replacing the file, when the path answers for the new file.
#[test]
fn read_streams_report_the_ranges_of_their_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = LocalProvider::new();
    let file = VPath::local(dir.path().join("s.bin")).unwrap();
    let mut writer = p.open_write(&file, WriteMode::CreateNew).unwrap();
    writer.write_all(b"a").unwrap();
    assert!(writer.write_hole(4 << 20).unwrap());
    writer.write_all(b"b").unwrap();
    writer.finish().unwrap();
    let sparse = p.data_ranges(&file).unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(sparse, None, "macOS never reports holes");
    } else if cfg!(windows) {
        assert!(sparse.is_some(), "NTFS keeps holes");
    } else {
        // A file with fewer blocks than its length needs has holes, and they must be found:
        // the rest of the test would pass with none.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(dir.path().join("s.bin")).unwrap();
            let has_holes = meta.blocks().saturating_mul(512) < meta.len();
            assert!(!has_holes || sparse.is_some(), "holes kept but not found");
        }
    }
    let mut reader = p.open_read(&file).unwrap();
    let dense = dir.path().join("dense.bin");
    std::fs::write(&dense, vec![7u8; 256 * 1024]).unwrap();
    std::fs::rename(&dense, dir.path().join("s.bin")).unwrap();
    assert_eq!(p.data_ranges(&file).unwrap(), None);
    let found = reader.data_ranges().unwrap();
    assert!(
        found
            .as_ref()
            .is_none_or(|found| found.len == 2 + (4 << 20))
    );
    assert_eq!(found.map(|found| found.ranges), sparse);
    // The stream is still at its start.
    let mut first = [0; 1];
    reader.read_exact(&mut first).unwrap();
    assert_eq!(first, *b"a");
}
