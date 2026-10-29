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
//! folder tree: one listing of the folders in a folder. [`Loader::list_names`] lists the names
//! of the folders in a folder that start with a prefix, for the address bar's lists (NAV-03),
//! at most a limit of them. [`Loader::follow`] works the same way for a link a tab opens, and
//! its result carries the tab's generation too.
//! [`Loader::nearest_existing_parent`], [`Loader::inspect_target`] and [`Loader::check_copy`]
//! block and are called on a worker thread.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use rf_core::{Entry, EntryKind, Error, ErrorKind, Os, VPath};
use rf_jobs::CreatedItem;
use rf_vfs::{
    CancelToken, Capabilities, Change, Provider, SortSpec, Vfs, VisibilityFilter, WatchEvent,
    WatchHandle, fold_name, sort_by_name, sort_entries,
};

use crate::pane::OpenAction;
use crate::pick::TargetFacts;
use crate::settings::Found;

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

/// What the address bar asks the [`Loader`] for ([`Loader::list_names`]): the names of the
/// folders in `folder` that start with `prefix`, as the user would see them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamesRequest {
    /// The generation of the list that asked; every event of this request carries it.
    pub generation: u64,
    /// The folder whose folders are listed.
    pub folder: VPath,
    /// The start of the names wanted ([`name_starts_with`]); empty for all of them.
    pub prefix: String,
    /// Which hidden folders count: those the panes show.
    pub filter: VisibilityFilter,
    /// How many names to send at most; the others are only counted.
    pub limit: usize,
    /// The system whose rule the prefix follows ([`name_starts_with`]).
    pub os: Os,
}

/// A folder a [`Loader::list_names`] found: its name, and whether it is a link (a symbolic link,
/// or a junction on Windows) that leads to a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderName {
    pub name: OsString,
    pub link: bool,
}

impl FolderName {
    /// A folder that is no link.
    pub fn folder(name: impl Into<OsString>) -> FolderName {
        FolderName {
            name: name.into(),
            link: false,
        }
    }
}

