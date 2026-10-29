//! Lists and watches a tab's folder on a worker thread, and finds where to go when it is gone.
//!
//! Every [`Provider`] call blocks, so none of them runs on the UI thread: [`Loader::request`]
//! starts a worker thread and returns at once. The worker starts the folder's watch first and
//! then lists the folder (so no change between the two is lost), sorts the listing and reads
//! the free space. What it finds reaches the caller as [`LoadEvent`]s through a sink closure:
//! an `mpsc` channel in tests, `slint::invoke_from_event_loop` in the app. Each event carries
//! the generation of its [`LoadRequest`], which is how a tab drops the events of a folder it
//! has left.
//!
//! [`Loader::list_folders`] is the same work without the watch and the free space, for the
//! folder tree: one listing of the folders in a folder. [`Loader::follow`] works the same way
//! for a link a tab opens, and its result carries the tab's generation too.
//! [`Loader::nearest_existing_parent`] and [`Loader::inspect_target`] block and are called on
//! a worker thread.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath};
use rf_vfs::{
    CancelToken, Capabilities, Change, Provider, SortSpec, Vfs, WatchEvent, WatchHandle,
    sort_entries,
};

use crate::pane::OpenAction;
use crate::pick::TargetFacts;

/// What a tab asks the [`Loader`] for: the listing of `path` in the order `sort`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadRequest {
    /// The tab's generation when it asked; every event of this request carries it.
    pub generation: u64,
    /// The folder to list and watch.
    pub path: VPath,
    /// The order the worker sorts the listing in.
    pub sort: SortSpec,
}

/// What the work of one [`LoadRequest`] reports, from a worker thread or the watch's thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadEvent {
    /// The whole folder, sorted by the request's `sort`. Sent once.
    Listed {
        generation: u64,
        entries: Vec<Entry>,
    },
    /// The folder could not be listed. No `Listed` or `Space` follows.
    Failed { generation: u64, kind: ErrorKind },
    /// Children of the folder changed (the watch's `WatchEvent::Changes`). It can come before
    /// `Listed`, and can repeat what the listing already shows.
    Changes {
        generation: u64,
        changes: Vec<Change>,
    },
    /// The watch may have lost changes: the folder must be listed again.
    Rescan { generation: u64 },
    /// The watch ended and no other change follows: the folder was removed, moved or replaced,
    /// or the watch failed while the folder is still there. The listing, if it is still
    /// running, still comes, so the ticket stays. [`Loader::nearest_existing_parent`] tells
    /// where to go; [`Handled::Vanished`](crate::pane::Handled::Vanished) has the whole
    /// protocol.
    Vanished { generation: u64 },
    /// The bytes the user can still write on the folder's volume. Sent after `Listed`, and
    /// only when the provider can tell.
    Space { generation: u64, available: u64 },
}

impl LoadEvent {
    /// The generation of the request this event answers.
    pub fn generation(&self) -> u64 {
        match self {
            LoadEvent::Listed { generation, .. }
            | LoadEvent::Failed { generation, .. }
            | LoadEvent::Changes { generation, .. }
            | LoadEvent::Rescan { generation }
            | LoadEvent::Vanished { generation }
            | LoadEvent::Space { generation, .. } => *generation,
        }
    }
}

/// Starts the worker threads that list and watch folders. Cloning is cheap.
#[derive(Clone, Debug)]
pub struct Loader {
    vfs: Vfs,
}

impl Loader {
    /// A loader that reaches folders through `vfs`.
    pub fn new(vfs: Vfs) -> Loader {
        Loader { vfs }
    }

    /// Lists and watches `request.path` on a new worker thread and returns at once; may be
    /// called on the UI thread.
    ///
    /// The worker starts the watch (when the location can be watched), lists the folder,
    /// sorts it by `request.sort` and sends one `Listed` (or one `Failed`), then a `Space`.
    /// The watch goes on sending `Changes`, `Rescan` and finally `Vanished` until the ticket
    /// is dropped or cancelled. A watch that cannot start leaves the listing without live
    /// updates.
    ///
    /// `sink` is called one call at a time, and never after the ticket's
    /// [`LoadTicket::cancel`] has returned: on the worker thread or the watch's thread, or,
    /// when no worker thread can start, once with `Failed` on the calling thread before
    /// `request` returns. It must not block, since a cancel waits for a call in progress, and
    /// it must never cancel or drop its own ticket, or wait for anything that does: that would
    /// wait for the very call it is in.
    pub fn request(
        &self,
        request: LoadRequest,
        sink: impl FnMut(LoadEvent) + Send + 'static,
    ) -> LoadTicket {
        self.start(request, Mode::Live, sink)
    }

