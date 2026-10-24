//! File-system operations that std does not offer in a safe form: a rename that never
//! replaces, syncs that make files and folders durable on every system, and an open for reading
//! that never blocks on an item that is no regular file.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;
use std::time::Duration;

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
/// - Windows: the folder is opened with `FILE_FLAG_BACKUP_SEMANTICS`, without which no folder
///   handle can be had, and with only the access the flush needs: `FILE_WRITE_DATA`, or
///   `FILE_APPEND_DATA` where that is denied (the default ACL of `C:\` grants users only the
///   second there), never the whole write access or `MAXIMUM_ALLOWED`; `FILE_READ_ATTRIBUTES`
///   comes with either, for the check that the item is a folder. It is shared for reading,
///   writing and deleting, as std opens every file. The folder is then flushed
///   (`FlushFileBuffers`); an item that turns out to be no folder is refused with
///   `NotADirectory`, as on Unix. NTFS writes its log lazily, so without the flush a power
///   loss can keep a later change (the delete of a moved source) and lose the new entry. An
///   open denied both rights fails with `PermissionDenied`. A file system that cannot flush a
///   folder answers the flush with `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED` or
///   `ERROR_INVALID_PARAMETER` and gets `Ok`, as `EINVAL` does on Unix. Every other error is
///   returned, one of the open too.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    imp::sync_dir(path)
}

/// Opens the regular file at `path` for reading, following links as an open does. Anything
/// else is refused without blocking, with [`not_a_regular_file`] (`io::ErrorKind::Unsupported`,
/// "not a regular file"), which [`is_not_a_regular_file`] tells from every other error.
///
/// - Unix: the open uses `O_NONBLOCK`, so the read end of a FIFO without a writer (one put in
///   the place of a file after the file was found) opens at once instead of waiting for a
///   writer forever. `fstat` of the open file then refuses anything but a regular file (a FIFO,
///   a socket, a device, a folder), and `O_NONBLOCK` is cleared again, so that the file reads
///   as after a plain open. `O_NOCTTY` keeps a terminal from becoming the process's own.
///
///   A file another program holds a write lease on (on Linux, Samba with kernel oplocks or the
///   NFS server's delegations) fails such an open with `EAGAIN` until that program lets go of
///   it, which the failed open asks it to. The open is tried again after a short wait, each
///   wait longer, for about 2 s in all; a file still held then fails with
///   `io::ErrorKind::ResourceBusy` ("the file is locked by another program").
/// - Windows: a plain open. A folder cannot be opened this way there, and no FIFO lives in a
///   folder of a file system.
pub fn open_regular_file(path: &Path) -> io::Result<File> {
    imp::open_regular_file(path)
}

/// How many milliseconds [`open_regular_file`] waits before each new try to open a file that
/// another program holds a lease on: about 2 s in all, short at first, since a program that
/// lets go at once lets go within milliseconds.
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "used on Unix; tested on every OS")
)]
const LEASE_WAITS_MS: [u64; 8] = [10, 20, 50, 100, 200, 400, 600, 600];

/// Opens with `open`, and while it fails with `EAGAIN` (`io::ErrorKind::WouldBlock`), waits
/// with `wait` for each of [`LEASE_WAITS_MS`] in turn and tries again. Another error is
/// returned as it is; an `EAGAIN` after the last wait gives `io::ErrorKind::ResourceBusy`.
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "used on Unix; tested on every OS")
)]
fn open_waiting_out_leases<T>(
    mut open: impl FnMut() -> io::Result<T>,
    mut wait: impl FnMut(Duration),
) -> io::Result<T> {
    let mut waits = LEASE_WAITS_MS.iter();
    loop {
        match open() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => match waits.next() {
                Some(&ms) => wait(Duration::from_millis(ms)),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::ResourceBusy,
                        "the file is locked by another program, which did not let go of it \
                         within 2 s",
                    ));
                }
            },
            opened => return opened,
        }
    }
}

/// The error with which [`open_regular_file`] refuses an item that is no regular file:
/// `io::ErrorKind::Unsupported`, reading "not a regular file". A provider that refuses such an
/// item itself gives it too, so that a caller can tell the refusal from every other
/// `Unsupported` with [`is_not_a_regular_file`].
pub fn not_a_regular_file() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, NotARegularFile)
}

