//! Test doubles for code that uses this crate. The module is always compiled (no feature
//! flag) so that the tests of the crates built on top of this one can use it too.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rf_core::VPath;

use crate::conflict::{Conflict, ConflictAnswer};
use crate::id::JobId;
use crate::preflight::Issue;
use crate::report::{ItemFailure, Stage};
use crate::resolver::{ErrorAnswer, PreflightAnswer, Resolver};

/// One question a [`ScriptedResolver`] was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolverCall {
    Conflict { target: VPath },
    Error { stage: Stage },
    Preflight { issues: usize },
}

#[derive(Debug, Default)]
struct Script {
    conflicts: VecDeque<ConflictAnswer>,
    errors: VecDeque<ErrorAnswer>,
    preflights: VecDeque<PreflightAnswer>,
    calls: Vec<ResolverCall>,
}

/// A [`Resolver`] that gives the answers it was loaded with, in order, and remembers what it
/// was asked. Each kind of question has its own queue. A question with no answer left
/// panics, so a test fails loudly when the code asks more than the test expected.
///
/// The queues and the call log sit behind one lock that is released before any panic, so an
/// unscripted question does not poison the resolver for the calls that follow.
#[derive(Debug, Default)]
pub struct ScriptedResolver {
    script: Mutex<Script>,
}

impl ScriptedResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues the answer to the next unanswered conflict.
    #[must_use]
    pub fn on_conflict(mut self, answer: ConflictAnswer) -> Self {
        self.script_mut().conflicts.push_back(answer);
        self
    }

    /// Queues the answer to the next unanswered failure.
    #[must_use]
    pub fn on_error(mut self, answer: ErrorAnswer) -> Self {
        self.script_mut().errors.push_back(answer);
        self
    }

    /// Queues the answer to the next unanswered pre-flight question.
    #[must_use]
    pub fn on_preflight(mut self, answer: PreflightAnswer) -> Self {
        self.script_mut().preflights.push_back(answer);
        self
    }

    /// Every question asked so far, in order, answered or not.
    pub fn calls(&self) -> Vec<ResolverCall> {
        self.lock().calls.clone()
    }

    /// The script, for the builder methods that own `self`.
    fn script_mut(&mut self) -> &mut Script {
        self.script
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The script lock. Every update is a queue operation or a push, so a holder that
    /// panicked cannot leave the script half-written; a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Resolver for ScriptedResolver {
    fn conflict(&self, _job: JobId, conflict: &Conflict) -> ConflictAnswer {
        let answer = {
            let mut script = self.lock();
            script.calls.push(ResolverCall::Conflict {
                target: conflict.target.clone(),
            });
            script.conflicts.pop_front()
        };
        // The guard is gone: the panic below leaves the lock clean.
        answer.unwrap_or_else(|| panic!("unscripted conflict for {}", conflict.target))
    }

    fn error(&self, _job: JobId, failure: &ItemFailure) -> ErrorAnswer {
        let answer = {
            let mut script = self.lock();
            script.calls.push(ResolverCall::Error {
                stage: failure.stage,
            });
            script.errors.pop_front()
        };
        answer.unwrap_or_else(|| panic!("unscripted error at {:?}", failure.stage))
    }

    fn preflight(&self, _job: JobId, issues: &[Issue]) -> PreflightAnswer {
        let answer = {
            let mut script = self.lock();
            script.calls.push(ResolverCall::Preflight {
                issues: issues.len(),
            });
            script.preflights.pop_front()
        };
        answer.unwrap_or_else(|| panic!("unscripted preflight with {} issues", issues.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unscripted_question_leaves_the_lock_unpoisoned() {
        let resolver = ScriptedResolver::new();
        assert!(std::panic::catch_unwind(|| resolver.preflight(JobId(1), &[])).is_err());
        assert!(!resolver.script.is_poisoned());
        assert_eq!(resolver.calls(), [ResolverCall::Preflight { issues: 0 }]);
    }
}
