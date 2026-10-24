use std::sync::mpsc;
use std::time::{Duration, Instant};

use rf_core::{Error, ErrorKind, Scheme, VPath};
use rf_platform::{DirEvent, watch_dir};

#[test]
fn reports_a_new_file() {
    let t = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let _w = watch_dir(
        &VPath::local(t.path()).unwrap(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(t.path().join("a.txt"), b"x").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(DirEvent::Changed(n) | DirEvent::Renamed { to: n, .. }) if n == "a.txt" => {
                seen = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(seen, "no event for a.txt");
}

#[test]
fn missing_dir_is_not_found() {
    let t = tempfile::tempdir().unwrap();
    let missing = VPath::local(t.path().join("missing")).unwrap();
    assert_eq!(
        watch_dir(&missing, Box::new(|_| {})).unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

/// Reads events until `found` says yes, for at most 10 seconds, so that a missing event fails
/// the test and does not hang it.
fn recv_until(rx: &mpsc::Receiver<DirEvent>, mut found: impl FnMut(&DirEvent) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(event) if found(&event) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Runs `work` on another thread and fails the test, instead of hanging it, when `work` does not
/// finish within 30 seconds.
fn finishes_in_time(work: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        work();
        let _ = tx.send(());
    });
    if let Err(e) = rx.recv_timeout(Duration::from_secs(30)) {
        panic!("the work did not finish: {e}");
    }
}

fn watch_channel(dir: &std::path::Path) -> (rf_platform::DirWatch, mpsc::Receiver<DirEvent>) {
    let (tx, rx) = mpsc::channel();
    let watch = watch_dir(
        &VPath::local(dir).unwrap(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .unwrap();
    (watch, rx)
}

#[test]
fn a_file_is_not_a_directory() {
    let t = tempfile::tempdir().unwrap();
    let file = t.path().join("f.txt");
    std::fs::write(&file, b"x").unwrap();
    let err = watch_dir(&VPath::local(&file).unwrap(), Box::new(|_| {})).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotADirectory);
    assert!(matches!(err, Error::Io { op: "watch", .. }));
}

#[test]
fn non_local_is_unsupported() {
    let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(
        watch_dir(&remote, Box::new(|_| {})).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}

#[test]
fn reports_both_names_of_a_rename() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("old.txt"), b"x").unwrap();
    let (_w, rx) = watch_channel(t.path());
    std::thread::sleep(Duration::from_millis(200));
    std::fs::rename(t.path().join("old.txt"), t.path().join("new.txt")).unwrap();
    // Each OS tells both names, as one `Renamed` or as two `Changed`.
    let (mut old_seen, mut new_seen) = (false, false);
    recv_until(&rx, |event| {
        match event {
            DirEvent::Changed(n) => {
                old_seen |= n == "old.txt";
                new_seen |= n == "new.txt";
            }
            DirEvent::Renamed { from, to } => {
                old_seen |= from == "old.txt";
                new_seen |= to == "new.txt";
            }
            DirEvent::Overflow | DirEvent::Ended => {}
        }
        old_seen && new_seen
    });
    assert!(
        old_seen && new_seen,
        "old seen: {old_seen}, new seen: {new_seen}"
    );
}

#[test]
fn ignores_changes_inside_subfolders() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("sub")).unwrap();
    let (_w, rx) = watch_channel(t.path());
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(t.path().join("sub").join("inner.txt"), b"x").unwrap();
    std::fs::write(t.path().join("top.txt"), b"x").unwrap();
    let mut inner_seen = false;
    let top_seen = recv_until(&rx, |event| {
        if let DirEvent::Changed(n) | DirEvent::Renamed { to: n, .. } = event {
            inner_seen |= n == "inner.txt";
            return n == "top.txt";
        }
        false
    });
    assert!(top_seen, "no event for top.txt");
    assert!(!inner_seen, "an event for a file inside a subfolder");
}

#[test]
fn removing_the_folder_ends_the_watch() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("watched");
    std::fs::create_dir(&dir).unwrap();
    let (_w, rx) = watch_channel(&dir);
    std::thread::sleep(Duration::from_millis(200));
    std::fs::remove_dir(&dir).unwrap();
    assert!(
        recv_until(&rx, |event| *event == DirEvent::Ended),
        "no Ended event"
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "an event after Ended"
    );
}

#[test]
fn dropping_a_watch_does_not_hang() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().to_path_buf();
    finishes_in_time(move || {
        // Dropped at once, while the backend is still starting up, and dropped when idle.
        for _ in 0..20 {
            drop(watch_channel(&dir));
        }
        let (watch, _rx) = watch_channel(&dir);
        std::thread::sleep(Duration::from_millis(200));
        drop(watch);
    });
}

#[cfg(windows)]
#[test]
fn no_event_arrives_after_the_drop_returns() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let t = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel();
    let counter = Arc::clone(&calls);
    let watch = watch_dir(
        &VPath::local(t.path()).unwrap(),
        Box::new(move |e| {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(e);
        }),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(t.path().join("first.txt"), b"x").unwrap();
    assert!(
        recv_until(&rx, |_| true),
        "the watch never reported anything"
    );
    drop(watch);
    let after_drop = calls.load(Ordering::SeqCst);
    for i in 0..10 {
        std::fs::write(t.path().join(format!("later{i}.txt")), b"x").unwrap();
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(calls.load(Ordering::SeqCst), after_drop);
}
