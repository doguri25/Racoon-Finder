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
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidPath,
    Unsupported,
    Cancelled,
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
            // `io::ErrorKind::InvalidFilename` is stable only since 1.87, above the MSRV.
            Self::Io { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound => ErrorKind::NotFound,
                std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
                std::io::ErrorKind::AlreadyExists => ErrorKind::AlreadyExists,
                std::io::ErrorKind::Unsupported => ErrorKind::Unsupported,
                _ => ErrorKind::Other,
            },
            Self::Path(_) => ErrorKind::InvalidPath,
            Self::Unsupported(_) => ErrorKind::Unsupported,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::MissingEnv(_) => ErrorKind::Other,
        }
    }
}
