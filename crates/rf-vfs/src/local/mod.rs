//! `LocalProvider`, the provider for the file systems of this computer.

mod list;
mod ops;
mod watch;

use std::ffi::OsString;
use std::fs::{FileType, Metadata, OpenOptions};
use std::ops::Range;
use std::path::{Path, PathBuf};

use rf_core::{Entry, EntryKind, Error, Result, Scheme, VPath};

use crate::cancel::CancelToken;
use crate::provider::{
    Capabilities, DeleteMode, Deleted, DiskSpace, LinkTarget, MetadataPatch, Provider, ReadStream,
    RenameMode, WriteMode, WriteStream,
};
use crate::watch::{WatchHandle, WatchSink};

/// Serves `Scheme::Local` paths from the file systems of this computer.
///
/// It holds no state, so it is `Copy`. A non-local path is refused with `Error::Unsupported`
/// before anything on disk is touched.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalProvider;

impl LocalProvider {
    pub fn new() -> LocalProvider {
        LocalProvider
    }
}

/// The path to hand to the file system API: on Windows the `\\?\` form, elsewhere the path
/// itself. A non-local path gives `Error::Unsupported`.
pub(crate) fn native(path: &VPath) -> Result<PathBuf> {
    path.to_extended_length()
        .ok_or_else(|| Error::Unsupported(format!("not a local path: {path}")))
}

/// What a file type is: a link, a folder or a regular file. Anything else (FIFO, socket, block
/// or character device) is `EntryKind::Other`. A Windows junction counts as a link, and
/// everything on Windows that is neither link nor folder counts as a file.
pub(crate) fn kind_of(file_type: FileType) -> EntryKind {
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

/// Builds the `Entry` named `name` from `meta`. For `stat` and `list` the metadata is taken
/// without following links; for `stat_target` it is taken through them and so describes what
/// the link leads to. On Unix the volume and file IDs come from the metadata, unless the file
/// system reports no usable ID (inode 0); on Windows they need an open handle per file. In both
/// of those cases they stay empty here.
pub(crate) fn entry_from_metadata(name: OsString, meta: &Metadata) -> Entry {
    let kind = kind_of(meta.file_type());
    let attributes = rf_platform::entry_attributes(&name, meta);
    let mut entry = Entry::new(name, kind);
    if kind != EntryKind::Dir {
        entry.size = meta.len();
    }
    entry.modified = meta.modified().ok();
    entry.created = meta.created().ok();
    entry.accessed = meta.accessed().ok();
    entry.attributes = attributes;
    if let Some((volume, file)) = rf_platform::metadata_identity(meta) {
        entry.volume_id = Some(volume);
        entry.file_id = Some(file);
    }
    entry
}

/// The entry `stat` gives for `path`, or with `follow` the one `stat_target` gives: the
/// metadata and the identity of what `path` leads to through every link.
fn stat_entry(path: &VPath, follow: bool) -> Result<Entry> {
    let local = native(path)?;
    let meta = if follow {
        std::fs::metadata(&local)
    } else {
        std::fs::symlink_metadata(&local)
    }
    .map_err(|e| Error::io("stat", path.clone(), e))?;
    // A root has no file name; it is shown under its whole path.
    let name = path
        .file_name()
        .or_else(|| path.as_local().map(Path::as_os_str))
        .unwrap_or_default();
    let mut entry = entry_from_metadata(name.to_owned(), &meta);
    if entry.identity().is_none() {
        // Windows: the IDs need a handle. A file system without usable IDs fails this too,
        // and without them the entry is still correct.
        let identity = if follow {
            rf_platform::resolved_file_identity(path)
        } else {
            rf_platform::file_identity(path)
        };
        if let Ok((volume, file)) = identity {
            entry.volume_id = Some(volume);
            entry.file_id = Some(file);
        }
    }
    Ok(entry)
}

impl Provider for LocalProvider {
    fn scheme(&self) -> Scheme {
        Scheme::Local
    }

    fn capabilities(&self, path: &VPath) -> Capabilities {
        let Ok(volume) = rf_platform::volume_info(path) else {
            return Capabilities::empty();
        };
        let mut caps = Capabilities::WATCH;
        if !volume.read_only {
            caps |= Capabilities::WRITE;
        }
        if rf_platform::trash_supported(path) {
            caps |= Capabilities::TRASH;
        }
        caps
    }

    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()> {
        list::list(dir, cancel, sink)
    }

    fn stat(&self, path: &VPath) -> Result<Entry> {
        stat_entry(path, false)
    }

    fn create_dir(&self, path: &VPath) -> Result<()> {
        std::fs::create_dir(native(path)?).map_err(|e| Error::io("create_dir", path.clone(), e))
    }

    fn create_file(&self, path: &VPath) -> Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(native(path)?)
            .map(drop)
            .map_err(|e| Error::io("create_file", path.clone(), e))
    }

    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
        ops::rename(from, to, mode)
    }

    fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted> {
        ops::delete(path, mode)
    }

    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
        ops::open_read(path)
    }

    fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
        ops::open_write(path, mode)
    }

    fn space(&self, path: &VPath) -> Result<Option<DiskSpace>> {
        rf_platform::disk_space(path).map(Some)
    }

    fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
        watch::start(dir, sink)
    }

    fn stat_target(&self, path: &VPath) -> Result<Entry> {
        stat_entry(path, true)
    }

    fn read_link(&self, path: &VPath) -> Result<LinkTarget> {
        ops::read_link(path)
    }

    fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> Result<()> {
        ops::create_symlink(link, target)
    }

    fn set_metadata(&self, path: &VPath, patch: &MetadataPatch) -> Result<()> {
        ops::set_metadata(path, patch)
    }

    fn copy_extras(&self, from: &VPath, to: &VPath) -> Result<()> {
        rf_platform::copy_extras(from, to)
    }

    fn remove_dir(&self, path: &VPath) -> Result<()> {
        ops::remove_dir(path)
    }

    fn delete_leaf(&self, path: &VPath) -> Result<()> {
        ops::delete_leaf(path)
    }

    fn sync_dir(&self, path: &VPath) -> Result<()> {
        ops::sync_dir(path)
    }

    fn fs_name(&self, path: &VPath) -> Option<String> {
        rf_platform::volume_info(path)
            .ok()
            .and_then(|info| info.fs_name)
    }

    fn data_ranges(&self, path: &VPath) -> Result<Option<Vec<Range<u64>>>> {
        rf_platform::data_ranges(path)
    }

    fn restore_trashed(&self, location: &VPath, original: &VPath) -> Result<()> {
        rf_platform::restore_from_trash(location, original)
    }
}
