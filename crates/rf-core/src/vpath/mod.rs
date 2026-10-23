//! `VPath`: one address type for every place a file manager can look, with a string form
//! that loses nothing. Only the local scheme exists so far.

mod escape;
mod local;
mod scheme;

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::PathError;

pub use scheme::Scheme;

/// An absolute, normalized address.
///
/// Names are kept exactly as the OS gave them: no Unicode normalization, no case folding,
/// and bytes (Unix) or UTF-16 units (Windows) that are not valid text survive a round trip
/// through [`VPath::to_uri`] and [`VPath::parse`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VPath {
    repr: Repr,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Repr {
    Local(PathBuf),
}

/// Splits `scheme:rest` when the part before the first `:` looks like a scheme name.
/// A single letter is a Windows drive (`C:\...`), not a scheme.
fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let (name, rest) = input.split_once(':')?;
    let mut chars = name.chars();
    let is_scheme = name.len() >= 2
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    is_scheme.then_some((name, rest))
}

impl VPath {
    /// Builds a local address from an absolute host path, resolving `.` and `..` and
    /// collapsing repeated separators. Nothing touches the file system.
    pub fn local(path: impl AsRef<Path>) -> Result<Self, PathError> {
        Ok(Self {
            repr: Repr::Local(local::normalize(path.as_ref())?),
        })
    }

    /// Reads user or stored text. `scheme:rest` is decoded per scheme (`local:` text is
    /// `%XX`-unescaped); text without a scheme is a native path and is taken literally,
    /// so a file named `a%41` stays `a%41`.
    pub fn parse(input: &str) -> Result<Self, PathError> {
        if input.is_empty() {
            return Err(PathError::Empty);
        }
        let Some((name, rest)) = split_scheme(input) else {
            return Self::local(input);
        };
        match Scheme::from_name(name) {
            None => Err(PathError::UnknownScheme(name.to_owned())),
            Some(Scheme::Local) => Self::local(escape::unescape_os(rest)?),
            Some(
                Scheme::Archive
                | Scheme::Remote
                | Scheme::Shell
                | Scheme::Collection
                | Scheme::Search,
            ) => Err(PathError::Unsupported(format!(
                "{name} paths are not implemented"
            ))),
        }
    }

    /// The lossless string form, `scheme:path`, that [`VPath::parse`] reads back.
    pub fn to_uri(&self) -> String {
        match &self.repr {
            Repr::Local(path) => {
                format!(
                    "{}:{}",
                    Scheme::Local,
                    escape::escape_os(path.as_os_str(), &[])
                )
            }
        }
    }

    pub fn scheme(&self) -> Scheme {
        match self.repr {
            Repr::Local(_) => Scheme::Local,
        }
    }

    /// The host path of a local address.
    pub fn as_local(&self) -> Option<&Path> {
        match &self.repr {
            Repr::Local(path) => Some(path),
        }
    }

    /// The last name, or `None` for a root (`/`, `C:\`, `\\server\share\`).
    pub fn file_name(&self) -> Option<&OsStr> {
        match &self.repr {
            Repr::Local(path) => path.file_name(),
        }
    }

    /// The containing folder, or `None` for a root. The stored path is already in normal
    /// form, so the parent needs no further normalization.
    pub fn parent(&self) -> Option<VPath> {
        match &self.repr {
            Repr::Local(path) => path.parent().map(|parent| Self {
                repr: Repr::Local(parent.to_path_buf()),
            }),
        }
    }

    /// Appends one name. A name that is empty, `.`, `..`, or contains a separator or NUL
    /// (on Windows also `\` or `:`) is rejected, so a join can never leave the folder.
    pub fn join(&self, name: impl AsRef<OsStr>) -> Result<VPath, PathError> {
        let name = name.as_ref();
        local::check_name(name)?;
        match &self.repr {
            Repr::Local(path) => Ok(Self {
                repr: Repr::Local(path.join(name)),
            }),
        }
    }

    /// Whether `base` is this address or one of its ancestors. Names are compared whole,
    /// so `/a/bc` does not start with `/a/b`.
    pub fn starts_with(&self, base: &VPath) -> bool {
        match (&self.repr, &base.repr) {
            (Repr::Local(path), Repr::Local(prefix)) => path.starts_with(prefix),
        }
    }

    /// The path to hand to Windows file APIs. On Windows it is always in the `\\?\` form,
    /// which lifts the 260-character limit and passes reserved names (`CON`) and names with
    /// a trailing dot or space (`foo.`) to the file system exactly as written. Shell APIs
    /// (`ShellExecute` and the like) do not reliably accept it; give them [`VPath::as_local`].
    /// On other systems this is the local path itself. Non-local addresses give `None`.
    pub fn to_extended_length(&self) -> Option<PathBuf> {
        match &self.repr {
            Repr::Local(path) => Some(local::extended_length(path)),
        }
    }
}

/// The human-readable form: the host path as the OS would show it. It can replace
/// invalid text with `U+FFFD`, so use [`VPath::to_uri`] when the value must survive.
impl fmt::Display for VPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Local(path) => fmt::Display::fmt(&path.display(), f),
        }
    }
}

impl FromStr for VPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
