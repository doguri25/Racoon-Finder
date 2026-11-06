//! Error types shared across crates. The `Display` text is English for logs;
//! the UI maps the variant to a localized message.

use crate::vpath::VPath;

/// Why a path string or `OsStr` could not become a `VPath`.
///
/// The `String` payloads are lossy renderings of the offending input.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path contains a NUL character")]
    Nul,
    #[error("path is not absolute: {0}")]
    NotAbsolute(String),
    #[error("unknown path scheme `{0}`")]
    UnknownScheme(String),
    #[error("invalid escape sequence in `{0}`")]
    InvalidEscape(String),
    #[error("invalid name `{0}`")]
    InvalidName(String),
    #[error("unsupported path form: {0}")]
    Unsupported(String),
}

/// Shorthand for results that fail with the shared [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The error type shared by every crate.
///
/// Match on [`Error::kind`] to decide what to do (retry, skip, ask about a
/// conflict, pick UI text); the `Display` text is English for logs only.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An I/O operation failed on `path`. `op` names the failed operation,
    /// for example `"open"` or `"create_dir_all"`.
    #[error("{op} failed for {path}: {source}")]
    Io {
        op: &'static str,
        path: VPath,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("environment variable {0} is not set; cannot locate the app data folders")]
    MissingEnv(&'static str),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("operation cancelled")]
    Cancelled,
}

/// The coarse category callers branch on, without matching every [`Error`] variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The file or folder does not exist.
    NotFound,
    /// The OS refused access to the item.
    PermissionDenied,
    /// The name is already taken.
    AlreadyExists,
    /// Any [`PathError`] (`PathError::Unsupported` too), or a name Windows rejected as invalid.
    InvalidPath,
    /// A path component that must be a folder is not one.
    NotADirectory,
    /// The operation needs something other than a folder but found one.
    IsADirectory,
    /// The folder must be empty and is not.
    DirectoryNotEmpty,
    /// The volume is full or a disk quota is used up.
    StorageFull,
    /// The volume is read-only.
    ReadOnly,
    /// A rename cannot move the item to another volume; copy and delete instead.
    CrossesDevices,
    /// Another process holds the item open or locked; a later retry may work.
    Busy,
    /// The operation took too long, for example on a network share.
    TimedOut,
    /// The OS interrupted the call; a retry may work.
    Interrupted,
    /// The file is larger than the file system allows.
    FileTooLarge,
    /// The operation is not supported for this item, provider or system.
    Unsupported,
    /// The operation was cancelled.
    Cancelled,
    /// Anything else: [`Error::MissingEnv`] and I/O errors no variant above covers.
    Other,
}

impl Error {
    /// Builds an [`Error::Io`] for the failed operation `op` on `path`.
    pub fn io(op: &'static str, path: VPath, source: std::io::Error) -> Self {
        Self::Io { op, path, source }
    }

    /// The category of this error.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Io { source, .. } => {
                #[cfg(windows)]
                if let Some(kind) = windows_code_kind(source) {
                    return kind;
                }
                io_kind(source.kind())
            }
            Self::Path(_) => ErrorKind::InvalidPath,
            Self::Unsupported(_) => ErrorKind::Unsupported,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::MissingEnv(_) => ErrorKind::Other,
        }
    }
}

/// The category of a std I/O error kind.
fn io_kind(kind: std::io::ErrorKind) -> ErrorKind {
    use std::io::ErrorKind as Io;

    // `io::ErrorKind::InvalidFilename` is stable only since 1.87, above the MSRV; on
    // Windows `windows_code_kind` covers it by OS code.
    match kind {
        Io::NotFound => ErrorKind::NotFound,
        Io::PermissionDenied => ErrorKind::PermissionDenied,
        Io::AlreadyExists => ErrorKind::AlreadyExists,
        Io::NotADirectory => ErrorKind::NotADirectory,
        Io::IsADirectory => ErrorKind::IsADirectory,
        Io::DirectoryNotEmpty => ErrorKind::DirectoryNotEmpty,
        Io::StorageFull | Io::QuotaExceeded => ErrorKind::StorageFull,
        Io::ReadOnlyFilesystem => ErrorKind::ReadOnly,
        Io::CrossesDevices => ErrorKind::CrossesDevices,
        Io::ResourceBusy => ErrorKind::Busy,
        Io::TimedOut => ErrorKind::TimedOut,
        Io::Interrupted => ErrorKind::Interrupted,
        Io::FileTooLarge => ErrorKind::FileTooLarge,
        Io::Unsupported => ErrorKind::Unsupported,
        _ => ErrorKind::Other,
    }
}

/// Windows OS codes that std files under no stable kind the MSRV can name
/// (`Uncategorized`, or `InvalidFilename` from 1.87).
#[cfg(windows)]
fn windows_code_kind(error: &std::io::Error) -> Option<ErrorKind> {
    match error.raw_os_error()? {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        32 | 33 => Some(ErrorKind::Busy),
        // ERROR_INVALID_NAME, ERROR_BAD_PATHNAME, ERROR_FILENAME_EXCED_RANGE
        123 | 161 | 206 => Some(ErrorKind::InvalidPath),
        _ => None,
    }
}
