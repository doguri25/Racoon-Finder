//! OS integration used by the VFS layer; every FFI call in the workspace lives here.

pub mod attrs;
pub mod fileid;
pub mod natural;
mod win_util;

pub use attrs::{
    attributes_from_bsd_flags, attributes_from_windows, entry_attributes, is_dot_file,
};
pub use fileid::file_identity;
pub use natural::{NaturalKey, natural_cmp, portable_natural_cmp};
