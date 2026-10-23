//! Reading and atomically replacing the ledger file.
//!
//! The file format is [`LedgerFile`] with the serde attributes of the types in it
//! ([`LedgerEntry`], `Change`, `Target`, `Value`, `Before`, …). Any new field or enum variant
//! in any of them changes the format and must bump [`Ledger::FORMAT`]: an older build would
//! otherwise read the file, silently drop what it does not know (unknown fields are ignored so
//! that every format can read the header) and write it back without it on its next save,
//! losing what a newer build recorded. With the bump, the older build refuses the file
//! untouched instead ([`LedgerError::UnsupportedFormat`]).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use rf_core::VPath;
use serde::{Deserialize, Serialize};

use crate::change::{Change, hkcu_subkey};
use crate::error::LedgerError;
use crate::ledger::{Ledger, LedgerEntry, is_valid_group};

/// The content of the ledger file.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LedgerFile {
    pub(crate) format: u32,
    pub(crate) next_id: u64,
    pub(crate) entries: Vec<LedgerEntry>,
}

/// An I/O failure of operation `op` on `path`.
pub(crate) fn io_error(op: &'static str, path: &Path, source: io::Error) -> LedgerError {
    match VPath::local(path) {
        Ok(path) => rf_core::Error::io(op, path, source).into(),
        Err(path_error) => rf_core::Error::from(path_error).into(),
    }
}

/// The part of the ledger file every format shares: the format number, read before the rest.
#[derive(Deserialize)]
struct Header {
    format: u32,
}

/// Reads the ledger file; `Ok(None)` if there is none yet.
///
/// A file that cannot be parsed, that says format 0 (no build wrote it: a 1 turned into a 0 is
/// a one-bit disk error), whose entry ids are not strictly ascending or leave no id for a next
/// entry, or that holds an entry the ledger could not have written (see [`check_entry`]), is
/// [`LedgerError::Corrupt`]; one written by a newer build is
/// [`LedgerError::UnsupportedFormat`]. Neither is ever written or removed here: replacing it
/// would forget the system changes it records.
pub(crate) fn load(path: &Path) -> Result<Option<LedgerFile>, LedgerError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error("read", path, e)),
    };
    let corrupt = |reason: String| LedgerError::Corrupt {
        path: path.to_owned(),
        reason,
    };
    let header: Header = serde_json::from_slice(&bytes).map_err(|e| corrupt(e.to_string()))?;
    // Not `< FORMAT`: a later build may still read format 1 files.
    if header.format == 0 {
        return Err(corrupt("format 0 was never written".into()));
    }
    if header.format > Ledger::FORMAT {
        return Err(LedgerError::UnsupportedFormat {
            path: path.to_owned(),
            found: header.format,
            supported: Ledger::FORMAT,
        });
    }
    let file: LedgerFile = serde_json::from_slice(&bytes).map_err(|e| corrupt(e.to_string()))?;
    // Ids are handed out in ascending order and never reused; a file that breaks this was not
    // written by the ledger, and trusting it could make a new entry reuse an id.
    if !file.entries.is_sorted_by(|a, b| a.id < b.id) {
        return Err(corrupt("entry ids are not strictly ascending".into()));
    }
    if file.entries.last().is_some_and(|e| e.id.0 == u64::MAX) {
        return Err(corrupt(format!(
            "entry id {} leaves no id for a next entry",
            u64::MAX
        )));
    }
    for entry in &file.entries {
        check_entry(entry).map_err(|reason| corrupt(format!("entry {}: {reason}", entry.id.0)))?;
    }
    Ok(Some(file))
}

