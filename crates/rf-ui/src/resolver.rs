//! The window's answers to the questions of the jobs: [`UiResolver`].
//!
//! A job asks on its own worker thread and waits for the answer, which stops its lane (and only
//! its lane) meanwhile. [`UiResolver`] hands the question to the UI thread as a [`Question`],
//! through the `post` function it was made with, and waits on a channel until the window calls
//! [`UiResolver::answer_conflict`] or [`UiResolver::answer_preflight`] with the question's id.
//! No lock is held while it waits.
//!
//! When the app ends, [`UiResolver::release_all`] answers every question still open with the
//! cancelling choice, and every later one at once, so that no worker waits for a window that is
//! gone. It is called after `JobEngine::shutdown` (carry-m1-03 "quit order"): the engine has
//! then marked the jobs as cut off by the app's end, so the cancel this answer brings is not
//! taken for the user's, and the next start recovers the jobs.

use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, PoisonError};

use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, ErrorAction, ErrorAnswer, Issue, ItemFailure, JobId,
    PreflightAnswer, Resolver,
};

/// The number of one question, unique for one [`UiResolver`].
pub type QuestionId = u64;

/// A question of a job for the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// What [`UiResolver::answer_conflict`] or [`UiResolver::answer_preflight`] answers.
    pub id: QuestionId,
    /// The job that asks.
    pub job: JobId,
    pub asked: Asked,
}

/// What a job asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The name of an item is taken at the target.
    Conflict(Box<Conflict>),
    /// The pre-flight check found problems on the target before anything was written.
    Preflight(Vec<Issue>),
}

/// The channel an open question waits on.
enum Waiting {
    Conflict(Sender<ConflictAnswer>),
    Preflight(Sender<PreflightAnswer>),
}

impl Waiting {
    /// Answers with the cancelling choice. A worker that has stopped waiting is no matter.
    fn cancel(self) {
        match self {
            Waiting::Conflict(answer) => {
                let _ = answer.send(CANCEL_CONFLICT);
            }
            Waiting::Preflight(answer) => {
                let _ = answer.send(PreflightAnswer::Cancel);
            }
        }
    }
}

const CANCEL_CONFLICT: ConflictAnswer = ConflictAnswer::once(ConflictAction::Cancel);

/// The open questions, behind the resolver's lock.
#[derive(Default)]
struct Open {
    /// Set by [`UiResolver::release_all`]: every question from then on is cancelled at once.
    closing: bool,
    next: QuestionId,
    waiting: HashMap<QuestionId, Waiting>,
}

/// A [`Resolver`] that asks the window. See the [module](self) documentation.
pub struct UiResolver {
    /// Hands a question to the UI thread; `false` when it cannot (the event loop has ended),
    /// and the question is then cancelled.
    post: Box<dyn Fn(Question) -> bool + Send + Sync>,
    open: Mutex<Open>,
}

impl fmt::Debug for UiResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let open = self.lock();
        f.debug_struct("UiResolver")
            .field("closing", &open.closing)
            .field("waiting", &open.waiting.len())
            .finish_non_exhaustive()
    }
}

impl UiResolver {
    /// A resolver that hands its questions to `post`, on the worker thread that asks. `post`
    /// must return at once (it queues the question for the UI thread) and says whether the
    /// question got there.
    pub fn new(post: impl Fn(Question) -> bool + Send + Sync + 'static) -> UiResolver {
        UiResolver {
            post: Box::new(post),
            open: Mutex::new(Open::default()),
        }
    }

    /// Answers the conflict question `id`; `false` when it is not open (answered, released, or
    /// not a conflict).
    pub fn answer_conflict(&self, id: QuestionId, answer: ConflictAnswer) -> bool {
        let waiting = self.take(id, |waiting| matches!(waiting, Waiting::Conflict(_)));
        match waiting {
            Some(Waiting::Conflict(channel)) => channel.send(answer).is_ok(),
            _ => false,
        }
    }

    /// Answers the pre-flight question `id`; `false` when it is not open.
    pub fn answer_preflight(&self, id: QuestionId, answer: PreflightAnswer) -> bool {
        let waiting = self.take(id, |waiting| matches!(waiting, Waiting::Preflight(_)));
        match waiting {
            Some(Waiting::Preflight(channel)) => channel.send(answer).is_ok(),
            _ => false,
        }
    }

    /// Answers every open question with its cancelling choice (`ConflictAction::Cancel`,
    /// `PreflightAnswer::Cancel`), and every later question at once. For the app's end, after
    /// `JobEngine::shutdown` (see the [module](self) documentation).
    pub fn release_all(&self) {
        let released: Vec<Waiting> = {
            let mut open = self.lock();
            open.closing = true;
            open.waiting.drain().map(|(_, waiting)| waiting).collect()
        };
        for waiting in released {
            waiting.cancel();
        }
    }

    /// Whether [`release_all`](Self::release_all) has been called.
    pub fn is_closing(&self) -> bool {
        self.lock().closing
    }

    /// How many questions wait for an answer.
    pub fn open_questions(&self) -> usize {
        self.lock().waiting.len()
    }

    /// Takes the open question `id` if `is_kind` says it is the right kind.
    fn take(&self, id: QuestionId, is_kind: impl Fn(&Waiting) -> bool) -> Option<Waiting> {
        let mut open = self.lock();
        if !open.waiting.get(&id).is_some_and(is_kind) {
            return None;
        }
        open.waiting.remove(&id)
    }

    /// Asks the window `asked` for `job` and waits for the answer: `cancel` when the resolver
    /// is closing, when the question cannot reach the window, or when its channel is dropped.
    fn ask<T>(&self, job: JobId, asked: Asked, waiting: fn(Sender<T>) -> Waiting, cancel: T) -> T {
        let (answer, answered) = mpsc::channel();
        let id = {
            let mut open = self.lock();
            if open.closing {
                return cancel;
            }
            open.next += 1;
            let id = open.next;
            open.waiting.insert(id, waiting(answer));
            id
        };
        // Without the lock: `post` is the window's code.
        if !(self.post)(Question { id, job, asked }) {
            self.lock().waiting.remove(&id);
            return cancel;
        }
        answered.recv().unwrap_or(cancel)
    }

    /// The lock of the open questions. Every change under it is a single insert, remove or
    /// assignment, so a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, Open> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Resolver for UiResolver {
    fn conflict(&self, job: JobId, conflict: &Conflict) -> ConflictAnswer {
        self.ask(
            job,
            Asked::Conflict(Box::new(conflict.clone())),
            Waiting::Conflict,
            CANCEL_CONFLICT,
        )
    }

    /// The demo submits every job with `ErrorPolicy::Continue`, so a job asks this only for a
    /// policy it never uses: the item is skipped, as the policy it uses would.
    fn error(&self, _job: JobId, _failure: &ItemFailure) -> ErrorAnswer {
        ErrorAnswer {
            action: ErrorAction::Skip,
            apply_to_all: false,
        }
    }

    fn preflight(&self, job: JobId, issues: &[Issue]) -> PreflightAnswer {
        self.ask(
            job,
            Asked::Preflight(issues.to_vec()),
            Waiting::Preflight,
            PreflightAnswer::Cancel,
        )
    }
}
