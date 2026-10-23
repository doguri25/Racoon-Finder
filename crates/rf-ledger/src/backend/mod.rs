//! The only way the app touches operating system settings.

pub mod fake;

use crate::change::{Before, Change};
use crate::error::LedgerError;

/// The only way the app touches OS settings. Every method must be safe to repeat.
pub trait SystemBackend: std::fmt::Debug + Send {
    /// Reads the live state of `change.target` as the before-state to record
    /// (for a registry value with `Some` value, also the keys `apply` would create).
    fn capture(&mut self, change: &Change) -> Result<Before, LedgerError>;
    /// Makes the live slot equal `change.value`; `None` removes it.
    fn apply(&mut self, change: &Change) -> Result<(), LedgerError>;
    /// Puts the slot back to `before` (succeeds when it already matches) and removes
    /// `before.created_keys` that are empty.
    fn restore(&mut self, change: &Change, before: &Before) -> Result<(), LedgerError>;
    /// Whether the live slot equals `change.value` right now.
    fn is_applied(&mut self, change: &Change) -> Result<bool, LedgerError>;
    /// Removes every key of a preferences domain the app owns (INST-05). Default: nothing to do.
    fn clear_domain(&mut self, _domain: &str) -> Result<(), LedgerError> {
        Ok(())
    }
}

/// The backend of an operating system that has none: it refuses every change.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedBackend;

fn unsupported<T>() -> Result<T, LedgerError> {
    Err(LedgerError::Unsupported(
        "this OS has no system backend".into(),
    ))
}

impl SystemBackend for UnsupportedBackend {
    fn capture(&mut self, _change: &Change) -> Result<Before, LedgerError> {
        unsupported()
    }

    fn apply(&mut self, _change: &Change) -> Result<(), LedgerError> {
        unsupported()
    }

    fn restore(&mut self, _change: &Change, _before: &Before) -> Result<(), LedgerError> {
        unsupported()
    }

    fn is_applied(&mut self, _change: &Change) -> Result<bool, LedgerError> {
        unsupported()
    }
}

/// The backend for the operating system this build runs on.
pub fn os_backend() -> Box<dyn SystemBackend> {
    Box::new(UnsupportedBackend)
}
