use std::path::Path;

use rf_core::{Attributes, ErrorKind, FileId, Scheme, VPath, VolumeId};
use rf_platform::{entry_attributes, file_identity};

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
