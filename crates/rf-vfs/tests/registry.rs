use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rf_core::{Entry, Error, ErrorKind, Result, Scheme, VPath};
use rf_vfs::{
    CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, Provider, ReadStream, RenameMode,
    Vfs, WatchHandle, WatchSink, WriteMode, WriteStream,
};

#[derive(Debug)]
struct Fake(Scheme);

fn no<T>() -> Result<T> {
    Err(Error::Unsupported("fake".into()))
}

impl Provider for Fake {
    fn scheme(&self) -> Scheme {
        self.0
    }
    fn capabilities(&self, _: &VPath) -> Capabilities {
        Capabilities::empty()
    }
    fn list(&self, _: &VPath, _: &CancelToken, _: &mut dyn FnMut(Vec<Entry>)) -> Result<()> {
        no()
    }
    fn stat(&self, _: &VPath) -> Result<Entry> {
        no()
    }
    fn create_dir(&self, _: &VPath) -> Result<()> {
        no()
    }
    fn create_file(&self, _: &VPath) -> Result<()> {
        no()
    }
    fn rename(&self, _: &VPath, _: &VPath, _: RenameMode) -> Result<()> {
        no()
    }
    fn delete(&self, _: &VPath, _: DeleteMode) -> Result<Deleted> {
        no()
    }
    fn open_read(&self, _: &VPath) -> Result<Box<dyn ReadStream>> {
        no()
    }
    fn open_write(&self, _: &VPath, _: WriteMode) -> Result<Box<dyn WriteStream>> {
        no()
    }
    fn space(&self, _: &VPath) -> Result<Option<DiskSpace>> {
        Ok(None)
    }
    fn watch(&self, _: &VPath, _: WatchSink) -> Result<WatchHandle> {
        no()
    }
}

#[test]
fn cancel_token_is_shared_between_clones() {
    let a = CancelToken::new();
    let b = a.clone();
    assert!(!b.is_cancelled());
    a.cancel();
    assert!(b.is_cancelled());
}

#[test]
fn check_reports_cancelled() {
    let t = CancelToken::default();
    assert!(t.check().is_ok());
    t.cancel();
    assert_eq!(t.check().unwrap_err().kind(), ErrorKind::Cancelled);
}

#[test]
fn registry_routes_by_scheme() {
    let mut vfs = Vfs::new();
    assert!(vfs.register(Arc::new(Fake(Scheme::Remote))).is_none());
    let p = VPath::new(Scheme::Remote, "conn", &["a"]).unwrap();
    assert_eq!(vfs.provider(&p).unwrap().scheme(), Scheme::Remote);
}

#[test]
fn registry_without_provider_is_unsupported() {
    let p = VPath::new(Scheme::Archive, "x", &[]).unwrap();
    assert_eq!(
        Vfs::new().provider(&p).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}

#[test]
fn register_replaces_same_scheme() {
    let mut vfs = Vfs::new();
    vfs.register(Arc::new(Fake(Scheme::Remote)));
    assert!(vfs.register(Arc::new(Fake(Scheme::Remote))).is_some());
}

#[test]
fn capability_bits_are_stable() {
    assert_eq!(Capabilities::WRITE.bits(), 1);
    assert_eq!(Capabilities::TRASH.bits(), 2);
    assert_eq!(Capabilities::WATCH.bits(), 4);
}

#[test]
fn watch_handle_drops_its_guard() {
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let flag = Arc::new(AtomicBool::new(false));
    let h = WatchHandle::new(Guard(Arc::clone(&flag)));
    assert!(!flag.load(Ordering::SeqCst));
    drop(h);
    assert!(flag.load(Ordering::SeqCst));
}

#[test]
fn new_methods_have_safe_defaults() {
    let f = Fake(Scheme::Remote);
    let p = VPath::new(Scheme::Remote, "c", &["x"]).unwrap();
    let link = rf_vfs::LinkTarget {
        target: "t".into(),
        is_dir: false,
    };
    assert_eq!(
        f.stat_target(&p).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(f.read_link(&p).unwrap_err().kind(), ErrorKind::Unsupported);
    assert_eq!(
        f.create_symlink(&p, &link).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        f.set_metadata(&p, &rf_vfs::MetadataPatch::default())
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(f.remove_dir(&p).unwrap_err().kind(), ErrorKind::Unsupported);
    assert_eq!(
        f.restore_trashed(&p, &p).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert!(f.copy_extras(&p, &p).is_ok());
    assert_eq!(f.fs_name(&p), None);
}

#[test]
fn disk_space_is_exported_from_provider_and_root() {
    use rf_vfs::provider::DiskSpace as FromProvider;

    let space: FromProvider = DiskSpace {
        total: 3,
        free: 2,
        available: 1,
    };
    assert_eq!(space.total, 3);
}

#[test]
fn data_ranges_default_is_none() {
    let p = VPath::new(Scheme::Remote, "c", &["x"]).unwrap();
    assert_eq!(Fake(Scheme::Remote).data_ranges(&p).unwrap(), None);
}
