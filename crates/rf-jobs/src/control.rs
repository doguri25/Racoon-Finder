//! Pause, resume and cancel for one running job.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use rf_core::{Error, Result};
use rf_vfs::CancelToken;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RunState {
    #[default]
    Running,
    Paused,
    /// Final: nothing leaves this state.
    Cancelled,
}

#[derive(Debug, Default)]
struct Inner {
    state: Mutex<RunState>,
    /// Signalled when the state leaves `Paused`.
    cv: Condvar,
    token: CancelToken,
}

/// The remote control of one job. The engine keeps a handle to pause, resume and cancel; the
/// worker keeps another and calls [`checkpoint`](Self::checkpoint) between steps. Clones share
/// one state.
///
/// Cancelling is final. A paused worker waits at its checkpoint until some other handle calls
/// [`resume`](Self::resume) or [`cancel`](Self::cancel); a worker that is the last handle
/// left has nobody to wake it, so whoever owns the other handles cancels before dropping the
/// last of them.
#[derive(Clone, Debug, Default)]
pub struct Control {
    inner: Arc<Inner>,
}

impl Control {
    pub fn new() -> Control {
        Control::default()
    }

    /// Holds the worker at its next [`checkpoint`](Self::checkpoint). No effect after
    /// [`cancel`](Self::cancel).
    pub fn pause(&self) {
        let mut state = self.lock();
        if *state == RunState::Running {
            *state = RunState::Paused;
        }
    }

    /// Lets a paused worker go on. No effect when the job is not paused, and none after
    /// [`cancel`](Self::cancel).
    pub fn resume(&self) {
        let mut state = self.lock();
        if *state == RunState::Paused {
            *state = RunState::Running;
            self.inner.cv.notify_all();
        }
    }

    /// Cancels the job: wakes a paused worker, which then fails its checkpoint, and cancels
    /// the [`cancel_token`](Self::cancel_token) so a provider call in progress stops too.
    pub fn cancel(&self) {
        let mut state = self.lock();
        self.inner.token.cancel();
        *state = RunState::Cancelled;
        self.inner.cv.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        *self.lock() == RunState::Paused
    }

    pub fn is_cancelled(&self) -> bool {
        *self.lock() == RunState::Cancelled
    }

    /// The token [`cancel`](Self::cancel) cancels, for the provider calls that take one. Every
    /// call returns a handle to the same token.
    pub fn cancel_token(&self) -> CancelToken {
        self.inner.token.clone()
    }

    /// Called by the worker between steps. Returns at once while running, waits while the
    /// job is paused until it is resumed or cancelled, and fails with `Error::Cancelled` once
    /// it is cancelled.
    ///
    /// Only [`cancel`](Self::cancel) cancels the job: cancelling the
    /// [`cancel_token`](Self::cancel_token) alone stops the provider calls that take it, but
    /// fails no checkpoint and wakes no paused one.
    pub fn checkpoint(&self) -> Result<()> {
        let state = self
            .inner
            .cv
            .wait_while(self.lock(), |state| *state == RunState::Paused)
            .unwrap_or_else(PoisonError::into_inner);
        if *state == RunState::Cancelled {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// The state lock. A holder that panicked cannot leave the state half-written (every
    /// update is one assignment), so a poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, RunState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
