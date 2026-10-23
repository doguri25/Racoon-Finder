//! OS integration used by the VFS layer; every FFI call in the workspace lives here.

pub mod attrs;
pub mod fileid;
pub mod fsops;
pub mod known;
pub mod meta;
pub mod natural;
pub mod sparse;
pub mod trash;
pub mod volume;
pub mod watch;
mod win_util;

pub use attrs::{
    attributes_from_bsd_flags, attributes_from_windows, entry_attributes, is_dot_file,
};
pub use fileid::{file_identity, metadata_identity, resolved_file_identity};
pub use fsops::rename_no_replace;
pub use known::{KnownFolder, known_folder};
pub use meta::{copy_extras, set_attributes, set_times};
pub use natural::{NaturalKey, natural_cmp, portable_natural_cmp};
pub use sparse::{data_ranges, mark_sparse};
pub use trash::{move_to_trash, restore_from_trash, trash_supported};
pub use volume::{DiskSpace, VolumeInfo, disk_space, volume_info};
pub use watch::{DirEvent, DirEventHandler, DirWatch, watch_dir};
