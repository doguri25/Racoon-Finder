//! Test doubles for code that uses this crate. The module is always compiled (no feature
//! flag) so that the tests of the crates built on top of this one can use it too.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::ReadDir;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rf_core::{Entry, Error, Result, Scheme, VPath, VolumeId};
use rf_vfs::{
    CancelToken, Capabilities, DataRanges, DeleteMode, Deleted, DiskSpace, LinkTarget,
    LocalProvider, MetadataPatch, Provider, ReadStream, RenameMode, Vfs, WatchHandle, WatchSink,
    WriteMode, WriteStream,
};

use crate::conflict::{Conflict, ConflictAnswer};
use crate::engine::JobEvent;
use crate::id::JobId;
use crate::naming::{TEMP_SUFFIX, temp_job};
use crate::preflight::Issue;
use crate::report::{ItemFailure, JobReport, Stage};
use crate::resolver::{ErrorAnswer, PreflightAnswer, Resolver};

/// One question a [`ScriptedResolver`] was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolverCall {
    Conflict { target: VPath },
    Error { stage: Stage },
    Preflight { issues: usize },
}

#[derive(Debug, Default)]
struct Script {
    conflicts: VecDeque<ConflictAnswer>,
    errors: VecDeque<ErrorAnswer>,
    preflights: VecDeque<PreflightAnswer>,
    calls: Vec<ResolverCall>,
}

/// A [`Resolver`] that gives the answers it was loaded with, in order, and remembers what it
/// was asked. Each kind of question has its own queue. A question with no answer left
/// panics, so a test fails loudly when the code asks more than the test expected.
///
/// The queues and the call log sit behind one lock that is released before any panic, so an
/// unscripted question does not poison the resolver for the calls that follow.
#[derive(Debug, Default)]
pub struct ScriptedResolver {
    script: Mutex<Script>,
}

impl ScriptedResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues the answer to the next unanswered conflict.
    #[must_use]
    pub fn on_conflict(mut self, answer: ConflictAnswer) -> Self {
        self.script_mut().conflicts.push_back(answer);
        self
    }

    /// Queues the answer to the next unanswered failure.
    #[must_use]
    pub fn on_error(mut self, answer: ErrorAnswer) -> Self {
        self.script_mut().errors.push_back(answer);
        self
    }

    /// Queues the answer to the next unanswered pre-flight question.
    #[must_use]
    pub fn on_preflight(mut self, answer: PreflightAnswer) -> Self {
        self.script_mut().preflights.push_back(answer);
        self
    }

    /// Every question asked so far, in order, answered or not.
    pub fn calls(&self) -> Vec<ResolverCall> {
        self.lock().calls.clone()
    }

    /// The script, for the builder methods that own `self`.
    fn script_mut(&mut self) -> &mut Script {
        self.script
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The script lock. Every update is a queue operation or a push, so a holder that
    /// panicked cannot leave the script half-written; a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Resolver for ScriptedResolver {
    fn conflict(&self, _job: JobId, conflict: &Conflict) -> ConflictAnswer {
        let answer = {
            let mut script = self.lock();
            script.calls.push(ResolverCall::Conflict {
                target: conflict.target.clone(),
            });
            script.conflicts.pop_front()
        };
        // The guard is gone: the panic below leaves the lock clean.
        answer.unwrap_or_else(|| panic!("unscripted conflict for {}", conflict.target))
    }

    fn error(&self, _job: JobId, failure: &ItemFailure) -> ErrorAnswer {
        let answer = {
            let mut script = self.lock();
            script.calls.push(ResolverCall::Error {
                stage: failure.stage,
            });
            script.errors.pop_front()
        };
        answer.unwrap_or_else(|| panic!("unscripted error at {:?}", failure.stage))
    }

    fn preflight(&self, _job: JobId, issues: &[Issue]) -> PreflightAnswer {
        let answer = {
            let mut script = self.lock();
            script.calls.push(ResolverCall::Preflight {
                issues: issues.len(),
            });
            script.preflights.pop_front()
        };
        answer.unwrap_or_else(|| panic!("unscripted preflight with {} issues", issues.len()))
    }
}

/// One item of a [`tree`] snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Dir,
    /// A file and its whole content.
    File(Vec<u8>),
    /// A symbolic link (or a Windows junction) and the target it stores.
    Link(PathBuf),
}

/// Everything under `root` (not `root` itself), keyed by the path relative to `root` with its
/// names joined by `/` (lossily converted to text). Links are not followed: a link is a
/// [`Node::Link`] with what [`std::fs::read_link`] gives.
///
/// Panics when `root` cannot be listed, a missing `root` included: an empty map always means
/// an existing, empty folder, so a test that expects "nothing written" cannot pass because the
/// folder itself is gone. Panics too on any other error and on anything that is not a file, a
/// folder or a link (a FIFO, a socket, a device), which it never opens.
pub fn tree(root: &Path) -> BTreeMap<String, Node> {
    let entries = std::fs::read_dir(root)
        .unwrap_or_else(|e| panic!("tree: cannot list {}: {e}", root.display()));
    let mut nodes = BTreeMap::new();
    walk(entries, root, None, &mut nodes);
    nodes
}

/// Adds `entries`, the items of `dir`, and everything under them to `nodes`, their keys
/// starting with `prefix/`.
fn walk(entries: ReadDir, dir: &Path, prefix: Option<&str>, nodes: &mut BTreeMap<String, Node>) {
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("tree: cannot list {}: {e}", dir.display()));
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let key = match prefix {
            Some(prefix) => format!("{prefix}/{name}"),
            None => name,
        };
        // `DirEntry::file_type` does not follow a link.
        let file_type = entry
            .file_type()
            .unwrap_or_else(|e| panic!("tree: cannot stat {}: {e}", path.display()));
        let node = if file_type.is_symlink() {
            let target = std::fs::read_link(&path)
                .unwrap_or_else(|e| panic!("tree: cannot read link {}: {e}", path.display()));
            Node::Link(target)
        } else if file_type.is_dir() {
            let children = std::fs::read_dir(&path)
                .unwrap_or_else(|e| panic!("tree: cannot list {}: {e}", path.display()));
            walk(children, &path, Some(&key), nodes);
            Node::Dir
        } else if file_type.is_file() {
            let content = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("tree: cannot read {}: {e}", path.display()));
            Node::File(content)
        } else {
            panic!("tree: {} is no file, folder or link", path.display());
        };
        nodes.insert(key, node);
    }
}

