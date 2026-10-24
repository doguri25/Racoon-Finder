//! The ledger: records every system change on disk before making it.

use std::path::PathBuf;

use rf_core::{AppPaths, InstallMode};
use serde::{Deserialize, Serialize};

use crate::backend::SystemBackend;
use crate::change::{Before, Change, Target, Value, key_holds};
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

/// What [`Ledger::open`] settled for the `Pending` entries a crash left, each list in
/// processing order (newest entry first).
///
/// A `Pending` entry beneath a newer entry of the same setting whose value is presumed in place
/// is in no list: whether its own change reached the system means nothing then, so it stays
/// `Pending` for a revert to walk back to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReconcileReport {
    /// Entries whose change the system holds; they are `Applied` now.
    pub confirmed: Vec<EntryId>,
    /// Entries whose setting held their before-state, or the nothing a half-done write leaves,
    /// instead of their change: their before-state was put back and they are no longer recorded.
    pub rolled_back: Vec<EntryId>,
    /// Entries whose setting holds someone else's value (neither the app's value, nor the
    /// entry's before-state, nor nothing), and the older entries of that setting, `Applied` ones
    /// included: the other value was left alone and the entries are no longer recorded.
    pub skipped: Vec<EntryId>,
    /// Entries that stay `Pending` because they could not be settled, or because a newer entry
    /// of the same setting could not.
    pub failed: Vec<EntryFailure>,
}

/// What [`Ledger::revert_all`] or [`Ledger::revert_group`] settled, each list in processing
/// order (newest entry first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RevertReport {
    /// Entries whose before-state was put back; they are no longer recorded. An entry whose
    /// setting already held its before-state (its restore had happened, but the ledger file did
    /// not say so yet) is put back again and listed here too.
    pub reverted: Vec<EntryId>,
    /// Entries whose setting holds neither the app's value nor the entry's before-state, because
    /// someone else changed it since, and the older entries of that setting: the other value was
    /// left alone and the entries are no longer recorded.
    pub skipped: Vec<EntryId>,
    /// Entries that are still recorded because they could not be settled.
    pub failed: Vec<EntryFailure>,
}

impl RevertReport {
    /// Whether every entry that was asked for is settled, that is, nothing `failed`.
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Records every change to the operating system, with the state it replaced, in a file under
/// the app's data folder, so that every change can be reverted.
///
/// A change is written to the file, and flushed to disk, before the system is touched.
///
/// In portable mode (`InstallMode::Portable`) the ledger never touches the system or its own
/// file: it stays empty, [`Ledger::apply`] is refused with [`LedgerError::PortableMode`] and
/// the revert methods have nothing to do. A ledger file in the data folder, say one copied
/// from an installed setup, is neither read nor changed, and the backend is never called.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    /// Whether the app runs portable: this ledger is empty and inert.
    portable: bool,
    backend: Box<dyn SystemBackend>,
    next_id: u64,
    /// In ascending id order.
    entries: Vec<LedgerEntry>,
}

/// Whether `group` is a non-empty name of lowercase letters, digits, `.` and `-`.
pub(crate) fn is_valid_group(group: &str) -> bool {
    !group.is_empty()
        && group
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-'))
}

/// What the newest entry of a setting left for the older entries of that setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotMark {
    /// It could not be settled and stays recorded. The older entries wait for it without a
    /// backend call and stay recorded; those that were to be settled are reported as failed.
    Stuck,
    /// It found someone else's value: neither its own value nor its before-state. The older
    /// entries are dropped without a backend call: whatever they recorded, the value is not the
    /// app's to overwrite.
    Foreign,
    /// It stays recorded with its value presumed in place. The older entries stay as they are.
    Live,
}

/// The slot-chain rule that both [`Ledger::revert_all`] and [`Ledger::open`] settle entries by.
///
/// Entries are settled newest first, and an entry is settled only while it is the newest one
/// left of its setting: the system holds that entry's value (or someone else's), so whether an
/// older entry's value is live means nothing. Once settled, the entry leaves a [`SlotMark`] for
/// the older entries of its setting, or none if its before-state was put back: then the next
/// older entry is the newest left and is settled in turn.
#[derive(Debug, Default)]
struct SlotChain(Vec<(Target, SlotMark)>);

impl SlotChain {
    /// The mark a newer entry left for the setting of `target`; `None` if the entry for
    /// `target` is the newest left of its setting.
    fn get(&self, target: &Target) -> Option<SlotMark> {
        self.0
            .iter()
            .find(|(slot, _)| slot.same_slot(target))
            .map(|&(_, mark)| mark)
    }

    /// Leaves `mark` for the older entries of the setting of `target`.
    fn mark(&mut self, target: &Target, mark: SlotMark) {
        self.0.push((target.clone(), mark));
    }
}

