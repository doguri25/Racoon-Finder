//! Reading and atomically replacing the ledger file.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use rf_core::VPath;
use serde::{Deserialize, Serialize};

use crate::error::LedgerError;
use crate::ledger::{Ledger, LedgerEntry};

/// The content of the ledger file.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LedgerFile {
    pub(crate) format: u32,
    pub(crate) next_id: u64,
    pub(crate) entries: Vec<LedgerEntry>,
}

/// An I/O failure of operation `op` on `path`.
fn io_error(op: &'static str, path: &Path, source: io::Error) -> LedgerError {
    match VPath::local(path) {
        Ok(path) => rf_core::Error::io(op, path, source).into(),
        Err(path_error) => rf_core::Error::from(path_error).into(),
    }
}

/// Reads the ledger file; `Ok(None)` if there is none yet.
pub(crate) fn load(path: &Path) -> Result<Option<LedgerFile>, LedgerError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error("read", path, e)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(file) => Ok(Some(file)),
        Err(e) => Err(LedgerError::Corrupt {
            path: path.to_owned(),
            reason: e.to_string(),
        }),
    }
}

/// Replaces the ledger file with `file`, durably.
///
/// The new content goes to a temporary file in the same folder, is flushed to disk and then
/// renamed over the old file, so a crash leaves either the old or the new content, never a
/// mix. The folder is created if needed.
pub(crate) fn save(path: &Path, file: &LedgerFile) -> Result<(), LedgerError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| io_error("create_dir_all", dir, e))?;

    let tmp = path.with_file_name(format!("{}.tmp", Ledger::FILE_NAME));
    let bytes = serde_json::to_vec_pretty(file).map_err(|e| io_error("write", &tmp, e.into()))?;
    let mut out = File::create(&tmp).map_err(|e| io_error("write", &tmp, e))?;
    out.write_all(&bytes)
        .map_err(|e| io_error("write", &tmp, e))?;
    out.sync_all().map_err(|e| io_error("sync_all", &tmp, e))?;
    drop(out);

    fs::rename(&tmp, path).map_err(|e| io_error("rename", path, e))?;

    // Make the rename itself durable. std cannot open a folder on Windows; NTFS journals the
    // metadata change.
    #[cfg(unix)]
    File::open(dir)
        .and_then(|folder| folder.sync_all())
        .map_err(|e| io_error("sync_all", dir, e))?;
    Ok(())
}
