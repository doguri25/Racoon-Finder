use std::collections::BTreeSet;
use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use rf_core::VPath;
use rf_vfs::{Change, LocalProvider, Provider, WatchEvent, WatchHandle, testkit};

const WAIT: Duration = Duration::from_secs(10);

fn start(p: LocalProvider, dir: &VPath) -> (WatchHandle, mpsc::Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let h = p
        .watch(
            dir,
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    (h, rx)
}

fn apply(model: &mut BTreeSet<OsString>, ev: WatchEvent, p: LocalProvider, dir: &VPath) {
    match ev {
        WatchEvent::Changes(changes) => {
            for c in changes {
                match c {
                    Change::Upsert(e) => {
                        model.insert(e.name);
                    }
                    Change::Removed(n) => {
                        model.remove(&n);
                    }
                    Change::Renamed { from, to } => {
                        model.remove(&from);
                        model.insert(to.name);
                    }
                }
            }
        }
        WatchEvent::Rescan => *model = testkit::names(&p, dir),
        WatchEvent::Ended => panic!("watch ended unexpectedly"),
    }
}

fn wait_for(
    rx: &mpsc::Receiver<WatchEvent>,
    model: &mut BTreeSet<OsString>,
    p: LocalProvider,
    dir: &VPath,
    want: &[&str],
) {
    let want: BTreeSet<OsString> = want.iter().map(OsString::from).collect();
    let deadline = Instant::now() + WAIT;
    while *model != want {
        let left = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("model {model:?} never became {want:?}"));
        if let Ok(ev) = rx.recv_timeout(left) {
            apply(model, ev, p, dir);
        }
    }
}

/// Starts a watch whose events are all appended to the returned log.
fn start_logged(p: LocalProvider, dir: &VPath) -> (WatchHandle, Arc<Mutex<Vec<WatchEvent>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink_log = Arc::clone(&log);
    let h = p
        .watch(dir, Box::new(move |e| sink_log.lock().unwrap().push(e)))
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    (h, log)
}

/// Waits until `Ended` is in the log, then checks that nothing is added to it afterwards.
fn assert_ends_once_and_stays_silent(log: &Mutex<Vec<WatchEvent>>) {
    let deadline = Instant::now() + WAIT;
    while !log.lock().unwrap().contains(&WatchEvent::Ended) {
        assert!(Instant::now() < deadline, "no Ended event");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Two idle wake-ups of the provider and a debounce window go by.
    std::thread::sleep(Duration::from_millis(1_200));
    let log = log.lock().unwrap();
    assert_eq!(log.last(), Some(&WatchEvent::Ended), "{log:?}");
    assert_eq!(
        log.iter().filter(|e| **e == WatchEvent::Ended).count(),
        1,
        "{log:?}"
    );
}

#[test]
fn watch_tracks_create_rename_and_remove() {
    let t = tempfile::tempdir().unwrap();
    let dir = VPath::local(t.path()).unwrap();
    let p = LocalProvider::new();
    let (_h, rx) = start(p, &dir);
    let mut model = BTreeSet::new();
    std::fs::write(t.path().join("a.txt"), b"1").unwrap();
    wait_for(&rx, &mut model, p, &dir, &["a.txt"]);
    std::fs::rename(t.path().join("a.txt"), t.path().join("b.txt")).unwrap();
    wait_for(&rx, &mut model, p, &dir, &["b.txt"]);
    std::fs::create_dir(t.path().join("d")).unwrap();
    wait_for(&rx, &mut model, p, &dir, &["b.txt", "d"]);
    std::fs::remove_file(t.path().join("b.txt")).unwrap();
    wait_for(&rx, &mut model, p, &dir, &["d"]);
}

/// A case-only rename leaves one entry under the new spelling, whether the volume tells the
/// two spellings apart (Linux) or not (NTFS, default APFS): a receiver that applies the events
/// must not keep the old spelling as a ghost.
#[test]
fn a_case_only_rename_leaves_one_entry() {
    let t = tempfile::tempdir().unwrap();
    let dir = VPath::local(t.path()).unwrap();
    let p = LocalProvider::new();
    let (_h, rx) = start(p, &dir);
    let mut model = BTreeSet::new();
    std::fs::write(t.path().join("b.txt"), b"1").unwrap();
    wait_for(&rx, &mut model, p, &dir, &["b.txt"]);
    std::fs::rename(t.path().join("b.txt"), t.path().join("B.TXT")).unwrap();
    wait_for(&rx, &mut model, p, &dir, &["B.TXT"]);
    // Whatever else is still on its way (a second window) must not bring the old name back.
    let deadline = Instant::now() + Duration::from_millis(600);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Ok(ev) = rx.recv_timeout(left) {
            apply(&mut model, ev, p, &dir);
        }
    }
    assert_eq!(model, BTreeSet::from([OsString::from("B.TXT")]));
}

