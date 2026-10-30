//! System change ledger (PRD INST-02): every OS change the app makes, with its before-state.

pub mod backend;
pub mod change;
pub mod cleanup;
pub mod error;
pub mod ledger;
mod store;

pub use backend::fake::{FakeBackend, FakeOp};
pub use backend::{SystemBackend, UnsupportedBackend, os_backend};
pub use change::{Before, Change, ServiceKind, Target, Value};
pub use cleanup::{CleanupAction, CleanupReport, plan_cleanup, run_cleanup};
pub use error::LedgerError;
pub use ledger::{
    ApplyOutcome, EntryFailure, EntryId, EntryState, Ledger, LedgerEntry, ReconcileReport,
    RevertReport,
};