/// Creates the items `entries` names under `root`: a path that ends with `/` is an empty
/// folder; any other path is a file with the given content, made with its missing parent
/// folders. Paths use `/` between names on every OS.
pub fn write_tree(root: &Path, entries: &[(&str, &str)]) {
    for &(path, content) in entries {
        let full: PathBuf = std::iter::once(root.as_os_str())
            .chain(
                path.split('/')
                    .filter(|name| !name.is_empty())
                    .map(OsStr::new),
            )
            .collect();
        if path.ends_with('/') {
            std::fs::create_dir_all(&full)
                .unwrap_or_else(|e| panic!("write_tree: cannot create {}: {e}", full.display()));
        } else {
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap_or_else(|e| {
                    panic!("write_tree: cannot create {}: {e}", parent.display())
                });
            }
            std::fs::write(&full, content)
                .unwrap_or_else(|e| panic!("write_tree: cannot write {}: {e}", full.display()));
        }
    }
}

/// The [`tree`] keys under `root` that are temporary files of a transfer (they end with
/// [`TEMP_SUFFIX`]).
pub fn temp_files(root: &Path) -> Vec<String> {
    tree(root)
        .into_keys()
        .filter(|key| key.ends_with(TEMP_SUFFIX))
        .collect()
}

/// The first event from `rx` that `pred` accepts; the events before it are dropped. It waits
/// up to `timeout` in all and panics when no such event comes in that time, or when the engine
/// has ended and no event will come. Past the deadline, every event that is ready is still
/// looked at, once each, before it panics.
pub fn wait_for(
    rx: &Receiver<JobEvent>,
    timeout: Duration,
    mut pred: impl FnMut(&JobEvent) -> bool,
) -> JobEvent {
    let started = Instant::now();
    loop {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(event) if pred(&event) => return event,
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                panic!("wait_for: the engine ended before a matching job event")
            }
        }
    }
    loop {
        match rx.try_recv() {
            Ok(event) if pred(&event) => return event,
            Ok(_) => {}
            Err(TryRecvError::Empty) => {
                panic!("wait_for: no matching job event within {timeout:?}")
            }
            Err(TryRecvError::Disconnected) => {
                panic!("wait_for: the engine ended before a matching job event")
            }
        }
    }
}

/// The report of the job `id`, from its [`JobEvent::Finished`]; the events before it are
/// dropped. Panics as [`wait_for`] does.
pub fn wait_finished(rx: &Receiver<JobEvent>, id: JobId, timeout: Duration) -> Arc<JobReport> {
    let finished =
        |event: &JobEvent| matches!(event, JobEvent::Finished { id: done, .. } if *done == id);
    let JobEvent::Finished { report, .. } = wait_for(rx, timeout, finished) else {
        unreachable!("wait_for gives only an event its predicate accepts")
    };
    report
}

/// A fault for a [`FaultyProvider`] to inject.
///
/// `file` counts the provider's `open_write` calls from 0, in the order they were made, failed
/// ones included; `call` counts its `rename` calls the same way.
///
/// The faults with `after` (`FailWrite`, `DiskFull`, `Crash`) strike on the first `write`
/// that would take the file past `after` bytes in all: that write passes its first bytes up
/// to `after` on and then fails (or crashes). Writes that stay at or below `after` are not
/// touched, so a file of exactly `after` bytes, or a smaller one, is never hit. With
/// `after: 0` the first write of any data strikes and passes nothing on. A hole
/// (`WriteStream::write_hole`) is passed on and does not count toward `after`, but once one of
/// these faults has struck, `write_hole` fails with the same error.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// The write that would take file `file` past `after` bytes passes them on up to `after`
    /// and fails with `injected write failure`.
    FailWrite { file: usize, after: u64 },
    /// Like `FailWrite`, but the error is `io::ErrorKind::StorageFull` (`injected disk full`).
    DiskFull { file: usize, after: u64 },
    /// `finish` of file `file` fails with `injected fsync failure` and does not finish the inner
    /// stream (it is closed unsynced).
    FailFinish { file: usize },
    /// The first `write` or `write_hole` of file `file` panics with `injected panic`.
    Panic { file: usize },
    /// The first `write` or `write_hole` of file `file` waits until
    /// [`FaultyProvider::release`]. It wakes [`FaultyProvider::wait_held`] before it waits.
    Hold { file: usize },
    /// Like a killed process: the write that would take file `file` past `after` bytes passes
    /// them on up to `after`, then the inner stream is flushed and closed,
    /// [`FaultyProvider::wait_crashed`] wakes, and the writing thread stops for good. No later
    /// code, no `Drop`, runs on it.
    Crash { file: usize, after: u64 },
    /// Rename number `call` fails with `injected rename failure`.
    FailRename { call: usize },
    /// `create_dir` number `call` (counted from 0 like `rename`'s) fails with
    /// `injected create_dir failure` and makes nothing.
    FailCreateDir { call: usize },
    /// Every rename fails with `io::ErrorKind::CrossesDevices`, as between two volumes, except
    /// the commit of a temporary file (a `from` whose name `temp_job` reads).
    CrossDevice,
    /// Deleting an item called `name` (`delete` or `delete_leaf`) fails with `PermissionDenied`.
    FailDelete { name: OsString },
    /// Just before the first permanent delete (`delete` with `DeleteMode::Permanent`, or
    /// `delete_leaf`) is passed on, a file `name` with the content `late` appears next to the
    /// item, as if another app wrote it during a move. Once only. If that file cannot be made,
    /// the delete fails with the reason and deletes nothing.
    CreateOnDelete { name: OsString },
    /// Paths under `under` (it included) lack `Capabilities::WRITE`.
    ReadOnly { under: VPath },
    /// `fs_name` gives this name.
    FsName(String),
    /// `space` gives this space.
    Space(DiskSpace),
    /// `stat` of an item called `name` fails with `PermissionDenied`: an item that exists or
    /// not, which cannot be examined.
    FailStat { name: OsString },
    /// `stat`, `stat_target` and `list` give entries without a volume or file ID, as a file
    /// system that has none (FAT) or reports zeros.
    NoIdentity,
    /// `list` gives entries without a volume or file ID, as the local provider lists them on
    /// Windows, while `stat` and `stat_target` still give them: a Windows listing on any OS.
    NoListedIdentity,
    /// `delete` with `DeleteMode::Trash` removes the item for good instead and says so
    /// (`Deleted::Permanently`), as a provider that breaks its promise would.
    TrashDeletesForGood,
    /// `set_metadata` fails with `Unsupported` and `copy_extras` copies nothing, as for a
    /// provider that keeps no metadata (the default implementations of both).
    NoMetadata,
    /// Every `sync_dir` fails with `injected sync_dir failure`.
    FailSyncDir,
    /// Every `remove_dir` fails with `injected remove_dir failure` and removes nothing.
    FailRemoveDir,
    /// `stat`, `stat_target` and `list` give the items at or under `under` another volume ID,
    /// as the same items show through a second mount of their file system (bindfs, an NFS or
    /// SMB loopback, one share mounted twice). Their file IDs stay as they are.
    OtherVolume { under: VPath },
    /// `ReadStream::data_ranges` of a stream that reads an item called `name` gives `ranges`,
    /// as for a sparse file, whatever the file holds: ranges that call a stretch of zeros a
    /// hole, or answers that break the promises of `data_ranges`. `Provider::data_ranges` is
    /// passed on as it is.
    DataRanges {
        name: OsString,
        ranges: Vec<Range<u64>>,
    },
    /// Every `write_hole` gives `Ok(false)` and passes nothing on, as on a file system that
    /// cannot make holes (FAT and exFAT on Windows).
    NoHoles,
    /// Every `seek` of a stream that reads an item called `name` fails with
    /// `injected seek failure`, as on a stream that cannot seek (a pipe-like FUSE file, a
    /// remote stream). Its `ReadStream::data_ranges` still answers.
    FailSeek { name: OsString },
}

