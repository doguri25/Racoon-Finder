//! Core types shared by every crate: app identity, errors, `VPath`, `Entry`, app data paths.

pub mod error;
pub mod identity;
pub mod vpath;
pub mod winpath;

pub use error::PathError;
pub use vpath::{Scheme, VPath};
