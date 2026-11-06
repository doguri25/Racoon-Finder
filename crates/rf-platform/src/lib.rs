//! OS integration used by the VFS layer and the app; every FFI call in the workspace lives here.

pub mod attrs;
pub mod bundle;
pub mod clipboard;
pub mod desktop;
pub mod dnd;
pub mod fileid;
pub mod fsops;
pub mod known;
pub mod links;
pub mod meta;
pub mod natural;
pub mod shell;
pub mod sparse;
pub mod thumbs;
pub mod trash;
pub mod volume;
pub mod watch;
mod win_util;

pub use attrs::{
    attributes_from_bsd_flags, attributes_from_windows, entry_attributes, is_dot_file,
};
pub use bundle::{app_bundle_of, move_app_bundle_to_trash};
pub use fileid::{file_identity, metadata_identity, resolved_file_identity};
pub use fsops::{
    is_not_a_regular_file, not_a_regular_file, open_regular_file, rename_no_replace, sync_dir,
    sync_file,
};
pub use known::{KnownFolder, known_folder};
pub use links::{
    LinkKind, drop_kind, kinds_for, link_name, link_points_to, make_link, mount_point_buffer,
    remove_link, symlink_needs_privilege, symlink_needs_privilege_cached,
};
pub use meta::{copy_extras, set_attributes, set_times};
pub use natural::{NaturalKey, natural_cmp, portable_natural_cmp};
pub use sparse::{DataRanges, data_ranges, file_data_ranges, mark_sparse};
pub use thumbs::{
    Thumb, ThumbOutcome, bgra_to_rgba_rows, fit_within, os_thumbnail, supports_os_thumbnails,
};
pub use trash::{move_to_trash, restore_from_trash, trash_supported};
pub use volume::{DiskSpace, VolumeInfo, disk_space, volume_info};
pub use watch::{DirEvent, DirEventHandler, DirWatch, watch_dir};
