//! Virtual file system layer: providers, the local provider, sorting and filtering.

pub mod cancel;
pub mod provider;
pub mod registry;
pub mod watch;

pub use cancel::CancelToken;
pub use provider::{
    Capabilities, DeleteMode, Deleted, LIST_BATCH_SIZE, Provider, ReadStream, RenameMode,
    WriteMode, WriteStream,
};
pub use registry::Vfs;
pub use rf_platform::DiskSpace;
pub use watch::{Change, MAX_PENDING_CHANGES, WATCH_DEBOUNCE, WatchEvent, WatchHandle, WatchSink};