/// Whether the folder name `name` starts with `prefix`, typed into the path field, on `os`:
/// ignoring case on Windows and macOS, whose file systems mostly do (folded as `rf_vfs` folds
/// names), and as spelled elsewhere.
pub fn name_starts_with(name: &OsStr, prefix: &str, os: Os) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if os == Os::Other {
        return name.to_string_lossy().starts_with(prefix);
    }
    fold_name(name).starts_with(&fold_name(OsStr::new(prefix)))
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
    /// The folders a [`Loader::list_names`] found, in the natural order of their names: at most
    /// its request's `limit` of them, and how many more there are (links past the limit are
    /// not looked at, so they are not counted). Sent once, in place of `Listed`; a listing that
    /// fails sends `Failed` instead.
    Names {
        generation: u64,
        names: Vec<FolderName>,
        more: usize,
    },
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
            | LoadEvent::Space { generation, .. }
            | LoadEvent::Names { generation, .. } => *generation,
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

    /// Lists the names of the folders in `request.folder` once, on a new worker thread, for a
    /// list of the address bar (NAV-03); returns at once, so it may be called on the UI thread.
    ///
    /// The worker keeps the folders and the links that `request.filter` shows and whose names
    /// start with `request.prefix` ([`name_starts_with`]), by name alone while it reads the
    /// folder, sorts them in the natural order of their names, and sends one `Names` with the
    /// first `request.limit` of them and the count of the rest, or one `Failed`, and ends. A
    /// link counts when it leads to a folder: the worker follows the links among the first
    /// `limit` names, and no other. However large the folder, the UI thread gets at most
    /// `limit` names, and the ticket stops the listing at its next batch (and between links).
    /// `sink` is called as for [`Loader::request`].
    pub fn list_names(
        &self,
        request: NamesRequest,
        sink: impl FnMut(LoadEvent) + Send + 'static,
    ) -> LoadTicket {
        let NamesRequest {
            generation,
            folder,
            prefix,
            filter,
            limit,
            os,
        } = request;
        let request = LoadRequest {
            generation,
            path: folder,
            sort: SortSpec::default(),
        };
        let rule = NameRule {
            prefix,
            filter,
            limit,
            os,
        };
        self.start(request, Mode::Names(rule), sink)
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

    /// The bytes free for writing on the drive of the folder `path`, when it can be told: what
    /// a listing reports after its entries ([`LoadEvent::Space`]), read again. Blocks; call it
    /// on a worker thread.
    pub fn free_space(&self, path: &VPath) -> Option<u64> {
        let provider = self.vfs.provider(path).ok()?;
        let space = provider.space(path).ok()??;
        Some(space.available)
    }

    /// How `made`, an item a copy job made that an undo could not send to the trash, and its
    /// original `original` are now: the copy beside what the job left
    /// ([`Loader::compare_with_made`]) and, while it is that, its original beside it
    /// ([`Loader::compare_with_original`]). A copy that is not as the job left it is not the
    /// copy any more: its original is not looked at (`Unknown`). `at` is when the job ended,
    /// and `cancel` stops the look (the app ends): what is not looked at then cannot be told.
    ///
    /// Blocks (a large folder, a slow drive); call it on a worker thread.
    pub fn check_copy(
        &self,
        made: &CreatedItem,
        at: Option<SystemTime>,
        original: &VPath,
        cancel: &CancelToken,
    ) -> CopyFound {
        let copy = self.compare_with_made(made, at, cancel);
        let original = if copy == Original::Same {
            self.compare_with_original(&made.path, original, cancel)
        } else {
            Original::Unknown
        };
        CopyFound { copy, original }
    }

    /// Whether `made`, an item a copy job made and that job ended at `at`, is still as the job
    /// left it, as the undo of the copy checks it before it trashes it (rf-jobs'
    /// `check_copied`): the same item when both its identities are known, changed when the
    /// job's record says (to the time), a file (or a link) of the recorded size, and a folder
    /// with nothing under it newer than `at`, down to [`rf_jobs::MAX_DEPTH`] levels (a folder
    /// on another volume, or below that depth, counts as a change). Gone, it is `Missing`;
    /// without a recorded time (or `at`, for a folder), past [`ORIGINAL_ITEMS`] items, after
    /// `cancel`, or for anything that cannot be looked at, it cannot be told.
    ///
    /// Blocks (a large folder, a slow drive); call it on a worker thread.
    pub fn compare_with_made(
        &self,
        made: &CreatedItem,
        at: Option<SystemTime>,
        cancel: &CancelToken,
    ) -> Original {
        if cancel.is_cancelled() {
            return Original::Unknown;
        }
        let Ok(provider) = self.vfs.provider(&made.path) else {
            return Original::Unknown;
        };
        let now = match provider.stat(&made.path) {
            Ok(now) => now,
            Err(error) if error.kind() == ErrorKind::NotFound => return Original::Missing,
            Err(_) => return Original::Unknown,
        };
        let Some(was) = made.modified else {
            return Original::Unknown;
        };
        let other_item = matches!(
            (made.identity, now.identity()),
            (Some(was), Some((volume, file))) if was != (volume.0, file.0)
        );
        if other_item || now.modified != Some(was) {
            return Original::Changed;
        }
        if !made.is_dir {
            return if now.kind != EntryKind::Dir && now.size == made.size {
                Original::Same
            } else {
                Original::Changed
            };
        }
        if now.kind != EntryKind::Dir {
            return Original::Changed;
        }
        let Some(at) = at else {
            return Original::Unknown;
        };
        let walk = Untouched {
            provider: provider.as_ref(),
            at,
            cancel,
        };
        let mut budget = ORIGINAL_ITEMS;
        walk.folder(&made.path, &now, 1, &mut budget)
    }

    /// How `original` is now beside `copy`, a copy of it that an undo could not send to the
    /// trash: whether deleting the copy for good would lose nothing that is not still in the
    /// original.
    ///
    /// Each is looked at as it is, a link as the link. They must be of one kind (a copy that
    /// followed a link is compared with what the link leads to). A file must be of the copy's
    /// size, and changed when the copy says (a copy keeps its source's time; within
    /// [`rf_jobs::MTIME_TOLERANCE`]), and a link lead where the copy's does. A folder must
    /// hold, under the same names, everything the copy holds, each the same way, down to
    /// [`rf_jobs::MAX_DEPTH`] levels; what the original has more does not matter. An original
    /// that is, at any level, the copy's own item (the same volume and file ID: a link to the
    /// copy, a linked folder above it, a hard link) cannot be told, as deleting the copy would
    /// lose it. Past [`ORIGINAL_ITEMS`] items, after `cancel` (the app ends), or for anything
    /// that cannot be looked at (the copy among them), it cannot be told either.
    ///
    /// Blocks (a large folder, a slow drive); call it on a worker thread.
    pub fn compare_with_original(
        &self,
        copy: &VPath,
        original: &VPath,
        cancel: &CancelToken,
    ) -> Original {
        let (Ok(at_copy), Ok(at_original)) = (self.vfs.provider(copy), self.vfs.provider(original))
        else {
            return Original::Unknown;
        };
        let Ok(copied) = at_copy.stat(copy) else {
            return Original::Unknown;
        };
        let compare = Compare {
            copy: at_copy.as_ref(),
            original: at_original.as_ref(),
            cancel,
        };
        let mut budget = ORIGINAL_ITEMS;
        match at_original.stat(original) {
            Ok(entry) => compare.items(copy, &copied, original, &entry, 0, &mut budget),
            Err(error) if error.kind() == ErrorKind::NotFound => Original::Missing,
            Err(_) => Original::Unknown,
        }
    }

    /// What is at `path`, a folder of the last run that a tab is to show again: the folder (or
    /// a link to one); nothing, while the folders that held it are there; or nothing because
    /// its drive, share or volume is not there ([`Found::Offline`]): no folder above it is there
    /// (a Windows drive or share that is not connected), or it lies where the system puts
    /// removable and network volumes and the nearest folder there is at or above that place
    /// ([`volume_not_there`]: an unplugged one's folder goes).
    ///
    /// Blocks (a slow or sleeping drive, a network folder); call it on a worker thread.
    pub fn look_at(&self, path: &VPath) -> Found {
        let Ok(provider) = self.vfs.provider(path) else {
            return Found::Offline;
        };
        if is_folder(provider.as_ref(), path) {
            return Found::Folder;
        }
        let mut above = path.parent();
        while let Some(folder) = above {
            if is_folder(provider.as_ref(), &folder) {
                let mounts = match (path.as_local(), folder.as_local()) {
                    (Some(path), Some(nearest)) => volume_not_there(path, nearest),
                    _ => false,
                };
                return if mounts { Found::Offline } else { Found::Gone };
            }
            above = folder.parent();
        }
        Found::Offline
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

/// Whether the link `name` in `folder` leads to a folder.
fn leads_to_folder(provider: &dyn Provider, folder: &VPath, name: &OsStr) -> bool {
    folder
        .join(name)
        .ok()
        .and_then(|link| provider.stat_target(&link).ok())
        .is_some_and(|target| target.kind == EntryKind::Dir)
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

/// How many items [`Loader::compare_with_original`] and [`Loader::compare_with_made`] look at,
/// at most, before they give up.
pub const ORIGINAL_ITEMS: usize = 20_000;

/// How the original of a copy is beside the copy ([`Loader::compare_with_original`]), or a
/// copy beside what its job left ([`Loader::compare_with_made`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Original {
    /// It holds everything the copy holds (the copy is as its job left it): deleting the copy
    /// loses nothing.
    Same,
    /// It is gone.
    Missing,
    /// It is there, but of another kind or size, or something of the copy is not in it (the
    /// copy was written to since its job).
    Changed,
    /// It could not be found out.
    Unknown,
}

/// What [`Loader::check_copy`] found of a copy that an undo could not send to the trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyFound {
    /// The copy beside what its job left ([`Loader::compare_with_made`]).
    pub copy: Original,
    /// Its original beside it ([`Loader::compare_with_original`]); `Unknown` while the copy is
    /// not as its job left it.
    pub original: Original,
}

impl CopyFound {
    /// Both as they were: deleting the copy loses nothing.
    pub const SAME: CopyFound = CopyFound {
        copy: Original::Same,
        original: Original::Same,
    };
}

/// The cancel tokens of the workers' looks at copies and their originals
/// ([`Loader::check_copy`]). Each look has its own: the next one cancels it, since a newer
/// check replaces it and its answer would be dropped, and [`CopyChecks::stop`] cancels it and
/// every look that starts after, as the app ends.
#[derive(Debug, Default)]
pub struct CopyChecks {
    current: CancelToken,
    stopped: bool,
}

impl CopyChecks {
    /// The token of a new look; the look before it stops. Once stopped, it is cancelled
    /// already.
    pub fn start(&mut self) -> CancelToken {
        self.current.cancel();
        self.current = CancelToken::new();
        if self.stopped {
            self.current.cancel();
        }
        self.current.clone()
    }

    /// Stops the look under way, and any that starts later: the app ends.
    pub fn stop(&mut self) {
        self.stopped = true;
        self.current.cancel();
    }
}

/// The provider and the job's end that [`Loader::compare_with_made`] looks under a copied
/// folder with.
struct Untouched<'p> {
    provider: &'p dyn Provider,
    at: SystemTime,
    cancel: &'p CancelToken,
}

