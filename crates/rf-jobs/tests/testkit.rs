#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::io::Write;

use common::*;
use rf_jobs::testkit::{Fault, FaultyProvider, Node, tree, write_tree};
use rf_jobs::{JobId, temp_name};
use rf_vfs::{Provider, RenameMode, WriteMode};

#[test]
fn tree_snapshots_files_and_folders() {
    let f = fx();
    write_tree(&f.src, &[("a.txt", "A"), ("d/b.txt", "B"), ("e/", "")]);
    assert_eq!(
        tree(&f.src).into_iter().collect::<Vec<_>>(),
        [
            ("a.txt".to_string(), Node::File(b"A".to_vec())),
            ("d".to_string(), Node::Dir),
            ("d/b.txt".to_string(), Node::File(b"B".to_vec())),
            ("e".to_string(), Node::Dir),
        ]
    );
}

#[test]
fn faulty_provider_fails_the_nth_write() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::FailWrite { file: 1, after: 2 }]);
    let write = |name: &str| {
        let mut s = p
            .open_write(&v(f.dst.join(name)), WriteMode::CreateNew)
            .unwrap();
        s.write_all(b"abc").map(|()| s.finish())
    };
    assert!(write("0").is_ok());
    assert!(write("1").is_err());
    assert_eq!(p.opened_writes(), 2);
}

#[test]
fn cross_device_spares_temp_renames() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    std::fs::write(f.dst.join("a"), b"a").unwrap();
    let err = p
        .rename(
            &v(f.dst.join("a")),
            &v(f.dst.join("b")),
            RenameMode::NoReplace,
        )
        .unwrap_err();
    assert!(
        matches!(&err, rf_core::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::CrossesDevices)
    );
    let tmp = f.dst.join(temp_name(JobId(1), 0));
    std::fs::write(&tmp, b"t").unwrap();
    p.rename(&v(&tmp), &v(f.dst.join("c")), RenameMode::NoReplace)
        .unwrap();
    assert_eq!(std::fs::read(f.dst.join("c")).unwrap(), b"t");
}