/// What the setting of an entry holds now, as [`holds`] finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holds {
    /// The entry's value: the app's.
    Ours,
    /// The entry's before-state. Its restore already happened (a crash came before the ledger
    /// file said so, or a power loss dropped the app's write), or its apply never did.
    Before,
    /// Nothing, where the before-state is a value: what an apply of the entry that died
    /// half-way leaves, since on macOS a write of another type removes the value before it
    /// writes it again. Found only for a `Pending` entry, the only kind whose apply may have
    /// died; for an `Applied` one an emptied setting is someone else's doing.
    Nothing,
    /// Anything else: someone else changed the setting since, and the value is theirs.
    Other,
}

/// Asks the system what the setting of `entry` holds: whether it is the entry's value; if not,
/// whether it is the entry's before-state; and if not that either, for a `Pending` entry,
/// whether it is nothing. Each question is one `is_applied` call, asked only when the one
/// before it said no.
fn holds(backend: &mut dyn SystemBackend, entry: &LedgerEntry) -> Result<Holds, LedgerError> {
    if backend.is_applied(&entry.change)? {
        return Ok(Holds::Ours);
    }
    let with = |value: Option<Value>| Change {
        target: entry.change.target.clone(),
        value,
    };
    if backend.is_applied(&with(entry.before.value.clone()))? {
        return Ok(Holds::Before);
    }
    if entry.state == EntryState::Pending
        && entry.before.value.is_some()
        && backend.is_applied(&with(None))?
    {
        return Ok(Holds::Nothing);
    }
    Ok(Holds::Other)
}

/// Adds the registry key `key` to `created_keys`, which stay shallowest first, unless it is
/// there already.
fn adopt(created_keys: &mut Vec<String>, key: &str) {
    if created_keys
        .iter()
        .any(|own| key_holds(own, key) && key_holds(key, own))
    {
        return;
    }
    created_keys.push(key.to_owned());
    // They are all the heir's key or above it, so the deeper key has more names.
    created_keys.sort_by_key(|key| key.matches('\\').count());
}

impl Ledger {
    /// The ledger file's name inside the data folder.
    pub const FILE_NAME: &'static str = "ledger.json";
    /// The ledger file format this build writes.
    ///
    /// Any new field or enum variant anywhere in the file (the entries, changes, targets,
    /// values, before-states) bumps it: an older build would otherwise drop what it does not
    /// know on its next save, and with it a change a newer build recorded. Format 0 was never
    /// written; a file that says so is corrupt.
    pub const FORMAT: u32 = 1;

    /// Where the ledger file of `paths` lives.
    pub fn file_path(paths: &AppPaths) -> PathBuf {
        paths.data_dir().join(Self::FILE_NAME)
    }

    /// Opens the ledger file, which may not exist yet; nothing is created. Then settles the
    /// `Pending` entries a crash left (see [`ReconcileReport`]) and saves the file once if that
    /// changed anything.
    ///
    /// A ledger file that does not parse, says format 0, or whose entry ids were tampered with
    /// or entries could not have been written by the ledger (an invalid group, change,
    /// before-state or created key), is refused with [`LedgerError::Corrupt`]; one a newer
    /// build wrote, with
    /// [`LedgerError::UnsupportedFormat`]. Neither is ever changed, and the backend is not
    /// called: the file is the only record of the system changes to revert. If the settled
    /// entries cannot be saved, that error is returned; the next open settles them again, with
    /// the same result.
    ///
    /// In portable mode none of this happens: the file is not read, so even one that would be
    /// refused is no obstacle, and the result is an empty ledger and an empty report.
    pub fn open(
        paths: &AppPaths,
        backend: Box<dyn SystemBackend>,
    ) -> Result<(Self, ReconcileReport), LedgerError> {
        let path = Self::file_path(paths);
        if paths.mode() == InstallMode::Portable {
            let ledger = Self {
                path,
                portable: true,
                backend,
                next_id: 1,
                entries: Vec::new(),
            };
            return Ok((ledger, ReconcileReport::default()));
        }
        let (entries, next_id) = match store::load(&path)? {
            Some(file) => {
                // `load` checked that the ids ascend and that the last one is below `u64::MAX`.
                let after_last = file.entries.last().map_or(1, |e| e.id.0 + 1);
                (file.entries, file.next_id.max(after_last))
            }
            None => (Vec::new(), 1),
        };
        let mut ledger = Self {
            path,
            portable: false,
            backend,
            next_id,
            entries,
        };
        let report = ledger.reconcile()?;
        Ok((ledger, report))
    }

