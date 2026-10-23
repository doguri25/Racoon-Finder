//! Metadata for one item in a folder listing.
//!
//! These types carry what a listing shows (name, kind, size, times, attributes)
//! and what lets a record find the same file again after a rename (volume and
//! file IDs). The OS-specific reading of that data lives in `rf-platform`.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::SystemTime;

bitflags::bitflags! {
    /// Platform-neutral file attributes.
    ///
    /// The bit values may be stored or sent over IPC, so they must never change.
    /// `rf-platform` maps OS flags onto these: for example Windows
    /// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` and `FILE_ATTRIBUTE_OFFLINE` both
    /// become [`Attributes::OFFLINE`], and on macOS `UF_HIDDEN` or a leading dot
    /// becomes [`Attributes::HIDDEN`].
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Attributes: u32 {
        const READONLY = 1 << 0;
        const HIDDEN = 1 << 1;
        const SYSTEM = 1 << 2;
        const ARCHIVE = 1 << 3;
        const COMPRESSED = 1 << 4;
        const ENCRYPTED = 1 << 5;
        const SPARSE = 1 << 6;
        const OFFLINE = 1 << 7;
        const REPARSE_POINT = 1 << 8;
    }
}

/// What an [`Entry`] is. A Windows junction is a [`EntryKind::Symlink`] too, with
/// [`Attributes::REPARSE_POINT`] set alongside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// Identifies a volume: the Windows volume serial number, or `st_dev` on Unix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VolumeId(pub u64);

/// Identifies a file within one volume: the 64-bit NTFS file ID, the 128-bit
/// ReFS `FILE_ID_128`, or `st_ino` on Unix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u128);

/// One item of a folder listing.
///
/// `name` is a single name relative to the parent folder; the full path is
/// `dir.join(&entry.name)`. The struct is `non_exhaustive`, so other crates
/// build it with [`Entry::new`] and then fill in fields. Adding a field later
/// does not break them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub attributes: Attributes,
    pub volume_id: Option<VolumeId>,
    pub file_id: Option<FileId>,
}

impl Entry {
    /// An entry with size 0, no times, no attributes and no IDs.
    pub fn new(name: impl Into<OsString>, kind: EntryKind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size: 0,
            modified: None,
            created: None,
            accessed: None,
            attributes: Attributes::empty(),
            volume_id: None,
            file_id: None,
        }
    }

    /// The text after the last dot, for the extension column. Folders have none,
    /// and neither do dot files (`.bashrc`) or names ending in a dot (`a.`).
    pub fn extension(&self) -> Option<&OsStr> {
        if self.kind == EntryKind::Dir {
            return None;
        }
        Path::new(&self.name)
            .extension()
            .filter(|ext| !ext.is_empty())
    }

    /// The key for reconnecting a record to a moved file. `Some` only when both
    /// the volume ID and the file ID are known.
    pub fn identity(&self) -> Option<(VolumeId, FileId)> {
        Some((self.volume_id?, self.file_id?))
    }
}
