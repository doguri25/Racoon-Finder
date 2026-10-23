//! Renaming, deleting, links, metadata, and the read and write streams.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};

use rf_core::{Error, ErrorKind, Result, VPath};

use super::native;
use crate::names::fold_name;
use crate::provider::{
    DeleteMode, Deleted, LinkTarget, MetadataPatch, ReadStream, RenameMode, WriteMode, WriteStream,
};

/// Renames or moves `from` to `to`; see `Provider::rename`.
///
/// A folder is never moved into itself, whether `to` names it by the same text or by another
/// path (`lands_inside`). `RenameMode::NoReplace` goes through
/// `rf_platform::rename_no_replace`, so an existing target is never overwritten. The one thing
/// it cannot do is change only the case of a name on a volume that ignores case (`b.txt` to
/// `B.TXT`), because the OS then sees the target as taken; that case is renamed plainly after
/// checking that both names are the same item.
pub(super) fn rename(from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
    if (to != from && to.starts_with(from)) || lands_inside(from, to) {
        return Err(Error::Unsupported(format!(
            "cannot move {from} into itself"
        )));
    }
    match mode {
        RenameMode::Replace => rename_plain(from, to),
        RenameMode::NoReplace => match rf_platform::rename_no_replace(from, to) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists && is_case_only_rename(from, to) => {
                rename_plain(from, to)
            }
            result => result,
        },
    }
}

/// Whether `to` lies inside the folder `from` although its text does not start with the text of
/// `from`: another letter case on a volume that ignores case (`C:\x\a\sub` for the folder
/// `C:\x\A`), an 8.3 short name, or a link or junction that leads to `from` (`alias/sub`).
///
/// Every folder above `to` that exists is compared with `from` by file identity. Those folders
/// are followed through links, as the OS follows them when it puts `to` in place; `from` is
/// not, because a link is moved itself and may go anywhere. Only a folder can be moved into
/// itself, and a rename within one folder never is, so a file or a rename in place costs one
/// `stat` at most. An identity that cannot be read proves nothing and decides nothing: the OS
/// then has the last word, and it refuses a move into itself too, with a less telling error.
/// So does a link that leads into a subfolder of `from`, which this walk does not see.
fn lands_inside(from: &VPath, to: &VPath) -> bool {
    if from.parent() == to.parent() {
        return false;
    }
    let is_folder = native(from)
        .ok()
        .and_then(|local| std::fs::symlink_metadata(local).ok())
        .is_some_and(|meta| meta.is_dir());
    if !is_folder {
        return false;
    }
    let Ok(folder) = rf_platform::file_identity(from) else {
        return false;
    };
    std::iter::successors(to.parent(), VPath::parent)
        .any(|dir| rf_platform::resolved_file_identity(&dir).is_ok_and(|id| id == folder))
}

/// `std::fs::rename`, which replaces an existing file at `to`.
fn rename_plain(from: &VPath, to: &VPath) -> Result<()> {
    std::fs::rename(native(from)?, native(to)?).map_err(|e| Error::io("rename", from.clone(), e))
}

