//! The `Provider` trait every kind of location implements, and the types its methods use.

use std::fmt::Debug;
use std::io::{Read, Seek, Write};

use rf_core::{Entry, Result, Scheme, VPath};
use rf_platform::DiskSpace;

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

/// One kind of location (local disk, archive, remote server, ...). `Vfs` picks the provider
/// by the scheme of a `VPath`.
///
/// Every method blocks, so none of them is called on the UI thread. A provider is shared
/// between threads (`Send + Sync`) and is used as `dyn Provider`.
pub trait Provider: Send + Sync + Debug {
    /// The scheme this provider serves; `Vfs` routes by it.
    fn scheme(&self) -> Scheme;
    /// What the location allows; errors yield `Capabilities::empty()`.
    fn capabilities(&self, path: &VPath) -> Capabilities;
    /// Streams the direct children of `dir` in batches of at most `LIST_BATCH_SIZE`
    /// (never empty); checks `cancel` at least between batches and then returns
    /// `Err(Error::Cancelled)`. Order is unspecified. An empty folder calls `sink` zero times.
    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()>;
    /// Metadata of `path` itself, without following a final symlink.
    fn stat(&self, path: &VPath) -> Result<Entry>;
    /// Creates one folder; `AlreadyExists` if anything is there, `NotFound` without a parent.
    fn create_dir(&self, path: &VPath) -> Result<()>;
    /// Creates one empty file with the same error rules as `create_dir`.
    fn create_file(&self, path: &VPath) -> Result<()>;
    /// Renames or moves inside one provider. `NoReplace` never overwrites; `Replace`
    /// atomically replaces an existing file. Moving a folder into itself is refused.
    fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()>;
    /// `Trash` never turns into a permanent delete; it fails instead. `Permanent` removes a
    /// folder tree, and for a symlink removes only the link.
    fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted>;
    /// Opens the file at `path` for reading and seeking.
    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>>;
    /// The data is durable only after `WriteStream::finish` returns `Ok`.
    fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>>;
    /// Space on the volume holding `path`; `None` when the provider cannot tell.
    fn space(&self, path: &VPath) -> Result<Option<DiskSpace>>;
    /// Starts watching `dir` (not its subfolders). `sink` runs on a background thread and
    /// is never called after the returned handle is dropped.
    fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle>;
}