    /// Lists the folders in `request.path` once, on a new worker thread, for a node of the
    /// folder tree; returns at once, so it may be called on the UI thread.
    ///
    /// The worker sends one `Listed` with the folders alone (no file, no link), sorted by
    /// `request.sort`, or one `Failed`, and ends: it starts no watch and reads no free space.
    /// `sink` is called as for [`Loader::request`], and the ticket stops the listing the same
    /// way.
    pub fn list_folders(
        &self,
        request: LoadRequest,
        sink: impl FnMut(LoadEvent) + Send + 'static,
    ) -> LoadTicket {
        self.start(request, Mode::Folders, sink)
    }

    /// Starts the worker of `request`, doing the work `mode` names.
    fn start(
        &self,
        request: LoadRequest,
        mode: Mode,
        sink: impl FnMut(LoadEvent) + Send + 'static,
    ) -> LoadTicket {
        let shared = Arc::new(Mutex::new(Shared {
            sink: Some(Box::new(sink)),
            watch: None,
        }));
        let ticket = LoadTicket {
            generation: request.generation,
            cancel: CancelToken::new(),
            shared: Arc::clone(&shared),
        };
        let worker = Worker {
            vfs: self.vfs.clone(),
            mode,
            cancel: ticket.cancel.clone(),
            shared,
        };
        let generation = request.generation;
        let path = request.path.clone();
        let spawned = std::thread::Builder::new()
            .name("rf-ui-loader".into())
            .spawn(move || worker.run(request));
        if let Err(error) = spawned {
            let kind = Error::io("spawn the folder loader", path, error).kind();
            emit(&ticket.shared, LoadEvent::Failed { generation, kind });
        }
        ticket
    }

    /// Where a tab goes once the watch of its folder `path` has ended ([`LoadEvent::Vanished`]):
    /// `path` itself when it is a folder (it was replaced by a new one, or only its watch
    /// failed), which the tab then lists again; else the nearest parent that is a folder; or
    /// `None` when none is left (a removed drive).
    ///
    /// Blocks; call it on a worker thread, a moment after the event, so that a folder replaced
    /// in one go (`rm -rf dist && mkdir dist`) is back by then.
    pub fn nearest_existing_parent(&self, path: &VPath) -> Option<VPath> {
        let provider = self.vfs.provider(path).ok()?;
        let mut candidate = Some(path.clone());
        while let Some(folder) = candidate {
            if is_folder(provider.as_ref(), &folder) {
                return Some(folder);
            }
            candidate = folder.parent();
        }
        None
    }

    /// Whether `path` is a folder, or a link to one: a folder of the last run that a tab is to
    /// show again.
    ///
    /// Blocks (a slow or sleeping drive, a network folder); call it on a worker thread.
    pub fn is_folder(&self, path: &VPath) -> bool {
        self.vfs
            .provider(path)
            .is_ok_and(|provider| is_folder(provider.as_ref(), path))
    }

