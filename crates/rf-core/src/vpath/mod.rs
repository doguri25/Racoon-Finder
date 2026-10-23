//! `VPath`: one address type for every place a file manager can look, with a string form
//! that loses nothing.

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
    /// Every scheme except `Scheme::Local`: `scheme://authority/segment/...`. What the
    /// authority and the segments mean is up to the provider that owns the scheme.
    Other {
        scheme: Scheme,
        authority: String,
        segments: Vec<String>,
    },
}

/// Rejects a segment that cannot be one name: empty, `.`, `..`, or containing `/` or NUL.
fn check_segment(name: &str) -> Result<(), PathError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(PathError::InvalidName(name.to_owned()));
    }
    Ok(())
}

/// Reads one `%XX`-escaped piece of a non-local address back into text.
fn unescape_piece(piece: &str) -> Result<String, PathError> {
    String::from_utf8(escape::unescape_bytes(piece)?)
        .map_err(|_| PathError::InvalidEscape(piece.to_owned()))
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

    /// Builds a non-local address `scheme://authority/segment/...`. The authority may be any
    /// text without NUL (an empty one and one containing `/` included); every segment must
    /// be one plain name. The meaning of both is up to the provider that owns the scheme, for
    /// example a connection profile ID, or the [`VPath::to_uri`] text of an enclosing archive.
    pub fn new(scheme: Scheme, authority: &str, segments: &[&str]) -> Result<Self, PathError> {
        if scheme == Scheme::Local {
            return Err(PathError::Unsupported(
                "use VPath::local for local paths".into(),
            ));
        }
        if authority.contains('\0') {
            return Err(PathError::Nul);
        }
        for segment in segments {
            check_segment(segment)?;
        }
        Ok(Self {
            repr: Repr::Other {
                scheme,
                authority: authority.to_owned(),
                segments: segments.iter().map(|&segment| segment.to_owned()).collect(),
            },
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
                scheme @ (Scheme::Archive
                | Scheme::Remote
                | Scheme::Shell
                | Scheme::Collection
                | Scheme::Search),
            ) => {
                let Some(rest) = rest.strip_prefix("//") else {
                    return Err(PathError::Unsupported(format!(
                        "expected `{}://`",
                        scheme.name()
                    )));
                };
                // Splitting at `/` is safe: escaping turned every `/` inside a piece into
                // `%2F`. A trailing or doubled `/` leaves an empty piece, which is skipped.
                let mut pieces = rest.split('/');
                let authority = unescape_piece(pieces.next().unwrap_or_default())?;
                let segments = pieces
                    .filter(|piece| !piece.is_empty())
                    .map(unescape_piece)
                    .collect::<Result<Vec<_>, _>>()?;
                let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
                Self::new(scheme, &authority, &segments)
            }
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
            Repr::Other {
                scheme,
                authority,
                segments,
            } => {
                let mut uri = format!(
                    "{scheme}://{}",
                    escape::escape_bytes(authority.as_bytes(), &['/'])
                );
                for segment in segments {
                    uri.push('/');
                    uri.push_str(&escape::escape_bytes(segment.as_bytes(), &['/']));
                }
                uri
            }
        }
    }

    pub fn scheme(&self) -> Scheme {
        match self.repr {
            Repr::Local(_) => Scheme::Local,
            Repr::Other { scheme, .. } => scheme,
        }
    }

    /// The authority of a non-local address; `None` for a local one.
    pub fn authority(&self) -> Option<&str> {
        match &self.repr {
            Repr::Local(_) => None,
            Repr::Other { authority, .. } => Some(authority),
        }
    }

    /// The names after the authority of a non-local address (none at its root); `None` for
    /// a local one.
    pub fn segments(&self) -> Option<&[String]> {
        match &self.repr {
            Repr::Local(_) => None,
            Repr::Other { segments, .. } => Some(segments),
        }
    }

    /// The host path of a local address.
    pub fn as_local(&self) -> Option<&Path> {
        match &self.repr {
            Repr::Local(path) => Some(path),
            Repr::Other { .. } => None,
        }
    }

    /// The last name, or `None` for a root (`/`, `C:\`, `\\server\share\`, `shell://thispc`).
    pub fn file_name(&self) -> Option<&OsStr> {
        match &self.repr {
            Repr::Local(path) => path.file_name(),
            Repr::Other { segments, .. } => segments.last().map(OsStr::new),
        }
    }

    /// The containing folder, or `None` for a root. The stored value is already in normal
    /// form, so the parent needs no further normalization.
    pub fn parent(&self) -> Option<VPath> {
        match &self.repr {
            Repr::Local(path) => path.parent().map(|parent| Self {
                repr: Repr::Local(parent.to_path_buf()),
            }),
            Repr::Other {
                scheme,
                authority,
                segments,
            } => {
                let (_, parent_segments) = segments.split_last()?;
                Some(Self {
                    repr: Repr::Other {
                        scheme: *scheme,
                        authority: authority.clone(),
                        segments: parent_segments.to_vec(),
                    },
                })
            }
        }
    }

    /// Appends one name. A name that is empty, `.`, `..`, or contains a separator or NUL
    /// (on Windows also `\` or `:` for a local address) is rejected, so a join can never
    /// leave the folder. A non-local address takes only names that are valid UTF-8.
    pub fn join(&self, name: impl AsRef<OsStr>) -> Result<VPath, PathError> {
        let name = name.as_ref();
        match &self.repr {
            Repr::Local(path) => {
                local::check_name(name)?;
                Ok(Self {
                    repr: Repr::Local(path.join(name)),
                })
            }
            Repr::Other {
                scheme,
                authority,
                segments,
            } => {
                let name = name
                    .to_str()
                    .ok_or_else(|| PathError::InvalidName(name.to_string_lossy().into_owned()))?;
                check_segment(name)?;
                let mut segments = segments.clone();
                segments.push(name.to_owned());
                Ok(Self {
                    repr: Repr::Other {
                        scheme: *scheme,
                        authority: authority.clone(),
                        segments,
                    },
                })
            }
        }
    }

    /// Whether `base` is this address or one of its ancestors. Names are compared whole,
    /// so `/a/bc` does not start with `/a/b`. A non-local address also needs the same scheme
    /// and authority, and a local address never starts with a non-local one or the reverse.
    pub fn starts_with(&self, base: &VPath) -> bool {
        match (&self.repr, &base.repr) {
            (Repr::Local(path), Repr::Local(prefix)) => path.starts_with(prefix),
            (
                Repr::Other {
                    scheme,
                    authority,
                    segments,
                },
                Repr::Other {
                    scheme: base_scheme,
                    authority: base_authority,
                    segments: base_segments,
                },
            ) => {
                scheme == base_scheme
                    && authority == base_authority
                    && segments.starts_with(base_segments)
            }
            (Repr::Local(_), Repr::Other { .. }) | (Repr::Other { .. }, Repr::Local(_)) => false,
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
            Repr::Other { .. } => None,
        }
    }
}

/// The human-readable form: for a local address the host path as the OS would show it, which
/// can replace invalid text with `U+FFFD`, so use [`VPath::to_uri`] when the value must
/// survive; for any other address it is [`VPath::to_uri`].
impl fmt::Display for VPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Local(path) => fmt::Display::fmt(&path.display(), f),
            Repr::Other { .. } => f.write_str(&self.to_uri()),
        }
    }
}

impl FromStr for VPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Stored as the [`VPath::to_uri`] string, the form journals, ledgers, settings and the
/// metadata database keep.
impl serde::Serialize for VPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_uri())
    }
}

/// Read back with [`VPath::parse`]; text it rejects is a deserialization error.
impl<'de> serde::Deserialize<'de> for VPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}