impl Fault {
    /// The `open_write` number the fault applies to, for the faults of one written file.
    fn file(&self) -> Option<usize> {
        match self {
            Fault::FailWrite { file, .. }
            | Fault::DiskFull { file, .. }
            | Fault::FailFinish { file }
            | Fault::Panic { file }
            | Fault::Hold { file }
            | Fault::Crash { file, .. } => Some(*file),
            _ => None,
        }
    }

    /// The byte count a write fault strikes at.
    fn after(&self) -> Option<u64> {
        match self {
            Fault::FailWrite { after, .. }
            | Fault::DiskFull { after, .. }
            | Fault::Crash { after, .. } => Some(*after),
            _ => None,
        }
    }
}

/// What a [`FaultyProvider`] has counted.
#[derive(Debug, Default)]
struct Counters {
    opened_writes: usize,
    renames: usize,
    created_dirs: usize,
    late_file_made: bool,
}

/// What the held and crashed writers have signalled, shared with them.
#[derive(Debug, Default)]
struct Signals {
    held: bool,
    released: bool,
    crashed: bool,
}

#[derive(Debug, Default)]
struct Shared {
    signals: Mutex<Signals>,
    changed: Condvar,
}

impl Shared {
    /// The signal lock. Every update is one assignment, so a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, Signals> {
        self.signals.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, update: impl FnOnce(&mut Signals)) {
        update(&mut self.lock());
        self.changed.notify_all();
    }

    /// Waits until `done` holds or `timeout` passes, and says whether it holds.
    fn wait_for(&self, timeout: Duration, done: impl Fn(&Signals) -> bool) -> bool {
        let (signals, _) = self
            .changed
            .wait_timeout_while(self.lock(), timeout, |signals| !done(signals))
            .unwrap_or_else(PoisonError::into_inner);
        done(&signals)
    }

    /// Waits as long as it takes until `done` holds.
    fn wait_until(&self, done: impl Fn(&Signals) -> bool) {
        let _signals = self
            .changed
            .wait_while(self.lock(), |signals| !done(signals))
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// A [`Provider`] that passes every call to an inner provider, except where one of its
/// [`Fault`]s strikes. It reproduces failed writes, a full disk, failed syncs and renames, a
/// panic, a writer stopped mid-file, a move between volumes and other conditions that are hard
/// to make on a real disk.
///
/// Every method without a fault goes to the inner provider, the ones with a default
/// implementation too, so the inner provider behaves exactly as it does on its own. Its locks
/// are released before an injected panic, and a poisoned lock is used as it is, so a panic
/// does not break the calls that follow.
#[derive(Debug)]
pub struct FaultyProvider {
    inner: Arc<dyn Provider>,
    faults: Vec<Fault>,
    counters: Mutex<Counters>,
    shared: Arc<Shared>,
}

impl FaultyProvider {
    pub fn new(inner: Arc<dyn Provider>, faults: Vec<Fault>) -> FaultyProvider {
        FaultyProvider {
            inner,
            faults,
            counters: Mutex::default(),
            shared: Arc::default(),
        }
    }

    /// A faulty [`LocalProvider`].
    pub fn local(faults: Vec<Fault>) -> Arc<FaultyProvider> {
        Arc::new(FaultyProvider::new(Arc::new(LocalProvider::new()), faults))
    }

    /// A registry whose only provider is this one.
    pub fn vfs(self: &Arc<Self>) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.register(Arc::clone(self) as Arc<dyn Provider>);
        vfs
    }

    /// How many times `open_write` was called.
    pub fn opened_writes(&self) -> usize {
        self.counters().opened_writes
    }

    /// Waits up to `timeout` for a writer to stop at a [`Fault::Hold`]; true if one has.
    pub fn wait_held(&self, timeout: Duration) -> bool {
        self.shared.wait_for(timeout, |signals| signals.held)
    }

    /// Lets the writer held by [`Fault::Hold`] go on, and any that reaches it later.
    pub fn release(&self) {
        self.shared.set(|signals| signals.released = true);
    }

    /// Waits up to `timeout` for a writer to stop at a [`Fault::Crash`]; true if one has.
    pub fn wait_crashed(&self, timeout: Duration) -> bool {
        self.shared.wait_for(timeout, |signals| signals.crashed)
    }

    /// The counter lock. Every update is one assignment or increment, so a poisoned lock is
    /// used as it is.
    fn counters(&self) -> MutexGuard<'_, Counters> {
        self.counters.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `entry`, which describes the item at `path`: without its identity under
    /// [`Fault::NoIdentity`], with another volume ID under a [`Fault::OtherVolume`] above it,
    /// else as it is.
    fn identity_of(&self, path: &VPath, mut entry: Entry) -> Entry {
        if self.faults.contains(&Fault::NoIdentity) {
            entry.volume_id = None;
            entry.file_id = None;
        }
        let other_volume = self
            .faults
            .iter()
            .any(|fault| matches!(fault, Fault::OtherVolume { under } if path.starts_with(under)));
        if other_volume {
            entry.volume_id = entry.volume_id.map(|volume| VolumeId(!volume.0));
        }
        entry
    }

    /// Fails with `PermissionDenied` when a [`Fault::FailDelete`] names the item at `path`.
    fn refuse_delete(&self, path: &VPath) -> Result<()> {
        let refused = self.faults.iter().any(|fault| {
            matches!(fault, Fault::FailDelete { name } if path.file_name() == Some(name.as_os_str()))
        });
        if refused {
            let error = io::Error::from(io::ErrorKind::PermissionDenied);
            return Err(Error::io("delete", path.clone(), error));
        }
        Ok(())
    }

    /// Makes the [`Fault::CreateOnDelete`] file next to `path`, the first time only.
    fn make_late_file(&self, path: &VPath) -> Result<()> {
        let Some(name) = self.faults.iter().find_map(|fault| match fault {
            Fault::CreateOnDelete { name } => Some(name),
            _ => None,
        }) else {
            return Ok(());
        };
        if std::mem::replace(&mut self.counters().late_file_made, true) {
            return Ok(());
        }
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let late = parent.join(name)?;
        let mut stream = self.inner.open_write(&late, WriteMode::CreateNew)?;
        stream
            .write_all(b"late")
            .map_err(|e| Error::io("write", late.clone(), e))?;
        stream.finish()
    }
}

