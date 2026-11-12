//! The window's answers to the questions of the jobs: [`UiResolver`].
//!
//! A job asks on its own worker thread and waits for the answer, which stops its lane (and only
//! its lane) meanwhile. [`UiResolver`] hands the question to the UI thread as a [`Question`],
//! through the `post` function it was made with, and waits on a channel until the window calls
//! [`UiResolver::answer_conflict`], [`UiResolver::answer_error`] or
//! [`UiResolver::answer_preflight`] with the question's id. No lock is held while it waits, and
//! the window holds nothing of its own for the question but the window that shows it: the
//! answer comes back as a call of its own, from another turn of the event loop.
//!
//! A job asks about an item that failed only when it runs under `ErrorPolicy::Ask`, which is
//! the choice 파일마다 묻기 of 보기 ▸ 설정 (XFER-09); under the default policy it carries on and
//! the failed items are listed at its end.
//!
//! When the app ends, [`UiResolver::release_all`] answers every question still open with the
//! cancelling choice, and every later one at once, so that no worker waits for a window that is
//! gone. It is called after `JobEngine::shutdown` (carry-m1-03 "quit order"): the engine has
//! then marked the jobs as cut off by the app's end, so the cancel this answer brings is not
//! taken for the user's, and the next start recovers the jobs. For an item that failed, the
//! cancelling choice is to leave it out ([`SKIP_ERROR`]): a job the engine has stopped ends at
//! its next step as cut off, where an answer that stops the job would end it for good.

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
    /// What [`UiResolver::answer_conflict`], [`UiResolver::answer_error`] or
    /// [`UiResolver::answer_preflight`] answers.
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
    /// An item failed, and the job's error policy is to ask.
    Error(Box<ItemFailure>),
    /// The pre-flight check found problems on the target before anything was written.
    Preflight(Vec<Issue>),
}

/// The channel an open question waits on.
enum Waiting {
    Conflict(Sender<ConflictAnswer>),
    Error(Sender<ErrorAnswer>),
    Preflight(Sender<PreflightAnswer>),
}

impl Waiting {
    /// Answers with the cancelling choice. A worker that has stopped waiting is no matter.
    fn cancel(self) {
        match self {
            Waiting::Conflict(answer) => {
                let _ = answer.send(CANCEL_CONFLICT);
            }
            Waiting::Error(answer) => {
                let _ = answer.send(SKIP_ERROR);
            }
            Waiting::Preflight(answer) => {
                let _ = answer.send(PreflightAnswer::Cancel);
            }
        }
    }
}

const CANCEL_CONFLICT: ConflictAnswer = ConflictAnswer::once(ConflictAction::Cancel);

