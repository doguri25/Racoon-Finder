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

#[cfg(target_os = "linux")]
#[test]
fn linux_has_no_trash_and_keeps_the_file() {
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
