use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rf_core::{PathError, VPath};

fn abs(parts: &[&str]) -> PathBuf {
    let mut p = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
    for part in parts {
        p.push(part);
    }
    p
}

fn local(parts: &[&str]) -> VPath {
    VPath::local(abs(parts)).unwrap()
}

#[test]
fn parent_file_name_join() {
    let ab = local(&["a", "b"]);
    assert_eq!(ab.file_name(), Some(OsStr::new("b")));
    assert_eq!(ab.parent(), Some(local(&["a"])));
    assert_eq!(local(&["a"]).parent(), Some(local(&[])));
    assert_eq!(local(&[]).parent(), None);
    assert_eq!(local(&[]).file_name(), None);
    assert_eq!(local(&["a"]).join("b"), Ok(ab));
}

#[test]
fn join_rejects_non_names() {
    let base = local(&["a"]);
    for bad in ["", ".", "..", "x/y", "nul\0"] {
        assert_eq!(
            base.join(bad),
            Err(PathError::InvalidName(bad.into())),
            "{bad:?}"
        );
    }
}

#[test]
fn starts_with_is_component_wise() {
    let ab = local(&["a", "b"]);
    assert!(local(&["a", "b", "c"]).starts_with(&ab));
    assert!(ab.starts_with(&ab));
    assert!(!local(&["a", "bc"]).starts_with(&ab));
    assert!(!local(&["a"]).starts_with(&ab));
}

#[cfg(unix)]
mod unix {
    use super::*;

    #[test]
    fn backslash_is_an_ordinary_character() {
        let p = VPath::local("/a").unwrap().join(r"b\c").unwrap();
        assert_eq!(p.as_local(), Some(Path::new(r"/a/b\c")));
        assert_eq!(p.to_extended_length(), Some(PathBuf::from(r"/a/b\c")));
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn join_rejects_backslash_and_colon() {
        let base = VPath::local(r"C:\a").unwrap();
        assert_eq!(
            base.join(r"b\c"),
            Err(PathError::InvalidName(r"b\c".into()))
        );
        assert_eq!(base.join("b:c"), Err(PathError::InvalidName("b:c".into())));
    }

    #[test]
    fn unc_roots() {
        let root = VPath::local(r"\\srv\share\x").unwrap().parent().unwrap();
        assert_eq!(root.as_local(), Some(Path::new(r"\\srv\share\")));
        assert_eq!(root.parent(), None);
        assert_eq!(VPath::local(r"C:\").unwrap().parent(), None);
    }

    #[test]
    fn extended_length_form() {
        let p = VPath::local(r"C:\a").unwrap();
        assert_eq!(p.to_extended_length(), Some(PathBuf::from(r"\\?\C:\a")));
        let u = VPath::local(r"\\srv\share\x").unwrap();
        assert_eq!(
            u.to_extended_length(),
            Some(PathBuf::from(r"\\?\UNC\srv\share\x"))
        );
        let long = (0..40).fold(VPath::local(r"C:\").unwrap(), |p, _| {
            p.join("abcdefghij").unwrap()
        });
        assert!(long.as_local().unwrap().as_os_str().len() > 260);
        let ext = long.to_extended_length().unwrap();
        assert!(ext.to_string_lossy().starts_with(r"\\?\C:\abcdefghij\"));
    }

    #[test]
    fn reserved_and_trailing_dot_names_stay_literal() {
        let con = VPath::local(r"C:\a\CON").unwrap();
        assert_eq!(
            con.to_extended_length(),
            Some(PathBuf::from(r"\\?\C:\a\CON"))
        );
        let dot = VPath::local(r"C:\a").unwrap().join("foo.").unwrap();
        assert_eq!(dot.file_name(), Some(OsStr::new("foo.")));
        assert_eq!(
            dot.to_extended_length(),
            Some(PathBuf::from(r"\\?\C:\a\foo."))
        );
    }
}
