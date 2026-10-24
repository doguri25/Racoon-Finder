use std::ffi::OsString;

use rf_core::{Attributes, Entry, EntryKind, FileId, VolumeId};

#[test]
fn new_entry_has_empty_metadata() {
    let e = Entry::new("a.txt", EntryKind::File);
    assert_eq!(e.name, OsString::from("a.txt"));
    assert_eq!(e.kind, EntryKind::File);
    assert_eq!(e.size, 0);
    assert_eq!((e.modified, e.created, e.accessed), (None, None, None));
    assert_eq!(e.attributes, Attributes::empty());
    assert_eq!((e.volume_id, e.file_id), (None, None));
}

#[test]
fn extension_rules() {
    let ext = |n: &str, k| {
        Entry::new(n, k)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
    };
    assert_eq!(ext("a.tar.gz", EntryKind::File).as_deref(), Some("gz"));
    assert_eq!(ext(".bashrc", EntryKind::File), None);
    assert_eq!(ext("a.", EntryKind::File), None);
    assert_eq!(ext("noext", EntryKind::File), None);
    assert_eq!(ext("photos.2026", EntryKind::Dir), None);
    assert_eq!(ext("link.lnk", EntryKind::Symlink).as_deref(), Some("lnk"));
}

#[test]
fn special_files_have_no_extension() {
    let fifo = Entry::new("events.pipe", EntryKind::Other);
    assert_eq!(fifo.kind, EntryKind::Other);
    assert_eq!(fifo.extension(), None);
}

#[test]
fn identity_needs_both_ids() {
    let mut e = Entry::new("a", EntryKind::File);
    e.volume_id = Some(VolumeId(7));
    assert_eq!(e.identity(), None);
    e.file_id = Some(FileId(u128::MAX));
    assert_eq!(e.identity(), Some((VolumeId(7), FileId(u128::MAX))));
}

#[test]
fn attribute_bits_are_stable() {
    let all = [
        (Attributes::READONLY, 1 << 0),
        (Attributes::HIDDEN, 1 << 1),
        (Attributes::SYSTEM, 1 << 2),
        (Attributes::ARCHIVE, 1 << 3),
        (Attributes::COMPRESSED, 1 << 4),
        (Attributes::ENCRYPTED, 1 << 5),
        (Attributes::SPARSE, 1 << 6),
        (Attributes::OFFLINE, 1 << 7),
        (Attributes::REPARSE_POINT, 1 << 8),
    ];
    for (flag, bits) in all {
        assert_eq!(flag.bits(), bits);
    }
    let hs = Attributes::HIDDEN | Attributes::SYSTEM;
    assert!(hs.contains(Attributes::HIDDEN) && !hs.contains(Attributes::READONLY));
}