impl Provider for FaultyProvider {
    fn scheme(&self) -> Scheme {
        self.inner.scheme()
    }

    fn capabilities(&self, path: &VPath) -> Capabilities {
        let mut capabilities = self.inner.capabilities(path);
        let read_only = self
            .faults
            .iter()
            .any(|fault| matches!(fault, Fault::ReadOnly { under } if path.starts_with(under)));
        if read_only {
            capabilities.remove(Capabilities::WRITE);
        }
        capabilities
    }

    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()> {
        let unlisted = self.faults.contains(&Fault::NoListedIdentity);
        self.inner.list(dir, cancel, &mut |batch: Vec<Entry>| {
            let batch = batch.into_iter().map(|entry| {
                let mut entry = match dir.join(&entry.name) {
                    Ok(path) => self.identity_of(&path, entry),
                    Err(_) => entry,
                };
                if unlisted {
                    (entry.volume_id, entry.file_id) = (None, None);
                }
                entry
            });
            sink(batch.collect());
        })
    }

    fn stat(&self, path: &VPath) -> Result<Entry> {
        let refused = self.faults.iter().any(|fault| {
            matches!(fault, Fault::FailStat { name } if path.file_name() == Some(name.as_os_str()))
        });
        if refused {
            let error = io::Error::from(io::ErrorKind::PermissionDenied);
            return Err(Error::io("stat", path.clone(), error));
        }
        self.inner.stat(path).map(|e| self.identity_of(path, e))
    }

    fn create_dir(&self, path: &VPath) -> Result<()> {
        let call = {
            let mut counters = self.counters();
            counters.created_dirs += 1;
            counters.created_dirs - 1
        };
        if self.faults.contains(&Fault::FailCreateDir { call }) {
            let error = io::Error::other("injected create_dir failure");
            return Err(Error::io("create_dir", path.clone(), error));
        }
        self.inner.create_dir(path)
    }

    fn create_file(&self, path: &VPath) -> Result<()> {
        self.inner.create_file(path)
    }

    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
        let call = {
            let mut counters = self.counters();
            counters.renames += 1;
            counters.renames - 1
        };
        if self.faults.contains(&Fault::FailRename { call }) {
            let error = io::Error::other("injected rename failure");
            return Err(Error::io("rename", from.clone(), error));
        }
        let commits_a_temp_file = from.file_name().and_then(temp_job).is_some();
        if self.faults.contains(&Fault::CrossDevice) && !commits_a_temp_file {
            let error = io::Error::from(io::ErrorKind::CrossesDevices);
            return Err(Error::io("rename", from.clone(), error));
        }
        self.inner.rename(from, to, mode)
    }

    fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted> {
        self.refuse_delete(path)?;
        if mode == DeleteMode::Permanent {
            self.make_late_file(path)?;
        }
        if mode == DeleteMode::Trash && self.faults.contains(&Fault::TrashDeletesForGood) {
            return self.inner.delete(path, DeleteMode::Permanent);
        }
        self.inner.delete(path, mode)
    }

    fn delete_leaf(&self, path: &VPath) -> Result<()> {
        self.refuse_delete(path)?;
        self.make_late_file(path)?;
        self.inner.delete_leaf(path)
    }

    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
        let inner = self.inner.open_read(path)?;
        let named = |name: &OsString| path.file_name() == Some(name.as_os_str());
        let ranges = self.faults.iter().find_map(|fault| match fault {
            Fault::DataRanges { name, ranges } if named(name) => Some(ranges.clone()),
            _ => None,
        });
        let fail_seek = self
            .faults
            .iter()
            .any(|fault| matches!(fault, Fault::FailSeek { name } if named(name)));
        Ok(Box::new(FaultyReader {
            inner,
            path: path.clone(),
            ranges,
            fail_seek,
        }))
    }

    fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
        let file = {
            let mut counters = self.counters();
            counters.opened_writes += 1;
            counters.opened_writes - 1
        };
        let inner = self.inner.open_write(path, mode)?;
        let faults = self
            .faults
            .iter()
            .filter(|fault| fault.file() == Some(file))
            .cloned()
            .collect();
        Ok(Box::new(FaultyWriter {
            inner: Some(inner),
            path: path.clone(),
            faults,
            no_holes: self.faults.contains(&Fault::NoHoles),
            written: 0,
            started: false,
            struck: None,
            shared: Arc::clone(&self.shared),
        }))
    }

    fn space(&self, path: &VPath) -> Result<Option<DiskSpace>> {
        let injected = self.faults.iter().find_map(|fault| match fault {
            Fault::Space(space) => Some(*space),
            _ => None,
        });
        match injected {
            Some(space) => Ok(Some(space)),
            None => self.inner.space(path),
        }
    }

    fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
        self.inner.watch(dir, sink)
    }

    fn stat_target(&self, path: &VPath) -> Result<Entry> {
        self.inner
            .stat_target(path)
            .map(|e| self.identity_of(path, e))
    }

    fn read_link(&self, path: &VPath) -> Result<LinkTarget> {
        self.inner.read_link(path)
    }

    fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> Result<()> {
        self.inner.create_symlink(link, target)
    }

    fn set_metadata(&self, path: &VPath, patch: &MetadataPatch) -> Result<()> {
        if self.faults.contains(&Fault::NoMetadata) {
            let what = format!("set_metadata is not supported for {path}");
            return Err(Error::Unsupported(what));
        }
        self.inner.set_metadata(path, patch)
    }

    fn copy_extras(&self, from: &VPath, to: &VPath) -> Result<()> {
        if self.faults.contains(&Fault::NoMetadata) {
            return Ok(());
        }
        self.inner.copy_extras(from, to)
    }

    fn remove_dir(&self, path: &VPath) -> Result<()> {
        if self.faults.contains(&Fault::FailRemoveDir) {
            let error = io::Error::other("injected remove_dir failure");
            return Err(Error::io("remove_dir", path.clone(), error));
        }
        self.inner.remove_dir(path)
    }

    fn sync_dir(&self, path: &VPath) -> Result<()> {
        if self.faults.contains(&Fault::FailSyncDir) {
            let error = io::Error::other("injected sync_dir failure");
            return Err(Error::io("sync_dir", path.clone(), error));
        }
        self.inner.sync_dir(path)
    }

    fn fs_name(&self, path: &VPath) -> Option<String> {
        let injected = self.faults.iter().find_map(|fault| match fault {
            Fault::FsName(name) => Some(name.clone()),
            _ => None,
        });
        injected.or_else(|| self.inner.fs_name(path))
    }

    fn restore_trashed(&self, location: &VPath, original: &VPath) -> Result<()> {
        self.inner.restore_trashed(location, original)
    }

    fn data_ranges(&self, path: &VPath) -> Result<Option<Vec<Range<u64>>>> {
        self.inner.data_ranges(path)
    }
}

