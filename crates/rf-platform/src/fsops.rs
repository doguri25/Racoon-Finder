//! File-system operations that std does not offer in a safe form: a rename that never
//! replaces, syncs that make files and folders durable on every system, and an open for reading
//! that never blocks on an item that is no regular file.

use std::fs::File;
use std::io;
use std::path::Path;

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

/// Makes the content and the metadata of the open file `file` durable: once this returns `Ok`,
/// they are on the disk, not only in the caches of the OS and of the drive.
///
/// - macOS: `fcntl(F_FULLFSYNC)`, which flushes the drive's own cache too (std's
///   `File::sync_all` does only this). A file system that cannot do it answers `ENOTSUP`,
///   `EINVAL` or `ENOTTY` (an SMB or NFS share, among others) and gets a plain `fsync(2)`
///   instead, as Go and SQLite do: that is the most such a file system can promise. Any other
///   error is returned.
/// - Elsewhere: `File::sync_all` (`fsync`, `FlushFileBuffers`).
pub fn sync_file(file: &File) -> io::Result<()> {
    imp::sync_file(file)
}

/// Makes the changes to the entries of the folder `path` durable: the items made in it and the
/// renames into and out of it, such as the commit that gave a copy its final name. `path` is
/// the native path (on Windows the extended-length form).
///
/// - Unix: the folder is opened read-only with `O_DIRECTORY | O_NONBLOCK`, so that an item that
///   is no folder (a FIFO put in its place) is refused at once instead of blocking the open, and
///   synced with [`sync_file`]. A file system that cannot sync a folder says so with `EINVAL`,
///   which PostgreSQL's `fsync_fname_ext` also ignores for a folder, or with an error std
///   reports as `io::ErrorKind::Unsupported` (`EOPNOTSUPP`, `ENOSYS`); there is nothing more
///   this can do, and it returns `Ok`. A folder that can be written to and entered but not read
///   (write and execute only, as the macOS `~/Public/Drop Box`) cannot be opened, and fails with
///   `PermissionDenied`.
/// - Windows: the folder is opened with write access and `FILE_FLAG_BACKUP_SEMANTICS`, without
///   which no folder handle can be had, and flushed (`FlushFileBuffers`); an item that turns
///   out to be no folder is refused with `NotADirectory`, as on Unix. NTFS writes its log
///   lazily, so without the flush a power loss can keep a later change (the delete of a moved
///   source) and lose the new entry. A file system that cannot flush a folder answers the flush
///   with `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED` or `ERROR_INVALID_PARAMETER` and gets
///   `Ok`, as `EINVAL` does on Unix. Every other error is returned, one of the open too.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    imp::sync_dir(path)
}

/// Opens the regular file at `path` for reading, following links as an open does. Anything
/// else is refused with `io::ErrorKind::Unsupported` ("not a regular file") without blocking.
///
/// - Unix: the open uses `O_NONBLOCK`, so the read end of a FIFO without a writer (one put in
///   the place of a file after the file was found) opens at once instead of waiting for a
///   writer forever. `fstat` of the open file then refuses anything but a regular file (a FIFO,
///   a socket, a device, a folder), and `O_NONBLOCK` is cleared again, so that the file reads
///   as after a plain open. `O_NOCTTY` keeps a terminal from becoming the process's own.
/// - Windows: a plain open. A folder cannot be opened this way there, and no FIFO lives in a
///   folder of a file system.
pub fn open_regular_file(path: &Path) -> io::Result<File> {
    imp::open_regular_file(path)
}

/// The macOS `errno` values with which `fcntl(F_FULLFSYNC)` says that the file system cannot
/// make a full sync: `ENOTSUP` (45), `EINVAL` (22) and `ENOTTY` (25).
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used on macOS; tested on every OS")
)]
const NO_FULL_SYNC: [i32; 3] = [45, 22, 25];

/// The result of a sync that tried a full sync first, `full`: when that failed with one of the
/// [`NO_FULL_SYNC`] errors, the result of `plain`, a plain `fsync`, instead.
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used on macOS; tested on every OS")
)]
fn full_or_plain_sync(
    full: io::Result<()>,
    plain: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match full {
        Err(error)
            if error
                .raw_os_error()
                .is_some_and(|code| NO_FULL_SYNC.contains(&code)) =>
        {
            plain()
        }
        other => other,
    }
}

/// The Windows error codes with which `FlushFileBuffers` says that the file system cannot flush
/// a folder: `ERROR_INVALID_FUNCTION` (1), `ERROR_NOT_SUPPORTED` (50) and
/// `ERROR_INVALID_PARAMETER` (87).
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows; tested on every OS")
)]
const NO_FOLDER_FLUSH: [i32; 3] = [1, 50, 87];