impl Untouched<'_> {
    /// Whether nothing under the folder `dir`, which `entry` describes, is newer than the
    /// job's end: every item it lists has a known time, none after it, and every real folder
    /// among them (a link is never followed) holds the same. `depth` is how far below the copied
    /// folder its items are (its own are at 1); each item looked at takes one from `budget`.
    fn folder(&self, dir: &VPath, entry: &Entry, depth: usize, budget: &mut usize) -> Original {
        let mut children = Vec::new();
        let listed = self
            .provider
            .list(dir, self.cancel, &mut |batch| children.extend(batch));
        if listed.is_err() || self.cancel.is_cancelled() {
            return Original::Unknown;
        }
        if depth > rf_jobs::MAX_DEPTH {
            // The copy went no deeper: a folder down there must be empty.
            return if children.is_empty() {
                Original::Same
            } else {
                Original::Changed
            };
        }
        for child in &children {
            if *budget == 0 {
                return Original::Unknown;
            }
            *budget -= 1;
            if child.modified.is_none_or(|modified| modified > self.at) {
                return Original::Changed;
            }
            if child.kind != EntryKind::Dir {
                continue;
            }
            let elsewhere = matches!(
                (entry.volume_id, child.volume_id),
                (Some(here), Some(there)) if here != there
            );
            if elsewhere {
                return Original::Changed;
            }
            let Ok(path) = dir.join(&child.name) else {
                return Original::Unknown;
            };
            let below = self.folder(&path, child, depth + 1, budget);
            if below != Original::Same {
                return below;
            }
        }
        Original::Same
    }
}