/// A file a [`FaultyProvider`] reads: the inner stream, the ranges of the
/// [`Fault::DataRanges`] for its item, if there is one, and whether a [`Fault::FailSeek`]
/// names it.
#[derive(Debug)]
struct FaultyReader {
    inner: Box<dyn ReadStream>,
    path: VPath,
    ranges: Option<Vec<Range<u64>>>,
    fail_seek: bool,
}

impl Read for FaultyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for FaultyReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.fail_seek {
            return Err(io::Error::other("injected seek failure"));
        }
        self.inner.seek(pos)
    }
}

impl ReadStream for FaultyReader {
    /// The injected ranges come with the length the file has now, which the inner stream tells
    /// by a seek to its end; then it is put back where it was. Without a [`Fault::DataRanges`]
    /// the call is passed on.
    ///
    /// It keeps the promise of `ReadStream::data_ranges` in every case: an error comes only
    /// before the stream has moved (a failed seek does not move it). Once it has moved, the
    /// ranges are given even when the seek back fails, since after `Ok(Some(_))` the position
    /// is unspecified and a reader seeks before it reads; the stream is never left moved behind
    /// an error that says it was not.
    fn data_ranges(&mut self) -> Result<Option<DataRanges>> {
        let Some(ranges) = &self.ranges else {
            return self.inner.data_ranges();
        };
        let fail = |e| Error::io("data_ranges", self.path.clone(), e);
        let at = self.inner.stream_position().map_err(fail)?;
        let len = self.inner.seek(SeekFrom::End(0)).map_err(fail)?;
        // A reader seeks after `Ok(Some(_))` anyway (see above).
        let _ = self.inner.seek(SeekFrom::Start(at));
        Ok(Some(DataRanges {
            len,
            ranges: ranges.clone(),
        }))
    }
}

/// A file a [`FaultyProvider`] writes, with the faults of its `open_write` number.
#[derive(Debug)]
struct FaultyWriter {
    /// `None` once a crash has closed it.
    inner: Option<Box<dyn WriteStream>>,
    path: VPath,
    faults: Vec<Fault>,
    /// Whether the provider has [`Fault::NoHoles`].
    no_holes: bool,
    /// The bytes passed to the inner stream so far; holes do not count.
    written: u64,
    /// Whether `write` or `write_hole` was called before.
    started: bool,
    /// The fault with a byte count that has struck, if one has.
    struck: Option<Fault>,
    shared: Arc<Shared>,
}

impl FaultyWriter {
    fn has(&self, wanted: impl Fn(&Fault) -> bool) -> bool {
        self.faults.iter().any(wanted)
    }

    /// Applies the faults of the first `write` or `write_hole`: a panic, or a wait for
    /// [`FaultyProvider::release`].
    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        // No lock is held here, so the panic poisons nothing.
        assert!(
            !self.has(|fault| matches!(fault, Fault::Panic { .. })),
            "injected panic"
        );
        if self.has(|fault| matches!(fault, Fault::Hold { .. })) {
            self.shared.set(|signals| signals.held = true);
            self.shared.wait_until(|signals| signals.released);
        }
    }

    /// The error that `fault`, one with a byte count other than a crash, fails a write with.
    fn injected(fault: &Fault) -> io::Error {
        match fault {
            Fault::DiskFull { .. } => {
                io::Error::new(io::ErrorKind::StorageFull, "injected disk full")
            }
            // `FailWrite`, the other fault with a byte count.
            _ => io::Error::other("injected write failure"),
        }
    }

    fn stream(&mut self) -> io::Result<&mut Box<dyn WriteStream>> {
        self.inner
            .as_mut()
            .ok_or_else(|| io::Error::other("the stream is closed"))
    }

    /// Flushes and closes the inner stream, says so, and stops this thread for good.
    fn crash(&mut self) -> ! {
        if let Some(mut stream) = self.inner.take() {
            let _ = stream.flush();
        }
        self.shared.set(|signals| signals.crashed = true);
        loop {
            std::thread::park();
        }
    }
}