/// Whether `to` is `from` in the same folder with only the letter case changed, and both names
/// already lead to one item. Only then is a taken target the item itself, not another one.
///
/// An identity that cannot be read, on a file system without usable file IDs too, counts as
/// two different items, so the rename keeps failing with `AlreadyExists`.
fn is_case_only_rename(from: &VPath, to: &VPath) -> bool {
    let same_folder = from.parent() == to.parent();
    let same_name_ignoring_case = matches!(
        (from.file_name(), to.file_name()),
        (Some(a), Some(b)) if fold_name(a) == fold_name(b)
    );
    same_folder
        && same_name_ignoring_case
        && matches!(
            (
                rf_platform::file_identity(from),
                rf_platform::file_identity(to)
            ),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Moves `path` to the trash, or removes it for good; see `Provider::delete`.
pub(super) fn delete(path: &VPath, mode: DeleteMode) -> Result<Deleted> {
    match mode {
        DeleteMode::Trash => {
            // A failure is returned as it is; the item is never deleted permanently instead.
            let location = rf_platform::move_to_trash(path)?;
            Ok(Deleted::Trashed {
                location: location.and_then(|p| VPath::local(p).ok()),
            })
        }
        DeleteMode::Permanent => delete_permanently(path).map(|()| Deleted::Permanently),
    }
}

/// Removes a file, a folder with everything in it, or only a link. The kind is read without
/// following links, so the target of a link is never touched.
fn delete_permanently(path: &VPath) -> Result<()> {
    let local = native(path)?;
    let fail = |e| Error::io("delete", path.clone(), e);
    let file_type = std::fs::symlink_metadata(&local).map_err(fail)?.file_type();
    if file_type.is_symlink() {
        // A folder link or junction on Windows is removed like a folder, and `remove_dir`
        // removes only the link.
        #[cfg(windows)]
        if std::os::windows::fs::FileTypeExt::is_symlink_dir(&file_type) {
            return std::fs::remove_dir(&local).map_err(fail);
        }
        std::fs::remove_file(&local).map_err(fail)
    } else if file_type.is_dir() {
        std::fs::remove_dir_all(&local).map_err(fail)
    } else {
        std::fs::remove_file(&local).map_err(fail)
    }
}

/// Removes the empty folder `path`; see `Provider::remove_dir`. The OS itself refuses a folder
/// with anything in it and a file, and does not follow a final link: Unix refuses a link, and
/// Windows removes a folder link or junction itself.
pub(super) fn remove_dir(path: &VPath) -> Result<()> {
    std::fs::remove_dir(native(path)?).map_err(|e| Error::io("remove_dir", path.clone(), e))
}

/// Makes the changes to the entries of the folder `path` durable; see `Provider::sync_dir`.
///
/// On Unix the folder is opened for reading and synced (`fsync`, `F_FULLFSYNC` on macOS). A
/// file system that cannot sync a folder says so with `EINVAL`, which PostgreSQL's
/// `fsync_fname_ext` also ignores for a folder (with `EBADF`), or with an error that std
/// reports as `ErrorKind::Unsupported` (`EOPNOTSUPP`, `ENOSYS`); then there is nothing more
/// this can do, and it returns `Ok`. The macOS `ENOTSUP` (45, not `EOPNOTSUPP`, 102) is
/// neither, and fails: a move then keeps its source. So does a folder that can be written to
/// and entered but not read (write and execute only, as the macOS `~/Public/Drop Box`): it
/// cannot be opened for reading, the sync fails with `PermissionDenied`, and a move across
/// volumes into it keeps its sources.
///
/// On Windows it does nothing: NTFS keeps the changes to its folders in its journal, and
/// `File::open` cannot open a folder there.
pub(super) fn sync_dir(path: &VPath) -> Result<()> {
    let local = native(path)?;
    #[cfg(unix)]
    {
        let fail = |e| Error::io("sync_dir", path.clone(), e);
        let dir = File::open(&local).map_err(fail)?;
        match dir.sync_all() {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
                ) =>
            {
                Ok(())
            }
            synced => synced.map_err(fail),
        }
    }
    #[cfg(windows)]
    {
        let _ = local;
        Ok(())
    }
}

/// What the link at `path` leads to; see `Provider::read_link`.
pub(super) fn read_link(path: &VPath) -> Result<LinkTarget> {
    let local = native(path)?;
    let fail = |e| Error::io("read_link", path.clone(), e);
    let target = std::fs::read_link(&local).map_err(fail)?;
    // Windows makes folder links apart from file links, so the link itself tells; elsewhere
    // only what it leads to can.
    #[cfg(windows)]
    let is_dir = std::os::windows::fs::FileTypeExt::is_symlink_dir(
        &std::fs::symlink_metadata(&local).map_err(fail)?.file_type(),
    );
    #[cfg(not(windows))]
    let is_dir = std::fs::metadata(&local).is_ok_and(|meta| meta.is_dir());
    Ok(LinkTarget { target, is_dir })
}

/// Makes the link `link` to `target`; see `Provider::create_symlink`. The OS never replaces
/// an existing item with a link.
pub(super) fn create_symlink(link: &VPath, target: &LinkTarget) -> Result<()> {
    let local = native(link)?;
    #[cfg(windows)]
    let made = if target.is_dir {
        std::os::windows::fs::symlink_dir(&target.target, &local)
    } else {
        std::os::windows::fs::symlink_file(&target.target, &local)
    };
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target.target, &local);
    made.map_err(|e| Error::io("create_symlink", link.clone(), e))
}

/// Puts the times, then the attributes of `patch` on `path`; see `Provider::set_metadata`.
pub(super) fn set_metadata(path: &VPath, patch: &MetadataPatch) -> Result<()> {
    // Without any time this opens nothing, but it still refuses a path that is not local.
    rf_platform::set_times(path, patch.modified, patch.accessed, patch.created)?;
    if let Some(attributes) = patch.attributes {
        rf_platform::set_attributes(path, attributes)?;
    }
    Ok(())
}

/// Opens `path` for reading; the `File` itself is the stream.
pub(super) fn open_read(path: &VPath) -> Result<Box<dyn ReadStream>> {
    let file = File::open(native(path)?).map_err(|e| Error::io("open_read", path.clone(), e))?;
    Ok(Box::new(file))
}

/// Opens `path` for writing as `mode` says.
pub(super) fn open_write(path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
    let mut options = OpenOptions::new();
    match mode {
        WriteMode::CreateNew => options.write(true).create_new(true),
        WriteMode::Truncate => options.write(true).create(true).truncate(true),
        WriteMode::Append => options.append(true).create(true),
    };
    let file = options
        .open(native(path)?)
        .map_err(|e| Error::io("open_write", path.clone(), e))?;
    Ok(Box::new(LocalWriter {
        file,
        path: path.clone(),
    }))
}

/// A file being written. `path` is for error messages only.
#[derive(Debug)]
struct LocalWriter {
    file: File,
    path: VPath,
}

impl Write for LocalWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl WriteStream for LocalWriter {
    fn finish(mut self: Box<Self>) -> Result<()> {
        self.file
            .flush()
            .and_then(|()| self.file.sync_all())
            .map_err(|e| Error::io("finish", self.path.clone(), e))
    }
}