/// The two providers [`Loader::compare_with_original`] compares a copy and its original
/// through, until `cancel`.
struct Compare<'p> {
    copy: &'p dyn Provider,
    original: &'p dyn Provider,
    cancel: &'p CancelToken,
}

impl Compare<'_> {
    /// How the item `original`, which `stat` found as `found`, is beside `copied`, the copy at
    /// `copy`, at `depth` levels below the items compared first; each item looked at takes one
    /// from `budget`.
    fn items(
        &self,
        copy: &VPath,
        copied: &Entry,
        original: &VPath,
        found: &Entry,
        depth: usize,
        budget: &mut usize,
    ) -> Original {
        if *budget == 0 || depth > rf_jobs::MAX_DEPTH || self.cancel.is_cancelled() {
            return Original::Unknown;
        }
        *budget -= 1;
        // A copy that followed a link is what the link leads to.
        let followed;
        let found = if found.kind == EntryKind::Symlink && copied.kind != EntryKind::Symlink {
            match self.original.stat_target(original) {
                Ok(target) => {
                    followed = target;
                    &followed
                }
                Err(error) if error.kind() == ErrorKind::NotFound => return Original::Missing,
                Err(_) => return Original::Unknown,
            }
        } else {
            found
        };
        if found.kind != copied.kind {
            return Original::Changed;
        }
        if self.is_the_copy(copy, copied, found) {
            // The original leads to the copy itself (a link to it, a linked folder above it, a
            // hard link): deleting the copy would lose what the original holds.
            return Original::Unknown;
        }
        match copied.kind {
            EntryKind::File => same_file(copied, found),
            EntryKind::Symlink => {
                match (self.copy.read_link(copy), self.original.read_link(original)) {
                    (Ok(a), Ok(b)) if a.target == b.target => Original::Same,
                    (Ok(_), Ok(_)) => Original::Changed,
                    _ => Original::Unknown,
                }
            }
            EntryKind::Dir => self.folders(copy, original, depth, budget),
            // A device or a pipe is never copied.
            EntryKind::Other => Original::Unknown,
        }
    }

    /// Whether `found`, the original as [`Compare::items`] looks at it, is `copied`, the copy
    /// at `copy`, itself: both have one identity (volume and file ID). A listing that gives no
    /// IDs (Windows) leaves the copy's to a `stat`, which only an original with a known
    /// identity needs.
    fn is_the_copy(&self, copy: &VPath, copied: &Entry, found: &Entry) -> bool {
        let Some(original) = found.identity() else {
            return false;
        };
        let copy_identity = copied
            .identity()
            .or_else(|| self.copy.stat(copy).ok()?.identity());
        copy_identity == Some(original)
    }

    /// Whether the folder `original` holds everything the folder `copy` holds, as
    /// [`Compare::items`] tells for each.
    fn folders(
        &self,
        copy: &VPath,
        original: &VPath,
        depth: usize,
        budget: &mut usize,
    ) -> Original {
        let (Some(copied), Some(found)) = (
            self.listing(self.copy, copy),
            self.listing(self.original, original),
        ) else {
            return Original::Unknown;
        };
        let found: std::collections::HashMap<&OsStr, &Entry> = found
            .iter()
            .map(|entry| (entry.name.as_os_str(), entry))
            .collect();
        for child in &copied {
            let Some(&there) = found.get(child.name.as_os_str()) else {
                return Original::Changed;
            };
            let (Ok(copy_child), Ok(original_child)) =
                (copy.join(&child.name), original.join(&child.name))
            else {
                return Original::Unknown;
            };
            let compared = self.items(
                &copy_child,
                child,
                &original_child,
                there,
                depth + 1,
                budget,
            );
            if compared != Original::Same {
                // Gone below a folder that is there is a change of the folder.
                return if compared == Original::Missing {
                    Original::Changed
                } else {
                    compared
                };
            }
        }
        Original::Same
    }

    /// Everything in the folder `dir` of `provider`; `None` when it cannot be listed.
    fn listing(&self, provider: &dyn Provider, dir: &VPath) -> Option<Vec<Entry>> {
        let mut entries = Vec::new();
        provider
            .list(dir, self.cancel, &mut |batch| entries.extend(batch))
            .ok()?;
        (!self.cancel.is_cancelled()).then_some(entries)
    }
}

