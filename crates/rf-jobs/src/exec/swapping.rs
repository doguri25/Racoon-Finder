//! A provider for the tests of the walks: it turns a folder into a link to another folder in
//! the middle of a job, as another program could, so a test can show that nothing the link
//! leads to is ever deleted.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rf_core::{Entry, Result, Scheme, VPath};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LinkTarget, MetadataPatch, Provider,
    ReadStream, RenameMode, Vfs, WatchHandle, WatchSink, WriteMode, WriteStream,
};

/// When a [`Swapping`] provider turns its folder into a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum When {
    /// As listing number `n` (from 0) of the folder starts.
    Listing(usize),
    /// Right before the first permanent delete of one item (`delete_leaf`, or `delete` with
    /// `DeleteMode::Permanent`) is passed on.
    DeletingALeaf,
    /// Right before `stat` number `nth` (from 0) of the item `of`, counting only those made
    /// once the provider is armed (see [`Swapping::arm`]).
    Stat { of: PathBuf, nth: usize },
}

/// An inner provider for every call, but the folder `dir` is moved away to `dir.moved` with
/// everything in it, and a link to `outside` put in its place, once, at `when`.
#[derive(Debug)]
pub(super) struct Swapping {
    inner: Arc<dyn Provider>,
    dir: PathBuf,
    outside: PathBuf,
    when: When,
    /// Whether listed entries come without their volume and file IDs, as the local provider
    /// lists them on Windows.
    no_listed_ids: bool,
    listings: AtomicUsize,
    /// Whether `stat`s of [`When::Stat`] are counted yet.
    armed: AtomicBool,
    stats: AtomicUsize,
    swapped: AtomicBool,
}

impl Swapping {
    pub(super) fn new(inner: Arc<dyn Provider>, dir: &Path, outside: &Path, when: When) -> Self {
        Swapping {
            inner,
            dir: dir.to_path_buf(),
            outside: outside.to_path_buf(),
            when,
            no_listed_ids: false,
            listings: AtomicUsize::new(0),
            armed: AtomicBool::new(false),
            stats: AtomicUsize::new(0),
            swapped: AtomicBool::new(false),
        }
    }

    /// Lists every entry without its volume and file IDs, as the local provider does on
    /// Windows; `stat` still gives them.
    #[must_use]
    pub(super) fn without_listed_ids(mut self) -> Self {
        self.no_listed_ids = true;
        self
    }

    /// A registry whose only provider is this one.
    pub(super) fn vfs(self: &Arc<Self>) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.register(Arc::clone(self) as Arc<dyn Provider>);
        vfs
    }

    /// Starts counting the `stat`s of [`When::Stat`], as a test does when the job's plan is
    /// made, so that only the run's are counted.
    pub(super) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Where the folder went when it was swapped.
    pub(super) fn moved(dir: &Path) -> PathBuf {
        dir.with_extension("moved")
    }

    fn swap(&self) {
        if !self.swapped.swap(true, Ordering::SeqCst) {
            std::fs::rename(&self.dir, Self::moved(&self.dir)).unwrap();
            symlink(&self.outside, &self.dir).unwrap();
        }
    }
}

impl Provider for Swapping {
    fn scheme(&self) -> Scheme {
        self.inner.scheme()
    }
    fn capabilities(&self, path: &VPath) -> Capabilities {
        self.inner.capabilities(path)
    }
    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()> {
        if dir.as_local() == Some(self.dir.as_path()) {
            let n = self.listings.fetch_add(1, Ordering::SeqCst);
            if self.when == When::Listing(n) {
                self.swap();
            }
        }
        let no_ids = self.no_listed_ids;
        self.inner.list(dir, cancel, &mut |mut batch: Vec<Entry>| {
            if no_ids {
                for entry in &mut batch {
                    (entry.volume_id, entry.file_id) = (None, None);
                }
            }
            sink(batch);
        })
    }
    fn stat(&self, path: &VPath) -> Result<Entry> {
        if let When::Stat { of, nth } = &self.when {
            let counted =
                path.as_local() == Some(of.as_path()) && self.armed.load(Ordering::SeqCst);
            if counted && self.stats.fetch_add(1, Ordering::SeqCst) == *nth {
                self.swap();
            }
        }
        self.inner.stat(path)
    }
    fn create_dir(&self, path: &VPath) -> Result<()> {
        self.inner.create_dir(path)
    }
    fn create_file(&self, path: &VPath) -> Result<()> {
        self.inner.create_file(path)
    }
    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
        self.inner.rename(from, to, mode)
    }
    fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted> {
        if self.when == When::DeletingALeaf && mode == DeleteMode::Permanent {
            self.swap();
        }
        self.inner.delete(path, mode)
    }
    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
        self.inner.open_read(path)
    }
    fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
        self.inner.open_write(path, mode)
    }
    fn space(&self, path: &VPath) -> Result<Option<DiskSpace>> {
        self.inner.space(path)
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
    fn delete_leaf(&self, path: &VPath) -> Result<()> {
        if self.when == When::DeletingALeaf {
            self.swap();
        }
        self.inner.delete_leaf(path)
    }
    fn sync_dir(&self, path: &VPath) -> Result<()> {
        self.inner.sync_dir(path)
    }
    fn fs_name(&self, path: &VPath) -> Option<String> {
        self.inner.fs_name(path)
    }
    fn restore_trashed(&self, location: &VPath, original: &VPath) -> Result<()> {
        self.inner.restore_trashed(location, original)
    }
}
