//! Provider conformance checks (PRD 9). Each check works in a new folder under `root` and panics on failure.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use rf_core::{EntryKind, ErrorKind, VPath};

use crate::{CancelToken, DeleteMode, Deleted, LIST_BATCH_SIZE, Provider, RenameMode, WriteMode};

fn fresh(p: &dyn Provider, root: &VPath, name: &str) -> VPath {
    let d = root.join(name).unwrap();
    p.create_dir(&d).unwrap();
    d
}

fn kind_of<T: std::fmt::Debug>(r: rf_core::Result<T>) -> ErrorKind {
    r.unwrap_err().kind()
}

pub fn names(p: &dyn Provider, dir: &VPath) -> BTreeSet<OsString> {
    let mut out = BTreeSet::new();
    p.list(dir, &CancelToken::new(), &mut |batch| {
        out.extend(batch.into_iter().map(|e| e.name));
    })
    .unwrap();
    out
}

pub fn write_all(p: &dyn Provider, path: &VPath, mode: WriteMode, bytes: &[u8]) {
    let mut w = p.open_write(path, mode).unwrap();
    w.write_all(bytes).unwrap();
    w.finish().unwrap();
}

pub fn read_all(p: &dyn Provider, path: &VPath) -> Vec<u8> {
    let mut v = Vec::new();
    p.open_read(path).unwrap().read_to_end(&mut v).unwrap();
    v
}

pub fn check_create_and_stat(p: &dyn Provider, root: &VPath) {
    let d = fresh(p, root, "create");
    let (sub, file, missing) = (
        d.join("sub").unwrap(),
        d.join("f.txt").unwrap(),
        d.join("missing").unwrap(),
    );
    p.create_dir(&sub).unwrap();
    p.create_file(&file).unwrap();
    let s = p.stat(&sub).unwrap();
    assert_eq!(
        (s.name.as_os_str(), s.kind),
        (OsStr::new("sub"), EntryKind::Dir)
    );
    let f = p.stat(&file).unwrap();
    assert_eq!(
        (f.name.as_os_str(), f.kind, f.size),
        (OsStr::new("f.txt"), EntryKind::File, 0)
    );
    assert!(f.modified.is_some());
    assert_eq!(kind_of(p.create_file(&file)), ErrorKind::AlreadyExists);
    assert_eq!(kind_of(p.create_dir(&sub)), ErrorKind::AlreadyExists);
    assert_eq!(kind_of(p.stat(&missing)), ErrorKind::NotFound);
    assert_eq!(
        kind_of(p.create_file(&missing.join("x").unwrap())),
        ErrorKind::NotFound
    );
    assert_eq!(
        names(p, &d),
        BTreeSet::from([OsString::from("f.txt"), OsString::from("sub")])
    );
}

