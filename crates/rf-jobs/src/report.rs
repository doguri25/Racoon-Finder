//! What a finished job hands back: how it ended, what it did and what it could not do.

use std::time::{Duration, SystemTime};

use rf_core::{ErrorKind, VPath};
use serde::{Deserialize, Serialize};

use crate::id::JobId;
use crate::preflight::Issue;
use crate::spec::{JobKind, JobSpec};
use crate::undo::UndoRecord;

/// The step of a job an item failed in, for the UI to pick the wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Walking the sources to count and list what the job will touch.
    Plan,
    /// Checking the target file system's limits before anything is written.
    Preflight,
    /// Looking for an existing item at the target and applying the conflict policy.
    Conflict,
    /// Opening or reading a source file or link.
    Read,
    /// Creating a target folder or writing a temporary file.
    Write,
    /// Renaming the finished temporary file to its final name.
    Commit,
    /// Copying times, attributes and the other metadata onto the target.
    Metadata,
    /// Removing a source after a move has committed its copy.
    DeleteSource,
    /// Deleting or trashing an item in a delete job.
    Delete,
    /// Renaming an item.
    Rename,
    /// Creating a new empty file or folder.
    Create,
    /// Reversing an earlier job.
    Undo,
    /// A fault in the engine itself, such as a panic in a worker.
    Internal,
}

/// An item a job could not process, or processed only in part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFailure {
    /// The item that was read. Below a link that a copy in Follow mode followed, it is the real
    /// path of the item with every link resolved (`…/outside/sub/x`), not the path through the
    /// link (`…/dir/dlink/x`).
    pub source: Option<VPath>,
    pub target: Option<VPath>,
    pub stage: Stage,
    pub kind: ErrorKind,
    /// The error's log text, in English. The UI words its own message from `stage` and `kind`.
    pub message: String,
}

impl ItemFailure {
    /// The failure of `stage` on `source` and `target`, described by `error`.
    pub fn new(
        stage: Stage,
        source: Option<&VPath>,
        target: Option<&VPath>,
        error: &rf_core::Error,
    ) -> ItemFailure {
        ItemFailure {
            source: source.cloned(),
            target: target.cloned(),
            stage,
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// How a job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    /// Every item was processed (skips the user chose count as processed).
    Completed,
    /// The job ran to the end but some items failed; see `JobReport::failures`.
    CompletedWithErrors,
    /// The user cancelled the job.
    Cancelled,
    /// The job stopped early on an error or a refused precondition.
    Aborted,
}

/// A count of files, folders and bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

/// The record of one job: what it was asked, how it ended and what it left behind.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JobReport {
    pub id: JobId,
    pub kind: JobKind,
    pub spec: JobSpec,
    pub outcome: Outcome,
    /// What the plan expected to process.
    pub planned: Totals,
    /// What was processed.
    pub done: Totals,
    /// Items left alone by the conflict policy or because they were the same item.
    pub skipped: u64,
    /// Items that could not be processed.
    pub failures: Vec<ItemFailure>,
    /// Items that were processed except for some part of them, such as their metadata.
    pub warnings: Vec<ItemFailure>,
    /// What the target file system cannot hold, found before the job wrote anything.
    pub issues: Vec<Issue>,
    /// The top-level paths the job made, with any automatic rename applied. The UI selects
    /// them and starts a rename on a new item.
    pub outputs: Vec<VPath>,
    /// A move that had to copy and delete because its source and target are on different
    /// volumes.
    pub copied_instead_of_moved: bool,
    pub started: SystemTime,
    pub elapsed: Duration,
    /// How to reverse the job, when it can be reversed.
    pub undo: Option<UndoRecord>,
}

impl JobReport {
    /// The report of a job that has not done anything yet: completed, with nothing counted.
    pub(crate) fn new(id: JobId, spec: &JobSpec) -> JobReport {
        JobReport {
            id,
            kind: spec.kind(),
            spec: spec.clone(),
            outcome: Outcome::Completed,
            planned: Totals::default(),
            done: Totals::default(),
            skipped: 0,
            failures: Vec::new(),
            warnings: Vec::new(),
            issues: Vec::new(),
            outputs: Vec::new(),
            copied_instead_of_moved: false,
            started: SystemTime::now(),
            elapsed: Duration::ZERO,
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use rf_core::VPath;

    use super::*;
    use crate::spec::CreateKind;

    #[test]
    fn a_new_report_is_complete_and_empty() {
        let before = SystemTime::now();
        let dir = VPath::local(std::env::temp_dir()).unwrap();
        let spec = JobSpec::Create {
            parent: dir,
            name: "x".into(),
            kind: CreateKind::Dir,
        };
        let id = JobId(7);
        let report = JobReport::new(id, &spec);
        assert_eq!(
            (report.id, report.kind, report.outcome),
            (id, JobKind::Create, Outcome::Completed)
        );
        assert_eq!(report.spec, spec);
        assert_eq!(
            (report.planned, report.done, report.skipped),
            (Totals::default(), Totals::default(), 0)
        );
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.warnings, Vec::new());
        assert_eq!(report.issues, Vec::new());
        assert_eq!(report.outputs, Vec::<VPath>::new());
        assert!(!report.copied_instead_of_moved);
        assert_eq!(report.undo, None);
        assert_eq!(report.elapsed, Duration::ZERO);
        assert!(report.started >= before);
    }
}
