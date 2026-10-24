use rf_core::{Error, ErrorKind, Scheme, VPath};
use rf_platform::{open_regular_file, rename_no_replace, sync_dir, sync_file};

fn setup() -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path()).unwrap();
    (t, v)
}

fn write(v: &VPath, s: &str) {
    std::fs::write(v.as_local().unwrap(), s).unwrap();
}

fn read(v: &VPath) -> String {
    std::fs::read_to_string(v.as_local().unwrap()).unwrap()
}

#[test]
fn renames_to_a_free_name() {
    let (_t, d) = setup();
    let (a, b) = (d.join("a").unwrap(), d.join("b").unwrap());
    write(&a, "A");
    rename_no_replace(&a, &b).unwrap();
    assert!(!a.as_local().unwrap().exists());
    assert_eq!(read(&b), "A");
}

#[test]
fn refuses_to_replace_a_file() {
    let (_t, d) = setup();
    let (a, b) = (d.join("a").unwrap(), d.join("b").unwrap());
    write(&a, "A");
    write(&b, "B");
    assert_eq!(
        rename_no_replace(&a, &b).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!((read(&a), read(&b)), (String::from("A"), String::from("B")));
}

#[test]
fn refuses_to_replace_a_directory() {
    let (_t, d) = setup();
    let (src, dst) = (d.join("src").unwrap(), d.join("dst").unwrap());
    std::fs::create_dir(src.as_local().unwrap()).unwrap();
    std::fs::create_dir(dst.as_local().unwrap()).unwrap();
    write(&src.join("keep").unwrap(), "k");
    assert_eq!(
        rename_no_replace(&src, &dst).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(read(&src.join("keep").unwrap()), "k");
    assert_eq!(
        std::fs::read_dir(dst.as_local().unwrap()).unwrap().count(),
        0
    );
}

#[test]
fn missing_source_is_not_found() {
    let (_t, d) = setup();
    let err = rename_no_replace(&d.join("missing").unwrap(), &d.join("x").unwrap()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn moves_into_another_directory() {
    let (_t, d) = setup();
    let (a, sub) = (d.join("a").unwrap(), d.join("sub").unwrap());
    write(&a, "A");
    std::fs::create_dir(sub.as_local().unwrap()).unwrap();
    rename_no_replace(&a, &sub.join("a").unwrap()).unwrap();
    assert_eq!(read(&sub.join("a").unwrap()), "A");
}

#[test]
fn non_local_is_unsupported() {
    let (_t, d) = setup();
    let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(
        rename_no_replace(&remote, &d).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}

#[test]
fn error_names_the_source_and_the_operation() {
    let (_t, d) = setup();
    let (a, b) = (d.join("a").unwrap(), d.join("b").unwrap());
    write(&a, "A");
    write(&b, "B");
    let Error::Io { op, path, .. } = rename_no_replace(&a, &b).unwrap_err() else {
        panic!("expected an I/O error");
    };
    assert_eq!((op, path), ("rename", a));
}

/// Needs a second volume; `/dev/shm` is one on Linux, and the test skips where there is none.
#[cfg(unix)]
#[test]
fn another_volume_crosses_devices() {
    use std::os::unix::fs::MetadataExt;

    let (_t, d) = setup();
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        eprintln!("skipped: no /dev/shm");
        return;
    };
    let device = |p: &std::path::Path| std::fs::metadata(p).unwrap().dev();
    if device(other.path()) == device(d.as_local().unwrap()) {
        eprintln!("skipped: /dev/shm is on the same volume");
        return;
    }
    let a = d.join("a").unwrap();
    let far = VPath::local(other.path()).unwrap().join("a").unwrap();
    write(&a, "A");
    assert_eq!(
        rename_no_replace(&a, &far).unwrap_err().kind(),
        ErrorKind::CrossesDevices
    );
    assert_eq!(read(&a), "A");
    assert!(!far.as_local().unwrap().exists());
}

/// A file and a folder with new entries are made durable; a folder that is missing is an error
/// on every system, never a silent success.
#[test]
fn files_and_folders_are_synced() {
    let (t, d) = setup();
    let a = d.join("a").unwrap();
    write(&a, "A");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(a.as_local().unwrap())
        .unwrap();
    sync_file(&file).unwrap();
    let native = d.to_extended_length().unwrap();
    sync_dir(&native).unwrap();
    let missing = sync_dir(&native.join("missing")).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    // A file is no folder to sync.
    assert!(sync_dir(&t.path().join("a")).is_err());
}

/// A regular file opens and reads from its start; a folder is refused.
#[test]
fn only_a_regular_file_opens_for_reading() {
    use std::io::Read;

    let (t, d) = setup();
    write(&d.join("a").unwrap(), "content");
    let mut file = open_regular_file(&t.path().join("a")).unwrap();
    let mut read = String::new();
    file.read_to_string(&mut read).unwrap();
    assert_eq!(read, "content");
    let missing = open_regular_file(&t.path().join("missing")).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    assert!(open_regular_file(t.path()).is_err());
}

/// The end of a FIFO with no writer would block a plain open for good; both the open for
/// reading and the folder sync refuse it at once instead.
#[cfg(unix)]
#[test]
fn a_fifo_never_blocks_an_open() {
    use std::sync::mpsc;
    use std::time::Duration;

    let t = tempfile::tempdir().unwrap();
    let fifo = t.path().join("fifo");
    // `mkfifo(1)` is on Linux and macOS alike; rustix has no `mkfifoat` on macOS.
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    assert!(made.unwrap().success(), "mkfifo failed");
    let (tx, rx) = mpsc::channel();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let read = open_regular_file(&path).map(drop);
        let synced = sync_dir(&path);
        let _ = tx.send((read, synced));
    });
    let Ok((read, synced)) = rx.recv_timeout(Duration::from_secs(10)) else {
        // Opening the other end lets the blocked open go on, so the thread ends.
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        panic!("an open of a FIFO blocked");
    };
    let read = read.unwrap_err();
    assert_eq!(read.kind(), std::io::ErrorKind::Unsupported, "{read}");
    assert_eq!(read.to_string(), "not a regular file");
    assert!(synced.is_err());
}
