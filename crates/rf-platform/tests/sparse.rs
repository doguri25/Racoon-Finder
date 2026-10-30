use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use rf_core::{ErrorKind, Scheme, VPath};
use rf_platform::{data_ranges, file_data_ranges, mark_sparse};

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

/// Whether the file at `path` keeps holes where it was written: NTFS does, and on Unix the
/// file has fewer blocks than its length needs (`st_blocks` counts units of 512 bytes). A test
/// may skip only where this is false, never where holes are there to be found.
fn keeps_holes(path: &VPath) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path.as_local().unwrap()).unwrap();
        meta.blocks().saturating_mul(512) < meta.len()
    }
    #[cfg(windows)]
    {
        let _ = path;
        true
    }
}

#[test]
fn holes_are_the_gaps_between_data_ranges() {
    let t = tempfile::tempdir().unwrap();
    let file = holey(t.path());
    let found = data_ranges(&file).unwrap();
    // macOS never reports holes, because its answers can hide data.
    if cfg!(target_os = "macos") {
        assert_eq!(found, None);
        return;
    }
    let Some(ranges) = found else {
        // A file that has holes must have them found; only one without may skip.
        assert!(
            !keeps_holes(&file),
            "the file has holes, but none were found"
        );
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

/// An open file answers for itself, also after an app saved over its path by replacing it.
#[test]
fn an_open_file_answers_for_itself() {
    let t = tempfile::tempdir().unwrap();
    let path = holey(t.path());
    let local = path.as_local().unwrap().to_path_buf();
    let opened = std::fs::File::open(&local).unwrap();
    let before = file_data_ranges(&opened).unwrap();
    // Off macOS, a file with holes has them found, so the rest is no vacuous pass.
    let found_where_kept = cfg!(target_os = "macos") || !keeps_holes(&path) || before.is_some();
    assert!(found_where_kept, "the file has holes, but none were found");
    assert_eq!(
        before.as_ref().map(|found| &found.ranges),
        data_ranges(&path).unwrap().as_ref()
    );
    assert!(before.as_ref().is_none_or(|found| found.len == 8 * MIB));
    let dense = t.path().join("dense.bin");
    std::fs::write(&dense, vec![7u8; 256 * 1024]).unwrap();
    std::fs::rename(&dense, &local).unwrap();
    assert_eq!(data_ranges(&path).unwrap(), None);
    assert_eq!(file_data_ranges(&opened).unwrap(), before);
}

/// Asking an open file for its ranges leaves its offset where it was, so a reader of it loses
/// nothing.
#[test]
fn an_open_file_keeps_its_offset() {
    let t = tempfile::tempdir().unwrap();
    let path = holey(t.path());
    let mut opened = std::fs::File::open(path.as_local().unwrap()).unwrap();
    let mut first = [0; 2];
    opened.read_exact(&mut first).unwrap();
    file_data_ranges(&opened).unwrap();
    let mut next = [0; 2];
    opened.read_exact(&mut next).unwrap();
    assert_eq!((first, next), (*b"he", *b"ad"));
}

#[test]
fn non_local_is_unsupported() {
    let r = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(data_ranges(&r).unwrap_err().kind(), ErrorKind::Unsupported);
}
