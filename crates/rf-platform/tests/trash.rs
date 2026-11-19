use std::path::{Path, PathBuf};

use rf_core::identity::TEMP_PREFIX;
use rf_core::{ErrorKind, Scheme, VPath};
use rf_platform::{move_to_trash, trash_supported};

fn unique(dir: &Path, suffix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    dir.join(format!(
        "{TEMP_PREFIX}trash-{}-{nanos}{suffix}",
        std::process::id()
    ))
}

/// The path to give file APIs: the `\\?\` form on Windows, which reaches beyond `MAX_PATH`.
#[cfg(any(windows, target_os = "macos"))]
fn extended(path: &Path) -> PathBuf {
    VPath::local(path).unwrap().to_extended_length().unwrap()
}

/// Removes a trashed test item from the real trash (and its `$I` record on Windows).
#[cfg(any(windows, target_os = "macos"))]
fn purge(location: &Path) {
    let item = extended(location);
    if item.is_dir() {
        std::fs::remove_dir_all(&item).unwrap();
    } else {
        std::fs::remove_file(&item).unwrap();
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
fn missing_path_is_not_found() {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path().join("missing.txt")).unwrap();
    assert_eq!(move_to_trash(&v).unwrap_err().kind(), ErrorKind::NotFound);
}

#[test]
fn non_local_path_is_unsupported() {
    let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert!(!trash_supported(&remote));
    assert_eq!(
        move_to_trash(&remote).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn trashes_a_file_and_reports_its_location() {
    let t = tempfile::tempdir().unwrap();
    let f = unique(t.path(), ".txt");
    std::fs::write(&f, b"trash me").unwrap();
    let v = VPath::local(&f).unwrap();
    assert!(trash_supported(&v));
    let location = move_to_trash(&v).unwrap().expect("trash location");
    assert!(!f.exists());
    assert!(location.exists(), "{}", location.display());
    purge(&location);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn trashes_a_directory_tree() {
    let t = tempfile::tempdir().unwrap();
    let d = unique(t.path(), "");
    std::fs::create_dir_all(d.join("inner")).unwrap();
    std::fs::write(d.join("inner").join("f.txt"), b"x").unwrap();
    let location = move_to_trash(&VPath::local(&d).unwrap())
        .unwrap()
        .expect("trash location");
    assert!(!d.exists());
    assert!(location.join("inner").join("f.txt").exists());
    purge(&location);
}

#[cfg(windows)]
#[test]
fn long_path_is_never_deleted_permanently() {
    let t = tempfile::tempdir().unwrap();
    let mut dir = t.path().to_path_buf();
    for i in 0..4 {
        dir.push(format!("{i}-{}", "x".repeat(80)));
    }
    std::fs::create_dir_all(&dir).unwrap();
    let f = unique(&dir, ".txt");
    std::fs::write(&f, b"keep").unwrap();
    assert!(f.as_os_str().len() > 300);
    match move_to_trash(&VPath::local(&f).unwrap()) {
        Ok(Some(location)) => {
            assert!(!f.exists());
            purge(&location);
        }
        Ok(None) => panic!("trashed without a recycle bin entry"),
        // Usually `Unsupported` (the recycle bin refused the item), but the kind is not
        // asserted: if the shell cannot even parse a path this long, the error is the I/O
        // error of `SHCreateItemFromParsingName`. Either way nothing may be gone.
        Err(e) => assert!(f.exists(), "file vanished although trashing failed: {e}"),
    }
}

/// A folder whose own path is short but which holds a file beyond `MAX_PATH`: the recycle bin
/// may take the whole tree or refuse it, but no part of it may be deleted for good.
#[cfg(windows)]
#[test]
fn long_descendant_folder_is_never_deleted_permanently() {
    let t = tempfile::tempdir().unwrap();
    let top = unique(t.path(), "");
    let mut deep = top.clone();
    for i in 0..4 {
        deep.push(format!("{i}-{}", "x".repeat(80)));
    }
    let deep_file = deep.strip_prefix(&top).unwrap().join("deep.txt");
    // Plain paths stop at `MAX_PATH`; the deep part is made through the `\\?\` form.
    std::fs::create_dir_all(extended(&deep)).unwrap();
    std::fs::write(extended(&top.join(&deep_file)), b"deep").unwrap();
    std::fs::write(top.join("shallow.txt"), b"shallow").unwrap();
    assert!(top.as_os_str().len() < 200, "{}", top.display());
    assert!(top.join(&deep_file).as_os_str().len() > 300);
    // Whether `root` holds both files with their bytes.
    let tree_intact = |root: &Path| {
        std::fs::read(extended(&root.join("shallow.txt"))).is_ok_and(|b| b == b"shallow")
            && std::fs::read(extended(&root.join(&deep_file))).is_ok_and(|b| b == b"deep")
    };

    let outcome = move_to_trash(&VPath::local(&top).unwrap());

    match outcome {
        Ok(Some(location)) => {
            let top_gone = std::fs::symlink_metadata(extended(&top))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
            let whole_tree_in_bin = tree_intact(&location);
            let in_bin = in_trash(&location);
            // Clean the trash before asserting, so a failed assertion leaves nothing behind.
            purge(&location);
            assert!(whole_tree_in_bin, "the bin entry lacks part of the tree");
            assert!(in_bin, "{} is not inside the trash", location.display());
            assert!(
                top_gone,
                "the bin took a copy, but the folder is still there"
            );
        }
        Ok(None) => panic!("trashed without a recycle bin entry"),
        Err(e) => {
            let intact = tree_intact(&top);
            let _ = std::fs::remove_dir_all(extended(&top));
            assert!(
                intact,
                "part of the tree vanished although trashing failed: {e}"
            );
            assert_eq!(e.kind(), ErrorKind::Unsupported, "{e}");
        }
    }
}

/// On Linux a temporary folder has no trash, so no test ever fills the user's home trash; the
/// freedesktop trash itself is tested in a temporary folder of its own (`FreedesktopTrash`).
#[cfg(target_os = "linux")]
#[test]
fn a_temporary_folder_has_no_trash_on_linux_and_keeps_the_file() {
    let t = tempfile::tempdir().unwrap();
    let f = unique(t.path(), ".txt");
    std::fs::write(&f, b"keep").unwrap();
    let v = VPath::local(&f).unwrap();
    assert!(!trash_supported(&v));
    assert_eq!(
        move_to_trash(&v).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    assert!(f.exists());
}

/// Makes `link` a directory link to `target`.
#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

/// Makes `link` a directory link to `target`.
#[cfg(target_os = "macos")]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Whether `location` lies inside a recycle bin or trash folder.
#[cfg(any(windows, target_os = "macos"))]
fn in_trash(location: &Path) -> bool {
    location.components().any(|part| {
        let part = part.as_os_str().to_string_lossy().to_lowercase();
        matches!(part.as_str(), ".trash" | ".trashes" | "$recycle.bin")
    })
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn trashes_a_symlink_not_its_target() {
    let t = tempfile::tempdir().unwrap();
    let target = unique(t.path(), "-target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep.txt"), b"keep").unwrap();
    let link = unique(t.path(), "-link");
    if let Err(e) = link_dir(&target, &link) {
        // Windows lets only an administrator or a developer-mode user make symbolic links.
        assert!(
            cfg!(windows)
                && (e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(1314)),
            "cannot create the link: {e}"
        );
        eprintln!("skipped: no right to create symbolic links ({e})");
        return;
    }
    let outcome = move_to_trash(&VPath::local(&link).unwrap());
    let link_now = std::fs::symlink_metadata(&link);
    let link_gone = link_now
        .as_ref()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    let link_present = link_now.is_ok();
    let target_intact = std::fs::read(target.join("keep.txt")).is_ok_and(|bytes| bytes == b"keep");
    let location = outcome.as_ref().ok().cloned().flatten();
    let in_bin = location.as_deref().is_none_or(in_trash);
    // Clean the trash before asserting, so a failed assertion leaves nothing behind.
    if let Some(location) = &location {
        purge(location);
    }
    // Data first, so that a failure shows whether anything was lost; the outcome comes last.
    assert!(
        target_intact,
        "the link target lost its content: {outcome:?}"
    );
    match &outcome {
        Ok(_) => {
            assert!(link_gone, "trashing succeeded, but the link is still there");
            assert!(in_bin, "{location:?} is not inside the trash");
        }
        Err(e) => assert!(
            link_present,
            "the link vanished although trashing failed: {e}"
        ),
    }
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[cfg(windows)]
#[test]
fn trailing_dot_name_never_trashes_its_sibling() {
    let t = tempfile::tempdir().unwrap();
    let plain = unique(t.path(), "");
    let mut dotted = plain.clone().into_os_string();
    dotted.push(".");
    let dotted = VPath::local(PathBuf::from(dotted)).unwrap();
    let dotted_ext = dotted.to_extended_length().unwrap();
    std::fs::write(&plain, b"plain").unwrap();
    std::fs::write(&dotted_ext, b"dotted").unwrap();

    let outcome = move_to_trash(&dotted);

    assert_eq!(std::fs::read(&plain).unwrap(), b"plain", "{outcome:?}");
    let dotted_gone = std::fs::symlink_metadata(&dotted_ext)
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    match outcome {
        Err(e) => {
            assert_eq!(e.kind(), ErrorKind::Unsupported, "{e}");
            assert!(!dotted_gone, "the file vanished although trashing failed");
            std::fs::remove_file(&dotted_ext).unwrap();
        }
        // The shell kept the dot after all: only `x.` may have been recycled.
        Ok(Some(location)) => {
            assert!(dotted_gone);
            purge(&location);
        }
        Ok(None) => panic!("trashed without a recycle bin entry"),
    }
}

/// An item that is not in a trash folder is never deleted or moved, whatever the item says.
#[test]
fn an_item_outside_the_trash_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let f = unique(t.path(), ".txt");
    std::fs::write(&f, b"keep").unwrap();
    let mut item = rf_platform::TrashItem::at(f.clone(), false);
    item.original = Some(t.path().join("back.txt"));
    let back = t.path().join("back.txt");
    for result in [
        rf_platform::trash::delete_forever(&item),
        rf_platform::trash::restore(&item, &back),
    ] {
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Unsupported);
    }
    assert_eq!(std::fs::read(&f).unwrap(), b"keep");
    assert!(!back.exists());
    // The trash folder itself is no item of it either.
    let folder = rf_platform::TrashItem::at(t.path().to_path_buf(), true);
    assert_eq!(
        rf_platform::trash::delete_forever(&folder)
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
    assert!(f.exists());
}

/// The item a test trashed, as the trash lists it; `None` when the trash cannot be listed here
/// (macOS lets only an app with Full Disk Access read `~/.Trash`).
#[cfg(any(windows, target_os = "macos"))]
#[allow(clippy::print_stderr, reason = "says why the test is skipped")]
fn listed(location: &Path) -> Option<rf_platform::TrashItem> {
    match rf_platform::trash::list() {
        Ok(items) => {
            let wanted = std::fs::canonicalize(location).unwrap();
            let found = items
                .into_iter()
                .find(|item| std::fs::canonicalize(&item.location).ok().as_ref() == Some(&wanted));
            Some(found.expect("the trashed item is listed"))
        }
        Err(e) if cfg!(target_os = "macos") && e.kind() == ErrorKind::PermissionDenied => {
            eprintln!("skipped: the trash cannot be listed here ({e})");
            None
        }
        Err(e) => panic!("the trash cannot be listed: {e}"),
    }
}

/// The `$I` record next to the `$R` item at `location` (Windows).
#[cfg(windows)]
fn record_of(location: &Path) -> PathBuf {
    let name = location.file_name().unwrap().to_string_lossy().into_owned();
    location.with_file_name(format!("$I{}", name.strip_prefix("$R").unwrap()))
}

/// A test's own item in the real trash, at the place the system reported for it. When the test
/// ends, however it ends, the item is removed from the trash, but only while that place still
/// holds the test's uniquely named item: on Windows the `$I` record names it as the original, on
/// macOS the place has its name.
#[cfg(any(windows, target_os = "macos"))]
struct Trashed {
    location: PathBuf,
    name: std::ffi::OsString,
}

#[cfg(any(windows, target_os = "macos"))]
impl Trashed {
    fn new(location: &Path, original: &Path) -> Trashed {
        Trashed {
            location: location.to_path_buf(),
            name: original.file_name().unwrap().to_owned(),
        }
    }

    /// Whether the place still holds the test's own item.
    fn is_ours(&self) -> bool {
        #[cfg(windows)]
        let ours = std::fs::read(record_of(&self.location))
            .ok()
            .and_then(|bytes| rf_platform::parse_recycle_index(&bytes))
            .is_some_and(|(original, _, _)| original.file_name() == Some(self.name.as_os_str()));
        #[cfg(target_os = "macos")]
        let ours = self.location.file_name() == Some(self.name.as_os_str());
        ours && std::fs::symlink_metadata(extended(&self.location)).is_ok()
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Drop for Trashed {
    fn drop(&mut self) {
        if !self.is_ours() {
            return;
        }
        let item = extended(&self.location);
        let _ = if item.is_dir() {
            std::fs::remove_dir_all(&item)
        } else {
            std::fs::remove_file(&item)
        };
        #[cfg(windows)]
        let _ = std::fs::remove_file(record_of(&self.location));
    }
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_trashed_file_is_listed_and_deleted_for_good() {
    let t = tempfile::tempdir().unwrap();
    let f = unique(t.path(), ".txt");
    std::fs::write(&f, b"trash me").unwrap();
    let before = std::time::SystemTime::now();
    let location = move_to_trash(&VPath::local(&f).unwrap())
        .unwrap()
        .expect("trash location");
    let _cleanup = Trashed::new(&location, &f);
    let Some(item) = listed(&location) else {
        return;
    };
    assert_eq!(item.name.as_os_str(), f.file_name().unwrap());
    assert!(!item.is_dir);
    assert_eq!(item.size, Some(8));
    if cfg!(windows) {
        let original = item
            .original
            .clone()
            .expect("the bin keeps the original place");
        assert_eq!(
            std::fs::canonicalize(original.parent().unwrap()).unwrap(),
            std::fs::canonicalize(t.path()).unwrap()
        );
        let deleted = item.deleted.expect("the bin keeps the time");
        let floor = before - std::time::Duration::from_secs(2);
        assert!(deleted >= floor, "{deleted:?}");
    } else {
        assert_eq!(item.original, None);
    }
    rf_platform::trash::delete_forever(&item).unwrap();
    assert!(std::fs::symlink_metadata(&location).is_err());
    #[cfg(windows)]
    assert!(!record_of(&location).exists());
    assert!(!f.exists());
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_trashed_folder_is_put_back_and_never_over_another_item() {
    let t = tempfile::tempdir().unwrap();
    let d = unique(t.path(), "");
    std::fs::create_dir_all(d.join("inner")).unwrap();
    std::fs::write(d.join("inner").join("f.txt"), b"x").unwrap();
    let location = move_to_trash(&VPath::local(&d).unwrap())
        .unwrap()
        .expect("trash location");
    let _cleanup = Trashed::new(&location, &d);
    let Some(item) = listed(&location) else {
        return;
    };
    // Only the test's own item is put back.
    assert_eq!(item.name.as_os_str(), d.file_name().unwrap());
    assert!(item.is_dir);
    // A new item in its place stays; the folder stays in the trash.
    std::fs::write(&d, b"new").unwrap();
    let clash = rf_platform::trash::restore(&item, &d);
    let kind = clash.as_ref().map_err(rf_core::Error::kind).err();
    std::fs::remove_file(&d).unwrap();
    assert_eq!(
        kind,
        Some(ErrorKind::AlreadyExists),
        "a taken place is a conflict: {clash:?}"
    );
    rf_platform::trash::restore(&item, &d).unwrap();
    assert_eq!(std::fs::read(d.join("inner").join("f.txt")).unwrap(), b"x");
    assert!(std::fs::symlink_metadata(&location).is_err());
    #[cfg(windows)]
    assert!(!record_of(&location).exists());
}
