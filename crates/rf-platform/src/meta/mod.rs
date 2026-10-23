//! Times, attributes and the OS-specific extras of a file or folder, for copies that keep them
//! (XFER-11).

use std::time::SystemTime;

use rf_core::{Attributes, Error, Result, VPath};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
use self::windows as imp;

/// Sets the times of the item at `path`; a time that is `None` stays as it is.
///
/// Works on files and folders, and does not follow a final link. The creation time exists on
/// Windows and macOS only; elsewhere `created` is ignored. If all three times are `None`,
/// nothing is opened. A non-local path gives `Error::Unsupported` and changes nothing.
pub fn set_times(
    path: &VPath,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    created: Option<SystemTime>,
) -> Result<()> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "setting times needs a local path: {path}"
        )));
    };
    if modified.is_none() && accessed.is_none() && created.is_none() {
        return Ok(());
    }
    imp::set_times(&local, modified, accessed, created)
        .map_err(|e| Error::io("set_times", path.clone(), e))
}

/// Makes the read-only, hidden, system and archive state of the item at `path` exactly what
/// `attributes` says, as far as the OS keeps it; the other attribute bits are ignored.
///
/// - Windows: the `READONLY`, `HIDDEN`, `SYSTEM` and `ARCHIVE` bits become exactly as given.
///   Other bits the user or the system set (not content indexed, temporary, offline, pinned,
///   unpinned) stay as they are.
/// - Unix: `READONLY` clears every write bit of the mode, and without it the owner's write bit
///   is turned on, even where the mode never had it (so a copy of such a file becomes writable
///   for its owner; its content is the same). The other mode bits stay. A symbolic link keeps
///   its mode, because changing it would change what the link points to.
/// - macOS, in addition: `HIDDEN` sets the `UF_HIDDEN` flag and its absence clears it. An item
///   whose name starts with a dot is hidden by its name, so for it the flag is left as it is
///   (`copy_extras` carries the flag itself).
///
/// A non-local path gives `Error::Unsupported` and changes nothing.
pub fn set_attributes(path: &VPath, attributes: Attributes) -> Result<()> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "setting attributes needs a local path: {path}"
        )));
    };
    imp::set_attributes(&local, attributes)
        .map_err(|e| Error::io("set_attributes", path.clone(), e))
}

/// Copies the OS-specific data of `from` that neither the content nor `set_times` and
/// `set_attributes` carry over to `to`, which must exist.
///
/// - Unix: the extended attributes (on Linux only those in the `user.` namespace, on macOS
///   all), read and written without following a final link, then the permission bits
///   (`mode & 0o777`; not between links, whose mode changes would follow them). Writing an
///   attribute needs write access, which a read-only mode would take away, so the mode comes
///   last. An attribute the OS refuses to read or write with `PermissionDenied` (a
///   system-protected one) is skipped. A source file system without extended attributes has
///   none to copy.
/// - macOS, in addition: the `UF_HIDDEN` flag.
/// - Windows: every alternate data stream (`Zone.Identifier` among them), not the main
///   `::$DATA` stream. A source file system that cannot list streams (FAT, exFAT) has none to
///   copy; a stream that cannot be made on `to` is an error.
///
/// A failure does not stop the copy of the rest; the first one is returned. A non-local path
/// gives `Error::Unsupported` and changes nothing.
pub fn copy_extras(from: &VPath, to: &VPath) -> Result<()> {
    let (Some(source), Some(target)) = (from.to_extended_length(), to.to_extended_length()) else {
        return Err(Error::Unsupported(format!(
            "copying extras needs local paths: {from} to {to}"
        )));
    };
    imp::copy_extras(from, to, &source, &target)
}