/// Whether `error` is the refusal of an item that is no regular file
/// ([`not_a_regular_file`]), not only of its kind: a provider that cannot read at all gives
/// `Unsupported` too.
pub fn is_not_a_regular_file(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync>::is::<NotARegularFile>)
}

/// The inner error of [`not_a_regular_file`], which marks it.
#[derive(Debug)]
struct NotARegularFile;

impl fmt::Display for NotARegularFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a regular file")
    }
}

impl std::error::Error for NotARegularFile {}

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

/// The access rights [`sync_dir`] opens a folder with on Windows, in the order it tries them:
/// `FILE_WRITE_DATA` (2, for a folder the right to add a file), then `FILE_APPEND_DATA` (4,
/// the right to add a subfolder). `FlushFileBuffers` needs one of the two, and a user can have
/// the second without the first, as on `C:\`. Each comes with `FILE_READ_ATTRIBUTES` (0x80),
/// which the check that the item is a folder needs and `CreateFileW` adds to every open anyway.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows; tested on every OS")
)]
const FOLDER_FLUSH_ACCESS: [u32; 2] = [0x2 | 0x80, 0x4 | 0x80];

/// `ERROR_ACCESS_DENIED`, the Windows error code of an open the folder's ACL refuses.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows; tested on every OS")
)]
const ACCESS_DENIED: i32 = 5;

