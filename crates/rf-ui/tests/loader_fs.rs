//! The loader against real folders: listing on a worker thread, live changes, cancelling, a
//! folder that disappears or is replaced, and links.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use rf_core::{Entry, ErrorKind, Scheme, VPath};
use rf_ui::loader::{LoadEvent, LoadRequest, LoadTicket, Loader};
use rf_ui::pane::{Handled, LoadStatus, Pane};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LocalProvider, Provider, ReadStream,
    RenameMode, SortSpec, Vfs, VisibilityFilter, WatchHandle, WatchSink, WriteMode, WriteStream,
};

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

/// Waits until the only sender of `rx`, the sink of a cancelled ticket, is dropped, and checks
/// that it sent nothing.
fn assert_no_events(rx: &Receiver<LoadEvent>) {
    let deadline = Instant::now() + WAIT;
    loop {
        match rx.recv_timeout(left(deadline, "the sink to be released")) {
            Ok(event) => panic!("a cancelled request sent {event:?}"),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// A pane fed by a loader, the way the app's controller feeds it.
struct Live {
    loader: Loader,
    pane: Pane,
    ticket: LoadTicket,
    tx: Sender<LoadEvent>,
    rx: Receiver<LoadEvent>,
    /// Each folder the pane reported as vanished, with the folder it went to instead.
    vanished: Vec<(VPath, Option<VPath>)>,
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
            vanished: Vec::new(),
        }
    }

    /// Starts `request` in place of the running one.
    fn load(&mut self, request: LoadRequest) {
        self.ticket = self.loader.request(request, sender(&self.tx));
    }

    /// Hands events to the pane until `done` holds or `deadline` passes, and says whether it
    /// holds. It follows the controller's protocol: a rescan reloads, and the tab of a
    /// vanished folder goes to the nearest folder there is, which lists the folder itself
    /// again when it is back.
    fn pump_until(&mut self, deadline: Instant, done: impl Fn(&Live) -> bool) -> bool {
        while !done(self) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let event = match self.rx.recv_timeout(left) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => return done(self),
                Err(RecvTimeoutError::Disconnected) => unreachable!("the test holds a sender"),
            };
            match self.pane.handle(event) {
                Handled::Reload { request, .. } => self.load(request),
                Handled::Vanished {
                    generation, path, ..
                } => {
                    let found = self.loader.nearest_existing_parent(&path);
                    self.vanished.push((path, found.clone()));
                    let request = found.and_then(|found| {
                        Some(self.pane.tab_by_generation_mut(generation)?.navigate(found))
                    });
                    if let Some(request) = request {
                        self.load(request);
                    }
                }
                Handled::Applied { .. } | Handled::Stale => {}
            }
        }
        true
    }

    /// [`Live::pump_until`] for at most [`WAIT`], panicking with `what` when `done` never holds.
    fn wait_for(&mut self, what: &str, done: impl Fn(&Live) -> bool) {
        assert!(
            self.pump_until(Instant::now() + WAIT, done),
            "timed out waiting for {what}"
        );
    }

    /// Waits until the watch of the tab's folder `dir` reports changes. A watch can start a
    /// little after `Provider::watch` has returned (on Windows the OS read is set up on a
    /// thread of its own), and a change before that goes unreported. So a probe file is
    /// written every 100 ms until the listed tab shows it, which only the watch can make it
    /// do, and then removed again.
    fn arm_watch(&mut self, dir: &Path) {
        self.wait_for("the listing", Live::ready);
        let probe = dir.join("probe");
        let deadline = Instant::now() + WAIT;
        loop {
            std::fs::write(&probe, b"probe").unwrap();
            let step = (Instant::now() + Duration::from_millis(100)).min(deadline);
            if self.pump_until(step, |live| live.shows("probe")) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the watch never reported the probe"
            );
        }
        std::fs::remove_file(&probe).unwrap();
        self.wait_for("the probe to go", |live| live.lacks("probe"));
    }

    fn ready(&self) -> bool {
        self.pane.active_tab().status() == LoadStatus::Ready
    }

    /// Whether the tab is listed and shows `name`.
    fn shows(&self, name: &str) -> bool {
        self.ready()
            && self
                .pane
                .active_tab()
                .visible()
                .iter()
                .any(|entry| entry.name == name)
    }

    /// Whether the tab is listed and does not show `name`.
    fn lacks(&self, name: &str) -> bool {
        self.ready() && !self.shows(name)
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
    live.arm_watch(t.path());

    std::fs::write(t.path().join("new.txt"), b"new").unwrap();
    live.wait_for("the new file", |live| live.shows("new.txt"));

    std::fs::remove_file(t.path().join("old.txt")).unwrap();
    live.wait_for("the removal", |live| live.lacks("old.txt"));

    std::fs::rename(t.path().join("new.txt"), t.path().join("renamed.txt")).unwrap();
    live.wait_for("the rename", |live| {
        live.shows("renamed.txt") && live.lacks("new.txt")
    });
    // The probe is gone too.
    assert_eq!(live.pane.active_tab().visible().len(), 1);
    assert_eq!(live.vanished, []);
}