/// How the file `found` is beside `copied`, a copy of it: the same when it has the copy's size
/// and was last changed when the copy says (a copy keeps its source's time, within the clock
/// tolerance of the jobs); changed when not; not known without both times.
fn same_file(copied: &Entry, found: &Entry) -> Original {
    let (Some(copy_time), Some(original_time)) = (copied.modified, found.modified) else {
        return Original::Unknown;
    };
    let apart = copy_time
        .duration_since(original_time)
        .or_else(|_| original_time.duration_since(copy_time))
        .unwrap_or_default();
    if found.size == copied.size && apart <= rf_jobs::MTIME_TOLERANCE {
        Original::Same
    } else {
        Original::Changed
    }
}

/// Whether `folder` is where the system puts the folders of removable and network volumes:
/// `/Volumes` (macOS), `/media`, `/media/<user>`, `/run/media/<user>` and `/mnt` (Linux). A
/// folder missing right below it is a volume that is not there.
pub fn holds_volumes(folder: &Path) -> bool {
    let mut parts = folder.components();
    if parts.next() != Some(Component::RootDir) {
        return false;
    }
    let names: Vec<&OsStr> = parts.map(Component::as_os_str).collect();
    let names: Vec<Option<&str>> = names.iter().map(|name| name.to_str()).collect();
    matches!(
        names.as_slice(),
        [Some("Volumes" | "media" | "mnt")]
            | [Some("media"), Some(_)]
            | [Some("run"), Some("media"), Some(_)]
    )
}