/// The result of a folder flush on Windows: `Ok` when `flushed` failed only because the file
/// system cannot flush a folder ([`NO_FOLDER_FLUSH`]), else `flushed` itself.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows; tested on every OS")
)]
fn folder_flushed(flushed: io::Result<()>) -> io::Result<()> {
    match flushed {
        Err(error)
            if error
                .raw_os_error()
                .is_some_and(|code| NO_FOLDER_FLUSH.contains(&code)) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// The result of a folder sync on Unix: `Ok` when `synced` failed only because the file system
/// cannot sync a folder (`EINVAL`, or an error std reports as `io::ErrorKind::Unsupported`),
/// else `synced` itself.
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "used on Unix; tested on every OS")
)]
fn folder_synced(synced: io::Result<()>) -> io::Result<()> {
    match synced {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use rustix::fs::{CWD, FileType, Mode, OFlags, RenameFlags, renameat_with};

    use super::folder_synced;

    #[cfg(target_os = "macos")]
    pub fn sync_file(file: &File) -> io::Result<()> {
        let full = rustix::fs::fcntl_fullfsync(file).map_err(io::Error::from);
        super::full_or_plain_sync(full, || rustix::fs::fsync(file).map_err(io::Error::from))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn sync_file(file: &File) -> io::Result<()> {
        file.sync_all()
    }

    pub fn sync_dir(path: &Path) -> io::Result<()> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let dir = File::from(rustix::fs::open(path, flags, Mode::empty())?);
        folder_synced(sync_file(&dir))
    }

    pub fn open_regular_file(path: &Path) -> io::Result<File> {
        let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
        let fd = rustix::fs::open(path, flags, Mode::empty())?;
        let stat = rustix::fs::fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "not a regular file",
            ));
        }
        let status = rustix::fs::fcntl_getfl(&fd)?;
        rustix::fs::fcntl_setfl(&fd, status.difference(OFlags::NONBLOCK))?;
        Ok(File::from(fd))
    }

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

    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, MOVE_FILE_FLAGS, MoveFileExW,
    };
    use windows::core::PCWSTR;

    use super::folder_flushed;
    use crate::win_util::{to_io, wide};

    pub fn sync_file(file: &File) -> io::Result<()> {
        file.sync_all()
    }

    pub fn sync_dir(path: &Path) -> io::Result<()> {
        let dir = OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)?;
        // The flag opens a file too; only a folder is synced here, as `O_DIRECTORY` does.
        if !dir.metadata()?.is_dir() {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        folder_flushed(dir.sync_all())
    }

    pub fn open_regular_file(path: &Path) -> io::Result<File> {
        File::open(path)
    }

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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::{NO_FOLDER_FLUSH, NO_FULL_SYNC, folder_flushed, folder_synced, full_or_plain_sync};

    #[test]
    fn a_file_system_without_full_syncs_gets_a_plain_one() {
        for code in NO_FULL_SYNC {
            let plain = Cell::new(false);
            let synced = full_or_plain_sync(Err(io::Error::from_raw_os_error(code)), || {
                plain.set(true);
                Ok(())
            });
            synced.unwrap();
            assert!(plain.get(), "errno {code}");
        }
        // The plain sync's own failure is the result.
        let failed = full_or_plain_sync(Err(io::Error::from_raw_os_error(45)), || {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(failed.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_full_sync_that_worked_or_failed_otherwise_is_the_result() {
        let unused = || -> io::Result<()> { panic!("no plain sync after this") };
        full_or_plain_sync(Ok(()), unused).unwrap();
        // EIO (5) is a failure of the disk, never a reason to sync less.
        let error = full_or_plain_sync(Err(io::Error::from_raw_os_error(5)), unused).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        let error = full_or_plain_sync(Err(io::Error::other("other")), unused).unwrap_err();
        assert_eq!(error.to_string(), "other");
    }

    #[test]
    fn only_a_file_system_that_cannot_flush_folders_counts_as_flushed() {
        for code in NO_FOLDER_FLUSH {
            folder_flushed(Err(io::Error::from_raw_os_error(code))).unwrap();
        }
        folder_flushed(Ok(())).unwrap();
        // ERROR_FILE_NOT_FOUND (2) and ERROR_ACCESS_DENIED (5) are failures.
        for code in [2, 5] {
            let error = folder_flushed(Err(io::Error::from_raw_os_error(code))).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
        }
    }

    #[test]
    fn only_a_file_system_that_cannot_sync_folders_counts_as_synced() {
        for kind in [io::ErrorKind::InvalidInput, io::ErrorKind::Unsupported] {
            folder_synced(Err(io::Error::from(kind))).unwrap();
        }
        folder_synced(Ok(())).unwrap();
        let error = folder_synced(Err(io::Error::from(io::ErrorKind::PermissionDenied)));
        assert_eq!(error.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    /// The numbers are the ones the systems define.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_full_sync_errors_are_the_macos_ones() {
        assert_eq!(NO_FULL_SYNC, [libc::ENOTSUP, libc::EINVAL, libc::ENOTTY]);
    }

    #[cfg(windows)]
    #[test]
    fn the_folder_flush_errors_are_the_windows_ones() {
        use windows::Win32::Foundation::{
            ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
        };

        let codes = [
            ERROR_INVALID_FUNCTION,
            ERROR_NOT_SUPPORTED,
            ERROR_INVALID_PARAMETER,
        ]
        .map(|code| i32::try_from(code.0).unwrap());
        assert_eq!(NO_FOLDER_FLUSH, codes);
    }
}
