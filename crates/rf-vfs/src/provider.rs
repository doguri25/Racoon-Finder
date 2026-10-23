//! The `Provider` trait every kind of location implements, and the types its methods use.

use std::fmt::Debug;
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use rf_core::{Attributes, Entry, Error, Result, Scheme, VPath};
pub use rf_platform::DiskSpace;

use crate::cancel::CancelToken;
use crate::watch::{WatchHandle, WatchSink};

/// How many entries `Provider::list` hands to its sink at most per call.
pub const LIST_BATCH_SIZE: usize = 2_000;

bitflags::bitflags! {
    /// What a location allows. The bit values may be stored or sent over IPC, so they must
    /// never change.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Capabilities: u32 {
        /// Items can be created, renamed, written and deleted permanently here. A read-only
        /// volume does not have it.
        const WRITE = 1 << 0;
        /// Items can be moved to the trash from here (`DeleteMode::Trash`).
        ///
        /// It is a guess for the whole volume (a fixed drive on Windows, a volume with a trash
        /// folder on macOS), not a promise for each item: the trash can still refuse one item,
        /// for example a Windows path longer than the recycle bin takes, and `delete` then
        /// fails with `ErrorKind::Unsupported` and leaves the item alone.
        const TRASH = 1 << 1;
        /// The folder can be watched for changes (`Provider::watch`).
        const WATCH = 1 << 2;
    }
}

/// What `Provider::rename` does when the target name is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenameMode {
    /// Never overwrites: an existing target is an `AlreadyExists` error.
    NoReplace,
    /// Atomically replaces an existing file.
    Replace,
}

/// How `Provider::delete` removes an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeleteMode {
    /// Moves the item to the trash. If that is impossible it fails; it never turns into a
    /// permanent delete.
    Trash,
    /// Removes the item for good: a folder with everything in it, and for a symlink only the
    /// link.
    Permanent,
}

/// What `Provider::delete` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deleted {
    /// The item is gone for good.
    Permanently,
    /// The item is in the trash. `location` is where it is now, when the system tells.
    Trashed { location: Option<VPath> },
}

/// What `Provider::open_write` does with an existing file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WriteMode {
    /// Creates the file; `AlreadyExists` if anything is there.
    CreateNew,
    /// Creates the file, or empties it if it exists.
    Truncate,
    /// Creates the file, or continues at its end if it exists (resuming a transfer, XFER-10).
    Append,
}

/// A file opened for reading. Implemented by every `Read + Seek + Send + Debug` type, for
/// example `std::fs::File`.
pub trait ReadStream: Read + Seek + Send + Debug {}

impl<T: Read + Seek + Send + Debug> ReadStream for T {}

/// A file opened for writing.
pub trait WriteStream: Write + Send + Debug {
    /// Flushes and closes the stream. Only after this returns `Ok` is the data durable;
    /// dropping the stream without it promises nothing.
    fn finish(self: Box<Self>) -> Result<()>;
}

/// What a symbolic link leads to, as `Provider::read_link` reads it and
/// `Provider::create_symlink` makes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkTarget {
    /// The target exactly as the link stores it. A relative target stays relative to the
    /// folder that holds the link, so a copied tree keeps its links pointing inside itself.
    pub target: PathBuf,
    /// Whether the link leads to a folder. Windows makes a folder link and a file link in
    /// different ways, so a copy of a link needs it there; elsewhere it is only information.
    pub is_dir: bool,
}

/// Times and attributes for `Provider::set_metadata` to put on an item. A field that is `None`
/// is left as it is.
///
/// The struct is `non_exhaustive`, so other crates start from `MetadataPatch::default()`,
/// `times_of` or `attributes_of` and set fields. Adding a field later does not break them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MetadataPatch {
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Kept on Windows and macOS only; the local provider ignores it elsewhere.
    pub created: Option<SystemTime>,
    /// The read-only, hidden, system and archive state the item gets, exactly. Other bits are
    /// ignored.
    pub attributes: Option<Attributes>,
}

/// The attribute bits that `set_metadata` sets, and so the ones `attributes_of` takes.
const SETTABLE: Attributes = Attributes::READONLY
    .union(Attributes::HIDDEN)
    .union(Attributes::SYSTEM)
    .union(Attributes::ARCHIVE);

impl MetadataPatch {
    /// The modification, access and creation times of `entry`, to put them on a copy; no
    /// attributes.
    pub fn times_of(entry: &Entry) -> MetadataPatch {
        MetadataPatch {
            modified: entry.modified,
            accessed: entry.accessed,
            created: entry.created,
            attributes: None,
        }
    }