#[test]
fn vanished_folder_goes_to_nearest_parent() {
    let t = tempfile::tempdir().unwrap();
    let inner = t.path().join("a").join("b");
    std::fs::create_dir_all(&inner).unwrap();
    let mut live = Live::open(local(&inner));
    live.wait_for("the listing", Live::ready);

    // The folder and its parent go: the tab goes two levels up.
    std::fs::remove_dir_all(t.path().join("a")).unwrap();
    let top = local(t.path());
    live.wait_for("the tab to show the temporary folder", |live| {
        live.ready() && live.pane.active_tab().location() == &top
    });
    assert_eq!(live.vanished, [(local(&inner), Some(top.clone()))]);
    assert_eq!(live.pane.active_tab().visible(), []);

    // A folder that is still there is its own nearest folder.
    assert_eq!(live.loader.nearest_existing_parent(&top), Some(top));
}

#[test]
fn a_replaced_folder_shows_its_new_contents() {
    let t = tempfile::tempdir().unwrap();
    let dist = t.path().join("dist");
    std::fs::create_dir(&dist).unwrap();
    std::fs::write(dist.join("old.txt"), b"old").unwrap();
    let mut live = Live::open(local(&dist));
    live.wait_for("the listing", |live| live.shows("old.txt"));

    // A build replaces its output folder in one go.
    std::fs::remove_dir_all(&dist).unwrap();
    std::fs::create_dir(&dist).unwrap();
    std::fs::write(dist.join("new.txt"), b"new").unwrap();
    live.wait_for("the new folder's contents", |live| {
        live.shows("new.txt") && live.lacks("old.txt")
    });
    assert_eq!(live.pane.active_tab().location(), &local(&dist));
    assert_eq!(live.vanished, [(local(&dist), Some(local(&dist)))]);
}

/// A step the worker of a request took with a [`Gated`] provider.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// `capabilities` was called (and waits for the gate when held there).
    Capabilities,
    /// A watch was started.
    Watch,
    /// That watch was dropped.
    WatchDropped,
    /// `list` has read the folder (and waits for the gate when held there).
    Listing,
    /// `list` returned this.
    Listed(Result<(), ErrorKind>),
    /// `space` was called.
    Space,
    /// The provider was dropped: nothing uses it any more, so the worker has ended.
    Dropped,
}

/// Where a [`Gated`] provider waits for the test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    Capabilities,
    List,
}

/// The local provider, except that it reports each step of the worker into a log and waits for
/// the test at `hold`.
#[derive(Debug)]
struct Gated {
    local: LocalProvider,
    hold: Hold,
    log: Sender<Step>,
    gate: Mutex<Receiver<()>>,
}

impl Gated {
    /// A loader whose only provider is a `Gated` one, the provider's log, and the sender that
    /// opens its gate.
    fn loader(hold: Hold) -> (Loader, Receiver<Step>, Sender<()>) {
        let (log, steps) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let mut vfs = Vfs::new();
        vfs.register(Arc::new(Gated {
            local: LocalProvider::new(),
            hold,
            log,
            gate: Mutex::new(gate),
        }));
        (Loader::new(vfs), steps, open)
    }

