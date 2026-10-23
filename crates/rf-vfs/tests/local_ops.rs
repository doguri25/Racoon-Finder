#[cfg(unix)]
use std::collections::BTreeMap;

use rf_core::identity::TEMP_PREFIX;
use rf_core::{EntryKind, ErrorKind, Scheme, VPath};
use rf_vfs::{CancelToken, Capabilities, DeleteMode, LocalProvider, Provider, RenameMode, Vfs};

fn tmp() -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path()).unwrap();
    (t, v)
}

/// Removes a trashed test item from the real trash (and its `$I` record on Windows).
#[cfg(any(windows, target_os = "macos"))]
fn purge(location: &std::path::Path) {
    if location.is_dir() {
        std::fs::remove_dir_all(location).unwrap();
    } else {
        std::fs::remove_file(location).unwrap();
    }
    #[cfg(windows)]
    {
        let name = location.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix("$R") {
            let _ = std::fs::remove_file(location.with_file_name(format!("$I{rest}")));
        }
    }
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

#[test]
fn trash_delete_follows_platform_support() {
    let (_t, root) = tmp();
    let p = LocalProvider::new();
    let f = root
        .join(format!("{TEMP_PREFIX}vfs-trash-{}.txt", std::process::id()))
        .unwrap();
    p.create_file(&f).unwrap();
    let r = p.delete(&f, DeleteMode::Trash);
    #[cfg(any(windows, target_os = "macos"))]
    {
        let rf_vfs::Deleted::Trashed { location } = r.unwrap() else {
            panic!("not trashed")
        };
        assert_eq!(p.stat(&f).unwrap_err().kind(), ErrorKind::NotFound);
        purge(location.expect("trash location").as_local().unwrap());
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        assert_eq!(r.unwrap_err().kind(), ErrorKind::Unsupported);
        assert!(p.stat(&f).is_ok());
    }
}

#[test]
fn permanent_delete_of_dir_symlink_keeps_target() {
    let (t, root) = tmp();
    let (target, link) = (t.path().join("target"), t.path().join("link"));
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep.txt"), b"keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    if let Err(e) = std::os::windows::fs::symlink_dir(&target, &link) {
        eprintln!("skipped: cannot create symlinks here: {e}");
        return;
    }
    let p = LocalProvider::new();
    let lv = root.join("link").unwrap();
    assert_eq!(p.stat(&lv).unwrap().kind, EntryKind::Symlink);
    p.delete(&lv, DeleteMode::Permanent).unwrap();
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert_eq!(std::fs::read(target.join("keep.txt")).unwrap(), b"keep");
}

#[test]
fn names_differing_only_in_case_never_overwrite_each_other() {
    let (_t, root) = tmp();
    let p = LocalProvider::new();
    let (lower, upper) = (root.join("a.txt").unwrap(), root.join("A.TXT").unwrap());
    p.create_file(&lower).unwrap();
    if p.create_file(&upper).is_err() {
        eprintln!("skipped: this volume ignores case, so the two names are one item");
        return;
    }
    // Two items whose names match ignoring case are not a case-only rename of one item.
    std::fs::write(lower.as_local().unwrap(), b"lower").unwrap();
    std::fs::write(upper.as_local().unwrap(), b"upper").unwrap();
    let r = p.rename(&lower, &upper, RenameMode::NoReplace);
    assert_eq!(r.unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(lower.as_local().unwrap()).unwrap(), b"lower");
    assert_eq!(std::fs::read(upper.as_local().unwrap()).unwrap(), b"upper");
}
