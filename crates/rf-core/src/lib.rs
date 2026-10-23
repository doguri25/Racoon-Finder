//! Core types shared by every crate: app identity, errors, `VPath`, `Entry`, app data paths.

pub mod entry;
pub mod error;
pub mod identity;
pub mod vpath;
pub mod winpath;

pub use entry::{Attributes, Entry, EntryKind, FileId, VolumeId};
pub use error::{Error, ErrorKind, PathError, Result};
pub use vpath::{Scheme, VPath};