#[test]
fn watch_ignores_nested_changes() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("sub")).unwrap();
    let dir = VPath::local(t.path()).unwrap();
    let p = LocalProvider::new();
    let (_h, rx) = start(p, &dir);
    std::fs::write(t.path().join("sub").join("inner.txt"), b"x").unwrap();
    std::fs::write(t.path().join("z.txt"), b"x").unwrap();
    let deadline = Instant::now() + WAIT;
    let mut seen = BTreeSet::new();
    while !seen.contains(&OsString::from("z.txt")) {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("z.txt never reported");
        match rx.recv_timeout(left) {
            Ok(WatchEvent::Changes(changes)) => {
                seen.extend(changes.into_iter().map(|change| match change {
                    Change::Upsert(entry) => entry.name,
                    Change::Removed(name) => name,
                    Change::Renamed { to, .. } => to.name,
                }));
            }
            Ok(other) => panic!("unexpected {other:?}"),
            Err(_) => {}
        }
    }
    assert!(seen.iter().all(|n| n == "sub" || n == "z.txt"), "{seen:?}");
}

#[test]
fn dropping_the_handle_stops_the_sink() {
    let t = tempfile::tempdir().unwrap();
    let dir = VPath::local(t.path()).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let h = LocalProvider::new()
        .watch(
            &dir,
            Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(t.path().join("a.txt"), b"x").unwrap();
    let deadline = Instant::now() + WAIT;
    while count.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "no event");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(h);
    let n = count.load(Ordering::SeqCst);
    std::fs::write(t.path().join("b.txt"), b"x").unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(count.load(Ordering::SeqCst), n);
}

/// The notify backend may call its handler once more after the watch is dropped, and events
/// that are already queued reach the pump of the provider after the drop. None of them may
/// reach the sink.
#[test]
fn the_sink_stays_silent_after_the_drop_while_events_keep_arriving() {
    let t = tempfile::tempdir().unwrap();
    let dir = VPath::local(t.path()).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let h = LocalProvider::new()
        .watch(
            &dir,
            Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // A writer keeps changing the folder before, during and after the drop.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (stop, root) = (Arc::clone(&stop), t.path().to_path_buf());
        std::thread::spawn(move || {
            let mut i = 0_u32;
            while !stop.load(Ordering::SeqCst) {
                let _ = std::fs::write(root.join(format!("f{}.txt", i % 50)), b"x");
                i += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let deadline = Instant::now() + WAIT;
    while count.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline, "fewer than two events");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(h);
    let n = count.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(700));
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), n);
}

#[test]
fn removing_the_watched_dir_ends_the_watch() {
    let t = tempfile::tempdir().unwrap();
    let sub = t.path().join("watched");
    std::fs::create_dir(&sub).unwrap();
    let dir = VPath::local(&sub).unwrap();
    let (_h, log) = start_logged(LocalProvider::new(), &dir);
    std::fs::remove_dir_all(&sub).unwrap();
    assert_ends_once_and_stays_silent(&log);
}

/// Windows does not report a renamed or moved watched folder, so the provider notices it
/// itself.
#[test]
fn renaming_the_watched_dir_ends_the_watch() {
    let t = tempfile::tempdir().unwrap();
    let sub = t.path().join("watched");
    std::fs::create_dir(&sub).unwrap();
    let dir = VPath::local(&sub).unwrap();
    let (_h, log) = start_logged(LocalProvider::new(), &dir);
    std::fs::rename(&sub, t.path().join("moved")).unwrap();
    assert_ends_once_and_stays_silent(&log);
}

/// A new folder under the old name is not the folder that was watched.
#[test]
fn replacing_the_watched_dir_ends_the_watch() {
    let t = tempfile::tempdir().unwrap();
    let sub = t.path().join("watched");
    std::fs::create_dir(&sub).unwrap();
    let dir = VPath::local(&sub).unwrap();
    let (_h, log) = start_logged(LocalProvider::new(), &dir);
    std::fs::rename(&sub, t.path().join("moved")).unwrap();
    std::fs::create_dir(&sub).unwrap();
    assert_ends_once_and_stays_silent(&log);
}

#[test]
fn watching_a_missing_folder_fails() {
    let t = tempfile::tempdir().unwrap();
    let dir = VPath::local(t.path().join("nope")).unwrap();
    let err = LocalProvider::new()
        .watch(&dir, Box::new(|_| {}))
        .unwrap_err();
    assert_eq!(err.kind(), rf_core::ErrorKind::NotFound);
}

#[test]
fn watching_a_non_local_path_is_unsupported() {
    let remote = VPath::new(rf_core::Scheme::Remote, "p", &["x"]).unwrap();
    let err = LocalProvider::new()
        .watch(&remote, Box::new(|_| {}))
        .unwrap_err();
    assert_eq!(err.kind(), rf_core::ErrorKind::Unsupported);
}
