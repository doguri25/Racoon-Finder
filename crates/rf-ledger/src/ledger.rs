//! The ledger: records every system change on disk before making it.

use std::path::PathBuf;

use rf_core::{AppPaths, InstallMode};
use serde::{Deserialize, Serialize};

use crate::backend::SystemBackend;
use crate::change::{Before, Change};
use crate::error::LedgerError;
use crate::store::{self, LedgerFile};

/// Identifies one ledger entry. Ids are never reused, not even after the entry is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(pub u64);

/// How far a recorded change got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// Recorded, but the system may or may not have been changed yet.
    Pending,
    /// The system was changed.
    Applied,
}

/// One recorded change with the state it replaced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LedgerEntry {
    pub id: EntryId,
    /// The feature that owns the change (see [`Ledger::apply`]).
    pub group: String,
    pub state: EntryState,
    pub change: Change,
    pub before: Before,
}

/// What [`Ledger::apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The system was changed and the change recorded under this id.
    Recorded(EntryId),
    /// The system already had the wanted value; nothing was recorded.
    Unchanged,
}

/// An entry the ledger could not settle.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryFailure {
    pub id: EntryId,
    /// The entry's target, as displayed in logs.
    pub change: String,
    /// Why it failed.
    pub error: String,
}

/// What [`Ledger::open`] settled for entries a crash left unfinished.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReconcileReport {
    pub confirmed: Vec<EntryId>,
    pub rolled_back: Vec<EntryId>,
    pub failed: Vec<EntryFailure>,
}

/// Records every change to the operating system, with the state it replaced, in a file under
/// the app's data folder, so that every change can be reverted.
///
/// A change is written to the file, and flushed to disk, before the system is touched.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    #[expect(
        dead_code,
        reason = "read by the portable-mode guard that a later change adds"
    )]
    portable: bool,
    backend: Box<dyn SystemBackend>,
    next_id: u64,
    /// In ascending id order.
    entries: Vec<LedgerEntry>,
}

/// Whether `group` is a non-empty name of lowercase letters, digits, `.` and `-`.
fn is_valid_group(group: &str) -> bool {
    !group.is_empty()
        && group
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-'))
}

impl Ledger {
    /// The ledger file's name inside the data folder.
    pub const FILE_NAME: &'static str = "ledger.json";
    /// The ledger file format this build writes.
    pub const FORMAT: u32 = 1;

    /// Where the ledger file of `paths` lives.
    pub fn file_path(paths: &AppPaths) -> PathBuf {
        paths.data_dir().join(Self::FILE_NAME)
    }

    /// Opens the ledger file, which may not exist yet; nothing is created.
    pub fn open(
        paths: &AppPaths,
        backend: Box<dyn SystemBackend>,
    ) -> Result<(Self, ReconcileReport), LedgerError> {
        let path = Self::file_path(paths);
        let (entries, next_id) = match store::load(&path)? {
            Some(file) => {
                let after_last = file
                    .entries
                    .iter()
                    .map(|e| e.id.0.saturating_add(1))
                    .max()
                    .unwrap_or(1);
                (file.entries, file.next_id.max(after_last))
            }
            None => (Vec::new(), 1),
        };
        let ledger = Self {
            path,
            portable: paths.mode() == InstallMode::Portable,
            backend,
            next_id,
            entries,
        };
        Ok((ledger, ReconcileReport::default()))
    }

    /// Reads the recorded entries without a backend.
    pub fn inspect(paths: &AppPaths) -> Result<Vec<LedgerEntry>, LedgerError> {
        Ok(store::load(&Self::file_path(paths))?.map_or_else(Vec::new, |file| file.entries))
    }

    /// The recorded entries, in ascending id order.
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    /// Records `change` for the feature `group` (lowercase letters, digits, `.` and `-`) and
    /// makes it.
    ///
    /// The order is the point: the previous state is read, a `Pending` entry is written and
    /// flushed to disk, and only then does the backend change the system. A crash in between
    /// leaves a record that the next [`Ledger::open`] can settle; the reverse order would leave
    /// a system change nobody knows about.
    ///
    /// Only one group may own a slot of system state at a time. If the system already holds
    /// the wanted value, nothing is changed or recorded.
    pub fn apply(&mut self, group: &str, change: Change) -> Result<ApplyOutcome, LedgerError> {
        if !is_valid_group(group) {
            return Err(LedgerError::InvalidGroup(group.to_owned()));
        }
        change.validate()?;
        if let Some(owner) = self
            .entries
            .iter()
            .find(|e| e.group != group && e.change.target.same_slot(&change.target))
        {
            return Err(LedgerError::SlotOwned {
                target: change.target.to_string(),
                owner: owner.group.clone(),
            });
        }
        if self.backend.is_applied(&change)? {
            return Ok(ApplyOutcome::Unchanged);
        }
        let before = self.backend.capture(&change)?;

        // Write ahead: the record is durable before the system changes.
        let id = EntryId(self.next_id);
        self.next_id += 1;
        self.entries.push(LedgerEntry {
            id,
            group: group.to_owned(),
            state: EntryState::Pending,
            change,
            before,
        });
        let index = self.entries.len() - 1;
        if let Err(e) = self.persist() {
            // The system is still untouched.
            self.entries.pop();
            return Err(e);
        }

        let entry = &self.entries[index];
        if let Err(apply_error) = self.backend.apply(&entry.change) {
            if self.backend.restore(&entry.change, &entry.before).is_ok() {
                self.entries.pop();
                // A failed save leaves a `Pending` entry on disk; the next open settles it.
                let _ = self.persist();
            }
            return Err(apply_error);
        }

        self.entries[index].state = EntryState::Applied;
        // A failed save leaves the entry `Pending` on disk; the next open confirms it.
        let _ = self.persist();
        Ok(ApplyOutcome::Recorded(id))
    }

    /// Writes the current entries to the ledger file.
    fn persist(&self) -> Result<(), LedgerError> {
        store::save(
            &self.path,
            &LedgerFile {
                format: Self::FORMAT,
                next_id: self.next_id,
                entries: self.entries.clone(),
            },
        )
    }
}
