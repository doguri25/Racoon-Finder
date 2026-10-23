//! `LocalProvider::watch`: the raw notifications of `rf_platform::watch_dir`, merged per
//! debounce window and checked with `stat`.
//!
//! Three threads are involved. The backend thread of `rf_platform` only forwards each
//! notification into a channel. The pump thread of this module collects the notifications of
//! one window, reads the entries' final state and calls the sink. The caller's thread holds
//! the `WatchHandle`; dropping it closes the sink and then stops the backend.
//!
//! Neither the pump nor the backend thread ever drops the `rf_platform::DirWatch`, because
//! dropping it can wait for the backend thread (Windows joins its watch thread, `FSEvents`
//! joins its run loop). The `DirWatch` belongs to the `LocalWatch` guard, so it is dropped on
//! whichever thread drops the `WatchHandle`.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rf_core::{Entry, Error, ErrorKind, FileId, Result, VPath, VolumeId};
use rf_platform::DirEvent;

use super::{entry_from_metadata, native};
use crate::watch::{Coalescer, WATCH_DEBOUNCE, WatchEvent, WatchHandle, WatchSink};

/// How long the pump waits for a first notification before it looks at the sink and at the
/// watched folder.
const IDLE_WAKE: Duration = Duration::from_millis(500);

/// Starts watching `dir`; see `Provider::watch`.
pub(super) fn start(dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
    let native_dir = native(dir)?;
    // Taken before the watch starts: if the folder is swapped in between, the watch is on a
    // different folder than this identity and ends at the first look, which is the safe way.
    let folder = Folder {
        path: dir.clone(),
        identity: rf_platform::file_identity(dir).ok(),
    };
    let (tx, rx) = mpsc::channel();
    let platform = rf_platform::watch_dir(
        dir,
        Box::new(move |event| {
            // The pump may be gone already (the watch ended or was dropped).
            let _ = tx.send(event);
        }),
    )?;
    let slot = Arc::new(SinkSlot::new(sink));
    let pump = Pump {
        rx,
        native_dir,
        folder,
        slot: Arc::clone(&slot),
    };
    // The thread is not joined: dropping the handle on the UI thread must not wait for it.
    std::thread::Builder::new()
        .name("vfs-watch".into())
        .spawn(move || pump.run())
        .map_err(|e| Error::io("watch", dir.clone(), e))?;
    Ok(WatchHandle::new(LocalWatch {
        slot,
        _platform: platform,
    }))
}

/// What a `WatchHandle` of the local provider keeps alive.
struct LocalWatch {
    slot: Arc<SinkSlot>,
    /// Dropped after `LocalWatch::drop` has closed the slot.
    _platform: rf_platform::DirWatch,
}

impl Drop for LocalWatch {
    fn drop(&mut self) {
        self.slot.close();
    }
}

/// The sink of one watch, shared by the pump and the `LocalWatch`.
///
/// The sink is only ever called while the lock is held and only after the lock showed it is
/// still there. `close` takes the lock too, so once it returns the sink is never called again,
/// whatever the backend or the pump still has queued. A call that is in progress is waited for.
struct SinkSlot(Mutex<Option<WatchSink>>);

impl SinkSlot {
    fn new(sink: WatchSink) -> SinkSlot {
        SinkSlot(Mutex::new(Some(sink)))
    }

    /// The lock; a sink that panicked does not make the slot unusable.
    fn lock(&self) -> MutexGuard<'_, Option<WatchSink>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_closed(&self) -> bool {
        self.lock().is_none()
    }

    /// Takes the sink away for good.
    fn close(&self) {
        // Dropped after the lock is released: the sink's destructor is the sink's business.
        let sink = self.lock().take();
        drop(sink);
    }

    /// Gives `events` to the sink in order. Returns `false` when the sink is gone or `Ended`
    /// was delivered, which closes the slot; nothing follows `Ended`.
    fn deliver(&self, events: Vec<WatchEvent>) -> bool {
        let mut guard = self.lock();
        for event in events {
            let Some(sink) = guard.as_mut() else {
                return false;
            };
            let last = event == WatchEvent::Ended;
            sink(event);
            if last {
                let sink = guard.take();
                drop(guard);
                drop(sink);
                return false;
            }
        }
        true
    }
}

/// The watched folder as it was when the watch started.
struct Folder {
    path: VPath,
    /// `None` if it could not be read then.
    identity: Option<(VolumeId, FileId)>,
}

impl Folder {
    /// Whether the folder is gone from its path: removed, moved, renamed, recycled or replaced
    /// by another folder. Windows does not report that to a watch, and it costs one `stat`, so
    /// it is checked on every OS. Any other failure to look (access, a busy volume) is not
    /// taken for a loss.
    fn is_gone(&self) -> bool {
        match rf_platform::file_identity(&self.path) {
            Ok(now) => self.identity.is_some_and(|start| start != now),
            Err(e) => matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory),
        }
    }

    /// The volume and file ID of the entry called `name` in the folder, if there is one and the
    /// OS tells. A hard link, an 8.3 short name or another spelling of the name on a
    /// case-insensitive volume has the ID of the file it names.
    fn identity_of(&self, name: &OsStr) -> Option<(VolumeId, FileId)> {
        let path = self.path.join(name).ok()?;
        rf_platform::file_identity(&path).ok()
    }
}

