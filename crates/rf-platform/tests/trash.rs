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
