//! The loader against real folders: listing on a worker thread, live changes, cancelling, a
//! folder that disappears or is replaced, and links.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use rf_core::{Entry, ErrorKind, Os, Scheme, VPath};
use rf_ui::loader::{FolderName, LoadEvent, LoadRequest, LoadTicket, Loader, NamesRequest};
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

/// What a [`Loader::list_names`] of `request` sent: the folders, how many more there are, and
/// whether links were left unexamined.
fn folders_of(loader: &Loader, request: NamesRequest) -> (Vec<FolderName>, usize, bool) {
    let generation = request.generation;
    let (tx, rx) = mpsc::channel();
    let _ticket = loader.list_names(request, sender(&tx));
    match recv_until(&rx, "the names", |_| true) {
        LoadEvent::Names {
            generation: sent,
            names,
            more,
            links_left,
        } => {
            assert_eq!(sent, generation);
            (names, more, links_left)
        }
        other => panic!("expected names, got {other:?}"),
    }
}

/// What a [`Loader::list_names`] of `request` sent: the names, and how many more there are.
fn names_of(loader: &Loader, request: NamesRequest) -> (Vec<String>, usize) {
    let (folders, more, links_left) = folders_of(loader, request);
    assert!(!links_left, "no link in these folders");
    let names = folders
        .iter()
        .map(|each| each.name.to_string_lossy().into_owned())
        .collect();
    (names, more)
}

#[test]
fn a_listing_of_names_keeps_the_folders_that_fit_and_counts_the_rest() {
    let t = tempfile::tempdir().unwrap();
    for dir in ["b10", "b2", "Alpha", "beta", "music"] {
        std::fs::create_dir(t.path().join(dir)).unwrap();
    }
    std::fs::write(t.path().join("b.txt"), b"a").unwrap();
    #[cfg(unix)]
    std::fs::create_dir(t.path().join(".cache")).unwrap();
    let request = |prefix: &str, filter: VisibilityFilter, limit: usize, os: Os| NamesRequest {
        generation: 7,
        folder: local(t.path()),
        prefix: prefix.to_owned(),
        filter,
        limit,
        os,
    };
    let shown = VisibilityFilter::default();
    let (gated, log, open) = Gated::loader(Hold::List);
    // The gate is open from the start.
    open.send(()).unwrap();
    // The folders, in natural order: no file, and no hidden folder while the panes hide them
    // (links: `a_listing_of_names_keeps_the_links_that_lead_to_folders`).
    assert_eq!(
        names_of(&gated, request("", shown, 100, Os::Other)),
        (strings(&["Alpha", "b2", "b10", "beta", "music"]), 0)
    );
    drop(gated);
    // The worker lists and ends: no watch, no free space.
    let steps = steps_until(&log, "the worker to end", |step| *step == Step::Dropped);
    assert_eq!(steps, [Step::Listing, Step::Listed(Ok(())), Step::Dropped]);

    // A prefix ignores case on Windows and macOS, and not elsewhere.
    let b = strings(&["b2", "b10", "beta"]);
    assert_eq!(
        names_of(&loader(), request("B", shown, 100, Os::Windows)),
        (b.clone(), 0)
    );
    assert_eq!(
        names_of(&loader(), request("B", shown, 100, Os::MacOs)),
        (b.clone(), 0)
    );
    assert_eq!(
        names_of(&loader(), request("B", shown, 100, Os::Other)),
        (Vec::new(), 0)
    );
    assert_eq!(
        names_of(&loader(), request("b", shown, 100, Os::Other)),
        (b, 0)
    );
    // At most the limit; the rest are counted.
    assert_eq!(
        names_of(&loader(), request("b", shown, 2, Os::Other)),
        (strings(&["b2", "b10"]), 1)
    );
    // Hidden folders count when the panes show them.
    #[cfg(unix)]
    {
        let hidden_too = VisibilityFilter {
            show_hidden: true,
            ..shown
        };
        let (names, _) = names_of(&loader(), request("", hidden_too, 100, Os::Other));
        assert!(names.contains(&".cache".to_owned()), "{names:?}");
    }

    // A folder that cannot be listed fails once.
    let (tx, rx) = mpsc::channel();
    let missing = NamesRequest {
        folder: local(&t.path().join("missing")),
        ..request("", shown, 100, Os::Other)
    };
    let _ticket = loader().list_names(missing, sender(&tx));
    assert_eq!(
        recv_until(&rx, "the failure", |_| true),
        LoadEvent::Failed {
            generation: 7,
            kind: ErrorKind::NotFound,
        }
    );
}