/// Keeps the first of several errors.
fn keep_first(first: &mut Option<Error>, error: Error) {
    if first.is_none() {
        *first = Some(error);
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::{Metadata, Permissions};
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::SystemTime;

    use rf_core::{Attributes, Error, Result, VPath};
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, UTIME_OMIT, XattrFlags};
    use rustix::io::Errno;

    use super::keep_first;

    pub fn set_times(
        local: &Path,
        modified: Option<SystemTime>,
        accessed: Option<SystemTime>,
        created: Option<SystemTime>,
    ) -> io::Result<()> {
        if modified.is_some() || accessed.is_some() {
            // `utimensat` needs no read access (unlike opening the file) and, with
            // `SYMLINK_NOFOLLOW`, changes a final link itself.
            let times = Timestamps {
                last_access: timespec(accessed)?,
                last_modification: timespec(modified)?,
            };
            rustix::fs::utimensat(CWD, local, &times, AtFlags::SYMLINK_NOFOLLOW)?;
        }
        // After the modification time: macOS moves a creation time that is later than a new
        // modification time back to it.
        #[cfg(target_os = "macos")]
        if let Some(created) = created {
            set_created(local, created)?;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = created;
        Ok(())
    }

    /// `time` for `utimensat`, or `UTIME_OMIT` to leave that time alone.
    fn timespec(time: Option<SystemTime>) -> io::Result<Timespec> {
        let out_of_range = |_| io::Error::new(io::ErrorKind::InvalidInput, "time out of range");
        match time.map(|time| time.duration_since(SystemTime::UNIX_EPOCH)) {
            None => Ok(Timespec {
                tv_sec: 0,
                tv_nsec: UTIME_OMIT,
            }),
            Some(Ok(after)) => Timespec::try_from(after).map_err(out_of_range),
            Some(Err(before)) => Timespec::try_from(before.duration())
                .map(|before| -before)
                .map_err(out_of_range),
        }
    }

    /// The creation time through `fsetattrlist`, which std uses on macOS. `O_SYMLINK` opens a
    /// final link itself, as `utimensat` changes it.
    #[cfg(target_os = "macos")]
    fn set_created(local: &Path, created: SystemTime) -> io::Result<()> {
        use std::fs::{FileTimes, OpenOptions};
        use std::os::macos::fs::FileTimesExt;
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_SYMLINK)
            .open(local)?
            .set_times(FileTimes::new().set_created(created))
    }

    pub fn set_attributes(local: &Path, attributes: Attributes) -> io::Result<()> {
        let meta = std::fs::symlink_metadata(local)?;
        // `chmod` follows links and Linux has no `lchmod`, so a link keeps its mode.
        if !meta.file_type().is_symlink() {
            let mode = meta.permissions().mode() & 0o7777;
            let wanted = if attributes.contains(Attributes::READONLY) {
                mode & !0o222
            } else {
                mode | 0o200
            };
            if wanted != mode {
                std::fs::set_permissions(local, Permissions::from_mode(wanted))?;
            }
        }
        #[cfg(target_os = "macos")]
        if !local.file_name().is_some_and(crate::attrs::is_dot_file) {
            super::macos::set_hidden_flag(local, attributes.contains(Attributes::HIDDEN))?;
        }
        Ok(())
    }

    pub fn copy_extras(from: &VPath, to: &VPath, source: &Path, target: &Path) -> Result<()> {
        let at_source = |e| Error::io("copy_extras", from.clone(), e);
        let at_target = |e| Error::io("copy_extras", to.clone(), e);
        let source_meta = std::fs::symlink_metadata(source).map_err(at_source)?;
        let target_meta = std::fs::symlink_metadata(target).map_err(at_target)?;
        let mut first = None;
        copy_xattrs(source, target, &mut |side, e| {
            let error = match side {
                Side::Source => at_source(e),
                Side::Target => at_target(e),
            };
            keep_first(&mut first, error);
        });
        #[cfg(target_os = "macos")]
        if let Err(e) = super::macos::copy_hidden_flag(&source_meta, target) {
            keep_first(&mut first, at_target(e));
        }
        if let Err(e) = copy_mode(&source_meta, &target_meta, target) {
            keep_first(&mut first, at_target(e));
        }
        first.map_or(Ok(()), Err)
    }

    /// Gives `target` the permission bits of `source`, unless either is a link.
    fn copy_mode(source: &Metadata, target: &Metadata, local: &Path) -> io::Result<()> {
        if source.file_type().is_symlink() || target.file_type().is_symlink() {
            return Ok(());
        }
        let mode = source.permissions().mode() & 0o777;
        if target.permissions().mode() & 0o777 == mode {
            return Ok(());
        }
        std::fs::set_permissions(local, Permissions::from_mode(mode))
    }

    /// Which side of a copy an error belongs to.
    enum Side {
        Source,
        Target,
    }

    /// Copies the extended attributes of `source` to `target` and hands every failure to
    /// `fail`, except a refused write or read of one attribute (`PermissionDenied`), which is
    /// skipped.
    fn copy_xattrs(source: &Path, target: &Path, fail: &mut dyn FnMut(Side, io::Error)) {
        let names = match xattr_names(source) {
            Ok(names) => names,
            // The file system keeps no extended attributes, so there are none to copy.
            Err(e) if e == Errno::NOTSUP || e == Errno::OPNOTSUPP => return,
            Err(e) => return fail(Side::Source, e.into()),
        };
        for name in names.split(|&byte| byte == 0) {
            if name.is_empty() || !copied(name) {
                continue;
            }
            let copy = read_xattr(source, name)
                .map_err(|e| (Side::Source, io::Error::from(e)))
                .and_then(|value| {
                    rustix::fs::lsetxattr(target, name, &value, XattrFlags::empty())
                        .map_err(|e| (Side::Target, io::Error::from(e)))
                });
            match copy {
                Ok(()) => {}
                // A system-protected attribute; the others are still copied.
                Err((_, e)) if e.kind() == io::ErrorKind::PermissionDenied => {}
                Err((side, e)) => fail(side, e),
            }
        }
    }

    /// Whether the attribute `name` is copied: on Linux only the `user.` namespace, whose
    /// attributes belong to the file's owner; on macOS every one.
    fn copied(name: &[u8]) -> bool {
        cfg!(target_os = "macos") || name.starts_with(b"user.")
    }

    /// The NUL-separated names of the extended attributes of `local`, without following a
    /// final link.
    fn xattr_names(local: &Path) -> rustix::io::Result<Vec<u8>> {
        let size = rustix::fs::llistxattr(local, &mut [0u8; 0])?;
        let mut names = vec![0; size];
        if size > 0 {
            let len = rustix::fs::llistxattr(local, &mut names[..])?;
            names.truncate(len);
        }
        Ok(names)
    }

    /// The value of the extended attribute `name` of `local`, without following a final link.
    fn read_xattr(local: &Path, name: &[u8]) -> rustix::io::Result<Vec<u8>> {
        let size = rustix::fs::lgetxattr(local, name, &mut [0u8; 0])?;
        let mut value = vec![0; size];
        if size > 0 {
            let len = rustix::fs::lgetxattr(local, name, &mut value[..])?;
            value.truncate(len);
        }
        Ok(value)
    }
}
