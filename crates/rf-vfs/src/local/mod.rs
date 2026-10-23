//! `LocalProvider`, the provider for the file systems of this computer.

mod list;
mod ops;

use std::ffi::OsString;
use std::fs::{FileType, Metadata, OpenOptions};
use std::path::{Path, PathBuf};

use rf_core::{Entry, EntryKind, Error, Result, Scheme, VPath};

use crate::cancel::CancelToken;
use crate::provider::{
    Capabilities, DeleteMode, Deleted, DiskSpace, Provider, ReadStream, RenameMode, WriteMode,
    WriteStream,
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

/// Builds the `Entry` named `name` from `meta`, which must have been taken without following
/// links. On Unix the volume and file IDs come from the metadata; on Windows they need an open
/// handle per file, so they stay empty here.
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        entry.volume_id = Some(rf_core::VolumeId(meta.dev()));
        entry.file_id = Some(rf_core::FileId(u128::from(meta.ino())));
    }
    entry
}

fn not_implemented<T>(method: &str) -> Result<T> {
    Err(Error::Unsupported(format!(
        "LocalProvider::{method} is not implemented yet"
    )))
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
        let local = native(path)?;
        let meta =
            std::fs::symlink_metadata(&local).map_err(|e| Error::io("stat", path.clone(), e))?;
        // A root has no file name; it is shown under its whole path.
        let name = path
            .file_name()
            .or_else(|| path.as_local().map(Path::as_os_str))
            .unwrap_or_default();
        let mut entry = entry_from_metadata(name.to_owned(), &meta);
        if entry.identity().is_none() {
            // Windows: the IDs need a handle. Without them the entry is still correct.
            if let Ok((volume, file)) = rf_platform::file_identity(path) {
                entry.volume_id = Some(volume);
                entry.file_id = Some(file);
            }
        }
        Ok(entry)
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

    fn watch(&self, _dir: &VPath, _sink: WatchSink) -> Result<WatchHandle> {
        not_implemented("watch")
    }
}