/// Opens a folder for its flush with `open`, given the access rights to ask for: with the
/// first of [`FOLDER_FLUSH_ACCESS`], and with the second when the first is denied
/// (`ERROR_ACCESS_DENIED`). Any other result of the first open is the result.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows; tested on every OS")
)]
fn open_for_flush<T>(mut open: impl FnMut(u32) -> io::Result<T>) -> io::Result<T> {
    let [write, append] = FOLDER_FLUSH_ACCESS;
    match open(write) {
        Err(error) if error.raw_os_error() == Some(ACCESS_DENIED) => open(append),
        opened => opened,
    }
}

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
        let fd = super::open_waiting_out_leases(
            || rustix::fs::open(path, flags, Mode::empty()).map_err(io::Error::from),
            std::thread::sleep,
        )?;
        let stat = rustix::fs::fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(super::not_a_regular_file());
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
        // Not `write(true)`: its `GENERIC_WRITE` needs every write right on the folder, which
        // a user does not have on `C:\`. The share mode stays std's, read, write and delete.
        let dir = super::open_for_flush(|access| {
            OpenOptions::new()
                .access_mode(access)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
                .open(path)
        })?;
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
    use std::time::Duration;

    use super::{
        ACCESS_DENIED, FOLDER_FLUSH_ACCESS, LEASE_WAITS_MS, NO_FOLDER_FLUSH, NO_FULL_SYNC,
        folder_flushed, folder_synced, full_or_plain_sync, is_not_a_regular_file,
        not_a_regular_file, open_for_flush, open_waiting_out_leases,
    };

    /// The waits [`open_waiting_out_leases`] makes for the open results `results`, which it
    /// takes in turn, and its result.
    fn waits_for(results: Vec<io::Result<()>>) -> (Vec<Duration>, io::Result<()>) {
        let mut results = results.into_iter();
        let mut waits = Vec::new();
        let opened = open_waiting_out_leases(
            || results.next().expect("no more tries than results"),
            |wait| waits.push(wait),
        );
        (waits, opened)
    }

    fn would_block() -> io::Result<()> {
        Err(io::ErrorKind::WouldBlock.into())
    }

    /// The `EAGAIN` of a held file is the error kind waited out.
    #[cfg(unix)]
    #[test]
    fn eagain_is_waited_out() {
        let eagain = io::Error::from(rustix::io::Errno::AGAIN);
        assert_eq!(eagain.kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn a_file_let_go_of_soon_is_opened_after_short_waits() {
        let (waits, opened) = waits_for(vec![would_block(), would_block(), Ok(())]);
        opened.unwrap();
        assert_eq!(
            waits,
            [Duration::from_millis(10), Duration::from_millis(20)]
        );
    }

    #[test]
    fn a_file_held_for_good_fails_as_locked_after_about_two_seconds() {
        let mut results: Vec<io::Result<()>> = Vec::new();
        results.resize_with(LEASE_WAITS_MS.len() + 1, would_block);
        let (waits, opened) = waits_for(results);
        let error = opened.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert!(
            error.to_string().contains("locked by another program"),
            "{error}"
        );
        let waited: Duration = waits.iter().sum();
        assert!(
            (Duration::from_millis(1500)..=Duration::from_millis(2500)).contains(&waited),
            "{waited:?}"
        );
        assert!(waits.is_sorted(), "{waits:?}");
    }

    /// Only `EAGAIN` is waited out; every other failure is the result at once, as it was.
    #[test]
    fn only_a_held_file_is_tried_again() {
        let (waits, opened) = waits_for(vec![Ok(())]);
        assert_eq!((waits, opened.is_ok()), (Vec::new(), true));
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
            let (waits, opened) = waits_for(vec![Err(io::Error::from(kind))]);
            assert_eq!((waits, opened.unwrap_err().kind()), (Vec::new(), kind));
        }
        let refused = waits_for(vec![Err(not_a_regular_file())]).1.unwrap_err();
        assert!(is_not_a_regular_file(&refused));
    }

    /// The refusal of an item that is no regular file reads as it always did, and is told from
    /// every other `Unsupported` error.
    #[test]
    fn the_refusal_of_an_item_that_is_no_file_is_told_apart() {
        let refused = not_a_regular_file();
        assert_eq!(refused.kind(), io::ErrorKind::Unsupported);
        assert_eq!(refused.to_string(), "not a regular file");
        assert!(is_not_a_regular_file(&refused));
        let others = [
            io::Error::from(io::ErrorKind::Unsupported),
            io::Error::new(io::ErrorKind::Unsupported, "not a regular file"),
            io::Error::from_raw_os_error(95),
        ];
        for other in others {
            assert!(!is_not_a_regular_file(&other), "{other}");
        }
    }

    /// A folder whose write right is denied is opened with the append right instead, as on
    /// `C:\`; nothing broader is ever asked for.
    #[test]
    fn a_folder_without_the_write_right_is_flushed_with_the_append_right() {
        let mut tried = Vec::new();
        let opened = open_for_flush(|access| {
            tried.push(access);
            if access == FOLDER_FLUSH_ACCESS[0] {
                Err(io::Error::from_raw_os_error(ACCESS_DENIED))
            } else {
                Ok(access)
            }
        });
        assert_eq!(opened.unwrap(), FOLDER_FLUSH_ACCESS[1]);
        assert_eq!(tried, FOLDER_FLUSH_ACCESS);
        // FILE_WRITE_DATA (2) or FILE_APPEND_DATA (4), each with FILE_READ_ATTRIBUTES (0x80).
        assert_eq!(FOLDER_FLUSH_ACCESS, [0x82, 0x84]);
    }

    /// An open that works, or fails for another reason than the ACL (`ERROR_FILE_NOT_FOUND`,
    /// `ERROR_SHARING_VIOLATION`), is not tried again.
    #[test]
    fn a_folder_open_is_tried_again_only_when_access_is_denied() {
        let mut tried = Vec::new();
        open_for_flush(|access| {
            tried.push(access);
            Ok(())
        })
        .unwrap();
        assert_eq!(tried, [FOLDER_FLUSH_ACCESS[0]]);
        for code in [2, 32] {
            let mut tried = Vec::new();
            let opened: io::Result<()> = open_for_flush(|access| {
                tried.push(access);
                Err(io::Error::from_raw_os_error(code))
            });
            assert_eq!(opened.unwrap_err().raw_os_error(), Some(code));
            assert_eq!(tried, [FOLDER_FLUSH_ACCESS[0]]);
        }
        // A second denial is the result.
        let denied: io::Result<()> =
            open_for_flush(|_| Err(io::Error::from_raw_os_error(ACCESS_DENIED)));
        assert_eq!(denied.unwrap_err().raw_os_error(), Some(ACCESS_DENIED));
    }

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

    #[cfg(windows)]
    #[test]
    fn the_folder_flush_rights_are_the_windows_ones() {
        use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
        use windows::Win32::Storage::FileSystem::{
            FILE_APPEND_DATA, FILE_READ_ATTRIBUTES, FILE_WRITE_DATA,
        };

        let rights =
            [FILE_WRITE_DATA, FILE_APPEND_DATA].map(|right| right.0 | FILE_READ_ATTRIBUTES.0);
        assert_eq!(FOLDER_FLUSH_ACCESS, rights);
        assert_eq!(ACCESS_DENIED, i32::try_from(ERROR_ACCESS_DENIED.0).unwrap());
    }
}
