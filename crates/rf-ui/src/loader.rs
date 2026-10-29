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
//! The other methods of [`Loader`] block too and are called on a worker thread.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath};
use rf_vfs::{
    CancelToken, Capabilities, Change, Provider, SortSpec, Vfs, WatchEvent, WatchHandle,
    sort_entries,
};

use crate::pane::OpenAction;

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
    /// The watch ended and no other change follows: the folder is gone, or the watch failed
    /// while the folder is still there. [`Loader::nearest_existing_parent`] tells which, and
    /// where to go.
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
    /// `sink` is called on the worker thread or the watch's thread, one call at a time, and
    /// never after the ticket's [`LoadTicket::cancel`] has returned. It must not block: a
    /// cancel waits for a call in progress.
    pub fn request(
        &self,
        request: LoadRequest,
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

    /// The folder to show once the folder `path` has vanished: `path` itself when it is still
    /// a folder (only its watch failed), else its nearest parent that is one, or `None` when
    /// none is left (a removed drive). Blocks; call it on a worker thread.
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

    /// What opening the link at `link` does (the [`OpenAction::Follow`] of a row):
    /// `Navigate` when it leads to a folder, else `OpenFile`. A link to nothing, or one that
    /// cannot be followed, gives the error's kind. Blocks; call it on a worker thread.
    pub fn follow(&self, link: &VPath) -> Result<OpenAction, ErrorKind> {
        let provider = self.vfs.provider(link).map_err(|error| error.kind())?;
        let target = provider.stat_target(link).map_err(|error| error.kind())?;
        Ok(if target.kind == EntryKind::Dir {
            OpenAction::Navigate(link.clone())
        } else {
            OpenAction::OpenFile(link.clone())
        })
    }
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

/// The worker thread's part of one request.
struct Worker {
    vfs: Vfs,
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
        if provider.capabilities(&path).contains(Capabilities::WATCH) {
            self.start_watch(provider.as_ref(), &path, generation);
        }

        let mut entries = Vec::new();
        let listed = provider.list(&path, &self.cancel, &mut |batch| entries.extend(batch));
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
