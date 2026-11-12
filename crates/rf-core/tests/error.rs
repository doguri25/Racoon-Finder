use std::io;

use rf_core::{Error, ErrorKind, PathError, Result, VPath};

fn here() -> VPath {
    VPath::local(std::env::temp_dir()).unwrap()
}

fn kind_of(source: io::Error) -> ErrorKind {
    Error::io("open", here(), source).kind()
}

#[test]
fn io_errors_classify_by_kind() {
    let cases = [
        (io::ErrorKind::NotFound, ErrorKind::NotFound),
        (io::ErrorKind::PermissionDenied, ErrorKind::PermissionDenied),
        (io::ErrorKind::AlreadyExists, ErrorKind::AlreadyExists),
        (io::ErrorKind::Unsupported, ErrorKind::Unsupported),
        (io::ErrorKind::InvalidData, ErrorKind::Other),
        (io::ErrorKind::UnexpectedEof, ErrorKind::Other),
    ];
    for (io_kind, kind) in cases {
        assert_eq!(kind_of(io::Error::from(io_kind)), kind, "{io_kind:?}");
    }
}

#[test]
fn file_system_io_kinds_have_their_own_category() {
    let cases = [
        (io::ErrorKind::NotADirectory, ErrorKind::NotADirectory),
        (io::ErrorKind::IsADirectory, ErrorKind::IsADirectory),
        (
            io::ErrorKind::DirectoryNotEmpty,
            ErrorKind::DirectoryNotEmpty,
        ),
        (io::ErrorKind::StorageFull, ErrorKind::StorageFull),
        (io::ErrorKind::QuotaExceeded, ErrorKind::StorageFull),
        (io::ErrorKind::ReadOnlyFilesystem, ErrorKind::ReadOnly),
        (io::ErrorKind::CrossesDevices, ErrorKind::CrossesDevices),
        (io::ErrorKind::ResourceBusy, ErrorKind::Busy),
        (io::ErrorKind::TimedOut, ErrorKind::TimedOut),
        (io::ErrorKind::Interrupted, ErrorKind::Interrupted),
        (io::ErrorKind::FileTooLarge, ErrorKind::FileTooLarge),
    ];
    for (io_kind, kind) in cases {
        assert_eq!(kind_of(io::Error::from(io_kind)), kind, "{io_kind:?}");
    }
}

/// The Windows-only OS codes must not be read on other systems, where the same numbers
/// mean something else (32 is `EPIPE`, 33 is `EDOM`).
#[cfg(unix)]
#[test]
fn windows_os_codes_are_not_special_elsewhere() {
    for code in [32, 33] {
        assert_eq!(
            kind_of(io::Error::from_raw_os_error(code)),
            ErrorKind::Other,
            "{code}"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_os_codes_classify() {
    let cases = [
        (32, ErrorKind::Busy),           // ERROR_SHARING_VIOLATION
        (33, ErrorKind::Busy),           // ERROR_LOCK_VIOLATION
        (123, ErrorKind::InvalidPath),   // ERROR_INVALID_NAME
        (161, ErrorKind::InvalidPath),   // ERROR_BAD_PATHNAME
        (206, ErrorKind::InvalidPath),   // ERROR_FILENAME_EXCED_RANGE
        (2, ErrorKind::NotFound),        // ERROR_FILE_NOT_FOUND, through std's kind
        (17, ErrorKind::CrossesDevices), // ERROR_NOT_SAME_DEVICE, through std's kind
    ];
    for (code, kind) in cases {
        assert_eq!(kind_of(io::Error::from_raw_os_error(code)), kind, "{code}");
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