    /// What is at `target`, a folder the destination picker may copy or move items to that
    /// are in the folder `from_dir`: whether it is a folder (following links), whether its
    /// location takes changes, the room free there, and whether it is on the volume of
    /// `from_dir`. A target that is no folder tells only that.
    ///
    /// Blocks (a slow or sleeping drive, a network folder); call it on a worker thread.
    pub fn inspect_target(&self, target: &VPath, from_dir: &VPath) -> TargetFacts {
        let refused = |kind: ErrorKind| TargetFacts {
            folder: Err(kind),
            writable: false,
            available: None,
            same_volume: false,
        };
        let provider = match self.vfs.provider(target) {
            Ok(provider) => provider,
            Err(error) => return refused(error.kind()),
        };
        let entry = match provider.stat_target(target) {
            Ok(entry) if entry.kind == EntryKind::Dir => entry,
            Ok(_) => return refused(ErrorKind::NotADirectory),
            Err(error) => return refused(error.kind()),
        };
        let writable = provider.capabilities(target).contains(Capabilities::WRITE);
        let available = provider
            .space(target)
            .ok()
            .flatten()
            .map(|space| space.available);
        let same_volume = self
            .vfs
            .provider(from_dir)
            .ok()
            .filter(|source| source.scheme() == provider.scheme())
            .and_then(|source| source.stat_target(from_dir).ok())
            .is_some_and(|source| {
                source.volume_id.is_some() && source.volume_id == entry.volume_id
            });
        TargetFacts {
            folder: Ok(()),
            writable,
            available,
            same_volume,
        }
    }

    /// Finds out on a new worker thread what opening the link at `link` does (the
    /// [`OpenAction::Follow`] of a row of the tab whose last request had `generation`) and
    /// hands it to `sink` with that generation; returns at once, so it may be called on the UI
    /// thread. The tab takes the result only if it still has `generation`: the user may have
    /// moved on while a slow link was looked up.
    ///
    /// `sink` is called once: on the worker thread, or on the calling thread, with the error,
    /// when no worker thread can start.
    pub fn follow(
        &self,
        generation: u64,
        link: VPath,
        sink: impl FnOnce(Followed) + Send + 'static,
    ) {
        // The worker takes the sink; if the worker cannot start, it is still here.
        let slot = Arc::new(Mutex::new(Some(sink)));
        let worker_slot = Arc::clone(&slot);
        let vfs = self.vfs.clone();
        let worker_link = link.clone();
        let spawned = std::thread::Builder::new()
            .name("rf-ui-follow".into())
            .spawn(move || {
                let result = resolve_link(&vfs, &worker_link);
                let sink = worker_slot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(sink) = sink {
                    sink(Followed { generation, result });
                }
            });
        if let Err(error) = spawned {
            let kind = Error::io("spawn the link follower", link, error).kind();
            let sink = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(sink) = sink {
                sink(Followed {
                    generation,
                    result: Err(kind),
                });
            }
        }
    }
}

/// What [`Loader::follow`] found for a link opened in the tab whose request had `generation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Followed {
    /// The tab's generation when it asked; a tab with another one by now drops the result.
    pub generation: u64,
    /// `Navigate` when the link leads to a folder, else `OpenFile`; the error's kind for a
    /// link to nothing, or one that cannot be followed.
    pub result: Result<OpenAction, ErrorKind>,
}

/// What opening the link at `link` does. Blocks.
fn resolve_link(vfs: &Vfs, link: &VPath) -> Result<OpenAction, ErrorKind> {
    let provider = vfs.provider(link).map_err(|error| error.kind())?;
    let target = provider.stat_target(link).map_err(|error| error.kind())?;
    Ok(if target.kind == EntryKind::Dir {
        OpenAction::Navigate(link.clone())
    } else {
        OpenAction::OpenFile(link.clone())
    })
}

/// Whether `path` is a folder, or a link to one.
fn is_folder(provider: &dyn Provider, path: &VPath) -> bool {
    match provider.stat(path) {
        Ok(entry) if entry.kind == EntryKind::Dir => true,
        Ok(entry) if entry.kind == EntryKind::Symlink => provider
            .stat_target(path)
            .is_ok_and(|target| target.kind == EntryKind::Dir),
        _ => false,
    }
}

/// The running work of one [`Loader::request`]. Dropping it cancels the work.
#[must_use = "dropping the ticket stops the listing and the watch at once"]
pub struct LoadTicket {
    generation: u64,
    cancel: CancelToken,
    shared: Arc<Mutex<Shared>>,
}