impl Write for FaultyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.start();
        let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let strike = self
            .faults
            .iter()
            .filter_map(|fault| fault.after().map(|after| (after, fault)))
            .filter(|&(after, _)| self.written.saturating_add(len) > after)
            .min_by_key(|&(after, _)| after)
            .map(|(after, fault)| (after, fault.clone()));
        if let Some((after, fault)) = strike {
            let room = usize::try_from(after.saturating_sub(self.written)).unwrap_or(buf.len());
            let part = &buf[..room.min(buf.len())];
            self.stream()?.write_all(part)?;
            self.written += u64::try_from(part.len()).unwrap_or(u64::MAX);
            if matches!(fault, Fault::Crash { .. }) {
                self.crash();
            }
            let error = Self::injected(&fault);
            self.struck = Some(fault);
            return Err(error);
        }
        let n = self.stream()?.write(buf)?;
        self.written += u64::try_from(n).unwrap_or(u64::MAX);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream()?.flush()
    }
}

impl WriteStream for FaultyWriter {
    fn finish(mut self: Box<Self>) -> Result<()> {
        let inner = self.inner.take();
        if self.has(|fault| matches!(fault, Fault::FailFinish { .. })) {
            // The inner stream is dropped, so the file is closed without a sync.
            drop(inner);
            let error = io::Error::other("injected fsync failure");
            return Err(Error::io("finish", self.path.clone(), error));
        }
        let Some(stream) = inner else {
            let error = io::Error::other("the stream is closed");
            return Err(Error::io("finish", self.path.clone(), error));
        };
        stream.finish()
    }

    /// Passed on, without counting toward the byte count of a fault, unless a fault with a byte
    /// count has struck (the same error again) or the provider has [`Fault::NoHoles`].
    fn write_hole(&mut self, len: u64) -> Result<bool> {
        self.start();
        let error = match &self.struck {
            Some(fault) => Self::injected(fault),
            None if self.no_holes => return Ok(false),
            None => match self.stream() {
                Ok(stream) => return stream.write_hole(len),
                Err(error) => error,
            },
        };
        Err(Error::io("write_hole", self.path.clone(), error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unscripted_question_leaves_the_lock_unpoisoned() {
        let resolver = ScriptedResolver::new();
        assert!(std::panic::catch_unwind(|| resolver.preflight(JobId(1), &[])).is_err());
        assert!(!resolver.script.is_poisoned());
        assert_eq!(resolver.calls(), [ResolverCall::Preflight { issues: 0 }]);
    }

    fn local(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    /// Writes `len` bytes to `name` under `dir` in chunks of 4, then finishes.
    fn write_file(p: &FaultyProvider, dir: &Path, name: &str, len: usize) -> Result<()> {
        let mut stream = p.open_write(&local(&dir.join(name)), WriteMode::CreateNew)?;
        for chunk in vec![b'x'; len].chunks(4) {
            stream
                .write_all(chunk)
                .map_err(|e| Error::io("write", local(dir), e))?;
        }
        stream.finish()
    }

    #[test]
    fn write_faults_strike_their_own_file_at_their_byte_count() {
        let dir = tempfile::tempdir().unwrap();
        let p = FaultyProvider::new(
            Arc::new(LocalProvider::new()),
            vec![
                Fault::DiskFull { file: 1, after: 6 },
                Fault::FailFinish { file: 2 },
                Fault::FailWrite { file: 3, after: 8 },
            ],
        );
        write_file(&p, dir.path(), "0", 10).unwrap();
        let full = write_file(&p, dir.path(), "1", 10).unwrap_err();
        assert_eq!(full.kind(), rf_core::ErrorKind::StorageFull);
        assert!(full.to_string().contains("injected disk full"), "{full}");
        assert_eq!(std::fs::read(dir.path().join("1")).unwrap(), b"xxxxxx");
        let unsynced = write_file(&p, dir.path(), "2", 3).unwrap_err();
        assert!(unsynced.to_string().contains("injected fsync failure"));
        // Exactly `after` bytes never pass `after`.
        write_file(&p, dir.path(), "3", 8).unwrap();
        assert_eq!(p.opened_writes(), 4);
    }

    #[test]
    fn holes_are_passed_on_uncounted_until_a_fault_strikes() {
        let dir = tempfile::tempdir().unwrap();
        let p = FaultyProvider::local(vec![
            Fault::FailWrite { file: 0, after: 4 },
            Fault::Panic { file: 1 },
        ]);
        let open = |name: &str| {
            p.open_write(&local(&dir.path().join(name)), WriteMode::CreateNew)
                .unwrap()
        };
        let mut stream = open("0");
        stream.write_all(b"ab").unwrap();
        assert!(stream.write_hole(1 << 20).unwrap());
        // Had the hole counted, these two bytes would be past `after`.
        stream.write_all(b"cd").unwrap();
        let error = stream.write_all(b"e").unwrap_err();
        assert_eq!(error.to_string(), "injected write failure");
        let error = stream.write_hole(1).unwrap_err();
        assert!(
            error.to_string().contains("injected write failure"),
            "{error}"
        );
        drop(stream);
        let mut panicking = open("1");
        let panicked =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| panicking.write_hole(1)));
        assert!(panicked.is_err());
        drop(panicking);

        let mut stream = open("2");
        stream.write_all(b"a").unwrap();
        assert!(stream.write_hole(3).unwrap());
        stream.write_all(b"b").unwrap();
        assert!(stream.write_hole(2).unwrap());
        stream.finish().unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("2")).unwrap(),
            b"a\0\0\0b\0\0"
        );

        let p = FaultyProvider::local(vec![Fault::NoHoles]);
        let mut stream = p
            .open_write(&local(&dir.path().join("3")), WriteMode::CreateNew)
            .unwrap();
        assert!(!stream.write_hole(1).unwrap());
    }