/// The answer to an error question nobody can answer (the window is gone, or it closed without
/// a choice): the item is left out, this once, and the job goes on, as it does under the
/// default policy.
pub const SKIP_ERROR: ErrorAnswer = ErrorAnswer {
    action: ErrorAction::Skip,
    apply_to_all: false,
};

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

    /// Answers the error question `id`; `false` when it is not open (answered, released, or not
    /// an error question).
    pub fn answer_error(&self, id: QuestionId, answer: ErrorAnswer) -> bool {
        let waiting = self.take(id, |waiting| matches!(waiting, Waiting::Error(_)));
        match waiting {
            Some(Waiting::Error(channel)) => channel.send(answer).is_ok(),
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
    /// `PreflightAnswer::Cancel`, and [`SKIP_ERROR`] for an item that failed), and every later
    /// question at once. For the app's end, after `JobEngine::shutdown` (see the
    /// [module](self) documentation).
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

    /// Asks the window about the item that failed (XFER-09, 파일마다 묻기), and waits for its
    /// answer. With nobody to answer, the item is left out and the job goes on
    /// ([`SKIP_ERROR`]).
    fn error(&self, job: JobId, failure: &ItemFailure) -> ErrorAnswer {
        self.ask(
            job,
            Asked::Error(Box::new(failure.clone())),
            Waiting::Error,
            SKIP_ERROR,
        )
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use rf_core::{ErrorKind, VPath};
    use rf_jobs::Stage;

    use super::*;

    /// The longest a test waits for the other thread.
    const WAIT: Duration = Duration::from_secs(20);

    fn failure(name: &str) -> ItemFailure {
        let path = |folder: &str| {
            VPath::local(std::env::temp_dir().join(folder).join(name)).expect("a local path")
        };
        ItemFailure {
            source: Some(path("src")),
            target: Some(path("dst")),
            stage: Stage::Write,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        }
    }

    /// A resolver whose questions come out of the receiver, as the window gets them.
    fn resolver() -> (Arc<UiResolver>, Receiver<Question>) {
        let (asked, questions) = mpsc::channel();
        let asked = Mutex::new(asked);
        let post = move |question| {
            let asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
            asked.send(question).is_ok()
        };
        (Arc::new(UiResolver::new(post)), questions)
    }

    /// Asks `resolver` about `failure` for `job` on a thread of its own, as a job's worker
    /// does; the answer comes out of the receiver.
    fn ask_on_a_worker(
        resolver: &Arc<UiResolver>,
        job: JobId,
        failure: ItemFailure,
    ) -> Receiver<ErrorAnswer> {
        let (answered, answer) = mpsc::channel();
        let resolver = Arc::clone(resolver);
        std::thread::spawn(move || {
            let _ = answered.send(resolver.error(job, &failure));
        });
        answer
    }

    #[test]
    fn an_error_question_reaches_the_window_and_its_answer_the_job() {
        let (resolver, questions) = resolver();
        for answer in [
            ErrorAnswer {
                action: ErrorAction::Retry,
                apply_to_all: false,
            },
            ErrorAnswer {
                action: ErrorAction::Skip,
                apply_to_all: true,
            },
            ErrorAnswer {
                action: ErrorAction::Abort,
                apply_to_all: false,
            },
        ] {
            let waiting = ask_on_a_worker(&resolver, JobId(7), failure("a.txt"));
            let question = questions.recv_timeout(WAIT).expect("the job asks");
            assert_eq!(question.job, JobId(7));
            assert_eq!(question.asked, Asked::Error(Box::new(failure("a.txt"))));
            // The worker waits: nothing has answered yet.
            assert_eq!(resolver.open_questions(), 1);
            assert!(waiting.try_recv().is_err());
            // An answer of another kind is not for this question.
            assert!(!resolver.answer_conflict(question.id, CANCEL_CONFLICT));
            assert!(!resolver.answer_preflight(question.id, PreflightAnswer::Cancel));
            assert_eq!(resolver.open_questions(), 1);
            // The window's answer is the job's, as it is; a second one finds no question.
            assert!(resolver.answer_error(question.id, answer));
            assert_eq!(waiting.recv_timeout(WAIT), Ok(answer));
            assert!(!resolver.answer_error(question.id, answer));
            assert_eq!(resolver.open_questions(), 0);
        }
    }

    #[test]
    fn an_error_nobody_can_answer_leaves_its_item_out() {
        // The app ends with the question open: it is released with the skip.
        let (resolver, questions) = resolver();
        let waiting = ask_on_a_worker(&resolver, JobId(1), failure("a.txt"));
        let question = questions.recv_timeout(WAIT).expect("the job asks");
        resolver.release_all();
        assert_eq!(waiting.recv_timeout(WAIT), Ok(SKIP_ERROR));
        assert!(!resolver.answer_error(question.id, SKIP_ERROR));
        // From then on a question is answered at once, without reaching the window.
        assert_eq!(resolver.error(JobId(1), &failure("b.txt")), SKIP_ERROR);
        assert!(questions.try_recv().is_err());

        // A window that cannot be reached (the event loop has ended) answers the same, and
        // leaves no question open.
        let unreachable = UiResolver::new(|_| false);
        assert_eq!(unreachable.error(JobId(2), &failure("a.txt")), SKIP_ERROR);
        assert_eq!(unreachable.open_questions(), 0);
        assert_eq!(
            SKIP_ERROR,
            ErrorAnswer {
                action: ErrorAction::Skip,
                apply_to_all: false
            }
        );
    }
}
