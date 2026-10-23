use rf_core::{ErrorKind, Scheme, VPath};
use rf_platform::{KnownFolder, disk_space, known_folder, volume_info};

fn tmp() -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path()).unwrap();
    (t, v)
}

#[test]
fn home_is_known_and_absolute() {
    let home = known_folder(KnownFolder::Home).expect("home folder");
    let p = home.as_local().unwrap();
    assert!(p.is_absolute() && p.is_dir(), "{}", p.display());
}

#[cfg(windows)]
#[test]
fn windows_known_folders_resolve() {
    for f in [
        KnownFolder::Desktop,
        KnownFolder::Documents,
        KnownFolder::Downloads,
    ] {
        assert!(known_folder(f).is_some(), "{f:?}");
    }
    let found: Vec<VPath> = KnownFolder::ALL
        .iter()
        .filter_map(|&f| known_folder(f))
        .collect();
    assert_eq!(
        found
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        found.len()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_known_folders_are_home_subfolders() {
    let home = known_folder(KnownFolder::Home).unwrap();
    let cases = [
        (KnownFolder::Desktop, "Desktop"),
        (KnownFolder::Documents, "Documents"),
        (KnownFolder::Downloads, "Downloads"),
        (KnownFolder::Pictures, "Pictures"),
        (KnownFolder::Music, "Music"),
        (KnownFolder::Videos, "Movies"),
    ];
    for (f, sub) in cases {
        assert_eq!(known_folder(f), Some(home.join(sub).unwrap()), "{f:?}");
    }
}

#[test]
fn disk_space_is_consistent() {
    let (_t, v) = tmp();
    let s = disk_space(&v).unwrap();
    assert!(
        s.total > 0 && s.free <= s.total && s.available <= s.free,
        "{s:?}"
    );
}

#[test]
fn disk_space_of_file_matches_its_dir() {
    let (t, v) = tmp();
    std::fs::write(t.path().join("f"), b"").unwrap();
    assert_eq!(
        disk_space(&v.join("f").unwrap()).unwrap().total,
        disk_space(&v).unwrap().total
    );
}

#[test]
fn disk_space_of_missing_path_is_not_found() {
    let (_t, v) = tmp();
    assert_eq!(
        disk_space(&v.join("missing").unwrap()).unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn volume_info_of_temp_dir() {
    let (_t, v) = tmp();
    let info = volume_info(&v).unwrap();
    assert!(!info.read_only);
    let named = info.fs_name.as_deref().is_some_and(|n| !n.is_empty());
    assert_eq!(named, cfg!(any(windows, target_os = "macos")), "{info:?}");
}

#[test]
fn non_local_paths_are_unsupported() {
    let remote = VPath::new(Scheme::Remote, "p", &[]).unwrap();
    assert_eq!(
        disk_space(&remote).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        volume_info(&remote).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}
