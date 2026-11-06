//! Questions a running job asks the person (or the program) that started it.

use crate::conflict::{Conflict, ConflictAction, ConflictAnswer};
use crate::id::JobId;
use crate::preflight::Issue;
use crate::report::ItemFailure;

/// What to do about a failed item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorAction {
    /// Try the item again.
    Retry,
    /// Leave the item undone and carry on with the rest.
    Skip,
    /// Stop the job.
    Abort,
}

/// An [`ErrorAction`] and whether it also settles the job's later failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ErrorAnswer {
    pub action: ErrorAction,
    /// Use `action` for every later failure of the job without asking again.
    pub apply_to_all: bool,
}

/// What to do about the problems the pre-flight check found on the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreflightAnswer {
    /// Run the job as it is.
    Proceed,
    /// Run the job but leave out the items with a blocking problem.
    SkipBlocked,
    /// Do not start the job.
    Cancel,
}

/// The one who answers a job's questions. A job calls it from its own worker thread and waits
/// for the answer, so only that job's lane stands still while a question is open: the jobs
/// queued behind it in its lane wait for it, and the other lanes keep running.
///
/// A window-based implementation sends the question to the UI thread and blocks on a channel
/// until the user answers. While it blocks, `JobEngine::cancel` on that job takes effect
/// only after the call returns: the worker is not at a checkpoint. An implementation
/// that can be interrupted (a closed window, say) should answer with the cancelling choice
/// instead of waiting forever.
///
/// `Send + Sync` because the engine hands the same resolver to every worker thread.
pub trait Resolver: Send + Sync {
    /// The target of an item already exists and the policy is to ask.
    ///
    /// An answer of `ConflictAction::Overwrite`, once or for all, is still refused when either
    /// side is a folder: that item fails as `Stage::Conflict` and both sides stay as they are.
    fn conflict(&self, job: JobId, conflict: &Conflict) -> ConflictAnswer;

    /// An item failed and the error policy is to ask.
    fn error(&self, job: JobId, failure: &ItemFailure) -> ErrorAnswer;

    /// The pre-flight check found `issues` on the target before anything was written.
    fn preflight(&self, job: JobId, issues: &[Issue]) -> PreflightAnswer;
}

/// The safe answers for when nobody is there to ask: skip every conflict and every failed
/// item (one at a time), and leave out the items with a blocking problem. Nothing is
/// overwritten, nothing is stopped.
#[derive(Clone, Copy, Debug, Default)]
pub struct NonInteractive;

impl Resolver for NonInteractive {
    fn conflict(&self, _job: JobId, _conflict: &Conflict) -> ConflictAnswer {
        ConflictAnswer::once(ConflictAction::Skip)
    }

    fn error(&self, _job: JobId, _failure: &ItemFailure) -> ErrorAnswer {
        ErrorAnswer {
            action: ErrorAction::Skip,
            apply_to_all: false,
        }
    }

    fn preflight(&self, _job: JobId, _issues: &[Issue]) -> PreflightAnswer {
        PreflightAnswer::SkipBlocked
    }
}
