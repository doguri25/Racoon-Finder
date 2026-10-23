use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use rf_core::{ErrorKind, Scheme, VPath};
use rf_platform::{data_ranges, mark_sparse};

const MIB: u64 = 1 << 20;

/// An 8 MiB file with data only at 0 and at 4 MiB.
fn holey(dir: &Path) -> VPath {
    let p = dir.join("holey.bin");
    let mut f = std::fs::File::create(&p).unwrap();
    mark_sparse(&f).unwrap();
    f.write_all(b"head").unwrap();
    f.seek(SeekFrom::Start(4 * MIB)).unwrap();
    f.write_all(b"mid").unwrap();
    f.set_len(8 * MIB).unwrap();
    VPath::local(&p).unwrap()
}

#[test]
fn holes_are_the_gaps_between_data_ranges() {
    let t = tempfile::tempdir().unwrap();
    let Some(ranges) = data_ranges(&holey(t.path())).unwrap() else {
        // NTFS and APFS keep holes, so there the test must not be skipped.
        let keeps_holes = cfg!(any(windows, target_os = "macos"));
        assert!(!keeps_holes, "NTFS and APFS keep holes");
        eprintln!("skipped: this temp file system keeps no holes");
        return;
    };
    assert!(
        ranges.windows(2).all(|w| w[0].end <= w[1].start),
        "{ranges:?}"
    );
    assert!(
        ranges.iter().any(|r| r.contains(&0)) && ranges.iter().any(|r| r.contains(&(4 * MIB))),
        "{ranges:?}"
    );
    assert!(
        ranges.iter().map(|r| r.end - r.start).sum::<u64>() < 8 * MIB,
        "{ranges:?}"
    );
    assert!(
        ranges.last().is_some_and(|r| r.end <= 8 * MIB),
        "{ranges:?}"
    );
}

#[test]
fn dense_files_have_no_ranges() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("dense.bin");
    std::fs::write(&p, vec![7u8; 256 * 1024]).unwrap();
    assert_eq!(data_ranges(&VPath::local(&p).unwrap()).unwrap(), None);
}

#[test]
fn non_local_is_unsupported() {
    let r = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(data_ranges(&r).unwrap_err().kind(), ErrorKind::Unsupported);
}