    /// Settles the `Pending` entries a crash left, newest first, under the rule of
    /// [`SlotChain`]; saves the file once if any entry changed.
    ///
    /// A `Pending` entry that is the newest left of its setting is confirmed if the system
    /// holds its value. If the system holds its before-state, or nothing (what an apply that
    /// died half-way leaves), the before-state is put back (again, which changes nothing but
    /// removes keys a crash left) and the entry is dropped as rolled back. Anything else is
    /// someone else's value: it is left alone, and the entry and every older entry of the
    /// setting are dropped as skipped without a backend call, as a revert would drop them.
    /// `Applied` entries are never asked about.
    fn reconcile(&mut self) -> Result<ReconcileReport, LedgerError> {
        let mut report = ReconcileReport::default();
        let mut chain = SlotChain::default();
        // Entries only leave the list at the index being settled, so lower indexes stay valid.
        for index in (0..self.entries.len()).rev() {
            let entry = &self.entries[index];
            let target = &entry.change.target;
            let id = entry.id;
            let fail = |error: String| EntryFailure {
                id,
                change: target.to_string(),
                error,
            };

            match chain.get(target) {
                None => {}
                Some(SlotMark::Stuck) => {
                    if entry.state == EntryState::Pending {
                        report.failed.push(fail(
                            "left pending: a newer change to the same setting could not be \
                             settled first"
                                .into(),
                        ));
                    }
                    continue;
                }
                // A newer entry's value is presumed in place.
                Some(SlotMark::Live) => continue,
                // A newer entry found someone else's value: whatever this older entry recorded,
                // the live value is not the app's to overwrite, `Pending` or not.
                Some(SlotMark::Foreign) => {
                    self.take(index);
                    report.skipped.push(id);
                    continue;
                }
            }
            if entry.state == EntryState::Applied {
                chain.mark(target, SlotMark::Live);
                continue;
            }
            match holds(self.backend.as_mut(), entry) {
                Ok(Holds::Ours) => {
                    chain.mark(target, SlotMark::Live);
                    self.entries[index].state = EntryState::Applied;
                    report.confirmed.push(id);
                }
                Ok(Holds::Other) => {
                    chain.mark(target, SlotMark::Foreign);
                    self.take(index);
                    report.skipped.push(id);
                }
                Ok(Holds::Before | Holds::Nothing) => {
                    match self.backend.restore(&entry.change, &entry.before) {
                        Ok(()) => {
                            self.take(index);
                            report.rolled_back.push(id);
                        }
                        Err(e) => {
                            chain.mark(target, SlotMark::Stuck);
                            report.failed.push(fail(e.to_string()));
                        }
                    }
                }
                Err(e) => {
                    chain.mark(target, SlotMark::Stuck);
                    report.failed.push(fail(e.to_string()));
                }
            }
        }
        if !(report.confirmed.is_empty()
            && report.rolled_back.is_empty()
            && report.skipped.is_empty())
        {
            self.persist()?;
        }
        Ok(report)
    }

    /// Reads the recorded entries without a backend; refuses the same files as [`Ledger::open`].
    /// In portable mode the file is not read and the list is empty.
    pub fn inspect(paths: &AppPaths) -> Result<Vec<LedgerEntry>, LedgerError> {
        if paths.mode() == InstallMode::Portable {
            return Ok(Vec::new());
        }
        Ok(store::load(&Self::file_path(paths))?.map_or_else(Vec::new, |file| file.entries))
    }

