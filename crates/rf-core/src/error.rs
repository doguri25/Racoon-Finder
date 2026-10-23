//! Error types shared across crates. The `Display` text is English for logs;
//! the UI maps the variant to a localized message.

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
