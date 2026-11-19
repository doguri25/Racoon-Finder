//! Core types shared by every crate: app identity, errors, `VPath`, `Entry`, app data paths.

pub mod app_paths;
pub mod entry;
pub mod error;
pub mod identity;
pub mod os;
pub mod vpath;
pub mod winpath;

pub use app_paths::{AppPaths, BaseDirs, InstallMode};
pub use entry::{Attributes, Entry, EntryKind, FileId, VolumeId};
pub use error::{Error, ErrorKind, PathError, Result};
pub use os::Os;
pub use vpath::{Scheme, VPath};
