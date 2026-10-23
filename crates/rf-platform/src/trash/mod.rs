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
/// The item is never deleted permanently instead, and never swapped for another one: a
/// symbolic link is moved itself, not what it points to, and a name that the Windows shell
/// would read as a different item (`report.` as `report`) is refused. On Windows the shell's
/// item is matched to the requested one by file identity, so an item on a file system that
/// reports no usable file ID is refused too. A non-local path, a place without a trash, and an
/// item the trash will not take (a refused recycle, a redirected name, no file identity) give
/// `Error::Unsupported`: the trash cannot take this item, and deleting it permanently is left
/// to the caller, after asking the user. A missing item gives `ErrorKind::NotFound`, and
/// anything else is an I/O error of the trash itself (on Windows the shell may fail to read a
/// very long path, for example). In every error case the item is left where it was.
///
/// This call blocks. On Windows a refused recycle is tried once more after 300 ms if the same
/// item is still there, and on macOS the call can wait for Finder for up to 20 seconds.
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

/// Puts an item back from the trash: moves it from `location`, its place inside the trash, to
/// `original`, the place it was trashed from.
///
/// This never replaces anything. If `original` is taken, by a file or a folder, the result is
/// `ErrorKind::AlreadyExists` and both items stay as they are. If the folder `original` was in
/// is gone, the result is `ErrorKind::NotFound`; on another volume it is
/// `ErrorKind::CrossesDevices`. A non-local path gives `Error::Unsupported`. An error carries
/// `location` as its path.
///
/// On Windows, once the item is back, the recycle bin's record of it is deleted too: the `$I…`
/// file next to a `$R…` location that sits where the bin keeps its items,
/// `<root>\$Recycle.Bin\<user SID>\$R…`. A failure there is ignored, because the item is back
/// either way. Any other pair of paths is renamed in the same way, and nothing else is deleted.
pub fn restore_from_trash(location: &VPath, original: &VPath) -> Result<()> {
    crate::fsops::rename_no_replace(location, original)?;
    #[cfg(windows)]
    remove_recycle_record(location);
    Ok(())
}

/// Deletes the `$I…` record that belongs to the recycle bin item `location` (`$R…`), if
/// `location` lies in a user folder of a `$Recycle.Bin` folder (its grandparent). Errors are
/// ignored.
#[cfg(windows)]
fn remove_recycle_record(location: &VPath) {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    let Some(local) = location.to_extended_length() else {
        return;
    };
    let in_bin = local
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|bin| bin.eq_ignore_ascii_case(OsStr::new("$Recycle.Bin")));
    let Some(name) = local.file_name().filter(|_| in_bin) else {
        return;
    };
    let units: Vec<u16> = name.encode_wide().collect();
    if let [dollar, r, rest @ ..] = units.as_slice() {
        if (*dollar, *r) == (u16::from(b'$'), u16::from(b'R')) {
            let mut record = vec![u16::from(b'$'), u16::from(b'I')];
            record.extend_from_slice(rest);
            // The record is the bin's bookkeeping; without the item the shell ignores it.
            let _ = std::fs::remove_file(local.with_file_name(OsString::from_wide(&record)));
        }
    }
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