    fn step(&self, step: Step) {
        let _ = self.log.send(step);
    }

    fn wait_at(&self, hold: Hold) {
        if self.hold == hold {
            // Bounded, so that a failing test cannot leave the worker waiting for ever.
            let _ = self.gate.lock().unwrap().recv_timeout(WAIT);
        }
    }
}

impl Drop for Gated {
    fn drop(&mut self) {
        self.step(Step::Dropped);
    }
}

/// Logs [`Step::WatchDropped`] when the watch it goes with is dropped.
struct WatchGuard(Sender<Step>);

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let _ = self.0.send(Step::WatchDropped);
    }
}

impl Provider for Gated {
    fn scheme(&self) -> Scheme {
        self.local.scheme()
    }

    fn capabilities(&self, path: &VPath) -> Capabilities {
        self.step(Step::Capabilities);
        self.wait_at(Hold::Capabilities);
        self.local.capabilities(path)
    }

    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> rf_core::Result<()> {
        let result = self.local.list(dir, cancel, sink).and_then(|()| {
            self.step(Step::Listing);
            self.wait_at(Hold::List);
            cancel.check()
        });
        self.step(Step::Listed(
            result.as_ref().copied().map_err(rf_core::Error::kind),
        ));
        result
    }

    fn stat(&self, path: &VPath) -> rf_core::Result<Entry> {
        self.local.stat(path)
    }

    fn create_dir(&self, path: &VPath) -> rf_core::Result<()> {
        self.local.create_dir(path)
    }

    fn create_file(&self, path: &VPath) -> rf_core::Result<()> {
        self.local.create_file(path)
    }

    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> rf_core::Result<()> {
        self.local.rename(from, to, mode)
    }

    fn delete(&self, path: &VPath, mode: DeleteMode) -> rf_core::Result<Deleted> {
        self.local.delete(path, mode)
    }

    fn open_read(&self, path: &VPath) -> rf_core::Result<Box<dyn ReadStream>> {
        self.local.open_read(path)
    }

    fn open_write(&self, path: &VPath, mode: WriteMode) -> rf_core::Result<Box<dyn WriteStream>> {
        self.local.open_write(path, mode)
    }

    fn space(&self, path: &VPath) -> rf_core::Result<Option<DiskSpace>> {
        self.step(Step::Space);
        self.local.space(path)
    }

    fn watch(&self, dir: &VPath, sink: WatchSink) -> rf_core::Result<WatchHandle> {
        self.step(Step::Watch);
        let watch = self.local.watch(dir, sink)?;
        Ok(WatchHandle::new((watch, WatchGuard(self.log.clone()))))
    }
}

