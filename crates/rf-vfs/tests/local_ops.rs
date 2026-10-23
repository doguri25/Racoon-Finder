#[cfg(unix)]
use std::collections::BTreeMap;

#[cfg(unix)]
use rf_core::EntryKind;
use rf_core::{ErrorKind, Scheme, VPath};
use rf_vfs::{CancelToken, Capabilities, LocalProvider, Provider, Vfs};

fn tmp() -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path()).unwrap();
    (t, v)
}

#[test]
fn capabilities_of_temp_dir() {
    let (_t, root) = tmp();
    let caps = LocalProvider::new().capabilities(&root);
    assert!(
        caps.contains(Capabilities::WRITE | Capabilities::WATCH),
        "{caps:?}"
    );
    assert_eq!(
        caps.contains(Capabilities::TRASH),
        cfg!(any(windows, target_os = "macos"))
    );
}

#[test]
fn space_of_temp_dir() {
    let (_t, root) = tmp();
    let s = LocalProvider::new()
        .space(&root)
        .unwrap()
        .expect("local space");
    assert!(s.total > 0 && s.available <= s.total, "{s:?}");
}

#[test]
fn stat_reports_identity() {
    let (_t, root) = tmp();
    let p = LocalProvider::new();
    let f = root.join("f").unwrap();
    p.create_file(&f).unwrap();
    assert_eq!(
        p.stat(&f).unwrap().identity(),
        Some(rf_platform::file_identity(&f).unwrap())
    );
}

#[cfg(unix)]
#[test]
fn list_reports_identity_on_unix() {
    let (_t, root) = tmp();
    let p = LocalProvider::new();
    p.create_file(&root.join("a").unwrap()).unwrap();
    p.create_dir(&root.join("b").unwrap()).unwrap();
    p.list(&root, &CancelToken::new(), &mut |batch| {
        for e in batch {
            let path = root.join(&e.name).unwrap();
            assert_eq!(
                e.identity(),
                Some(rf_platform::file_identity(&path).unwrap())
            );
        }
    })
    .unwrap();
}

#[cfg(unix)]
#[test]
fn sockets_are_other_and_links_are_not_followed() {
    let (t, root) = tmp();
    let p = LocalProvider::new();
    let _socket = std::os::unix::net::UnixListener::bind(t.path().join("sock")).unwrap();
    std::os::unix::fs::symlink(t.path().join("nowhere"), t.path().join("dangling")).unwrap();
    for (name, kind) in [("sock", EntryKind::Other), ("dangling", EntryKind::Symlink)] {
        assert_eq!(p.stat(&root.join(name).unwrap()).unwrap().kind, kind);
    }
    let mut listed = BTreeMap::new();
    p.list(&root, &CancelToken::new(), &mut |batch| {
        listed.extend(batch.into_iter().map(|e| (e.name, e.kind)));
    })
    .unwrap();
    assert_eq!(
        listed,
        BTreeMap::from([
            ("dangling".into(), EntryKind::Symlink),
            ("sock".into(), EntryKind::Other)
        ])
    );
}

#[test]
fn non_local_paths_are_unsupported() {
    let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    let p = LocalProvider::new();
    assert_eq!(p.stat(&remote).unwrap_err().kind(), ErrorKind::Unsupported);
    assert_eq!(
        p.list(&remote, &CancelToken::new(), &mut |_| {})
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        p.create_dir(&remote).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(p.capabilities(&remote), Capabilities::empty());
}

#[test]
fn vfs_with_local_routes_local_paths() {
    let (_t, root) = tmp();
    let vfs = Vfs::with_local();
    assert_eq!(vfs.provider(&root).unwrap().scheme(), Scheme::Local);
    let remote = VPath::new(Scheme::Remote, "p", &[]).unwrap();
    assert_eq!(
        vfs.provider(&remote).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}
