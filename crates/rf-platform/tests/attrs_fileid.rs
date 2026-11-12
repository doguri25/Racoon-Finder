use std::path::Path;

use rf_core::{Attributes, ErrorKind, FileId, Scheme, VPath, VolumeId};
use rf_platform::{entry_attributes, file_identity, metadata_identity, resolved_file_identity};

fn attrs(p: &Path) -> Attributes {
    entry_attributes(
        p.file_name().unwrap(),
        &std::fs::symlink_metadata(p).unwrap(),
    )
}

fn id(p: &Path) -> (VolumeId, FileId) {
    file_identity(&VPath::local(p).unwrap()).unwrap()
}

#[test]
fn dot_files_are_hidden_except_on_windows() {
    let t = tempfile::tempdir().unwrap();
    let (dot, plain) = (t.path().join(".hidden"), t.path().join("plain.txt"));
    std::fs::write(&dot, b"").unwrap();
    std::fs::write(&plain, b"").unwrap();
    assert_eq!(attrs(&dot).contains(Attributes::HIDDEN), !cfg!(windows));
    assert!(!attrs(&plain).contains(Attributes::HIDDEN));
}

#[test]
#[allow(
    clippy::permissions_set_readonly_false,
    reason = "restore write access so the temp dir can be removed"
)]
fn readonly_permission_maps_to_readonly() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("ro.txt");
    std::fs::write(&p, b"x").unwrap();
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm.clone()).unwrap();
    assert!(attrs(&p).contains(Attributes::READONLY));
    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_hidden_attribute_is_read() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("h.txt");
    std::fs::write(&p, b"").unwrap();
    assert!(
        std::process::Command::new("attrib")
            .arg("+h")
            .arg(&p)
            .status()
            .unwrap()
            .success()
    );
    let a = attrs(&p);
    assert!(
        a.contains(Attributes::HIDDEN) && !a.contains(Attributes::SYSTEM),
        "{a:?}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_hidden_flag_is_read() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("visible-name.txt");
    std::fs::write(&p, b"").unwrap();
    assert!(
        std::process::Command::new("chflags")
            .arg("hidden")
            .arg(&p)
            .status()
            .unwrap()
            .success()
    );
    assert!(attrs(&p).contains(Attributes::HIDDEN));
}

#[test]
fn hard_links_share_identity() {
    let t = tempfile::tempdir().unwrap();
    let (a, b, c) = (
        t.path().join("a.txt"),
        t.path().join("b.txt"),
        t.path().join("c.txt"),
    );
    std::fs::write(&a, b"a").unwrap();
    std::fs::hard_link(&a, &b).unwrap();
    std::fs::write(&c, b"c").unwrap();
    assert_eq!(id(&a), id(&b));
    assert_ne!(id(&a).1, id(&c).1);
    assert_eq!(id(&a).0, id(&c).0);
}

#[test]
fn identity_survives_rename() {
    let t = tempfile::tempdir().unwrap();
    let (a, z) = (t.path().join("a.txt"), t.path().join("z.txt"));
    std::fs::write(&a, b"a").unwrap();
    let before = id(&a);
    std::fs::rename(&a, &z).unwrap();
    assert_eq!(id(&z), before);
    assert_eq!(id(t.path()).0, before.0);
}

#[test]
fn identity_errors() {
    let t = tempfile::tempdir().unwrap();
    let missing = VPath::local(t.path().join("missing")).unwrap();
    assert_eq!(
        file_identity(&missing).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(
        file_identity(&remote).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}

/// Makes `link` lead to the folder `target`: a symbolic link on Unix.
#[cfg(unix)]
fn dir_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Makes `link` lead to the folder `target`: a junction on Windows, which unlike a directory
/// symbolic link needs no privilege.
#[cfg(windows)]
fn dir_alias(target: &Path, link: &Path) -> std::io::Result<()> {
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

/// A link has an identity of its own; following it gives the identity of the folder it leads
/// to, and without a link both calls agree.
#[test]
fn resolved_identity_follows_links() {
    let t = tempfile::tempdir().unwrap();
    let (target, link) = (t.path().join("target"), t.path().join("link"));
    std::fs::create_dir(&target).unwrap();
    if let Err(e) = dir_alias(&target, &link) {
        eprintln!("skipped: cannot make a folder link here: {e}");
        return;
    }
    let (link, target_v) = (VPath::local(&link).unwrap(), VPath::local(&target).unwrap());
    assert_eq!(resolved_file_identity(&link).unwrap(), id(&target));
    assert_ne!(file_identity(&link).unwrap(), id(&target));
    assert_eq!(resolved_file_identity(&target_v).unwrap(), id(&target));
    let missing = VPath::local(t.path().join("missing")).unwrap();
    assert_eq!(
        resolved_file_identity(&missing).unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

/// Unix metadata holds the identity that `file_identity` reads; Windows metadata holds none.
#[test]
fn metadata_identity_matches_file_identity() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("f.txt");
    std::fs::write(&f, b"f").unwrap();
    for p in [f.as_path(), t.path()] {
        let meta = std::fs::symlink_metadata(p).unwrap();
        let want = cfg!(unix).then(|| id(p));
        assert_eq!(metadata_identity(&meta), want, "{}", p.display());
    }
}