#[test]
fn a_cancelled_listing_of_names_sends_nothing() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("docs")).unwrap();
    let (loader, log, open) = Gated::loader(Hold::List);
    let (tx, rx) = mpsc::channel();
    let request = NamesRequest {
        generation: 1,
        folder: local(t.path()),
        prefix: String::new(),
        filter: VisibilityFilter::default(),
        limit: 10,
        os: Os::CURRENT,
    };
    let ticket = loader.list_names(request, sender(&tx));
    drop((tx, loader));
    // No watch: the listing comes first.
    let before = steps_until(&log, "the listing", |step| *step == Step::Listing);
    assert_eq!(before, [Step::Listing]);
    // The text changed, or the list closed: the ticket goes.
    drop(ticket);
    open.send(()).unwrap();
    let rest = steps_until(&log, "the worker to end", |step| *step == Step::Dropped);
    assert_eq!(
        rest,
        [Step::Listed(Err(ErrorKind::Cancelled)), Step::Dropped]
    );
    assert_no_events(&rx);
}

#[cfg(unix)]
#[test]
fn a_listing_of_names_keeps_the_links_that_lead_to_folders() {
    use std::os::unix::fs::symlink;

    let t = tempfile::tempdir().unwrap();
    for dir in ["docs", "music"] {
        std::fs::create_dir(t.path().join(dir)).unwrap();
    }
    std::fs::write(t.path().join("a.txt"), b"a").unwrap();
    symlink(t.path().join("docs"), t.path().join("docs link")).unwrap();
    symlink(t.path().join("a.txt"), t.path().join("file link")).unwrap();
    symlink(t.path().join("missing"), t.path().join("broken link")).unwrap();
    let request = |prefix: &str, limit: usize| NamesRequest {
        generation: 3,
        folder: local(t.path()),
        prefix: prefix.to_owned(),
        filter: VisibilityFilter::default(),
        limit,
        os: Os::Other,
    };
    let link = |name: &str| FolderName {
        name: name.into(),
        link: true,
    };
    // A link that leads to a folder is listed among the folders by its name, as a link; one to
    // a file or to nothing is not.
    assert_eq!(
        folders_of(&loader(), request("", 100)),
        (
            vec![
                FolderName::folder("docs"),
                link("docs link"),
                FolderName::folder("music"),
            ],
            0,
            false
        )
    );
    // Past the limit a link is not followed: the listing says links were left unexamined. A
    // folder past it is counted.
    assert_eq!(
        folders_of(&loader(), request("d", 1)),
        (vec![FolderName::folder("docs")], 0, true)
    );
    assert_eq!(
        folders_of(&loader(), request("", 1)),
        (vec![FolderName::folder("docs")], 1, true)
    );
    assert_eq!(
        folders_of(&loader(), request("m", 1)),
        (vec![FolderName::folder("music")], 0, false)
    );
}

#[cfg(unix)]
#[test]
fn a_listing_of_names_follows_at_most_twice_its_limit_of_links() {
    use std::os::unix::fs::symlink;

    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.txt"), b"a").unwrap();
    for name in ["a1", "a2", "a3"] {
        symlink(t.path().join("a.txt"), t.path().join(name)).unwrap();
    }
    std::fs::create_dir(t.path().join("b")).unwrap();
    let request = NamesRequest {
        generation: 4,
        folder: local(t.path()),
        prefix: String::new(),
        filter: VisibilityFilter::default(),
        limit: 1,
        os: Os::Other,
    };
    // Two links to files are followed (twice the limit), the third is left; the folder after
    // them is listed all the same.
    assert_eq!(
        folders_of(&loader(), request),
        (vec![FolderName::folder("b")], 0, true)
    );
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
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

#[test]
fn inspect_target_tells_folder_room_and_volume() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("from")).unwrap();
    std::fs::create_dir(t.path().join("to")).unwrap();
    std::fs::write(t.path().join("notes.txt"), b"x").unwrap();
    let at = |name: &str| local(&t.path().join(name));
    let loader = loader();

    // A folder beside the items' folder: on its volume, with room and writable.
    let facts = loader.inspect_target(&at("to"), &at("from"));
    assert_eq!(facts.folder, Ok(()));
    assert!(facts.writable);
    assert!(facts.available.is_some_and(|room| room > 0), "{facts:?}");
    assert!(facts.same_volume, "{facts:?}");

    // A file and nothing at all are no folders, and tell nothing more.
    let file = loader.inspect_target(&at("notes.txt"), &at("from"));
    assert_eq!(file.folder, Err(ErrorKind::NotADirectory));
    assert!(!file.writable && file.available.is_none() && !file.same_volume);
    let gone = loader.inspect_target(&at("gone"), &at("from"));
    assert_eq!(gone.folder, Err(ErrorKind::NotFound));

    // A link to a folder is the folder.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("to", t.path().join("to-link")).unwrap();
        let linked = loader.inspect_target(&at("to-link"), &at("from"));
        assert_eq!(linked.folder, Ok(()));
        assert!(linked.same_volume);
    }
}