/// The steps in `log` up to the first that matches `wanted`, that one included.
fn steps_until(log: &Receiver<Step>, what: &str, wanted: impl Fn(&Step) -> bool) -> Vec<Step> {
    let deadline = Instant::now() + WAIT;
    let mut steps = Vec::new();
    loop {
        match log.recv_timeout(left(deadline, what)) {
            Ok(step) => {
                let done = wanted(&step);
                steps.push(step);
                if done {
                    return steps;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("the log ended before {what}"),
        }
    }
}

#[test]
fn a_ticket_dropped_before_the_watch_starts_opens_none() {
    let t = tempfile::tempdir().unwrap();
    let (loader, log, open) = Gated::loader(Hold::Capabilities);
    let (tx, rx) = mpsc::channel();
    let ticket = loader.request(request(local(t.path())), sender(&tx));
    drop((tx, loader));

    // The worker asks what the folder allows, which comes before the watch.
    steps_until(&log, "the capabilities", |step| *step == Step::Capabilities);
    drop(ticket);
    open.send(()).unwrap();
    // The worker ends without a watch, a listing or the free space.
    let rest = steps_until(&log, "the worker to end", |step| *step == Step::Dropped);
    assert_eq!(rest, [Step::Dropped]);
    assert_no_events(&rx);
}

#[test]
fn cancel_stops_the_listing_and_the_watch() {
    let t = tempfile::tempdir().unwrap();
    let (loader, log, open) = Gated::loader(Hold::List);
    let (tx, rx) = mpsc::channel();
    let ticket = loader.request(request(local(t.path())), sender(&tx));
    drop((tx, loader));

    // The watch comes first, then the listing.
    let before = steps_until(&log, "the listing", |step| *step == Step::Listing);
    assert_eq!(before, [Step::Capabilities, Step::Watch, Step::Listing]);
    ticket.cancel();
    // The watch is gone when the cancel returns.
    assert_eq!(log.try_recv(), Ok(Step::WatchDropped));
    open.send(()).unwrap();
    // The listing stops at its next check, and nothing more is asked of the provider.
    let rest = steps_until(&log, "the worker to end", |step| *step == Step::Dropped);
    assert_eq!(
        rest,
        [Step::Listed(Err(ErrorKind::Cancelled)), Step::Dropped]
    );
    assert_no_events(&rx);
    drop(ticket);
}

#[test]
fn a_folder_listing_has_only_folders_and_no_watch() {
    let t = tempfile::tempdir().unwrap();
    for dir in ["b10", "b2", "Alpha"] {
        std::fs::create_dir(t.path().join(dir)).unwrap();
    }
    std::fs::write(t.path().join("a.txt"), b"a").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(t.path().join("b2"), t.path().join("link")).unwrap();
    let (gated, log, open) = Gated::loader(Hold::List);
    // The gate is open from the start.
    open.send(()).unwrap();
    let (tx, rx) = mpsc::channel();
    let ticket = gated.list_folders(request(local(t.path())), sender(&tx));
    drop((tx, gated));

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
    // The folders, in natural order; no file, no link.
    let names: Vec<&OsStr> = entries.iter().map(|entry| entry.name.as_os_str()).collect();
    assert_eq!(names, ["Alpha", "b2", "b10"]);
    // The worker lists and ends: no watch, no free space.
    let steps = steps_until(&log, "the worker to end", |step| *step == Step::Dropped);
    assert_eq!(steps, [Step::Listing, Step::Listed(Ok(())), Step::Dropped]);
    drop(ticket);
    assert_no_events(&rx);

    // A folder that cannot be listed fails once.
    let (tx, rx) = mpsc::channel();
    let _ticket = loader().list_folders(request(local(&t.path().join("missing"))), sender(&tx));
    assert_eq!(
        recv_until(&rx, "the failure", |_| true),
        LoadEvent::Failed {
            generation: 1,
            kind: ErrorKind::NotFound,
        }
    );
}

#[cfg(unix)]
#[test]
fn links_open_by_what_they_lead_to() {
    use rf_ui::loader::Followed;
    use rf_ui::pane::OpenAction;

    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("dir")).unwrap();
    std::fs::write(t.path().join("file"), b"x").unwrap();
    std::os::unix::fs::symlink("dir", t.path().join("to-dir")).unwrap();
    std::os::unix::fs::symlink("file", t.path().join("to-file")).unwrap();
    std::os::unix::fs::symlink("nothing", t.path().join("dangling")).unwrap();
    let at = |name: &str| local(&t.path().join(name));

    let loader = loader();
    let (tx, rx) = mpsc::channel();
    for (generation, name) in [(1, "to-dir"), (2, "to-file"), (3, "dangling")] {
        let tx = tx.clone();
        loader.follow(generation, at(name), move |followed| {
            let _ = tx.send((thread::current().id(), followed));
        });
    }
    let mut results: Vec<(ThreadId, Followed)> = (0..3)
        .map(|_| recv_until(&rx, "a link's target", |_| true))
        .collect();
    // Each result comes from a worker thread, with the generation it was asked with.
    assert!(results.iter().all(|(on, _)| *on != thread::current().id()));
    results.sort_by_key(|(_, followed)| followed.generation);
    let results: Vec<Followed> = results.into_iter().map(|(_, followed)| followed).collect();
    assert_eq!(
        results,
        [
            Followed {
                generation: 1,
                result: Ok(OpenAction::Navigate(at("to-dir"))),
            },
            Followed {
                generation: 2,
                result: Ok(OpenAction::OpenFile(at("to-file"))),
            },
            Followed {
                generation: 3,
                result: Err(ErrorKind::NotFound),
            },
        ]
    );
}