    /// The read-only, hidden, system and archive bits of `entry`, to put them on a copy; no
    /// times. The other bits (compressed, sparse, reparse point, ...) tell how the item is
    /// stored, not what a copy should be, so they are left out.
    ///
    /// Off Windows a leading dot hides an item by its name, and the entry reports that as
    /// `HIDDEN` too, which cannot be told apart from a hidden flag. For a name that starts with
    /// a dot `HIDDEN` is therefore left out, so that the dot never turns into the macOS
    /// `UF_HIDDEN` flag. A copy of the same name is hidden by its dot, and a real `UF_HIDDEN`
    /// goes with `Provider::copy_extras`.
    pub fn attributes_of(entry: &Entry) -> MetadataPatch {
        let mut attributes = entry.attributes & SETTABLE;
        if cfg!(not(windows)) && rf_platform::is_dot_file(&entry.name) {
            attributes.remove(Attributes::HIDDEN);
        }
        MetadataPatch {
            attributes: Some(attributes),
            ..MetadataPatch::default()
        }
    }

    /// Whether the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == MetadataPatch::default()
    }
}

/// The error of a `Provider` method that the provider does not implement.
fn unsupported(op: &str, path: &VPath) -> Error {
    Error::Unsupported(format!("{op} is not supported for {path}"))
}

/// One kind of location (local disk, archive, remote server, ...). `Vfs` picks the provider
/// by the scheme of a `VPath`.
///
/// Every method blocks, so none of them is called on the UI thread. A provider is shared
/// between threads (`Send + Sync`) and is used as `dyn Provider`.
///
/// The methods after `watch` have default implementations, so that a provider without links,
/// metadata or a trash to restore from needs none of them: they fail with
/// `Error::Unsupported`, except `copy_extras` and `fs_name`, which have nothing to give.
pub trait Provider: Send + Sync + Debug {
    /// The scheme this provider serves; `Vfs` routes by it.
    fn scheme(&self) -> Scheme;
    /// What the location allows; errors yield `Capabilities::empty()`.
    fn capabilities(&self, path: &VPath) -> Capabilities;
    /// Streams the direct children of `dir` in batches of at most `LIST_BATCH_SIZE`
    /// (never empty); checks `cancel` at least between batches and then returns
    /// `Err(Error::Cancelled)`. Order is unspecified. An empty folder calls `sink` zero times.
    ///
    /// `Entry::identity()` may be `None`, as for `stat`. The local provider leaves it empty on
    /// Windows for every listed entry, because the IDs need an open handle per file.
    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()>;
    /// Metadata of `path` itself, without following a final symlink.
    ///
    /// `Entry::identity()` (the volume and file ID) may be `None`: a file system without file
    /// IDs, one that reports an ID of 0, or a remote provider that cannot tell. A caller that
    /// needs the identity handles `None`, and never takes two entries without one for the
    /// same item.
    fn stat(&self, path: &VPath) -> Result<Entry>;
    /// Creates one folder; `AlreadyExists` if anything is there, `NotFound` without a parent.
    fn create_dir(&self, path: &VPath) -> Result<()>;
    /// Creates one empty file with the same error rules as `create_dir`.
    fn create_file(&self, path: &VPath) -> Result<()>;
    /// Renames or moves inside one provider. `NoReplace` never overwrites; `Replace`
    /// atomically replaces an existing file. Moving a folder into itself is refused with
    /// `ErrorKind::Unsupported`.
    ///
    /// A target on another volume gives `ErrorKind::CrossesDevices`; the caller copies and
    /// deletes instead. A rename that only changes the letter case of a name works on a volume
    /// that ignores case, but one that only changes its Unicode normalization (NFC to NFD or
    /// back, I18N-04) can fail with `AlreadyExists` on APFS, which takes both spellings for
    /// one name.
    ///
    /// A target inside a subfolder of `from` that is reached through a link
    /// (`link-to-a-subfolder/x`) is not caught as a move into itself: the OS refuses it with an
    /// error of its own kind (`ErrorKind::Other` on Linux). That error comes back on every try,
    /// so a caller must not retry it automatically.
    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()>;
    /// Moves `path` to the trash or removes it for good.
    ///
    /// `Trash` never turns into a permanent delete; it fails instead, and every error leaves
    /// the item untouched (principle 1). `ErrorKind::Unsupported` means that the trash cannot
    /// take this item (no trash here, or the trash refuses it): the caller may offer a
    /// permanent delete, after asking the user. Other kinds come from the trash itself (the
    /// Windows shell may fail to read a very long path, for example).
    ///
    /// `Permanent` removes a folder tree, and for a symlink removes only the link, never what
    /// it leads to. An error names `path`, not the item inside the tree that failed, and a tree
    /// may then be partly removed.
    fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted>;
    /// Opens the file at `path` for reading and seeking.
    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>>;
    /// The data is durable only after `WriteStream::finish` returns `Ok`.
    fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>>;
    /// Space on the volume holding `path`; `None` when the provider cannot tell.
    fn space(&self, path: &VPath) -> Result<Option<DiskSpace>>;
    /// Starts watching `dir` (not its subfolders). `sink` runs on a background thread and
    /// is never called after the returned handle is dropped. Dropping the handle waits for a
    /// sink call in progress, so never drop it while holding a lock (or anything else) the sink
    /// may wait on.
    ///
    /// Start the watch before listing the folder, so that no change between the two is lost;
    /// a change that the listing already shows may then also arrive as an event.
    fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle>;

