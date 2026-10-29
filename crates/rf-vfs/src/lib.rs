//! Virtual file system layer: providers, the local provider, sorting and filtering.

pub mod cancel;
pub mod filter;
pub mod local;
mod names;
pub mod provider;
pub mod registry;
pub mod sort;
pub mod testkit;
pub mod watch;

pub use cancel::CancelToken;
pub use filter::VisibilityFilter;
pub use local::LocalProvider;
pub use names::fold_name;
pub use provider::{
    Capabilities, DeleteMode, Deleted, LIST_BATCH_SIZE, LinkTarget, MetadataPatch, Provider,
    ReadStream, RenameMode, WriteMode, WriteStream,
};
pub use registry::Vfs;
pub use rf_platform::{DataRanges, DiskSpace};
pub use sort::{SortBy, SortKey, SortOrder, SortSpec, compare_entries, sort_by_name, sort_entries};
pub use watch::{Change, MAX_PENDING_CHANGES, WATCH_DEBOUNCE, WatchEvent, WatchHandle, WatchSink};
