use std::io;

use rf_core::{Error, ErrorKind, PathError, Result, VPath};

fn here() -> VPath {
    VPath::local(std::env::temp_dir()).unwrap()
}

#[test]
fn io_errors_classify_by_kind() {
    let cases = [
        (io::ErrorKind::NotFound, ErrorKind::NotFound),
        (io::ErrorKind::PermissionDenied, ErrorKind::PermissionDenied),
        (io::ErrorKind::AlreadyExists, ErrorKind::AlreadyExists),
        (io::ErrorKind::Unsupported, ErrorKind::Unsupported),
        (io::ErrorKind::TimedOut, ErrorKind::Other),
    ];
    for (io_kind, kind) in cases {
        assert_eq!(
            Error::io("open", here(), io::Error::from(io_kind)).kind(),
            kind
        );
    }
}

#[test]
fn display_names_operation_and_path() {
    let e = Error::io("open", here(), io::Error::from(io::ErrorKind::NotFound));
    let msg = e.to_string();
    assert!(
        msg.starts_with(&format!("open failed for {}: ", here())),
        "{msg}"
    );
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn other_variants_classify() {
    assert_eq!(Error::from(PathError::Empty).kind(), ErrorKind::InvalidPath);
    assert_eq!(Error::Cancelled.kind(), ErrorKind::Cancelled);
    assert_eq!(
        Error::Unsupported("x".into()).kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(Error::MissingEnv("HOME").kind(), ErrorKind::Other);
    assert_eq!(Error::from(PathError::Empty).to_string(), "path is empty");
}

#[test]
fn question_mark_converts_path_errors() {
    fn f() -> Result<VPath> {
        Ok(VPath::parse("relative")?)
    }
    assert!(matches!(f(), Err(Error::Path(PathError::NotAbsolute(_)))));
}

#[test]
fn shared_types_cross_threads() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<Error>();
    assert_send_sync::<VPath>();
}