    /// The recorded entries, in ascending id order.
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    /// The backend, for the steps of a clean uninstall that are not recorded changes (clearing
    /// the preferences domain). In portable mode the system must not be touched: callers never
    /// get here then.
    pub(crate) fn backend_mut(&mut self) -> &mut dyn SystemBackend {
        self.backend.as_mut()
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
    ///
    /// In portable mode this is refused first of all, with [`LedgerError::PortableMode`]: the
    /// system is never changed.
    pub fn apply(&mut self, group: &str, change: Change) -> Result<ApplyOutcome, LedgerError> {
        if self.portable {
            return Err(LedgerError::PortableMode);
        }
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
        // The id this change takes must leave one for the next. Only a tampered file gets here.
        if self.next_id == u64::MAX {
            return Err(LedgerError::Corrupt {
                path: self.path.clone(),
                reason: "no entry id is left for a new change".into(),
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
                self.take(index);
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

    /// Puts the system back to what it was before the changes of `group`, newest first, and
    /// drops them from the ledger. See [`Ledger::revert_all`] for the rules.
    pub fn revert_group(&mut self, group: &str) -> RevertReport {
        self.revert_where(|entry| entry.group == group)
    }

    /// Puts the system back to what it was before every recorded change, newest first, and
    /// drops them from the ledger.
    ///
    /// A value is put back only if the system still holds the value the app wrote, or the
    /// entry's own before-state: then the entry's restore already happened (a crash came before
    /// the ledger file said so, or a power loss dropped the app's write) and it is done again,
    /// which changes nothing but removes keys a crash left, and the entry is `reverted`. An
    /// entry a crash or a failed rollback left `Pending` is also put back over a setting that
    /// holds nothing, which is what its apply leaves if it died half-way. A setting is someone
    /// else's only when it holds none of these: the entry is dropped as `skipped` and the
    /// system is left alone, and so is every older entry for that setting, which is dropped as
    /// `skipped` without asking the system.
    ///
    /// An entry that fails stays recorded, and so do the older entries for the same setting:
    /// the system still holds the newer value, so they could not be told apart from a value
    /// someone else wrote, and dropping them would lose the original before-state. Running
    /// the revert again retries them. After every entry settled the file is saved, so a crash
    /// part-way leaves a ledger that matches what is left to revert.
    ///
    /// In portable mode the report is empty and neither the system nor the file is touched.
    pub fn revert_all(&mut self) -> RevertReport {
        self.revert_where(|_| true)
    }

    /// Reverts the entries `pick` accepts, newest first, under the rule of [`SlotChain`]. In
    /// portable mode there is nothing to revert and the system is not asked.
    fn revert_where(&mut self, pick: impl Fn(&LedgerEntry) -> bool) -> RevertReport {
        let mut report = RevertReport::default();
        if self.portable {
            return report;
        }
        let mut chain = SlotChain::default();
        // Entries only leave the list at the index being settled, so lower indexes stay valid.
        for index in (0..self.entries.len()).rev() {
            let entry = &self.entries[index];
            if !pick(entry) {
                continue;
            }
            let id = entry.id;
            let label = entry.change.target.to_string();
            let fail = |error: String| EntryFailure {
                id,
                change: label,
                error,
            };

            let put_back = match chain.get(&entry.change.target) {
                // A newer entry of the setting stays recorded (in a revert only because it
                // failed): this one waits for it.
                Some(SlotMark::Stuck | SlotMark::Live) => {
                    report.failed.push(fail(
                        "not reverted: a newer change to the same setting could not be reverted \
                         first"
                            .into(),
                    ));
                    continue;
                }
                // A newer entry found someone else's value here: whatever this older entry
                // recorded, the live value is not the app's to overwrite, `Pending` or not.
                Some(SlotMark::Foreign) => false,
                // The app's value, the entry's before-state (then its restore already happened
                // and is done again) or, for a `Pending` entry, the nothing a half-done apply
                // leaves. Only anything else is someone else's.
                None => match holds(self.backend.as_mut(), entry) {
                    Ok(found) => found != Holds::Other,
                    Err(e) => {
                        chain.mark(&entry.change.target, SlotMark::Stuck);
                        report.failed.push(fail(e.to_string()));
                        continue;
                    }
                },
            };
            if !put_back {
                chain.mark(&entry.change.target, SlotMark::Foreign);
                match self.forget(index) {
                    Ok(()) => report.skipped.push(id),
                    Err(e) => report.failed.push(fail(format!(
                        "left alone, but the ledger file could not be updated: {e}"
                    ))),
                }
                continue;
            }
            if let Err(e) = self.backend.restore(&entry.change, &entry.before) {
                chain.mark(&entry.change.target, SlotMark::Stuck);
                report.failed.push(fail(e.to_string()));
                continue;
            }
            match self.forget(index) {
                Ok(()) => report.reverted.push(id),
                Err(e) => report.failed.push(fail(format!(
                    "restored, but the ledger file could not be updated: {e}"
                ))),
            }
        }
        report
    }

    /// Drops the entry at `index` (see [`Ledger::take`]) and saves the file. If the file cannot
    /// be saved, the entries are put back as they were, so that memory and disk agree.
    fn forget(&mut self, index: usize) -> Result<(), LedgerError> {
        let kept = self.entries.clone();
        self.take(index);
        let saved = self.persist();
        if saved.is_err() {
            self.entries = kept;
        }
        saved
    }

    /// Takes the entry at `index` out of the list.
    ///
    /// Each registry key the entry created goes to the newest entry left whose key is that key
    /// or one below it: the key is not empty while that entry's key is in it, so the entry's
    /// restore could not remove it, and the revert of the entry it goes to removes it instead.
    /// Without this, turning off one feature would leave a key it created behind for good once
    /// another feature's key had been put in it (INST-07).
    fn take(&mut self, index: usize) -> LedgerEntry {
        let entry = self.entries.remove(index);
        for created in &entry.before.created_keys {
            let heir = self.entries.iter_mut().rev().find(|other| {
                matches!(&other.change.target, Target::Registry { key, .. } if key_holds(created, key))
            });
            if let Some(heir) = heir {
                adopt(&mut heir.before.created_keys, created);
            }
        }
        entry
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
