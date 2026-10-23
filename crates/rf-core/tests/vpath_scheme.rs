use std::ffi::OsStr;
use std::path::PathBuf;

use rf_core::{PathError, Scheme, VPath};

fn local_root() -> VPath {
    VPath::local(PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" })).unwrap()
}

#[test]
fn non_local_uri_and_navigation() {
    let p = VPath::new(Scheme::Remote, "prof 1", &["dir", "a b", "50%"]).unwrap();
    assert_eq!(p.to_uri(), "remote://prof 1/dir/a b/50%25");
    assert_eq!(p.to_string(), p.to_uri());
    assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
    assert_eq!(p.scheme(), Scheme::Remote);
    assert_eq!(p.authority(), Some("prof 1"));
    let segs: Vec<String> = ["dir", "a b", "50%"].map(String::from).to_vec();
    assert_eq!(p.segments(), Some(segs.as_slice()));
    assert_eq!(p.file_name(), Some(OsStr::new("50%")));
    assert_eq!(p.parent().unwrap().to_uri(), "remote://prof 1/dir/a b");
    assert_eq!(p.as_local(), None);
    assert_eq!(p.to_extended_length(), None);
    assert_eq!(local_root().authority(), None);
    assert_eq!(local_root().segments(), None);
}

#[test]
fn non_local_root_and_join() {
    let root = VPath::new(Scheme::Shell, "thispc", &[]).unwrap();
    assert_eq!(root.to_uri(), "shell://thispc");
    assert_eq!(root.parent(), None);
    assert_eq!(root.file_name(), None);
    assert_eq!(
        root.join("C:"),
        VPath::new(Scheme::Shell, "thispc", &["C:"])
    );
    assert_eq!(root.join(".."), Err(PathError::InvalidName("..".into())));
    assert_eq!(VPath::parse("shell://thispc/"), Ok(root));
}

#[test]
fn archive_authority_nests_a_vpath() {
    let outer = local_root().join("x.zip").unwrap();
    let arc = VPath::new(Scheme::Archive, &outer.to_uri(), &["inner", "a.txt"]).unwrap();
    let escaped = outer.to_uri().replace('%', "%25").replace('/', "%2F");
    assert_eq!(arc.to_uri(), format!("archive://{escaped}/inner/a.txt"));
    assert_eq!(VPath::parse(&arc.to_uri()), Ok(arc.clone()));
    assert_eq!(arc.authority().map(VPath::parse), Some(Ok(outer)));
}

#[test]
fn new_and_parse_reject_bad_input() {
    assert_eq!(
        VPath::new(Scheme::Local, "", &[]),
        Err(PathError::Unsupported(
            "use VPath::local for local paths".into()
        ))
    );
    assert_eq!(
        VPath::new(Scheme::Remote, "p", &[".."]),
        Err(PathError::InvalidName("..".into()))
    );
    assert_eq!(
        VPath::new(Scheme::Remote, "p", &["a/b"]),
        Err(PathError::InvalidName("a/b".into()))
    );
    assert_eq!(VPath::new(Scheme::Remote, "p\0", &[]), Err(PathError::Nul));
    assert_eq!(
        VPath::parse("remote:/x"),
        Err(PathError::Unsupported("expected `remote://`".into()))
    );
}

#[test]
fn starts_with_never_crosses_schemes() {
    let a = VPath::new(Scheme::Remote, "p1", &["a"]).unwrap();
    let ab = a.join("b").unwrap();
    assert!(ab.starts_with(&a));
    assert!(!ab.starts_with(&VPath::new(Scheme::Remote, "p2", &["a"]).unwrap()));
    assert!(!ab.starts_with(&VPath::new(Scheme::Archive, "p1", &["a"]).unwrap()));
    assert!(!ab.starts_with(&local_root()));
    assert!(!local_root().starts_with(&a));
}

#[test]
fn serde_uses_uri_string() {
    let p = VPath::new(Scheme::Remote, "p1", &["a b"]).unwrap();
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(json, "\"remote://p1/a b\"");
    assert_eq!(serde_json::from_str::<VPath>(&json).unwrap(), p);
    assert!(serde_json::from_str::<VPath>("\"sftp://x\"").is_err());
}

proptest::proptest! {
    #[test]
    fn non_local_roundtrip(
        auth in "[^\\x00]{0,12}",
        segs in proptest::collection::vec("[^/\\x00]{1,12}", 0..5),
    ) {
        let segs: Vec<&str> = segs.iter().map(String::as_str).filter(|s| *s != "." && *s != "..").collect();
        let p = VPath::new(Scheme::Remote, &auth, &segs).unwrap();
        proptest::prop_assert_eq!(VPath::parse(&p.to_uri()), Ok(p));
    }
}
