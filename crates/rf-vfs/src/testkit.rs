//! Provider conformance checks (PRD 9). Each check works in a new folder under `root` and panics on failure.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};

use rf_core::{EntryKind, ErrorKind, VPath};

use crate::{CancelToken, LIST_BATCH_SIZE, Provider, WriteMode};

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