/// Returns the batch sizes so a provider's own test can pin its batching.
pub fn check_list_batches(p: &dyn Provider, root: &VPath) -> Vec<usize> {
    let d = fresh(p, root, "batches");
    let files = 2 * LIST_BATCH_SIZE + 500;
    for i in 0..files {
        p.create_file(&d.join(format!("f{i:05}")).unwrap()).unwrap();
    }
    for i in 0..3 {
        p.create_dir(&d.join(format!("d{i}")).unwrap()).unwrap();
    }
    let (mut sizes, mut seen) = (Vec::new(), BTreeSet::new());
    p.list(&d, &CancelToken::new(), &mut |batch| {
        sizes.push(batch.len());
        for e in batch {
            let want = if e.name.to_string_lossy().starts_with('d') {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            assert_eq!(e.kind, want, "{:?}", e.name);
            assert!(seen.insert(e.name), "duplicate entry");
        }
    })
    .unwrap();
    assert!(
        sizes.len() >= 3 && sizes.iter().all(|&n| n > 0 && n <= LIST_BATCH_SIZE),
        "{sizes:?}"
    );
    assert_eq!(seen.len(), files + 3);
    let empty = fresh(p, root, "empty");
    let mut calls = 0;
    p.list(&empty, &CancelToken::new(), &mut |_| calls += 1)
        .unwrap();
    assert_eq!(calls, 0);
    assert_eq!(
        kind_of(p.list(
            &d.join("missing").unwrap(),
            &CancelToken::new(),
            &mut |_| {}
        )),
        ErrorKind::NotFound
    );
    assert!(
        p.list(&d.join("f00000").unwrap(), &CancelToken::new(), &mut |_| {})
            .is_err()
    );
    sizes
}

pub fn check_list_cancel(p: &dyn Provider, root: &VPath) {
    let d = fresh(p, root, "cancel");
    for i in 0..=LIST_BATCH_SIZE {
        p.create_file(&d.join(format!("f{i:05}")).unwrap()).unwrap();
    }
    let token = CancelToken::new();
    let mut calls = 0;
    let r = p.list(&d, &token, &mut |_| {
        calls += 1;
        token.cancel();
    });
    assert_eq!(kind_of(r), ErrorKind::Cancelled);
    assert_eq!(calls, 1);
    let mut calls = 0;
    assert_eq!(
        kind_of(p.list(&d, &token, &mut |_| calls += 1)),
        ErrorKind::Cancelled
    );
    assert_eq!(calls, 0);
}

pub fn check_rename(p: &dyn Provider, root: &VPath) {
    let dir = fresh(p, root, "rename");
    let (a, b, c) = (
        dir.join("a.txt").unwrap(),
        dir.join("b.txt").unwrap(),
        dir.join("c.txt").unwrap(),
    );
    write_all(p, &a, WriteMode::CreateNew, b"A");
    write_all(p, &b, WriteMode::CreateNew, b"B");
    assert_eq!(
        kind_of(p.rename(&a, &b, RenameMode::NoReplace)),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        (read_all(p, &a), read_all(p, &b)),
        (b"A".to_vec(), b"B".to_vec())
    );
    p.rename(&a, &c, RenameMode::NoReplace).unwrap();
    assert_eq!(kind_of(p.stat(&a)), ErrorKind::NotFound);
    p.rename(&c, &b, RenameMode::Replace).unwrap();
    assert_eq!(read_all(p, &b), b"A");
    // Case-only rename must work on case-insensitive volumes (OPS-02, REN-04).
    p.rename(&b, &dir.join("B.TXT").unwrap(), RenameMode::NoReplace)
        .unwrap();
    assert_eq!(names(p, &dir), BTreeSet::from([OsString::from("B.TXT")]));
    let (src, dst) = (dir.join("src").unwrap(), dir.join("dst").unwrap());
    p.create_dir(&src).unwrap();
    write_all(
        p,
        &src.join("inner.txt").unwrap(),
        WriteMode::CreateNew,
        b"in",
    );
    p.rename(&src, &dst, RenameMode::NoReplace).unwrap();
    assert_eq!(read_all(p, &dst.join("inner.txt").unwrap()), b"in");
    // A folder can never move into itself (DND-09).
    assert_eq!(
        kind_of(p.rename(&dst, &dst.join("dst").unwrap(), RenameMode::NoReplace)),
        ErrorKind::Unsupported
    );
    assert_eq!(read_all(p, &dst.join("inner.txt").unwrap()), b"in");
    assert_eq!(
        kind_of(p.rename(&dir.join("missing").unwrap(), &a, RenameMode::NoReplace)),
        ErrorKind::NotFound
    );
}

/// A link is never the item it points to. With `A.TXT` a file and `a.txt` a link to it, renaming
/// either name onto the other must fail with `AlreadyExists` and leave both as they were; the
/// case-only rename guard (OPS-02, REN-04) must not take the link for the file it leads to.
///
/// Skipped, with a note on stderr, where the two names cannot exist side by side (a volume that
/// ignores case), where links cannot be made (Windows without the privilege) and for a root
/// that is not a local path, because links are made with the file system directly.
#[allow(
    clippy::print_stderr,
    reason = "a check that cannot run says so, as the skipped tests do"
)]
pub fn check_rename_symlink_guard(p: &dyn Provider, root: &VPath) {
    let dir = fresh(p, root, "rename-link");
    let (real, link) = (dir.join("A.TXT").unwrap(), dir.join("a.txt").unwrap());
    let Some(link_path) = link.to_extended_length() else {
        eprintln!("skipped: links are made on the local file system only");
        return;
    };
    write_all(p, &real, WriteMode::CreateNew, b"real");
    match symlink_file("A.TXT", &link_path) {
        Ok(()) => {}
        Err(e) if link_unavailable(&e) => {
            eprintln!("skipped: no link next to `A.TXT` here: {e}");
            return;
        }
        Err(e) => panic!("cannot make a link: {e}"),
    }
    assert_eq!(p.stat(&real).unwrap().kind, EntryKind::File);
    assert_eq!(p.stat(&link).unwrap().kind, EntryKind::Symlink);
    for (from, to) in [(&link, &real), (&real, &link)] {
        assert_eq!(
            kind_of(p.rename(from, to, RenameMode::NoReplace)),
            ErrorKind::AlreadyExists
        );
        assert_eq!(p.stat(&real).unwrap().kind, EntryKind::File);
        assert_eq!(p.stat(&link).unwrap().kind, EntryKind::Symlink);
        assert_eq!(read_all(p, &real), b"real");
    }
    assert_eq!(
        names(p, &dir),
        BTreeSet::from([OsString::from("A.TXT"), OsString::from("a.txt")])
    );
}

/// Whether `e` only means that this machine cannot hold the link: the name is taken because the
/// volume ignores case, or the user may not make links. Windows reports the missing privilege
/// as `ERROR_PRIVILEGE_NOT_HELD` (1314), which std leaves uncategorized.
fn link_unavailable(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
    ) || (cfg!(windows) && e.raw_os_error() == Some(1314))
}

