//! Renaming, deleting, links, metadata, and the read and write streams.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};

use rf_core::{Error, ErrorKind, Result, VPath};

use super::native;
use crate::names::{LOCAL_GUARD, fold_name};
use crate::provider::{
    DataRanges, DeleteMode, Deleted, LinkTarget, MetadataPatch, ReadStream, RenameMode, WriteMode,
    WriteStream,
};

/// Renames or moves `from` to `to`; see `Provider::rename`.
///
/// A folder is never moved into itself, whether `to` names it by the same text or by another
/// path (`lands_inside`). `RenameMode::NoReplace` goes through
/// `rf_platform::rename_no_replace`, so an existing target is never overwritten. The one thing
/// it cannot do is change only the case of a name on a volume that ignores case (`b.txt` to
/// `B.TXT`), or its Unicode form on macOS, because the OS then sees the target as taken; that
/// case is renamed plainly after checking that both names are the same item.
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

/// Whether `to` is `from` in the same folder with only the letter case changed (or, on macOS,
/// the Unicode form: NFC and NFD, which APFS takes for one name), and both names already lead
/// to one item. Only then is a taken target the item itself, not another one.
///
/// An identity that cannot be read, on a file system without usable file IDs too, counts as
/// two different items, so the rename keeps failing with `AlreadyExists`.
fn is_case_only_rename(from: &VPath, to: &VPath) -> bool {
    let same_folder = from.parent() == to.parent();
    let same_name_ignoring_case = matches!(
        (from.file_name(), to.file_name()),
        (Some(a), Some(b)) if fold_name(a, LOCAL_GUARD) == fold_name(b, LOCAL_GUARD)
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

/// Removes one file, link or other item, never a folder tree; see `Provider::delete_leaf`.
///
/// On Unix it is one `unlink` (`std::fs::remove_file`), which removes a link itself and refuses
/// a folder (`EISDIR` on Linux, `EPERM` on macOS); a refusal of an item that is a folder is
/// reported as `IsADirectory` on every system. On Windows the kind is read first, without
/// following links: a folder link or junction is removed with `std::fs::remove_dir`, which
/// removes the link itself (and, should a real folder have taken its place in between, only an
/// empty one); a real folder is refused; anything else goes through `std::fs::remove_file`,
/// which refuses a folder too.
pub(super) fn delete_leaf(path: &VPath) -> Result<()> {
    let local = native(path)?;
    let fail = |e| Error::io("delete", path.clone(), e);
    let a_folder = || fail(io::Error::from(io::ErrorKind::IsADirectory));
    #[cfg(windows)]
    {
        let file_type = std::fs::symlink_metadata(&local).map_err(fail)?.file_type();
        if std::os::windows::fs::FileTypeExt::is_symlink_dir(&file_type) {
            return std::fs::remove_dir(&local).map_err(fail);
        }
        if file_type.is_dir() {
            return Err(a_folder());
        }
    }
    std::fs::remove_file(&local).map_err(|e| {
        let folder = std::fs::symlink_metadata(&local).is_ok_and(|meta| meta.is_dir());
        if folder { a_folder() } else { fail(e) }
    })
}

/// Removes the empty folder `path`; see `Provider::remove_dir`. The OS itself refuses a folder
/// with anything in it and a file, and does not follow a final link: Unix refuses a link, and
/// Windows removes a folder link or junction itself.
pub(super) fn remove_dir(path: &VPath) -> Result<()> {
    std::fs::remove_dir(native(path)?).map_err(|e| Error::io("remove_dir", path.clone(), e))
}

/// Makes the changes to the entries of the folder `path` durable; see `Provider::sync_dir`.
///
/// It is `rf_platform::sync_dir`: on Unix the folder is opened with `O_DIRECTORY | O_NONBLOCK`
/// (an item that is no folder, a FIFO among them, is refused at once) and synced, with the
/// macOS fallback of `rf_platform::sync_file` from `F_FULLFSYNC` to `fsync`; on Windows it is
/// opened with the one right the flush needs (`FILE_WRITE_DATA`, or `FILE_APPEND_DATA` where
/// only that is granted, as to users on `C:\`) and flushed (`FlushFileBuffers`), since NTFS
/// writes its log lazily. Only a file system that cannot sync a folder at all (`EINVAL`,
/// `EOPNOTSUPP` or `ENOSYS` on Unix; `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED` or
/// `ERROR_INVALID_PARAMETER` on Windows) counts as synced. Every other error fails, with the op
/// `"sync_dir"`, and a move then keeps its source: a folder that can be written to and entered
/// but not read (the macOS `~/Public/Drop Box`) cannot be opened for the sync, so a move across
/// volumes into it keeps its sources.
pub(super) fn sync_dir(path: &VPath) -> Result<()> {
    rf_platform::sync_dir(&native(path)?).map_err(|e| Error::io("sync_dir", path.clone(), e))
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
/// an existing item with a link. On Windows `CreateSymbolicLinkW` refuses a path a folder
/// takes with `ERROR_ACCESS_DENIED`, not `ERROR_ALREADY_EXISTS`; see [`link_refused`].
pub(super) fn create_symlink(link: &VPath, target: &LinkTarget) -> Result<()> {
    let local = native(link)?;
    #[cfg(windows)]
    let made = if target.is_dir {
        std::os::windows::fs::symlink_dir(&target.target, &local)
    } else {
        std::os::windows::fs::symlink_file(&target.target, &local)
    }
    .map_err(|e| link_refused(e, || std::fs::symlink_metadata(&local).is_ok()));
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target.target, &local);
    made.map_err(|e| Error::io("create_symlink", link.clone(), e))
}

/// The error of a link the OS would not make: `error`, except that a `PermissionDenied` while
/// something is at the link's path (`taken`) is `AlreadyExists`. Windows answers a path a
/// folder takes with `ERROR_ACCESS_DENIED`, and the path being taken is the reason the link
/// cannot be made there, as `EEXIST` says on Unix.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows; tested on every OS")
)]
fn link_refused(error: io::Error, taken: impl FnOnce() -> bool) -> io::Error {
    if error.kind() == io::ErrorKind::PermissionDenied && taken() {
        io::ErrorKind::AlreadyExists.into()
    } else {
        error
    }
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

/// Opens `path` for reading, if it is a regular file (`rf_platform::open_regular_file`): an item
/// that is no file by then, such as a FIFO put in the place of a file after the file was found,
/// fails with `ErrorKind::Unsupported` (`rf_platform::not_a_regular_file`) at once, and never
/// blocks. A file another program holds a lease on is waited for about 2 s, and then fails with
/// `ErrorKind::Busy`.
pub(super) fn open_read(path: &VPath) -> Result<Box<dyn ReadStream>> {
    let file = rf_platform::open_regular_file(&native(path)?)
        .map_err(|e| Error::io("open_read", path.clone(), e))?;
    Ok(Box::new(LocalReader {
        file,
        path: path.clone(),
    }))
}

/// A file being read. `path` is for error messages only.
#[derive(Debug)]
struct LocalReader {
    file: File,
    path: VPath,
}

impl Read for LocalReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for LocalReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl ReadStream for LocalReader {
    /// `rf_platform::file_data_ranges` of the open file, so the length and the ranges are those
    /// of the bytes this stream reads. It never moves the stream's position.
    fn data_ranges(&mut self) -> Result<Option<DataRanges>> {
        rf_platform::file_data_ranges(&self.file)
            .map_err(|e| Error::io("data_ranges", self.path.clone(), e))
    }
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
        mode,
        holes: Holes::Untried,
    }))
}

