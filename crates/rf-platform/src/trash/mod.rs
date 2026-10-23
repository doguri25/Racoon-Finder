//! Sending items to the system trash, which never turns into a permanent delete.
//!
//! A request that the trash cannot serve (a network share, a USB stick, a path the Windows
//! recycle bin cannot take, a system without a trash) fails and leaves the item alone. Only the
//! caller, after asking the user, may delete permanently.

use std::path::PathBuf;

use rf_core::{Error, Result, VPath};

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(windows)]
use self::windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// Whether `path` lies where the system trash can take it.
///
/// On Windows that is a fixed local drive; network shares, removable and optical drives and
/// UNC paths give `false`. On macOS it is a volume for which the user's trash folder can be
/// found. Anywhere else, and for a non-local path, it is `false`. The item itself is not
/// looked at, so a path that does not exist gives the answer for its volume.
pub fn trash_supported(path: &VPath) -> bool {
    path.to_extended_length()
        .is_some_and(|local| imp::supported(&local))
}

/// Moves `path` into the system trash.
///
/// `Ok(Some(location))` is the new place inside the trash, `Ok(None)` means the item went to
/// the trash but the system did not say where. On Windows the item goes to the recycle bin in
/// a way that "Restore" works, and on macOS through Finder so that "Put Back" works, if
/// Finder may be automated.
///
/// The item is never deleted permanently instead. A non-local path, and a place without a
/// trash, give `Error::Unsupported`, and a missing item gives `ErrorKind::NotFound`; in every
/// error case the item is left where it was.
///
/// This call blocks, and on macOS it can wait for Finder for up to 20 seconds.
pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "trash needs a local path: {path}"
        )));
    };
    std::fs::symlink_metadata(&local).map_err(|e| Error::io("trash", path.clone(), e))?;
    if !imp::supported(&local) {
        return Err(Error::Unsupported(format!(
            "no trash available for {path}; delete permanently instead"
        )));
    }
    imp::move_to_trash(path)
}

/// Systems without a trash: nothing is supported, so `move_to_trash` stops before it gets here.
#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use std::path::{Path, PathBuf};

    use rf_core::{Error, Result, VPath};

    pub fn supported(_local: &Path) -> bool {
        false
    }

    pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
        Err(Error::Unsupported(format!(
            "no trash available for {path}; delete permanently instead"
        )))
    }
}