/// Checks that `entry` is one the ledger could have written: its group is valid, its change
/// passes [`Change::validate`], its before-state is a value its target can hold, and every key
/// it created lies under `HKEY_CURRENT_USER`. Anything else was not written by the ledger, and
/// the backends would refuse it on every revert, so the entry could never be settled.
fn check_entry(entry: &LedgerEntry) -> Result<(), String> {
    if !is_valid_group(&entry.group) {
        return Err(LedgerError::InvalidGroup(entry.group.clone()).to_string());
    }
    entry.change.validate().map_err(|e| e.to_string())?;
    let before = Change {
        target: entry.change.target.clone(),
        value: entry.before.value.clone(),
    };
    before
        .validate()
        .map_err(|e| format!("before-state: {e}"))?;
    for key in &entry.before.created_keys {
        hkcu_subkey(key).map_err(|e| format!("created key: {e}"))?;
    }
    Ok(())
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

    // Make the rename itself durable. On Unix that is a flush of the folder that holds the
    // name.
    #[cfg(unix)]
    File::open(dir)
        .and_then(|folder| folder.sync_all())
        .map_err(|e| io_error("sync_all", dir, e))?;
    // On Windows std cannot open a folder, and NTFS writes its log of the rename lazily, so a
    // power loss soon after could still bring the old file back. Flushing the renamed file
    // (`FlushFileBuffers`, which needs write access) commits that log, the rename included.
    #[cfg(windows)]
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_error("sync_all", path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Format 1 of the ledger file, pinned byte for byte by a golden file that holds every kind
    //! of target, value, service and entry state, a before-state with and without created keys,
    //! and missing values (`null`). A field or variant renamed by accident fails here; one
    //! renamed on purpose needs a new format (see `Ledger::FORMAT`), not a new golden file.

    use super::*;
    use crate::change::{Before, Change, ServiceKind, Target, Value};
    use crate::ledger::{EntryId, EntryState};

    const GOLDEN: &[u8] = include_bytes!("../tests/golden/ledger-format-1.json");

    fn registry(name: &str) -> Target {
        Target::Registry {
            key: r"HKCU\Software\G".into(),
            name: name.into(),
        }
    }

    fn preference(domain: &str, key: &str) -> Target {
        Target::Preference {
            domain: domain.into(),
            key: key.into(),
        }
    }

    fn service(service: ServiceKind) -> Target {
        Target::Service {
            service,
            name: "indexd".into(),
        }
    }

    fn was(value: Option<Value>) -> Before {
        Before {
            value,
            created_keys: Vec::new(),
        }
    }

    /// What the golden file holds. The ids have gaps and `next_id` is past the last one, as
    /// after some reverts.
    fn golden() -> LedgerFile {
        use EntryState::{Applied, Pending};
        let program = |path: &str, args: &[&str]| Value::Program {
            path: path.into(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
        };
        let entries = [
            (
                Applied,
                registry(""),
                Some(Value::Str("a".into())),
                Before {
                    value: None,
                    created_keys: vec![r"HKCU\Software\G".into()],
                },
            ),
            (
                Pending,
                registry("e"),
                Some(Value::ExpandStr("%A%".into())),
                was(Some(Value::Raw {
                    ty: 1,
                    data: vec![97, 0],
                })),
            ),
            (
                Applied,
                registry("m"),
                Some(Value::MultiStr(vec!["a".into(), "b".into()])),
                was(Some(Value::U32(1))),
            ),
            (
                Applied,
                registry("q"),
                Some(Value::U64(2)),
                was(Some(Value::Bytes(vec![3]))),
            ),
            (
                Applied,
                preference("com.example.golden", "flag"),
                Some(Value::Bool(true)),
                was(Some(Value::Plist(vec![4]))),
            ),
            (
                Applied,
                preference("com.example.golden", "count"),
                None,
                was(Some(Value::I64(-1))),
            ),
            (
                Pending,
                service(ServiceKind::WindowsService),
                Some(program(r"C:\x\indexd.exe", &["--service"])),
                was(None),
            ),
            (
                Applied,
                service(ServiceKind::LaunchAgent),
                None,
                was(Some(program("/x/indexd", &[]))),
            ),
        ];
        let ids = [1, 2, 4, 5, 7, 8, 10, 11];
        LedgerFile {
            format: 1,
            next_id: 12,
            entries: ids
                .into_iter()
                .zip(entries)
                .map(|(id, (state, target, value, before))| LedgerEntry {
                    id: EntryId(id),
                    group: "winx.golden".into(),
                    state,
                    change: Change { target, value },
                    before,
                })
                .collect(),
        }
    }

    #[test]
    fn format_1_reads_the_golden_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(Ledger::FILE_NAME);
        fs::write(&path, GOLDEN).unwrap();
        let file = load(&path).unwrap().unwrap();
        let want = golden();
        assert_eq!((file.format, file.next_id), (want.format, want.next_id));
        assert_eq!(file.entries, want.entries);
    }

    #[test]
    fn format_1_writes_the_golden_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(Ledger::FILE_NAME);
        save(&path, &golden()).unwrap();
        let written = fs::read(&path).unwrap();
        assert!(written == GOLDEN, "{}", String::from_utf8_lossy(&written));
    }
}
