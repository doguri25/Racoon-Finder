#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::io::{Seek, SeekFrom, Write};

use common::*;
use rf_jobs::Outcome;

const MIB: u64 = 1 << 20;

#[test]
fn sparse_files_stay_sparse() {
    let f = fx();
    let a = f.src.join("disk.img");
    let mut file = std::fs::File::create(&a).unwrap();
    rf_platform::mark_sparse(&file).unwrap();
    file.write_all(&pattern(MIB)).unwrap();
    file.seek(SeekFrom::Start(63 * MIB)).unwrap();
    file.write_all(&pattern(MIB)).unwrap();
    drop(file);
    if rf_platform::data_ranges(&v(&a)).unwrap().is_none() {
        // NTFS and APFS keep holes, so there the test must not be skipped.
        let keeps_holes = cfg!(any(windows, target_os = "macos"));
        assert!(!keeps_holes, "NTFS and APFS keep holes");
        eprintln!("skipped: this temp file system keeps no holes");
        return;
    }
    let r = run(&copy(std::slice::from_ref(&a), &f.dst));
    assert_eq!(r.outcome, Outcome::Completed, "{:?}", r.failures);
    assert_eq!((r.planned.bytes, r.done.bytes), (64 * MIB, 64 * MIB));
    let b = f.dst.join("disk.img");
    assert_eq!(std::fs::read(&b).unwrap(), std::fs::read(&a).unwrap());
    let ranges = rf_platform::data_ranges(&v(&b))
        .unwrap()
        .expect("the copy has holes too");
    assert!(
        ranges.iter().map(|x| x.end - x.start).sum::<u64>() < 16 * MIB,
        "{ranges:?}"
    );
}

#[test]
fn a_trailing_hole_keeps_the_file_size() {
    let f = fx();
    let a = f.src.join("tail.img");
    let mut file = std::fs::File::create(&a).unwrap();
    rf_platform::mark_sparse(&file).unwrap();
    file.write_all(b"x").unwrap();
    file.set_len(16 * MIB).unwrap();
    drop(file);
    assert_eq!(
        run(&copy(std::slice::from_ref(&a), &f.dst)).outcome,
        Outcome::Completed
    );
    let b = f.dst.join("tail.img");
    assert_eq!(std::fs::metadata(&b).unwrap().len(), 16 * MIB);
    assert_eq!(std::fs::read(&b).unwrap(), std::fs::read(&a).unwrap());
}
