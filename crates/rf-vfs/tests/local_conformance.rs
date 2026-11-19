use rf_core::{Entry, Result, Scheme, VPath};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LIST_BATCH_SIZE, LinkTarget,
    LocalProvider, Provider, ReadStream, RenameMode, WatchHandle, WatchSink, WriteMode,
    WriteStream, testkit,
};

fn run(check: fn(&dyn Provider, &VPath)) {
    let t = tempfile::tempdir().unwrap();
    check(&LocalProvider::new(), &VPath::local(t.path()).unwrap());
}

#[test]
fn create_and_stat() {
    run(testkit::check_create_and_stat);
}

#[test]
fn list_batches_are_full_except_the_last() {
    let t = tempfile::tempdir().unwrap();
    let sizes =
        testkit::check_list_batches(&LocalProvider::new(), &VPath::local(t.path()).unwrap());
    assert_eq!(sizes, [LIST_BATCH_SIZE, LIST_BATCH_SIZE, 503]);
}

#[test]
fn list_cancel() {
    run(testkit::check_list_cancel);
}

#[test]
fn rename() {
    run(testkit::check_rename);
}

#[test]
fn read_write() {
    run(testkit::check_read_write);
}

#[test]
fn delete_permanent() {
    run(testkit::check_delete_permanent);
}

#[test]
fn delete_tree_keeps_link_target() {
    run(testkit::check_delete_tree_keeps_link_target);
}

#[test]
fn rename_symlink_guard() {
    run(testkit::check_rename_symlink_guard);
}

#[test]
fn unicode_names() {
    run(testkit::check_unicode_names);
}

#[test]
fn long_paths() {
    run(testkit::check_long_paths);
}

#[test]
fn delete_leaf() {
    run(testkit::check_delete_leaf);
}

/// The local provider for everything a provider must implement, and the default
/// implementation of every method that has one, except the links the checks make.
#[derive(Debug)]
struct Defaults(LocalProvider);

impl Provider for Defaults {
    fn scheme(&self) -> Scheme {
        self.0.scheme()
    }
    fn capabilities(&self, path: &VPath) -> Capabilities {
        self.0.capabilities(path)
    }
    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()> {
        self.0.list(dir, cancel, sink)
    }
    fn stat(&self, path: &VPath) -> Result<Entry> {
        self.0.stat(path)
    }
    fn create_dir(&self, path: &VPath) -> Result<()> {
        self.0.create_dir(path)
    }
    fn create_file(&self, path: &VPath) -> Result<()> {
        self.0.create_file(path)
    }
    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
        self.0.rename(from, to, mode)
    }
    fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted> {
        self.0.delete(path, mode)
    }
    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
        self.0.open_read(path)
    }
    fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
        self.0.open_write(path, mode)
    }
    fn space(&self, path: &VPath) -> Result<Option<DiskSpace>> {
        self.0.space(path)
    }
    fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
        self.0.watch(dir, sink)
    }
    fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> Result<()> {
        self.0.create_symlink(link, target)
    }
}

#[test]
fn the_default_delete_leaf_never_removes_a_folder() {
    let t = tempfile::tempdir().unwrap();
    testkit::check_delete_leaf(
        &Defaults(LocalProvider::new()),
        &VPath::local(t.path()).unwrap(),
    );
}