    #[test]
    fn data_ranges_are_passed_on_unless_injected() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (local(&dir.path().join("a")), local(&dir.path().join("b")));
        std::fs::write(dir.path().join("a"), b"dense").unwrap();
        // Sparse where the file system keeps holes, so the inner answer is not the default.
        let mut sparse = LocalProvider::new()
            .open_write(&b, WriteMode::CreateNew)
            .unwrap();
        sparse.write_all(b"x").unwrap();
        assert!(sparse.write_hole(1 << 20).unwrap());
        sparse.finish().unwrap();
        let p = FaultyProvider::local(vec![
            Fault::DataRanges {
                name: "a".into(),
                ranges: vec![1..2, 4..5],
            },
            Fault::FailSeek { name: "b".into() },
        ]);
        let local = LocalProvider::new();
        let streamed = |p: &dyn Provider, path| p.open_read(path).unwrap().data_ranges().unwrap();
        // The fault is for the streams of `a`, with the length `a` has; everything else is
        // passed on.
        let injected = DataRanges {
            len: 5,
            ranges: vec![1..2, 4..5],
        };
        assert_eq!(streamed(&*p, &a), Some(injected));
        assert_eq!(streamed(&*p, &b), streamed(&local, &b));
        for path in [&a, &b] {
            assert_eq!(
                p.data_ranges(path).unwrap(),
                local.data_ranges(path).unwrap()
            );
        }
        // The stream of `b` reads but does not seek, and the injected answer did not move `a`'s.
        let mut stream = p.open_read(&b).unwrap();
        let error = stream.seek(SeekFrom::Start(0)).unwrap_err();
        assert_eq!(error.to_string(), "injected seek failure");
        let mut first = [0; 1];
        stream.read_exact(&mut first).unwrap();
        assert_eq!(first, *b"x");
        let mut stream = p.open_read(&a).unwrap();
        stream.data_ranges().unwrap();
        stream.read_exact(&mut first).unwrap();
        assert_eq!(first, *b"d");
    }

    #[test]
    #[should_panic(expected = "tree: cannot list")]
    fn tree_of_a_missing_folder_panics() {
        let dir = tempfile::tempdir().unwrap();
        tree(&dir.path().join("missing"));
    }

    #[test]
    fn hold_waits_for_release_and_crash_stops_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let p = FaultyProvider::local(vec![
            Fault::Hold { file: 0 },
            Fault::Crash { file: 1, after: 5 },
        ]);
        let held = {
            let (p, dir) = (Arc::clone(&p), dir.path().to_path_buf());
            std::thread::spawn(move || write_file(&p, &dir, "held", 8))
        };
        assert!(p.wait_held(Duration::from_secs(10)));
        assert!(!held.is_finished());
        p.release();
        held.join().unwrap().unwrap();
        // The crashed thread never returns; it is left parked, like a killed process.
        let _crashed = {
            let (p, dir) = (Arc::clone(&p), dir.path().to_path_buf());
            std::thread::spawn(move || write_file(&p, &dir, "crashed", 8))
        };
        assert!(p.wait_crashed(Duration::from_secs(10)));
        assert_eq!(std::fs::read(dir.path().join("crashed")).unwrap(), b"xxxxx");
    }

    #[test]
    fn rename_delete_and_volume_faults() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, b"a").unwrap();
        let p = FaultyProvider::local(vec![
            Fault::FailRename { call: 1 },
            Fault::FailDelete {
                name: "keep".into(),
            },
            Fault::CreateOnDelete {
                name: "late".into(),
            },
            Fault::ReadOnly {
                under: local(&dir.path().join("ro")),
            },
            Fault::FsName("exFAT".into()),
            Fault::Space(DiskSpace {
                total: 9,
                free: 8,
                available: 7,
            }),
        ]);
        p.rename(&local(&a), &local(&b), RenameMode::NoReplace)
            .unwrap();
        let second = p.rename(&local(&b), &local(&a), RenameMode::NoReplace);
        assert!(
            second
                .unwrap_err()
                .to_string()
                .contains("injected rename failure")
        );
        p.rename(&local(&b), &local(&a), RenameMode::NoReplace)
            .unwrap();

        std::fs::write(dir.path().join("keep"), b"k").unwrap();
        let refused = p.delete(&local(&dir.path().join("keep")), DeleteMode::Permanent);
        assert_eq!(
            refused.unwrap_err().kind(),
            rf_core::ErrorKind::PermissionDenied
        );
        assert!(!dir.path().join("late").exists());
        p.delete(&local(&a), DeleteMode::Permanent).unwrap();
        assert_eq!(std::fs::read(dir.path().join("late")).unwrap(), b"late");
        std::fs::remove_file(dir.path().join("late")).unwrap();
        std::fs::write(&a, b"a").unwrap();
        p.delete(&local(&a), DeleteMode::Permanent).unwrap();
        assert!(!dir.path().join("late").exists(), "only the first delete");

        let ro = local(&dir.path().join("ro").join("x"));
        assert!(!p.capabilities(&ro).contains(Capabilities::WRITE));
        let root = local(dir.path());
        assert_eq!(
            p.capabilities(&root),
            LocalProvider::new().capabilities(&root)
        );
        assert_eq!(p.fs_name(&root).as_deref(), Some("exFAT"));
        assert_eq!(p.space(&root).unwrap().map(|s| s.available), Some(7));
    }

    #[test]
    fn delete_leaf_meets_the_delete_faults() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["keep", "a"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let p = FaultyProvider::local(vec![
            Fault::FailDelete {
                name: "keep".into(),
            },
            Fault::CreateOnDelete {
                name: "late".into(),
            },
        ]);
        let at = |name: &str| local(&dir.path().join(name));
        let refused = p.delete_leaf(&at("keep")).unwrap_err();
        assert_eq!(refused.kind(), rf_core::ErrorKind::PermissionDenied);
        assert!(!dir.path().join("late").exists());
        p.delete_leaf(&at("a")).unwrap();
        assert_eq!(std::fs::read(dir.path().join("late")).unwrap(), b"late");
        // The inner provider's answer: a folder is never removed.
        let folder = p.delete_leaf(&at("folder")).unwrap_err();
        assert_eq!(folder.kind(), rf_core::ErrorKind::IsADirectory);
        assert!(dir.path().join("folder").is_dir());
        assert_eq!(std::fs::read(dir.path().join("keep")).unwrap(), b"keep");
    }

    #[test]
    fn a_create_dir_fault_strikes_its_own_call_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = FaultyProvider::local(vec![Fault::FailCreateDir { call: 1 }]);
        let at = |name: &str| local(&dir.path().join(name));
        p.create_dir(&at("a")).unwrap();
        let error = p.create_dir(&at("b")).unwrap_err();
        assert!(error.to_string().contains("injected create_dir failure"));
        assert!(!dir.path().join("b").exists());
        p.create_dir(&at("b")).unwrap();
    }

    #[test]
    fn sync_dir_is_passed_on_unless_it_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = local(dir.path());
        let p = FaultyProvider::local(vec![]);
        p.sync_dir(&root).unwrap();
        // The local provider's own answer, not the default one, which never fails.
        #[cfg(unix)]
        assert_eq!(
            p.sync_dir(&local(&dir.path().join("missing")))
                .unwrap_err()
                .kind(),
            rf_core::ErrorKind::NotFound
        );
        let p = FaultyProvider::local(vec![Fault::FailSyncDir]);
        let error = p.sync_dir(&root).unwrap_err();
        assert!(
            error.to_string().contains("injected sync_dir failure"),
            "{error}"
        );
    }

    #[test]
    fn a_remove_dir_fault_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let p = FaultyProvider::local(vec![Fault::FailRemoveDir]);
        let error = p.remove_dir(&local(&empty)).unwrap_err();
        assert!(
            error.to_string().contains("injected remove_dir failure"),
            "{error}"
        );
        assert!(empty.is_dir());
        FaultyProvider::local(vec![])
            .remove_dir(&local(&empty))
            .unwrap();
        assert!(!empty.exists());
    }

    #[test]
    fn other_volume_changes_the_volume_of_what_is_under_it() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), &[("a/x", "x"), ("b", "b")]);
        let (a, x, b) = (
            local(&dir.path().join("a")),
            local(&dir.path().join("a").join("x")),
            local(&dir.path().join("b")),
        );
        let plain = FaultyProvider::local(vec![]);
        let p = FaultyProvider::local(vec![Fault::OtherVolume { under: a.clone() }]);
        for path in [&a, &x] {
            let (real, seen) = (plain.stat(path).unwrap(), p.stat(path).unwrap());
            assert_ne!(seen.volume_id, real.volume_id);
            assert_eq!(seen.file_id, real.file_id);
            assert_eq!(p.stat_target(path).unwrap().volume_id, seen.volume_id);
        }
        assert_eq!(
            p.stat(&b).unwrap().volume_id,
            plain.stat(&b).unwrap().volume_id
        );
        // A listing has identities off Windows only.
        #[cfg(unix)]
        {
            let mut listed = Vec::new();
            p.list(&local(dir.path()), &CancelToken::new(), &mut |batch| {
                listed.extend(batch.into_iter().map(|e| (e.name, e.volume_id)));
            })
            .unwrap();
            listed.sort();
            let expected = [
                ("a".into(), p.stat(&a).unwrap().volume_id),
                ("b".into(), plain.stat(&b).unwrap().volume_id),
            ];
            assert_eq!(listed, expected);
        }
    }

    #[test]
    fn stat_faults_refuse_one_name_and_hide_identities() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        let (a, b) = (local(&dir.path().join("a")), local(&dir.path().join("b")));
        let p = FaultyProvider::local(vec![Fault::FailStat { name: "b".into() }]);
        assert_eq!(
            p.stat(&b).unwrap_err().kind(),
            rf_core::ErrorKind::PermissionDenied
        );
        assert!(p.stat(&a).unwrap().identity().is_some());

        let p = FaultyProvider::local(vec![Fault::NoIdentity]);
        assert_eq!(p.stat(&a).unwrap().identity(), None);
        assert_eq!(p.stat_target(&a).unwrap().identity(), None);
        let mut listed = Vec::new();
        p.list(&local(dir.path()), &CancelToken::new(), &mut |batch| {
            listed.extend(batch.into_iter().map(|e| e.identity()));
        })
        .unwrap();
        assert_eq!(listed, [None, None]);

        // A Windows listing: none in the listing, but `stat` still tells.
        let p = FaultyProvider::local(vec![Fault::NoListedIdentity]);
        let mut listed = Vec::new();
        p.list(&local(dir.path()), &CancelToken::new(), &mut |batch| {
            listed.extend(batch.into_iter().map(|e| e.identity()));
        })
        .unwrap();
        assert_eq!(listed, [None, None]);
        let real = LocalProvider::new();
        assert_eq!(
            p.stat(&a).unwrap().identity(),
            real.stat(&a).unwrap().identity()
        );
        assert_eq!(
            p.stat_target(&b).unwrap().identity(),
            real.stat_target(&b).unwrap().identity()
        );
    }

    #[test]
    fn a_trash_that_deletes_for_good_says_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "a").unwrap();
        let p = FaultyProvider::local(vec![Fault::TrashDeletesForGood]);
        let a = local(&dir.path().join("a"));
        assert_eq!(
            p.delete(&a, DeleteMode::Trash).unwrap(),
            Deleted::Permanently
        );
        assert!(!dir.path().join("a").exists());
    }

    /// A stream that can seek to its end but fails every seek back to a position from its
    /// start once it has moved.
    #[derive(Debug)]
    struct OneWay {
        inner: io::Cursor<Vec<u8>>,
        moved: bool,
    }

    impl Read for OneWay {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl Seek for OneWay {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            match pos {
                SeekFrom::Start(_) if self.moved => Err(io::Error::other("no way back")),
                SeekFrom::End(_) => {
                    self.moved = true;
                    self.inner.seek(pos)
                }
                _ => self.inner.seek(pos),
            }
        }
    }

    impl ReadStream for OneWay {}

    /// Injected ranges never come with an error once the stream has moved: a failed seek back
    /// gives the ranges, after which a reader seeks anyway.
    #[test]
    fn injected_ranges_are_given_when_the_stream_cannot_get_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut reader = FaultyReader {
            inner: Box::new(OneWay {
                inner: io::Cursor::new(b"12345".to_vec()),
                moved: false,
            }),
            path: local(dir.path()),
            ranges: Some(vec![1..2, 3..4]),
            fail_seek: false,
        };
        let found = reader.data_ranges().unwrap();
        assert_eq!(
            found,
            Some(DataRanges {
                len: 5,
                ranges: vec![1..2, 3..4],
            })
        );
    }
}
