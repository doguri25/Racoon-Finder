//! OS integration used by the VFS layer; every FFI call in the workspace lives here.

pub mod natural;

pub use natural::{NaturalKey, natural_cmp, portable_natural_cmp};