impl LoadTicket {
    /// The generation of the request this ticket runs.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Stops the work: the listing stops at its next batch, the watch stops, and the sink is
    /// dropped. Once this returns the sink is not called again. Quick, so the UI thread may
    /// call it (and drop the ticket); calling it again does nothing.
    pub fn cancel(&self) {
        self.cancel.cancel();
        // Taken out under the lock, dropped after it: dropping the watch waits for a watch
        // event in progress, which may be waiting for this lock to call the sink.
        let (sink, watch) = {
            let mut shared = lock(&self.shared);
            (shared.sink.take(), shared.watch.take())
        };
        drop(watch);
        drop(sink);
    }
}

impl Drop for LoadTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl fmt::Debug for LoadTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadTicket")
            .field("generation", &self.generation)
            .field("cancelled", &self.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// The sink type [`Loader::request`] stores.
type Sink = Box<dyn FnMut(LoadEvent) + Send>;

/// What a ticket shares with its worker and its watch.
struct Shared {
    /// `None` once the ticket is cancelled.
    sink: Option<Sink>,
    /// The folder's watch, kept here so that the ticket, not the sink, drops it.
    watch: Option<WatchHandle>,
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Calls the sink with `event`, unless the ticket is cancelled.
fn emit(shared: &Mutex<Shared>, event: LoadEvent) {
    if let Some(sink) = lock(shared).sink.as_mut() {
        sink(event);
    }
}

/// What a worker does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// A tab's folder ([`Loader::request`]): the watch, the listing and the free space.
    Live,
    /// A tree node's folders ([`Loader::list_folders`]): the listing of the folders alone.
    Folders,
}

/// The worker thread's part of one request.
struct Worker {
    vfs: Vfs,
    mode: Mode,
    cancel: CancelToken,
    shared: Arc<Mutex<Shared>>,
}

impl Worker {
    fn run(self, request: LoadRequest) {
        let LoadRequest {
            generation,
            path,
            sort,
        } = request;
        let provider = match self.vfs.provider(&path) {
            Ok(provider) => provider,
            Err(error) => {
                let kind = error.kind();
                return emit(&self.shared, LoadEvent::Failed { generation, kind });
            }
        };
        let live = self.mode == Mode::Live;
        if live {
            let watchable = provider.capabilities(&path).contains(Capabilities::WATCH);
            if self.cancel.is_cancelled() {
                return;
            }
            if watchable {
                self.start_watch(provider.as_ref(), &path, generation);
            }
        }

        let mut entries = Vec::new();
        let listed = provider.list(&path, &self.cancel, &mut |batch| {
            if live {
                entries.extend(batch);
            } else {
                entries.extend(
                    batch
                        .into_iter()
                        .filter(|entry| entry.kind == EntryKind::Dir),
                );
            }
        });
        if self.cancel.is_cancelled() {
            return;
        }
        if let Err(error) = listed {
            let kind = error.kind();
            return emit(&self.shared, LoadEvent::Failed { generation, kind });
        }
        sort_entries(&mut entries, &sort);
        if self.cancel.is_cancelled() {
            return;
        }
        emit(
            &self.shared,
            LoadEvent::Listed {
                generation,
                entries,
            },
        );

        if !live || self.cancel.is_cancelled() {
            return;
        }
        if let Ok(Some(space)) = provider.space(&path) {
            let available = space.available;
            emit(
                &self.shared,
                LoadEvent::Space {
                    generation,
                    available,
                },
            );
        }
    }

    /// Starts the watch and hands it to the ticket; a watch that fails to start is left out.
    fn start_watch(&self, provider: &dyn Provider, path: &VPath, generation: u64) {
        let shared = Arc::clone(&self.shared);
        let sink = Box::new(move |event: WatchEvent| {
            let event = match event {
                WatchEvent::Changes(changes) => LoadEvent::Changes {
                    generation,
                    changes,
                },
                WatchEvent::Rescan => LoadEvent::Rescan { generation },
                WatchEvent::Ended => LoadEvent::Vanished { generation },
            };
            emit(&shared, event);
        });
        let Ok(watch) = provider.watch(path, sink) else {
            return;
        };
        // A ticket cancelled meanwhile has no use for it; dropped after the lock.
        let unused = {
            let mut shared = lock(&self.shared);
            if shared.sink.is_some() {
                shared.watch = Some(watch);
                None
            } else {
                Some(watch)
            }
        };
        drop(unused);
    }
}
