//! Test doubles for code that uses this crate. The module is always compiled (no feature
//! flag) so that the tests of the crates built on top of this one can use it too.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::ReadDir;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rf_core::{Entry, Error, Result, Scheme, VPath};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LinkTarget, LocalProvider,
    MetadataPatch, Provider, ReadStream, RenameMode, Vfs, WatchHandle, WatchSink, WriteMode,
    WriteStream,
};

use crate::conflict::{Conflict, ConflictAnswer};
use crate::id::JobId;
use crate::naming::{TEMP_SUFFIX, temp_job};
use crate::preflight::Issue;
use crate::report::{ItemFailure, Stage};
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
/// [`Node::Link`] with what [`std::fs::read_link`] gives. A missing `root` gives an empty map.
///
/// Panics on anything that is not a file, a folder or a link (a FIFO, a socket, a device),
/// which it never opens, and on any error other than a missing `root`.
pub fn tree(root: &Path) -> BTreeMap<String, Node> {
    let mut nodes = BTreeMap::new();
    match std::fs::read_dir(root) {
        Ok(entries) => walk(entries, root, None, &mut nodes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => panic!("tree: cannot list {}: {e}", root.display()),
    }
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

/// A fault for a [`FaultyProvider`] to inject.
///
/// `file` counts the provider's `open_write` calls from 0, in the order they were made, failed
/// ones included; `call` counts its `rename` calls the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// The write that would take file `file` past `after` bytes writes up to `after` and fails
    /// with `injected write failure`.
    FailWrite { file: usize, after: u64 },
    /// Like `FailWrite`, but the error is `io::ErrorKind::StorageFull` (`injected disk full`).
    DiskFull { file: usize, after: u64 },
    /// `finish` of file `file` fails with `injected fsync failure` and does not finish the inner
    /// stream (it is closed unsynced).
    FailFinish { file: usize },
    /// The first write to file `file` panics with `injected panic`.
    Panic { file: usize },
    /// The first write to file `file` waits until [`FaultyProvider::release`]. It wakes
    /// [`FaultyProvider::wait_held`] before it waits.
    Hold { file: usize },
    /// Like a killed process: once file `file` has `after` bytes, the inner stream is flushed
    /// and closed, [`FaultyProvider::wait_crashed`] wakes, and the writing thread stops for
    /// good. No later code, no `Drop`, runs on it.
    Crash { file: usize, after: u64 },
    /// Rename number `call` fails with `injected rename failure`.
    FailRename { call: usize },
    /// Every rename fails with `io::ErrorKind::CrossesDevices`, as between two volumes, except
    /// the commit of a temporary file (a `from` whose name `temp_job` reads).
    CrossDevice,
    /// Deleting an item called `name` fails with `PermissionDenied`.
    FailDelete { name: OsString },
    /// Just before the first permanent delete is passed on, a file `name` with the content
    /// `late` appears next to the item, as if another app wrote it during a move. Once only.
    /// If that file cannot be made, the delete fails with the reason and deletes nothing.
    CreateOnDelete { name: OsString },
    /// Paths under `under` (it included) lack `Capabilities::WRITE`.
    ReadOnly { under: VPath },
    /// `fs_name` gives this name.
    FsName(String),
    /// `space` gives this space.
    Space(DiskSpace),
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
        self.inner.list(dir, cancel, sink)
    }

    fn stat(&self, path: &VPath) -> Result<Entry> {
        self.inner.stat(path)
    }

    fn create_dir(&self, path: &VPath) -> Result<()> {
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
        let refused = self.faults.iter().any(|fault| {
            matches!(fault, Fault::FailDelete { name } if path.file_name() == Some(name.as_os_str()))
        });
        if refused {
            let error = io::Error::from(io::ErrorKind::PermissionDenied);
            return Err(Error::io("delete", path.clone(), error));
        }
        if mode == DeleteMode::Permanent {
            self.make_late_file(path)?;
        }
        self.inner.delete(path, mode)
    }

    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
        self.inner.open_read(path)
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
            written: 0,
            started: false,
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
        self.inner.stat_target(path)
    }

    fn read_link(&self, path: &VPath) -> Result<LinkTarget> {
        self.inner.read_link(path)
    }

    fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> Result<()> {
        self.inner.create_symlink(link, target)
    }

    fn set_metadata(&self, path: &VPath, patch: &MetadataPatch) -> Result<()> {
        self.inner.set_metadata(path, patch)
    }

    fn copy_extras(&self, from: &VPath, to: &VPath) -> Result<()> {
        self.inner.copy_extras(from, to)
    }

    fn remove_dir(&self, path: &VPath) -> Result<()> {
        self.inner.remove_dir(path)
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
}

/// A file a [`FaultyProvider`] writes, with the faults of its `open_write` number.
#[derive(Debug)]
struct FaultyWriter {
    /// `None` once a crash has closed it.
    inner: Option<Box<dyn WriteStream>>,
    path: VPath,
    faults: Vec<Fault>,
    /// The bytes passed to the inner stream so far.
    written: u64,
    /// Whether `write` was called before.
    started: bool,
    shared: Arc<Shared>,
}

impl FaultyWriter {
    fn has(&self, wanted: impl Fn(&Fault) -> bool) -> bool {
        self.faults.iter().any(wanted)
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
        if !self.started {
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
            return match fault {
                Fault::DiskFull { .. } => Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected disk full",
                )),
                Fault::Crash { .. } => self.crash(),
                // `FailWrite`, the other fault with a byte count.
                _ => Err(io::Error::other("injected write failure")),
            };
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
            ],
        );
        write_file(&p, dir.path(), "0", 10).unwrap();
        let full = write_file(&p, dir.path(), "1", 10).unwrap_err();
        assert_eq!(full.kind(), rf_core::ErrorKind::StorageFull);
        assert!(full.to_string().contains("injected disk full"), "{full}");
        assert_eq!(std::fs::read(dir.path().join("1")).unwrap(), b"xxxxxx");
        let unsynced = write_file(&p, dir.path(), "2", 3).unwrap_err();
        assert!(unsynced.to_string().contains("injected fsync failure"));
        assert_eq!(p.opened_writes(), 3);
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
}
