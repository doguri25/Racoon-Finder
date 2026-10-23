//! File operation jobs that run on background threads, never on the caller's thread.

pub mod conflict;
pub mod control;
pub mod engine;
pub mod exec;
pub mod id;
pub mod journal;
pub mod naming;
pub mod preflight;
pub mod progress;
pub mod report;
pub mod resolver;
pub mod spec;
pub mod testkit;
pub mod undo;

pub use conflict::{Conflict, ConflictAction, ConflictAnswer, Decision, MTIME_TOLERANCE, decide};
pub use control::Control;
pub use engine::{
    DefaultLanes, EngineConfig, JobEngine, JobEvent, JobSnapshot, JobState, LaneId, LanePolicy,
};
pub use exec::{ExecEnv, ExecEvent, MAX_DEPTH, execute};
pub use id::JobId;
pub use journal::{IncompleteJob, JOURNAL_FILE};
pub use naming::{MAX_NUMBERED, TEMP_SUFFIX, numbered_name, temp_job, temp_name};
pub use preflight::{
    CaseIndex, FsKind, Issue, IssueKind, Severity, check_name, check_path_len, check_size,
};
pub use progress::{PROGRESS_INTERVAL, Progress, SPEED_WINDOW, SpeedMeter};
pub use report::{ItemFailure, JobReport, Outcome, Stage, Totals};
pub use resolver::{ErrorAction, ErrorAnswer, NonInteractive, PreflightAnswer, Resolver};
pub use spec::{
    ConflictPolicy, CreateKind, DeleteMethod, DeleteSpec, ErrorPolicy, JobKind, JobSpec,
    SymlinkMode, TransferItem, TransferMode, TransferOptions, TransferSpec,
};
pub use undo::{CreatedItem, MovedItem, TrashedItem, UndoRecord};
