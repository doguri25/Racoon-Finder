//! The loader against real folders: listing on a worker thread, live changes, cancelling and a
//! folder that disappears.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use rf_core::{ErrorKind, VPath};
use rf_ui::loader::{LoadEvent, LoadRequest, LoadTicket, Loader};
use rf_ui::pane::{Handled, LoadStatus, Pane};
use rf_vfs::{Change, SortSpec, Vfs, VisibilityFilter};

/// The longest any test waits for an event.
const WAIT: Duration = Duration::from_secs(5);

fn loader() -> Loader {
    Loader::new(Vfs::with_local())
}

fn local(path: &Path) -> VPath {
    VPath::local(path).expect("a temporary folder is a local path")
}

fn request(path: VPath) -> LoadRequest {
    LoadRequest {
        generation: 1,
        path,
        sort: SortSpec::default(),
    }
}

/// A sink that sends every event into `tx`.
fn sender(tx: &Sender<LoadEvent>) -> impl FnMut(LoadEvent) + Send + 'static {
    let tx = tx.clone();
    move |event| {
        // The test may have stopped listening.
        let _ = tx.send(event);
    }
}

/// The time left until `deadline`, or a panic naming `what` once it has passed.
fn left(deadline: Instant, what: &str) -> Duration {
    deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_else(|| panic!("timed out waiting for {what}"))
}

/// Receives events until one matches `wanted` and returns it.
fn recv_until<T>(rx: &Receiver<T>, what: &str, wanted: impl Fn(&T) -> bool) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        match rx.recv_timeout(left(deadline, what)) {
            Ok(event) if wanted(&event) => return event,
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("the sink was dropped before {what}"),
        }
    }
}

/// A pane fed by a loader, as the app's controller feeds it.
struct Live {
    loader: Loader,
    pane: Pane,
    ticket: LoadTicket,
    tx: Sender<LoadEvent>,
    rx: Receiver<LoadEvent>,
    /// The last `Handled::Vanished` the pane reported: the generation and the folder.
    vanished: Option<(u64, VPath)>,
}

impl Live {
    fn open(path: VPath) -> Live {
        let loader = loader();
        let (pane, request) = Pane::new(path, VisibilityFilter::default());
        let (tx, rx) = mpsc::channel();
        let ticket = loader.request(request, sender(&tx));
        Live {
            loader,
            pane,
            ticket,
            tx,
            rx,
            vanished: None,
        }
    }

    /// Hands events to the pane until `done` holds, reloading on a rescan.
    fn wait_for(&mut self, what: &str, done: impl Fn(&Live) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self) {
            let event = match self.rx.recv_timeout(left(deadline, what)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => unreachable!("the test holds a sender"),
            };
            match self.pane.handle(event) {
                Handled::Reload { request, .. } => {
                    self.ticket = self.loader.request(request, sender(&self.tx));
                }
                Handled::Vanished {
                    generation, path, ..
                } => self.vanished = Some((generation, path)),
                Handled::Applied { .. } | Handled::Stale => {}
            }
        }
    }

    fn shows(&self, name: &str) -> bool {
        let tab = self.pane.active_tab();
        tab.status() == LoadStatus::Ready && tab.visible().iter().any(|entry| entry.name == name)
    }

    fn ready(&self) -> bool {
        self.pane.active_tab().status() == LoadStatus::Ready
    }
}

#[test]
fn listing_runs_off_the_ui_thread() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.txt"), b"a").unwrap();
    let (tx, rx) = mpsc::channel::<(ThreadId, LoadEvent)>();
    let ticket = loader().request(request(local(t.path())), move |event| {
        let _ = tx.send((thread::current().id(), event));
    });
    let (listed_on, listed) = recv_until(&rx, "the listing", |(_, event)| {
        matches!(event, LoadEvent::Listed { .. })
    });
    assert_ne!(listed_on, thread::current().id());
    assert_eq!(listed.generation(), 1);
    let (space_on, _) = recv_until(&rx, "the free space", |(_, event)| {
        matches!(event, LoadEvent::Space { .. })
    });
    assert_ne!(space_on, thread::current().id());
    assert_eq!(ticket.generation(), 1);
}

#[test]
fn lists_and_sorts_a_folder() {
    let t = tempfile::tempdir().unwrap();
    for file in ["file10.txt", "file2.txt", "B.txt"] {
        std::fs::write(t.path().join(file), b"x").unwrap();
    }
    for dir in ["zeta", "Alpha"] {
        std::fs::create_dir(t.path().join(dir)).unwrap();
    }
    let (tx, rx) = mpsc::channel();
    let _ticket = loader().request(request(local(t.path())), sender(&tx));
    let LoadEvent::Listed {
        generation,
        entries,
    } = recv_until(&rx, "the listing", |event| {
        matches!(event, LoadEvent::Listed { .. })
    })
    else {
        unreachable!("only a listing matches");
    };
    assert_eq!(generation, 1);
    let names: Vec<&OsStr> = entries.iter().map(|entry| entry.name.as_os_str()).collect();
    assert_eq!(names, ["Alpha", "zeta", "B.txt", "file2.txt", "file10.txt"]);
    let LoadEvent::Space { available, .. } = recv_until(&rx, "the free space", |event| {
        matches!(event, LoadEvent::Space { .. })
    }) else {
        unreachable!("only the free space matches");
    };
    assert!(available > 0);
}