/// A file being written. `path` is for error messages only; `mode` is how it was opened.
#[derive(Debug)]
struct LocalWriter {
    file: File,
    path: VPath,
    mode: WriteMode,
    holes: Holes,
}

/// What a [`LocalWriter`] knows about leaving holes in its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holes {
    /// `write_hole` has not been called.
    Untried,
    /// The file can have holes, and has had at least one passed over.
    Made,
    /// The file system refused to mark the file sparse (FAT and exFAT on Windows).
    Refused,
}

impl LocalWriter {
    /// Makes the file as long as the position it is written at, which a hole at its end leaves
    /// past its last byte.
    fn extend_to_position(&mut self) -> io::Result<()> {
        let position = self.file.stream_position()?;
        if self.file.metadata()?.len() < position {
            self.file.set_len(position)?;
        }
        Ok(())
    }
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
    /// A file with a hole at its end is first made as long as that hole reaches (`set_len`).
    /// Only a hole moves the position past the end of the file, so a file without one is
    /// never measured. The sync is `rf_platform::sync_file`: on macOS `F_FULLFSYNC`, or a plain
    /// `fsync` on a file system that cannot do a full one (an SMB share), so that a copy to a
    /// network share from a Mac can finish at all.
    fn finish(mut self: Box<Self>) -> Result<()> {
        let extended = if self.holes == Holes::Made {
            self.extend_to_position()
        } else {
            Ok(())
        };
        extended
            .and_then(|()| self.file.flush())
            .and_then(|()| rf_platform::sync_file(&self.file))
            .map_err(|e| Error::io("finish", self.path.clone(), e))
    }

    /// The first call marks the file sparse (`rf_platform::mark_sparse`); after that, a hole
    /// is a seek past it. A file system that refuses the mark gets `Ok(false)` for this hole and
    /// every later one, and so does a file opened with `WriteMode::Append`, which writes at its
    /// end wherever it was moved to.
    fn write_hole(&mut self, len: u64) -> Result<bool> {
        if self.mode == WriteMode::Append || self.holes == Holes::Refused {
            return Ok(false);
        }
        if self.holes == Holes::Untried && rf_platform::mark_sparse(&self.file).is_err() {
            self.holes = Holes::Refused;
            return Ok(false);
        }
        let fail = |e| Error::io("write_hole", self.path.clone(), e);
        let offset = i64::try_from(len).map_err(|_| fail(io::ErrorKind::InvalidInput.into()))?;
        self.file.seek(SeekFrom::Current(offset)).map_err(fail)?;
        self.holes = Holes::Made;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::link_refused;

    /// A link refused for a path that something takes (on Windows a folder's, with
    /// `ERROR_ACCESS_DENIED`) is `AlreadyExists`.
    #[test]
    fn a_link_refused_where_something_is_already_exists() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(
            link_refused(denied, || true).kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    /// A denial at a free path stays one, and another error is kept without a look at the
    /// path (`ERROR_PRIVILEGE_NOT_HELD`, 1314, of a user who may not make links).
    #[test]
    fn a_link_refused_otherwise_keeps_its_error() {
        let kept = link_refused(io::ErrorKind::PermissionDenied.into(), || false);
        assert_eq!(kept.kind(), io::ErrorKind::PermissionDenied);
        let looked = Cell::new(false);
        let refused = link_refused(io::Error::from_raw_os_error(1314), || {
            looked.set(true);
            true
        });
        assert_eq!(refused.raw_os_error(), Some(1314));
        assert!(!looked.get());
    }
}
