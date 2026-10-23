//! File operation jobs that run on background threads, never on the caller's thread.

pub mod id;
pub mod spec;

pub use id::JobId;
pub use spec::{
    ConflictPolicy, CreateKind, DeleteMethod, DeleteSpec, ErrorPolicy, JobKind, JobSpec,
    SymlinkMode, TransferItem, TransferMode, TransferOptions, TransferSpec,
};
