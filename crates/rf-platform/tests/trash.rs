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

/// Removes a trashed test item from the real trash (and its `$I` record on Windows).
#[cfg(any(windows, target_os = "macos"))]
fn purge(location: &Path) {
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
        Err(_) => assert!(f.exists(), "file vanished although trashing failed"),
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
    let link_gone =
        std::fs::symlink_metadata(&link).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    let target_intact = std::fs::read(target.join("keep.txt")).is_ok_and(|bytes| bytes == b"keep");
    let location = outcome.as_ref().ok().cloned().flatten();
    let in_bin = location.as_deref().is_none_or(in_trash);
    // Clean the trash before asserting, so a failed assertion leaves nothing behind.
    if let Some(location) = &location {
        purge(location);
    }
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(link_gone, "the link is still there");
    assert!(target_intact, "the link target lost its content");
    assert!(in_bin, "{location:?} is not inside the trash");
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
