//! Normalization of host paths into the canonical absolute form `VPath` stores.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::error::PathError;

fn lossy(os: &OsStr) -> String {
    os.to_string_lossy().into_owned()
}

/// Rejects a name that cannot be one path component: empty, `.`, `..`, or containing a
/// separator or NUL. On Windows `\` and `:` are rejected too, because `a:b` addresses an
/// NTFS alternate data stream rather than a file named `a:b`.
pub(super) fn check_name(name: &OsStr) -> Result<(), PathError> {
    let bytes = name.as_encoded_bytes();
    let forbidden: &[u8] = if cfg!(windows) { b"/\\:\0" } else { b"/\0" };
    if bytes.is_empty()
        || bytes == b"."
        || bytes == b".."
        || bytes.iter().any(|b| forbidden.contains(b))
    {
        return Err(PathError::InvalidName(lossy(name)));
    }
    Ok(())
}

/// Appends the components that follow the root to `out`: names are checked and pushed,
/// `..` pops (and stops at the root), `.` is dropped.
fn push_components<'a>(
    mut out: PathBuf,
    rest: impl Iterator<Item = Component<'a>>,
) -> Result<PathBuf, PathError> {
    for component in rest {
        match component {
            Component::Normal(name) => {
                check_name(name)?;
                out.push(name);
            }
            Component::ParentDir => {
                out.pop();
            }
            // The caller already placed the root; `components()` yields these only first.
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Ok(out)
}

/// Turns an absolute Unix path into its normal form: rooted at `/`, no `.`, no `..`,
/// no repeated or trailing separators. Names are kept byte for byte.
#[cfg(unix)]
pub(super) fn normalize(path: &Path) -> Result<PathBuf, PathError> {
    if path.as_os_str().is_empty() {
        return Err(PathError::Empty);
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(PathError::Nul);
    }
    if !path.has_root() {
        return Err(PathError::NotAbsolute(lossy(path.as_os_str())));
    }
    push_components(PathBuf::from("/"), path.components())
}

/// Turns an absolute Windows path into its normal form: `\\?\` stripped where a plain form
/// exists, uppercase drive letter, `\` separators, no `.` or `..`. Relative, drive-relative,
/// rooted-without-drive and device (`\\.\`) paths are rejected.
#[cfg(windows)]
pub(super) fn normalize(path: &Path) -> Result<PathBuf, PathError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Prefix;

    use crate::winpath;

    let input = path.as_os_str();
    if input.is_empty() {
        return Err(PathError::Empty);
    }
    let units: Vec<u16> = input.encode_wide().collect();
    if units.contains(&0) {
        return Err(PathError::Nul);
    }
    let stripped = OsString::from_wide(&winpath::strip_extended_length(&units));
    let mut components = Path::new(&stripped).components();

    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(PathError::NotAbsolute(lossy(input)));
    };
    let mut root = match prefix.kind() {
        Prefix::Disk(letter) => {
            OsString::from(format!("{}:", char::from(letter.to_ascii_uppercase())))
        }
        Prefix::UNC(server, share) => {
            let mut unc = OsString::from(r"\\");
            unc.push(server);
            unc.push(r"\");
            unc.push(share);
            unc
        }
        Prefix::Verbatim(_) | Prefix::VerbatimUNC(..) | Prefix::VerbatimDisk(_) => {
            prefix.as_os_str().to_owned()
        }
        Prefix::DeviceNS(_) => return Err(PathError::Unsupported(lossy(input))),
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(PathError::NotAbsolute(lossy(input)));
    }
    root.push(r"\");
    push_components(PathBuf::from(root), components)
}

/// The extended-length (`\\?\`) form of a normalized local path, for Windows file APIs.
/// Every `VPath` is absolute, so the conversion always applies; the fallback returns the
/// input unchanged and cannot be reached.
#[cfg(windows)]
pub(super) fn extended_length(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use crate::winpath;

    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    match winpath::to_extended_length(&units) {
        Some(extended) => PathBuf::from(OsString::from_wide(&extended)),
        None => path.to_path_buf(),
    }
}

/// Other systems have no separate long-path form: the path is used as it is.
#[cfg(not(windows))]
pub(super) fn extended_length(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_name_rejects_what_is_not_one_component() {
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert_eq!(
                check_name(OsStr::new(bad)),
                Err(PathError::InvalidName(bad.into())),
                "{bad:?}"
            );
        }
        assert_eq!(check_name(OsStr::new("a b.txt")), Ok(()));
        assert_eq!(check_name(OsStr::new("...")), Ok(()));
        assert_eq!(
            check_name(OsStr::new(r"a\b")).is_err(),
            cfg!(windows),
            "backslash is a separator only on Windows"
        );
        assert_eq!(
            check_name(OsStr::new("a:b")).is_err(),
            cfg!(windows),
            "colon is a stream marker only on Windows"
        );
    }
}