/// The thread that turns notifications into `WatchEvent`s.
struct Pump {
    rx: Receiver<DirEvent>,
    native_dir: PathBuf,
    folder: Folder,
    slot: Arc<SinkSlot>,
}

impl Pump {
    /// Runs until the sink is gone or `Ended` has been delivered.
    fn run(self) {
        let mut window = Coalescer::new();
        loop {
            match self.rx.recv_timeout(IDLE_WAKE) {
                Ok(first) => {
                    window.push(first);
                    self.collect(&mut window);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.slot.is_closed() {
                        return;
                    }
                    if !self.folder.is_gone() {
                        continue;
                    }
                    window.push(DirEvent::Ended);
                }
                // The backend is gone without having said so.
                Err(RecvTimeoutError::Disconnected) => window.push(DirEvent::Ended),
            }
            if !window.has_ended() && self.folder.is_gone() {
                window.push(DirEvent::Ended);
            }
            if self.slot.is_closed() {
                return;
            }
            let events = window.flush(&mut |name| self.stat(name), &mut |name| {
                self.folder.identity_of(name)
            });
            if !self.slot.deliver(events) {
                return;
            }
        }
    }

    /// Adds the notifications that arrive until `WATCH_DEBOUNCE` has passed since this call.
    fn collect(&self, window: &mut Coalescer) {
        let deadline = Instant::now() + WATCH_DEBOUNCE;
        while !window.has_ended() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match self.rx.recv_timeout(left) {
                Ok(event) => window.push(event),
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => window.push(DirEvent::Ended),
            }
        }
    }

    /// The entry now called `name` in the watched folder.
    fn stat(&self, name: &OsStr) -> Option<Entry> {
        let meta = std::fs::symlink_metadata(self.native_dir.join(name)).ok()?;
        Some(entry_from_metadata(name.to_owned(), &meta))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread::JoinHandle;

    use super::*;
    use crate::watch::Change;

    const WAIT: Duration = Duration::from_secs(10);

    /// A pump on the folder `dir` that is fed by hand (`tx`) instead of by a backend.
    struct Rig {
        tx: mpsc::Sender<DirEvent>,
        slot: Arc<SinkSlot>,
        pump: JoinHandle<()>,
    }

    fn rig(dir: &Path, sink: WatchSink) -> Rig {
        let path = VPath::local(dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let slot = Arc::new(SinkSlot::new(sink));
        let pump = Pump {
            rx,
            native_dir: native(&path).unwrap(),
            folder: Folder {
                identity: rf_platform::file_identity(&path).ok(),
                path,
            },
            slot: Arc::clone(&slot),
        };
        Rig {
            tx,
            slot,
            pump: std::thread::spawn(move || pump.run()),
        }
    }

    fn channel_sink() -> (WatchSink, Receiver<WatchEvent>) {
        let (tx, rx) = mpsc::channel();
        (
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
            rx,
        )
    }

    /// Waits for the pump thread to finish.
    fn join_within(pump: JoinHandle<()>, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !pump.is_finished() {
            assert!(Instant::now() < deadline, "the pump thread did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
        pump.join().unwrap();
    }

    #[test]
    fn a_moved_folder_ends_the_watch_without_a_backend_event() {
        let root = tempfile::tempdir().unwrap();
        let watched = root.path().join("watched");
        std::fs::create_dir(&watched).unwrap();
        let (sink, events) = channel_sink();
        // The backend (`rig.tx`) stays silent, as the Windows one does for a moved folder.
        let rig = rig(&watched, sink);
        std::fs::rename(&watched, root.path().join("moved")).unwrap();
        assert_eq!(events.recv_timeout(WAIT), Ok(WatchEvent::Ended));
        // `Ended` closed the slot and dropped the sink, so nothing can follow.
        assert_eq!(
            events.recv_timeout(WAIT),
            Err(RecvTimeoutError::Disconnected)
        );
        join_within(rig.pump, WAIT);
        drop(rig.tx);
    }

    #[test]
    fn a_folder_that_is_moved_replaced_or_removed_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let watched = root.path().join("watched");
        std::fs::create_dir(&watched).unwrap();
        let path = VPath::local(&watched).unwrap();
        let folder = Folder {
            identity: rf_platform::file_identity(&path).ok(),
            path,
        };
        assert!(!folder.is_gone());
        std::fs::rename(&watched, root.path().join("moved")).unwrap();
        assert!(folder.is_gone());
        // A new folder under the old name is not the watched one.
        std::fs::create_dir(&watched).unwrap();
        assert!(folder.is_gone());
        std::fs::remove_dir(&watched).unwrap();
        assert!(folder.is_gone());
    }

    #[test]
    fn a_name_in_the_folder_has_the_identity_of_its_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("x"), b"1").unwrap();
        std::fs::hard_link(root.path().join("x"), root.path().join("y")).unwrap();
        std::fs::write(root.path().join("z"), b"2").unwrap();
        let path = VPath::local(root.path()).unwrap();
        let folder = Folder {
            identity: None,
            path,
        };
        let x = folder.identity_of(OsStr::new("x"));
        assert!(x.is_some());
        assert_eq!(folder.identity_of(OsStr::new("y")), x);
        let z = folder.identity_of(OsStr::new("z"));
        assert!(z.is_some());
        assert_ne!(z, x);
        assert_eq!(folder.identity_of(OsStr::new("missing")), None);
    }

    /// The whole path: real files, the real `stat` and ID lookup, a pump fed by hand. Two names
    /// of one file in one window reach the sink as a rescan, not as two entries.
    #[test]
    fn two_names_of_one_file_in_a_window_reach_the_sink_as_a_rescan() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("x"), b"1").unwrap();
        std::fs::hard_link(root.path().join("x"), root.path().join("y")).unwrap();
        std::fs::write(root.path().join("z"), b"2").unwrap();
        let (sink, events) = channel_sink();
        let rig = rig(root.path(), sink);
        rig.tx.send(DirEvent::Changed("x".into())).unwrap();
        rig.tx.send(DirEvent::Changed("y".into())).unwrap();
        assert_eq!(events.recv_timeout(WAIT), Ok(WatchEvent::Rescan));
        // Another file in the next window is reported as itself.
        rig.tx.send(DirEvent::Changed("z".into())).unwrap();
        let Ok(WatchEvent::Changes(changes)) = events.recv_timeout(WAIT) else {
            panic!("expected the change of z");
        };
        assert!(
            matches!(&changes[..], [Change::Upsert(e)] if e.name == "z"),
            "{changes:?}"
        );
        rig.slot.close();
        drop(rig.tx);
        join_within(rig.pump, WAIT);
    }

    #[test]
    fn a_dropped_backend_ends_the_watch() {
        let root = tempfile::tempdir().unwrap();
        let (sink, events) = channel_sink();
        let rig = rig(root.path(), sink);
        rig.tx.send(DirEvent::Changed("a".into())).unwrap();
        drop(rig.tx);
        // Whatever came first, `Ended` is the last event.
        let mut last = None;
        while let Ok(event) = events.recv_timeout(WAIT) {
            last = Some(event);
        }
        assert_eq!(last, Some(WatchEvent::Ended));
        join_within(rig.pump, WAIT);
    }

    #[test]
    fn deliver_never_calls_a_closed_sink() {
        let calls = Arc::new(AtomicUsize::new(0));
        let slot = {
            let calls = Arc::clone(&calls);
            SinkSlot::new(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };
        assert!(slot.deliver(vec![WatchEvent::Rescan]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        slot.close();
        assert!(slot.is_closed());
        assert!(!slot.deliver(vec![WatchEvent::Rescan, WatchEvent::Ended]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn nothing_follows_ended() {
        let calls = Arc::new(AtomicUsize::new(0));
        let slot = {
            let calls = Arc::clone(&calls);
            SinkSlot::new(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };
        assert!(!slot.deliver(vec![WatchEvent::Ended, WatchEvent::Rescan]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(slot.is_closed());
    }

    /// The pump has a window ready while the sink is inside a call and the handle is dropped:
    /// the drop waits for the call, and the ready window never reaches the sink.
    #[test]
    fn a_window_that_is_ready_when_the_slot_closes_is_not_delivered() {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let sink: WatchSink = {
            let calls = Arc::clone(&calls);
            Box::new(move |_| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered_tx.send(()).unwrap();
                    released
                        .recv_timeout(WAIT)
                        .expect("the test releases the sink");
                }
            })
        };
        let rig = rig(root.path(), sink);
        // The first window reaches the sink, which stays inside the call.
        rig.tx.send(DirEvent::Changed("a".into())).unwrap();
        entered.recv_timeout(WAIT).expect("the sink is called");
        // More events arrive during the call, so a second window is due.
        rig.tx.send(DirEvent::Changed("b".into())).unwrap();
        let (closed_tx, closed) = mpsc::channel();
        let slot = Arc::clone(&rig.slot);
        let closer = std::thread::spawn(move || {
            slot.close();
            closed_tx.send(()).unwrap();
        });
        assert_eq!(
            closed.recv_timeout(Duration::from_millis(300)),
            Err(RecvTimeoutError::Timeout),
            "close must wait for the call in progress"
        );
        release.send(()).unwrap();
        closed
            .recv_timeout(WAIT)
            .expect("close returns after the call");
        closer.join().unwrap();
        // Events keep arriving after the close.
        for i in 0..20 {
            let _ = rig.tx.send(DirEvent::Changed(format!("c{i}").into()));
            std::thread::sleep(Duration::from_millis(20));
        }
        join_within(rig.pump, WAIT);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_idle_pump_stops_when_the_slot_is_closed() {
        let root = tempfile::tempdir().unwrap();
        let (sink, _events) = channel_sink();
        let rig = rig(root.path(), sink);
        rig.slot.close();
        join_within(rig.pump, WAIT);
        drop(rig.tx);
    }
}
