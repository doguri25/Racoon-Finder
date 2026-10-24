//! A flag that tells a long-running operation to stop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rf_core::{Error, Result};

/// A cancellation flag shared between the caller and the operation it started.
///
/// Clones share one flag: cancelling any clone cancels them all. The operation polls the
/// token (at least between batches of work); cancelling never interrupts a call that is
/// already in progress. A token cannot be reset, so a new operation takes a new token.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that is not cancelled.
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// Cancels this token and every clone of it.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// `Err(Error::Cancelled)` once the token is cancelled, so an operation can write
    /// `cancel.check()?` between steps.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
