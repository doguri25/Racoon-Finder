//! System change ledger (PRD INST-02): every OS change the app makes, with its before-state.

pub mod change;
pub mod error;

pub use change::{Before, Change, ServiceKind, Target, Value};
pub use error::LedgerError;
