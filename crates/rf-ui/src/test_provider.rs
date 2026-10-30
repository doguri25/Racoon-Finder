//! A provider for the tests of the workers: the local one, with a hook that changes what
//! `stat` gives and one that hears of each `open_read` before the file opens, so that a test
//! can stand in for what only another system or a slow drive does (a file a cloud drive
//! dehydrated after it was listed, a file that takes long to open).

use std::fmt;
use std::sync::Arc;

use rf_core::{Entry, Scheme, VPath};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LocalProvider, Provider, ReadStream,
    RenameMode, Vfs, WatchHandle, WatchSink, WriteMode, WriteStream,
};

/// Changes the entry `stat` gives for a path.
type StatHook = dyn Fn(&VPath, &mut Entry) + Send + Sync;
/// Hears of an `open_read` of a path, before it opens.
type OpenHook = dyn Fn(&VPath) + Send + Sync;

/// The local provider, with its hooks.
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
}