/// Whether `path`, missing, whose nearest folder that is there is `nearest`, is on a
/// removable or network volume that is not there: a folder where the system puts such volumes
/// ([`holds_volumes`]) holds it, and is `nearest` or is missing too (after a restart,
/// `/run/media/<user>` is made only once a volume is).
pub fn volume_not_there(path: &Path, nearest: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .any(|folder| holds_volumes(folder) && folder.starts_with(nearest))
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
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// A tab's folder ([`Loader::request`]): the watch, the listing and the free space.
    Live,
    /// A tree node's folders ([`Loader::list_folders`]): the listing of the folders alone.
    Folders,
    /// The names of some folders in a folder, for the address bar ([`Loader::list_names`]).
    Names(NameRule),
}

/// Which folders a [`Mode::Names`] listing keeps, and how many of their names it sends.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NameRule {
    prefix: String,
    filter: VisibilityFilter,
    limit: usize,
    os: Os,
}

impl NameRule {
    /// What the listing keeps of `entry`: its name, when it is a folder or a link (which may
    /// lead to one) that the filter shows and whose name starts with the prefix.
    fn candidate(&self, entry: Entry) -> Option<FolderName> {
        let link = match entry.kind {
            EntryKind::Dir => false,
            EntryKind::Symlink => true,
            EntryKind::File | EntryKind::Other => return None,
        };
        (self.filter.is_visible(&entry) && name_starts_with(&entry.name, &self.prefix, self.os))
            .then_some(FolderName {
                name: entry.name,
                link,
            })
    }
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
        if let Mode::Names(rule) = &self.mode {
            return self.send_names(provider.as_ref(), &path, generation, rule);
        }
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
        let listed = provider.list(&path, &self.cancel, &mut |batch| match &self.mode {
            Mode::Live => entries.extend(batch),
            // A listing of names went to `send_names`.
            Mode::Folders | Mode::Names(_) => entries.extend(
                batch
                    .into_iter()
                    .filter(|entry| entry.kind == EntryKind::Dir),
            ),
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

    /// The work of a [`Mode::Names`] request for `folder`: the names that `rule` keeps, sorted,
    /// at most its limit of them, the links among them that lead to folders only.
    fn send_names(
        &self,
        provider: &dyn Provider,
        folder: &VPath,
        generation: u64,
        rule: &NameRule,
    ) {
        let mut found = Vec::new();
        let listed = provider.list(folder, &self.cancel, &mut |batch| {
            found.extend(batch.into_iter().filter_map(|entry| rule.candidate(entry)));
        });
        if self.cancel.is_cancelled() {
            return;
        }
        if let Err(error) = listed {
            let kind = error.kind();
            return emit(&self.shared, LoadEvent::Failed { generation, kind });
        }
        sort_by_name(&mut found, |each| each.name.as_os_str());
        let mut names = Vec::with_capacity(found.len().min(rule.limit));
        let mut more = 0;
        for each in found {
            if names.len() < rule.limit {
                if self.cancel.is_cancelled() {
                    return;
                }
                if !each.link || leads_to_folder(provider, folder, &each.name) {
                    names.push(each);
                }
            } else if !each.link {
                more += 1;
            }
        }
        if self.cancel.is_cancelled() {
            return;
        }
        emit(
            &self.shared,
            LoadEvent::Names {
                generation,
                names,
                more,
            },
        );
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