#[test]
fn a_folder_of_the_last_run_is_there_gone_or_offline() {
    use rf_ui::loader::holds_volumes;
    use rf_ui::settings::Found;

    let dir = tempfile::tempdir().expect("a temp dir");
    let here = dir.path().join("here");
    std::fs::create_dir(&here).expect("made");
    std::fs::write(dir.path().join("file"), "x").expect("written");
    let loader = loader();

    assert_eq!(loader.look_at(&local(&here)), Found::Folder);
    // Its parent is there: it was removed.
    assert_eq!(loader.look_at(&local(&here.join("gone"))), Found::Gone);
    assert_eq!(
        loader.look_at(&local(&dir.path().join("a/b/c"))),
        Found::Gone
    );
    // A file where the folder was is no folder.
    assert_eq!(
        loader.look_at(&local(&dir.path().join("file"))),
        Found::Gone
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&here, dir.path().join("link")).expect("linked");
        assert_eq!(
            loader.look_at(&local(&dir.path().join("link"))),
            Found::Folder
        );
    }

    // Right below where the system mounts volumes, a missing folder is a volume not there.
    for mounts in ["/Volumes", "/media", "/media/kim", "/run/media/kim", "/mnt"] {
        assert!(holds_volumes(Path::new(mounts)), "{mounts}");
    }
    for other in [
        "/",
        "/home/kim",
        "/media/kim/USB",
        "/run",
        "/run/media",
        "Volumes",
    ] {
        assert!(!holds_volumes(Path::new(other)), "{other}");
    }
}

#[test]
fn a_volume_is_not_there_when_the_folders_that_hold_it_are_missing_too() {
    use rf_ui::loader::volume_not_there;

    let not_there =
        |path: &str, nearest: &str| volume_not_there(Path::new(path), Path::new(nearest));
    // The nearest folder there is where the volumes go: the volume is not there.
    assert!(not_there("/Volumes/USB/docs", "/Volumes"));
    assert!(not_there("/run/media/kim/USB/docs", "/run/media/kim"));
    // After a restart `/run/media/kim` is made only once a volume is: a folder above where
    // the volumes go is the nearest one there.
    assert!(not_there("/run/media/kim/USB/docs", "/run/media"));
    assert!(not_there("/run/media/kim/USB", "/run"));
    assert!(not_there("/media/kim/USB/docs", "/media"));
    assert!(not_there("/mnt/nas/share", "/"));
    // The volume is there, and the folder on it is gone.
    assert!(!not_there("/run/media/kim/USB/docs", "/run/media/kim/USB"));
    assert!(!not_there("/Volumes/USB/a/b", "/Volumes/USB/a"));
    // Nowhere near where the volumes go.
    assert!(!not_there("/home/kim/docs", "/home"));
    assert!(!not_there("/run/user/docs", "/run"));
}

#[test]
fn a_look_at_a_copy_is_the_one_the_delete_of_copies_makes() {
    use rf_jobs::{Compared, CopyFound};

    let dir = tempfile::TempDir::new().expect("a temporary folder");
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    let at = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for folder in [&src, &dst] {
        std::fs::create_dir(folder).expect("a folder");
        let path = folder.join("a.txt");
        std::fs::write(&path, "AAAA").expect("a test file");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(at))
            .expect("the time is set");
    }
    let copy = local(&dst.join("a.txt"));
    let entry = LocalProvider::new().stat(&copy).expect("the copy is there");
    let made = rf_jobs::CreatedItem::from_entry(copy, &entry);
    let (loader, go_on) = (loader(), CancelToken::new());
    let original = local(&src.join("a.txt"));
    // rf-jobs' look, which the delete of copies runs again before it deletes a copy.
    assert_eq!(
        loader.check_copy(&made, None, &original, &go_on),
        CopyFound::SAME
    );
    std::fs::write(src.join("a.txt"), "changed").expect("written");
    assert_eq!(
        loader.check_copy(&made, None, &original, &go_on),
        CopyFound {
            copy: Compared::Same,
            original: Compared::Changed,
        }
    );
}

#[test]
fn a_newer_look_at_copies_stops_the_one_before() {
    use rf_ui::loader::CopyChecks;

    let mut checks = CopyChecks::default();
    let first = checks.start();
    assert!(!first.is_cancelled());
    // A newer check replaces it: its answer would be dropped, so it stops looking.
    let second = checks.start();
    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
    // The app ends: the look under way stops, and so does any that starts after.
    checks.stop();
    assert!(second.is_cancelled());
    assert!(checks.start().is_cancelled());
}
