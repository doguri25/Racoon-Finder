//! A provider for the tests of the workers: the local one, with a hook that changes what
//! `stat` gives and one that hears of each `open_read` before the file opens, so that a test
//! can stand in for what only another system or a slow drive does (a file a cloud drive
//! dehydrated after it was listed, a file that takes long to open).

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use rf_core::{Entry, Scheme, VPath};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LinkTarget, LocalProvider,
    MetadataPatch, Provider, ReadStream, RenameMode, Vfs, WatchHandle, WatchSink, WriteMode,
    WriteStream,
};

/// Changes the entry `stat` gives for a path.
type StatHook = dyn Fn(&VPath, &mut Entry) + Send + Sync;
/// Hears of an `open_read` of a path, before it opens.
type OpenHook = dyn Fn(&VPath) + Send + Sync;

/// The local provider, with its hooks: `stat` and `stat_target` give what the stat hook makes
/// of the local entry, `open_read` tells the open hook first, and every other method of
/// [`Provider`], the optional ones too, is the local provider's own.
pub struct Hooked {
    local: LocalProvider,
    stat: Box<StatHook>,
    open: Box<OpenHook>,
}

impl Hooked {
    /// A registry whose local paths go through the local provider with these hooks.
    pub fn vfs(
        stat: impl Fn(&VPath, &mut Entry) + Send + Sync + 'static,
        open: impl Fn(&VPath) + Send + Sync + 'static,
    ) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.register(Arc::new(Hooked {
            local: LocalProvider::new(),
            stat: Box::new(stat),
            open: Box::new(open),
        }));
        vfs
    }
}

impl fmt::Debug for Hooked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooked").finish_non_exhaustive()
    }
}

impl Provider for Hooked {
    fn scheme(&self) -> Scheme {
        self.local.scheme()
    }

    fn capabilities(&self, path: &VPath) -> Capabilities {
        self.local.capabilities(path)
    }

    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> rf_core::Result<()> {
        self.local.list(dir, cancel, sink)
    }

    fn stat(&self, path: &VPath) -> rf_core::Result<Entry> {
        let mut entry = self.local.stat(path)?;
        (self.stat)(path, &mut entry);
        Ok(entry)
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
        (self.open)(path);
        self.local.open_read(path)
    }

    fn open_write(&self, path: &VPath, mode: WriteMode) -> rf_core::Result<Box<dyn WriteStream>> {
        self.local.open_write(path, mode)
    }

    fn space(&self, path: &VPath) -> rf_core::Result<Option<DiskSpace>> {
        self.local.space(path)
    }

    fn watch(&self, dir: &VPath, sink: WatchSink) -> rf_core::Result<WatchHandle> {
        self.local.watch(dir, sink)
    }

    fn stat_target(&self, path: &VPath) -> rf_core::Result<Entry> {
        let mut entry = self.local.stat_target(path)?;
        (self.stat)(path, &mut entry);
        Ok(entry)
    }

    fn read_link(&self, path: &VPath) -> rf_core::Result<LinkTarget> {
        self.local.read_link(path)
    }

    fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> rf_core::Result<()> {
        self.local.create_symlink(link, target)
    }

    fn set_metadata(&self, path: &VPath, patch: &MetadataPatch) -> rf_core::Result<()> {
        self.local.set_metadata(path, patch)
    }

    fn copy_extras(&self, from: &VPath, to: &VPath) -> rf_core::Result<()> {
        self.local.copy_extras(from, to)
    }

    fn remove_dir(&self, path: &VPath) -> rf_core::Result<()> {
        self.local.remove_dir(path)
    }

    fn delete_leaf(&self, path: &VPath) -> rf_core::Result<()> {
        self.local.delete_leaf(path)
    }

    fn sync_dir(&self, path: &VPath) -> rf_core::Result<()> {
        self.local.sync_dir(path)
    }

    fn fs_name(&self, path: &VPath) -> Option<String> {
        self.local.fs_name(path)
    }

    fn data_ranges(&self, path: &VPath) -> rf_core::Result<Option<Vec<Range<u64>>>> {
        self.local.data_ranges(path)
    }

    fn restore_trashed(&self, location: &VPath, original: &VPath) -> rf_core::Result<()> {
        self.local.restore_trashed(location, original)
    }
}
