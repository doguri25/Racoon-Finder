//! Errors of the change ledger.

use std::path::PathBuf;

/// Everything that can go wrong while recording, applying or reverting a system change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LedgerError {
    #[error("portable mode never changes the operating system")]
    PortableMode,
    #[error("registry key is outside HKEY_CURRENT_USER: {0}")]
    OutsideHkcu(String),
    #[error("invalid change target: {0}")]
    InvalidTarget(String),
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("invalid change group `{0}`: use lowercase letters, digits, '.' and '-'")]
    InvalidGroup(String),
    #[error("{target} already belongs to change group `{owner}`")]
    SlotOwned { target: String, owner: String },
    #[error("not supported here: {0}")]
    Unsupported(String),
    #[error("system backend failed: {0}")]
    Backend(String),
    #[error("ledger file {} is corrupt: {reason}", path.display())]
    Corrupt { path: PathBuf, reason: String },
    #[error("ledger file {} has format {found}; this build reads up to {supported}", path.display())]
    UnsupportedFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error(transparent)]
    Core(#[from] rf_core::Error),
}