#[cfg(unix)]
fn symlink_file(target: &str, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &str, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

pub fn check_read_write(p: &dyn Provider, root: &VPath) {
    let dir = fresh(p, root, "rw");
    let file = dir.join("data.bin").unwrap();
    let data: Vec<u8> = (0..1u32 << 20)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    write_all(p, &file, WriteMode::CreateNew, &data);
    assert_eq!(read_all(p, &file), data);
    assert_eq!(p.stat(&file).unwrap().size, 1 << 20);
    let mut reader = p.open_read(&file).unwrap();
    reader.seek(SeekFrom::Start(512 * 1024)).unwrap();
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).unwrap();
    assert_eq!(&buf[..], &data[512 * 1024..512 * 1024 + 4]);
    assert_eq!(
        kind_of(p.open_write(&file, WriteMode::CreateNew)),
        ErrorKind::AlreadyExists
    );
    // `CreateNew` refuses an existing file without touching it.
    assert_eq!(read_all(p, &file), data);
    write_all(p, &file, WriteMode::Truncate, b"new");
    write_all(p, &file, WriteMode::Append, b"+more");
    assert_eq!(read_all(p, &file), b"new+more");
    let appended = dir.join("appended.txt").unwrap();
    write_all(p, &appended, WriteMode::Append, b"x");
    assert_eq!(read_all(p, &appended), b"x");
    assert_eq!(
        kind_of(p.open_read(&dir.join("missing").unwrap())),
        ErrorKind::NotFound
    );
}

pub fn check_delete_permanent(p: &dyn Provider, root: &VPath) {
    let d = fresh(p, root, "delete");
    let f = d.join("f.txt").unwrap();
    p.create_file(&f).unwrap();
    assert_eq!(
        p.delete(&f, DeleteMode::Permanent).unwrap(),
        Deleted::Permanently
    );
    assert_eq!(kind_of(p.stat(&f)), ErrorKind::NotFound);
    let tree = d.join("tree").unwrap();
    p.create_dir(&tree).unwrap();
    let mut deep = tree.clone();
    for name in ["a", "b", "c"] {
        deep = deep.join(name).unwrap();
        p.create_dir(&deep).unwrap();
        write_all(
            p,
            &deep.join("file.txt").unwrap(),
            WriteMode::CreateNew,
            b"x",
        );
    }
    p.delete(&tree, DeleteMode::Permanent).unwrap();
    assert_eq!(names(p, &d), BTreeSet::new());
    assert_eq!(
        kind_of(p.delete(&f, DeleteMode::Permanent)),
        ErrorKind::NotFound
    );
}

/// Names every provider must store and list byte-for-byte. Each one is tested in its own
/// folder because APFS treats the NFC and NFD spellings of a name as the same file.
pub const UNICODE_NAMES: &[&str] = &[
    "한글 문서.txt",
    "\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}.txt",
    "🦝📁 emoji.png",
    "日本語のファイル.md",
    "Ünïcödé ñ.txt",
    "100% done & more #1.txt",
    "[brackets] {braces} 'quote'.txt",
    "name.with.many.dots.tar.gz",
];

pub fn check_unicode_names(p: &dyn Provider, root: &VPath) {
    let d = fresh(p, root, "unicode");
    for (i, name) in UNICODE_NAMES.iter().enumerate() {
        let sub = fresh(p, &d, &format!("case{i}"));
        let path = sub.join(name).unwrap();
        write_all(p, &path, WriteMode::CreateNew, name.as_bytes());
        assert_eq!(
            names(p, &sub),
            BTreeSet::from([OsString::from(*name)]),
            "{name}"
        );
        assert_eq!(p.stat(&path).unwrap().name, OsString::from(*name));
        let renamed = sub.join(format!("{name}.renamed")).unwrap();
        p.rename(&path, &renamed, RenameMode::NoReplace).unwrap();
        assert_eq!(read_all(p, &renamed), name.as_bytes());
        p.delete(&renamed, DeleteMode::Permanent).unwrap();
        assert_eq!(names(p, &sub), BTreeSet::new());
    }
}

/// Builds a path longer than 300 UTF-16 units (Windows `MAX_PATH` is 260) and under
/// 1,024 UTF-8 bytes (macOS `PATH_MAX`).
pub fn check_long_paths(p: &dyn Provider, root: &VPath) {
    let top = fresh(p, root, "long");
    let mut deep = top.clone();
    for i in 0..7 {
        deep = deep
            .join(format!(
                "긴 경로 long path segment 0123456789 abcdefghij {i}"
            ))
            .unwrap();
        p.create_dir(&deep).unwrap();
    }
    let file = deep.join("마지막 파일 🦝 final.txt").unwrap();
    assert!(file.to_string().encode_utf16().count() > 300);
    write_all(p, &file, WriteMode::CreateNew, b"deep");
    assert_eq!(read_all(p, &file), b"deep");
    assert_eq!(
        names(p, &deep),
        BTreeSet::from([OsString::from("마지막 파일 🦝 final.txt")])
    );
    let renamed = deep.join("renamed.txt").unwrap();
    p.rename(&file, &renamed, RenameMode::NoReplace).unwrap();
    p.delete(&top, DeleteMode::Permanent).unwrap();
    assert_eq!(kind_of(p.stat(&renamed)), ErrorKind::NotFound);
}
