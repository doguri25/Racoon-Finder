//! File-system operations that std does not offer in a safe form.

use rf_core::{Error, Result, VPath};

/// Renames or moves `from` to `to` on one volume, atomically, and never replaces `to`.
///
/// If `to` exists, whether a file or a folder, the result is `ErrorKind::AlreadyExists` and
/// nothing changes. If `to` is on another volume, the result is `ErrorKind::CrossesDevices`,
/// and the caller copies and deletes instead. A non-local path gives `Error::Unsupported`.
/// An error carries `from` as its path.
///
/// `std::fs::rename` replaces an existing `to`, so user commands must not call it.
pub fn rename_no_replace(from: &VPath, to: &VPath) -> Result<()> {
    let (Some(source), Some(target)) = (from.to_extended_length(), to.to_extended_length()) else {
        return Err(Error::Unsupported(format!(
            "rename needs local paths: {from} to {to}"
        )));
    };
    imp::rename_no_replace(&source, &target).map_err(|e| Error::io("rename", from.clone(), e))
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::path::Path;

    use rustix::fs::{CWD, RenameFlags, renameat_with};

    pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        // `renameat2(RENAME_NOREPLACE)` on Linux, `renameatx_np(RENAME_EXCL)` on macOS.
        match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(()),
            #[cfg(target_os = "linux")]
            Err(rustix::io::Errno::INVAL) => fallback(from, to),
            Err(e) => Err(e.into()),
        }
    }

    /// `EINVAL` means the file system does not know `RENAME_NOREPLACE`. Check for `to` first and
    /// rename plainly. Another process can create `to` in between and have it replaced; Linux is
    /// a development platform only, and macOS never gets here.
    #[cfg(target_os = "linux")]
    fn fallback(from: &Path, to: &Path) -> io::Result<()> {
        match std::fs::symlink_metadata(to) {
            Ok(_) => Err(io::ErrorKind::AlreadyExists.into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::rename(from, to),
            Err(e) => Err(e),
        }
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code, reason = "MoveFileExW FFI")]

    use std::io;
    use std::path::Path;

    use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
    use windows::core::PCWSTR;

    use crate::win_util::{to_io, wide};

    pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (wide(from.as_os_str()), wide(to.as_os_str()));
        // Without `MOVEFILE_REPLACE_EXISTING` an existing target gives `ERROR_ALREADY_EXISTS`,
        // and without `MOVEFILE_COPY_ALLOWED` another volume gives `ERROR_NOT_SAME_DEVICE`.
        // SAFETY: `from` and `to` are NUL-terminated and outlive the call.
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVE_FILE_FLAGS(0),
            )
        }
        .map_err(|e| to_io(&e))
    }
}
