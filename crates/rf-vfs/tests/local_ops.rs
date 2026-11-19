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

/// Whether the volume of the folder `root` is APFS that ignores both letter case and the
/// Unicode form of a name (NFC or NFD): macOS's usual system volume. Each is tried on a file
/// made for it.
#[cfg(target_os = "macos")]
fn case_and_form_blind_apfs(root: &VPath) -> bool {
    let apfs = rf_platform::volume::volume_info(root)
        .ok()
        .and_then(|info| info.fs_name)
        .is_some_and(|name| name.eq_ignore_ascii_case("apfs"));
    let dir = root.as_local().unwrap();
    // Made under one spelling, looked for under the other.
    let finds = |made: &str, other: &str| {
        std::fs::write(dir.join(made), b"").unwrap();
        let found = std::fs::metadata(dir.join(other)).is_ok();
        std::fs::remove_file(dir.join(made)).unwrap();
        found
    };
    apfs && finds("probe.txt", "PROBE.TXT")
        && finds("\u{D55C}.probe", "\u{1112}\u{1161}\u{11ab}.probe")
}

/// On APFS the two Unicode forms of a name (NFC and NFD) are one name, as `a.txt` and `A.TXT`
/// are on its usual case-insensitive volumes: a rename that changes only the form is a rename
/// of the item itself, not onto another, and the folder lists the new spelling. Only there: on
/// case-sensitive APFS such a rename finds one item under both names and leaves its name as it
/// was, and HFS+ stores every name decomposed (NFD), so a folder never lists the composed one.
#[cfg(target_os = "macos")]
#[test]
fn a_rename_that_changes_only_the_unicode_form_renames_the_item() {
    let (_t, root) = tmp();
    if !case_and_form_blind_apfs(&root) {
        eprintln!("skipped: not APFS that ignores both letter case and the Unicode form");
        return;
    }
    let p = LocalProvider::new();
    let nfc = root.join("한글.txt").unwrap();
    let nfd = root
        .join("\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}.txt")
        .unwrap();
    std::fs::write(nfc.as_local().unwrap(), b"one").unwrap();
    // The folder lists the item under the spelling it was renamed to, each way round.
    let listed = || -> Vec<std::ffi::OsString> {
        std::fs::read_dir(root.as_local().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    };
    for (from, to) in [(&nfc, &nfd), (&nfd, &nfc)] {
        p.rename(from, to, RenameMode::NoReplace).unwrap();
        assert_eq!(listed(), [to.file_name().unwrap()], "{from} -> {to}");
        assert_eq!(std::fs::read(to.as_local().unwrap()).unwrap(), b"one");
    }
}

/// Makes `link` lead to the folder `target`: a symbolic link on Unix.
#[cfg(unix)]
fn dir_alias(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Makes `link` lead to the folder `target`: a junction on Windows, which unlike a directory
/// symbolic link needs no privilege.
#[cfg(windows)]
fn dir_alias(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("mklink /J failed: {status}")))
    }
}

/// Asserts that renaming `from` to `to` is refused as a move into itself in both modes, and
/// that `from` still holds `keep.txt` and nothing else.
fn assert_refused_as_into_itself(p: LocalProvider, from: &VPath, to: &VPath) {
    for mode in [RenameMode::NoReplace, RenameMode::Replace] {
        let r = p.rename(from, to, mode);
        assert_eq!(
            r.as_ref().map_err(rf_core::Error::kind),
            Err(ErrorKind::Unsupported),
            "{mode:?}: {r:?}"
        );
        let keep = from.join("keep.txt").unwrap();
        assert_eq!(std::fs::read(keep.as_local().unwrap()).unwrap(), b"keep");
        assert_eq!(
            rf_vfs::testkit::names(&p, from),
            std::collections::BTreeSet::from(["keep.txt".into()])
        );
    }
}

/// A folder reached through a link (a junction on Windows) is still the folder itself, so
/// moving it below that link is a move into itself (DND-09), refused before the OS sees it.
#[test]
fn a_folder_never_moves_into_itself_through_a_link_to_it() {
    let (t, root) = tmp();
    let p = LocalProvider::new();
    let from = root.join("from").unwrap();
    p.create_dir(&from).unwrap();
    std::fs::write(t.path().join("from").join("keep.txt"), b"keep").unwrap();
    if let Err(e) = dir_alias(&t.path().join("from"), &t.path().join("alias")) {
        eprintln!("skipped: cannot make a folder link here: {e}");
        return;
    }
    let below_alias = root.join("alias").unwrap().join("sub").unwrap();
    assert_refused_as_into_itself(p, &from, &below_alias);
    // Deeper below the link, under folders that do not exist yet, is inside the folder too.
    let deeper = root.join("alias").unwrap().join("x").unwrap();
    assert_refused_as_into_itself(p, &from, &deeper.join("y").unwrap());
}

/// A link to another folder is no reason to refuse: the move goes through.
#[test]
fn a_folder_moves_through_a_link_to_another_folder() {
    let (t, root) = tmp();
    let p = LocalProvider::new();
    let (from, other) = (root.join("from").unwrap(), root.join("other").unwrap());
    p.create_dir(&from).unwrap();
    p.create_dir(&other).unwrap();
    if let Err(e) = dir_alias(&t.path().join("other"), &t.path().join("alias")) {
        eprintln!("skipped: cannot make a folder link here: {e}");
        return;
    }
    let to = root.join("alias").unwrap().join("moved").unwrap();
    p.rename(&from, &to, RenameMode::NoReplace).unwrap();
    assert!(t.path().join("other").join("moved").is_dir());
    assert_eq!(p.stat(&from).unwrap_err().kind(), ErrorKind::NotFound);
}

/// On a volume that ignores case (NTFS, default APFS) `a` is the folder `A`, so moving `A` to
/// `a/sub` is a move into itself although the text of the paths differs.
#[test]
fn a_folder_never_moves_into_itself_under_another_case() {
    let (t, root) = tmp();
    let p = LocalProvider::new();
    let (upper, lower) = (root.join("A").unwrap(), root.join("a").unwrap());
    p.create_dir(&upper).unwrap();
    std::fs::write(t.path().join("A").join("keep.txt"), b"keep").unwrap();
    if p.stat(&lower).is_err() {
        eprintln!("skipped: this volume tells `A` and `a` apart");
        return;
    }
    assert_refused_as_into_itself(p, &upper, &lower.join("sub").unwrap());
}