#[test]
fn a_missing_folder_fails() {
    let t = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let _ticket = loader().request(request(local(&t.path().join("missing"))), sender(&tx));
    let event = recv_until(&rx, "the failure", |event| {
        !matches!(event, LoadEvent::Changes { .. })
    });
    assert_eq!(
        event,
        LoadEvent::Failed {
            generation: 1,
            kind: ErrorKind::NotFound,
        }
    );
}

#[test]
fn watch_reports_new_and_removed_files() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("old.txt"), b"old").unwrap();
    let mut live = Live::open(local(t.path()));
    live.wait_for("the listing", |live| live.shows("old.txt"));

    std::fs::write(t.path().join("new.txt"), b"new").unwrap();
    live.wait_for("the new file", |live| live.shows("new.txt"));

    std::fs::remove_file(t.path().join("old.txt")).unwrap();
    live.wait_for("the removal", |live| !live.shows("old.txt"));

    std::fs::rename(t.path().join("new.txt"), t.path().join("renamed.txt")).unwrap();
    live.wait_for("the rename", |live| {
        live.shows("renamed.txt") && !live.shows("new.txt")
    });
    assert_eq!(live.pane.active_tab().visible().len(), 1);
    assert_eq!(live.vanished, None);
}

#[test]
fn cancel_stops_events() {
    let t = tempfile::tempdir().unwrap();
    let path = local(t.path());

    // Cancelled while the watch runs: the sink is released at once and hears of nothing
    // that happens afterwards.
    let (tx, rx) = mpsc::channel();
    let ticket = loader().request(request(path.clone()), sender(&tx));
    drop(tx);
    recv_until(&rx, "the listing", |event| {
        matches!(event, LoadEvent::Listed { .. })
    });
    ticket.cancel();
    std::fs::write(t.path().join("after.txt"), b"x").unwrap();
    assert_released(&rx, "after.txt");

    // Dropping the ticket cancels too, even before the worker has started.
    let (tx, rx) = mpsc::channel();
    let ticket = loader().request(request(path), sender(&tx));
    drop(tx);
    drop(ticket);
    assert_released(&rx, "after.txt");
}

/// Takes what is left in `rx` until its only sender, the sink, is dropped, and checks that no
/// change of `name` is among it.
fn assert_released(rx: &Receiver<LoadEvent>, name: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        match rx.recv_timeout(left(deadline, "the sink to be released")) {
            Ok(LoadEvent::Changes { changes, .. }) => {
                assert!(
                    !changes.iter().any(|change| match change {
                        Change::Upsert(entry) | Change::Renamed { to: entry, .. } =>
                            entry.name == name,
                        Change::Removed(removed) => removed == name,
                    }),
                    "a change of {name} arrived after the cancel: {changes:?}"
                );
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[test]
fn vanished_folder_goes_to_nearest_parent() {
    let t = tempfile::tempdir().unwrap();
    let inner = t.path().join("a").join("b");
    std::fs::create_dir_all(&inner).unwrap();
    let mut live = Live::open(local(&inner));
    live.wait_for("the listing", Live::ready);

    std::fs::remove_dir_all(t.path().join("a")).unwrap();
    live.wait_for("the folder to vanish", |live| live.vanished.is_some());
    let (generation, gone) = live.vanished.take().expect("the folder vanished");
    assert_eq!(gone, local(&inner));

    // The UI looks for the nearest folder off its thread and goes there.
    let parent = live
        .loader
        .nearest_existing_parent(&gone)
        .expect("the temporary folder is still there");
    assert_eq!(parent, local(t.path()));
    let tab = live
        .pane
        .tab_by_generation_mut(generation)
        .expect("the tab is still at the vanished folder");
    let request = tab.navigate(parent.clone());
    live.ticket = live.loader.request(request, sender(&live.tx));
    live.wait_for("the parent's listing", Live::ready);
    assert_eq!(live.pane.active_tab().location(), &parent);
    assert_eq!(live.pane.active_tab().visible(), []);

    // A folder that is still there is its own nearest folder.
    assert_eq!(live.loader.nearest_existing_parent(&parent), Some(parent));
}

#[cfg(unix)]
#[test]
fn links_open_by_what_they_lead_to() {
    use rf_ui::pane::OpenAction;

    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("dir")).unwrap();
    std::fs::write(t.path().join("file"), b"x").unwrap();
    std::os::unix::fs::symlink("dir", t.path().join("to-dir")).unwrap();
    std::os::unix::fs::symlink("file", t.path().join("to-file")).unwrap();
    std::os::unix::fs::symlink("nothing", t.path().join("dangling")).unwrap();
    let loader = loader();
    let at = |name: &str| local(&t.path().join(name));
    assert_eq!(
        loader.follow(&at("to-dir")),
        Ok(OpenAction::Navigate(at("to-dir")))
    );
    assert_eq!(
        loader.follow(&at("to-file")),
        Ok(OpenAction::OpenFile(at("to-file")))
    );
    assert_eq!(loader.follow(&at("dangling")), Err(ErrorKind::NotFound));
}