    /// Metadata of what `path` leads to, following links all the way, a final one too: the
    /// kind, size, times, attributes and identity of the target, under the name of `path`. For
    /// an item that is no link it is what `stat` gives.
    ///
    /// The kind is never `EntryKind::Symlink`, and a target of `EntryKind::Other` (FIFO,
    /// socket, device) is never opened for data. A link whose target is missing gives
    /// `ErrorKind::NotFound`, and a loop of links an error too. The default implementation
    /// gives `Error::Unsupported`: the provider cannot follow links, so a caller that wanted to
    /// skips the link and reports it.
    fn stat_target(&self, path: &VPath) -> Result<Entry> {
        Err(unsupported("stat_target", path))
    }

    /// What the link at `path` leads to, exactly as it is stored; a relative target stays
    /// relative. `LinkTarget::is_dir` tells whether it is a folder link: on Windows a folder
    /// symlink or a junction, elsewhere a link whose target is a folder now (so `false` for a
    /// link to nothing). An item that is no link is an error.
    fn read_link(&self, path: &VPath) -> Result<LinkTarget> {
        Err(unsupported("read_link", path))
    }

    /// Makes a symbolic link at `link` that leads to `target.target`, stored as given, so a
    /// relative target stays relative to the folder of `link`. The target need not exist. On
    /// Windows `target.is_dir` makes a folder link instead of a file link; elsewhere it is not
    /// needed.
    ///
    /// Never replaces anything: if `link` is taken, by a link too, the result is
    /// `ErrorKind::AlreadyExists` and the item there stays as it is. Windows lets a user make
    /// links only with Developer Mode on or the privilege for it, and refuses them otherwise.
    fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> Result<()> {
        let _ = target;
        Err(unsupported("create_symlink", link))
    }

    /// Puts the times and attributes in `patch` on the item at `path`, a file or a folder. A
    /// field that is `None` stays as it is, so an empty patch changes nothing. The attributes
    /// become exactly `patch.attributes` for the read-only, hidden, system and archive state,
    /// as far as the location keeps it; other bits are ignored.
    ///
    /// The times are set before the attributes, so that an item the patch makes read-only
    /// still gets its times. Off Windows a leading dot hides an item by its name: for such a
    /// name `HIDDEN` is neither set nor cleared (the local provider leaves the macOS
    /// `UF_HIDDEN` flag of a dot name as it is). Links are not meant to get metadata: the local
    /// provider sets the times of a link itself, but on Unix leaves its mode alone, because
    /// changing it would change what the link leads to.
    fn set_metadata(&self, path: &VPath, patch: &MetadataPatch) -> Result<()> {
        let _ = patch;
        Err(unsupported("set_metadata", path))
    }

    /// Copies the data of `from` that neither the content nor `set_metadata` carries onto
    /// `to`, which must exist; both are paths of this provider. For the local provider that is
    /// what `rf_platform::copy_extras` copies: the Unix permission bits and extended
    /// attributes, the macOS hidden flag, and the Windows alternate data streams. The default
    /// implementation has nothing to copy and returns `Ok(())`.
    fn copy_extras(&self, from: &VPath, to: &VPath) -> Result<()> {
        let _ = (from, to);
        Ok(())
    }

    /// Removes the folder at `path` only if it is empty. A move removes the folders it moved
    /// from this way, so it never deletes anything it did not move (DND-06).
    ///
    /// A folder with anything in it gives `ErrorKind::DirectoryNotEmpty` and stays as it is,
    /// and a file is never removed. A link is never followed: on Unix `remove_dir` of a link
    /// fails, and on Windows a folder link or junction is removed itself, like an empty folder,
    /// while what it leads to stays.
    fn remove_dir(&self, path: &VPath) -> Result<()> {
        Err(unsupported("remove_dir", path))
    }

    /// The name of the file system that holds `path` (`NTFS`, `FAT32`, `exFAT`, `apfs`, ...)
    /// for the checks that depend on it (XFER-12), or `None` when the provider cannot tell, as
    /// the local provider on Linux.
    fn fs_name(&self, path: &VPath) -> Option<String> {
        let _ = path;
        None
    }

    /// Puts an item back from the trash: moves it from `location`, where `Deleted::Trashed`
    /// said it went, to `original`, the place it was trashed from.
    ///
    /// Never replaces anything: if `original` is taken, the result is
    /// `ErrorKind::AlreadyExists` and both items stay as they are. If the folder of `original`
    /// is gone, the result is `ErrorKind::NotFound`.
    fn restore_trashed(&self, location: &VPath, original: &VPath) -> Result<()> {
        let _ = original;
        Err(unsupported("restore_trashed", location))
    }
}
