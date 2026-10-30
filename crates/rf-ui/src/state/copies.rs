//! The copies that an undo of a copy could not send to the trash (where there is none: Linux,
//! a USB stick, a network share). An undo never deletes anything for good (the undo contract
//! of rf-jobs), so they stay, and only the user may have them deleted for good. The window
//! offers that only once a worker has found each copy as its job left it and its original as
//! it was ([`Loader::check_copy`](crate::loader::Loader::check_copy)), while a notice says it
//! looks: the danger confirmation lists every copy by its whole path. The user's yes submits a
//! delete of exactly those copies (`JobSpec::DeleteCopies`), which looks at each copy and its
//! original once more right before it deletes it, whenever it runs, and keeps any that changed
//! meanwhile. Once it has deleted them all, the copy's record leaves the history: the copy is
//! undone. A child of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The originals come from the copy jobs of this run: when one ends, the state notes which
//! source each item it made came from ([`copy_origins`]). A copy whose original cannot be told
//! (a copy of an earlier run, a name two items could have taken) is never offered.

use std::collections::VecDeque;
use std::time::SystemTime;

use rf_core::{Os, VPath};
use rf_jobs::{
    CopyFound, CopyOf, CreatedItem, DeleteCopiesSpec, FailureReason, ItemFailure, JobReport,
    JobSpec, MAX_NUMBERED, Outcome, TransferSpec, UNDO_LIMIT, UndoRecord, numbered_name,
};

use super::jobs::{deleted_for_good, submit};
use super::{AppState, Effect, ToastAction};
use crate::dialogs::{Confirm, Failures, Modal};
use crate::jobs::JobEffect;
use crate::pane::ancestors;
use crate::words::{self, CopyKept};

/// The items a copy job of this run made, each with the source it came from when that can be
/// told, under the job's undo record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopyOrigins {
    record: UndoRecord,
    pairs: Vec<(VPath, Option<VPath>)>,
}

/// The copies an undo could not trash, while a worker looks at them and their originals, to
/// find out whether to offer them for a permanent delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopiesCheck {
    token: u64,
    /// The token of the notice that shows while the worker looks.
    note: u64,
    /// The record of the copy the undo was to undo.
    record: UndoRecord,
    /// The copies, in the order the worker's answers come.
    copies: Vec<VPath>,
    /// Each copy's original, in the same order.
    originals: Vec<VPath>,
    /// Each copy as its job left it, in the same order, and when the job ended; empty until
    /// the look starts.
    made: Vec<CreatedItem>,
    at: Option<SystemTime>,
    /// The lines of the undo's other failures, and of what it deleted for good instead of
    /// trashing it, for the toast's list.
    other_lines: Vec<String>,
    /// How many of the undo's failures those are, and how many items it deleted for good.
    other_failed: usize,
    gone: usize,
}

/// The copies of a copy record whose originals are known.
#[derive(Debug, Default)]
pub(super) struct Copies {
    /// The latest copy jobs' origins, the oldest first: as many as the history keeps records.
    origins: VecDeque<CopyOrigins>,
    /// The check under way, if any; a newer one replaces it.
    checking: Option<CopiesCheck>,
    /// The token of the last check.
    token: u64,
    /// The deletes of copies the user confirmed, each with the record of the copy it undoes.
    deleting: Vec<(JobSpec, UndoRecord)>,
}

impl AppState {
    /// Whether a worker looks at copies an undo could not trash, a look that may end in a
    /// question to the user.
    pub(super) fn checking_copies(&self) -> bool {
        self.copies.checking.is_some()
    }

    /// Notes where the items of the copy job that gave `report` came from, when its record is
    /// a copy's.
    pub(super) fn note_copies(&mut self, report: &JobReport) {
        let (JobSpec::Transfer(spec), Some(record @ UndoRecord::Copied { created, .. })) =
            (&report.spec, &report.undo)
        else {
            return;
        };
        let origins = &mut self.copies.origins;
        origins.retain(|noted| noted.record != *record);
        origins.push_back(CopyOrigins {
            record: record.clone(),
            pairs: copy_origins(spec, created),
        });
        while origins.len() > UNDO_LIMIT {
            origins.pop_front();
        }
    }

    /// Takes the end of the undo of the copy `record`, whose trash refused the copies
    /// `refused` and which failed for `others` besides (and deleted the items of `report`'s
    /// `DeletedNotTrashed` warnings for good). When every copy's original is known, a worker
    /// looks at them first ([`Effect::CheckCopies`]); otherwise nothing is offered, and a
    /// toast lists the items and why.
    pub(super) fn copies_refused(
        &mut self,
        record: &UndoRecord,
        refused: Vec<VPath>,
        others: &[&ItemFailure],
        report: &JobReport,
    ) -> Vec<Effect> {
        let gone: Vec<&ItemFailure> = deleted_for_good(report).collect();
        let other_lines: Vec<String> = others
            .iter()
            .chain(&gone)
            .map(|failure| words::failure_line(failure))
            .collect();
        let origins = self
            .copies
            .origins
            .iter()
            .find(|noted| noted.record == *record);
        let originals: Option<Vec<VPath>> = refused
            .iter()
            .map(|copy| {
                origins?
                    .pairs
                    .iter()
                    .find(|(made, _)| made == copy)
                    .and_then(|(_, original)| original.clone())
            })
            .collect();
        let check = CopiesCheck {
            token: 0,
            note: 0,
            record: record.clone(),
            copies: refused,
            originals: originals.unwrap_or_default(),
            made: Vec::new(),
            at: None,
            other_lines,
            other_failed: others.len(),
            gone: gone.len(),
        };
        self.start_check(check)
    }

    /// The user said yes to `spec`, the delete for good of the copies of the copy `record`:
    /// it is submitted, and it looks at each copy and its original once more itself before it
    /// deletes it.
    pub(super) fn copies_confirmed(
        &mut self,
        spec: DeleteCopiesSpec,
        record: UndoRecord,
    ) -> Vec<Effect> {
        let spec = JobSpec::DeleteCopies(spec);
        self.copies.deleting.push((spec.clone(), record));
        vec![submit(spec, None)]
    }

    /// Has a worker look at the copies of `check` and their originals, under a notice that
    /// says so; it replaces a check under way. Copies whose originals, or what their job left,
    /// are not known are never looked at: the toast says why they stay.
    fn start_check(&mut self, mut check: CopiesCheck) -> Vec<Effect> {
        let made = match &check.record {
            UndoRecord::Copied { created, at, .. } => check
                .copies
                .iter()
                .map(|copy| created.iter().find(|item| item.path == *copy).cloned())
                .collect::<Option<Vec<CreatedItem>>>()
                .map(|made| (made, *at)),
            _ => None,
        };
        let Some((made, at)) = made.filter(|_| check.originals.len() == check.copies.len()) else {
            // No answer for a copy: its original cannot be told.
            return vec![self.copies_kept(&check, &[])];
        };
        self.copies.token += 1;
        check.token = self.copies.token;
        let (note, busy) = self.busy(words::checking_originals());
        check.note = note;
        let token = check.token;
        let copies = made.iter().cloned().zip(check.originals.clone()).collect();
        (check.made, check.at) = (made, at);
        self.copies.checking = Some(check);
        vec![busy, Effect::CheckCopies { token, copies, at }]
    }

    /// What the worker found of the copies of the check `token` and their originals, one for
    /// each copy, in order. With every one as it was, the user is asked whether the copies go
    /// for good; otherwise a toast lists them and why they stay. The notice of the check goes.
    /// A check that has been replaced since, or that ends while the app ends, gives nothing.
    pub fn copies_checked(&mut self, token: u64, found: &[CopyFound]) -> Vec<Effect> {
        let Some(check) = self.copies.checking.take_if(|check| check.token == token) else {
            return Vec::new();
        };
        if self.quitting {
            return Vec::new();
        }
        let mut effects = vec![Effect::NotBusy(check.note)];
        let intact =
            found.len() == check.copies.len() && found.iter().all(|&f| f == CopyFound::SAME);
        if !intact {
            effects.push(self.copies_kept(&check, found));
            return effects;
        }
        if !check.other_lines.is_empty() {
            let shown = Failures {
                title: words::failures_title().to_owned(),
                lines: check.other_lines.clone(),
            };
            let text = words::and_gone(words::items_failed(check.other_failed), check.gone);
            effects.push(self.toast(text, Some(ToastAction::Details(shown))));
        }
        let spec = DeleteCopiesSpec {
            copies: check
                .made
                .into_iter()
                .zip(check.originals)
                .map(|(made, original)| CopyOf { made, original })
                .collect(),
            at: check.at,
            resumed: false,
        };
        self.open(Modal::Confirm(Confirm::delete_copies(spec, check.record)));
        effects
    }

    /// Takes the end of the job that gave `report` when it is a delete of copies the user
    /// confirmed: with every copy gone, the copy's record leaves the history (the window has
    /// undone it) and a toast says so, counting the copies it deleted (not those that were gone
    /// already). `None` for any other job; for one that did not delete them all (a copy or an
    /// original changed after the offer: the job kept that copy), the usual notice tells why,
    /// and the record stays to be undone again.
    pub(super) fn copies_deleted(&mut self, report: &JobReport) -> Option<Vec<Effect>> {
        let index = self
            .copies
            .deleting
            .iter()
            .position(|(spec, _)| *spec == report.spec)?;
        let (spec, record) = self.copies.deleting.remove(index);
        let all = report.outcome == Outcome::Completed && report.failures.is_empty();
        if !all || self.quitting {
            return None;
        }
        // A copy that was gone already is skipped, not deleted.
        let deleted = match &spec {
            JobSpec::DeleteCopies(delete) => {
                let skipped = usize::try_from(report.skipped).unwrap_or(usize::MAX);
                delete.copies.len().saturating_sub(skipped)
            }
            _ => 0,
        };
        let text = if deleted == 0 {
            words::copies_were_gone().to_owned()
        } else {
            words::copies_deleted(deleted)
        };
        self.copies.origins.retain(|noted| noted.record != record);
        Some(vec![
            Effect::Job(JobEffect::ForgetUndo(record)),
            self.toast(text, None),
        ])
    }

    /// The toast of the copies of `check` that stay, `found` saying how each one and its
    /// original are (none for a copy whose original is not known): each copy with why, after
    /// the undo's other failures.
    fn copies_kept(&mut self, check: &CopiesCheck, found: &[CopyFound]) -> Effect {
        let refused = words::failure_reason(&FailureReason::TrashRefused);
        let mut lines = check.other_lines.clone();
        for (i, copy) in check.copies.iter().enumerate() {
            // `None` for a copy as it was: another copy, or its original, stands in the way.
            let why = found
                .get(i)
                .map_or(Some(CopyKept::OriginalUnknown), |&found| {
                    CopyKept::of(found)
                });
            lines.push(match why {
                Some(why) => format!("{copy}: {refused}. {}", words::copy_kept(why)),
                None => format!("{copy}: {refused}"),
            });
        }
        let shown = Failures {
            title: words::failures_title().to_owned(),
            lines,
        };
        let failed = check.other_failed + check.copies.len();
        let text = words::and_gone(words::items_failed(failed), check.gone);
        self.toast(text, Some(ToastAction::Details(shown)))
    }
}

/// Each item a copy of `spec` made (`created`, as its record lists them) with the source it
/// came from, when that can be told from the paths:
///
/// - the item is the target of one of the copy's items: that item's source;
/// - it is inside such a target, a folder the copy merged into: the item of that source at the
///   same place below it;
/// - it is beside such a target, under the target's name with a number
///   ([`rf_jobs::numbered_name`]): that item's source.
///
/// A path that more than one item could have made, or none, has no source (`None`). A merged
/// folder's item that took a numbered name is taken for the source of its literal name; the
/// worker's look at the original catches it, as that differs.
pub fn copy_origins(spec: &TransferSpec, created: &[CreatedItem]) -> Vec<(VPath, Option<VPath>)> {
    created
        .iter()
        .map(|made| (made.path.clone(), original_of(spec, made)))
        .collect()
}

/// The source the copy of `spec` made `made` from ([`copy_origins`]).
fn original_of(spec: &TransferSpec, made: &CreatedItem) -> Option<VPath> {
    let path = &made.path;
    let only = |matching: Vec<&rf_jobs::TransferItem>| match matching.as_slice() {
        [one] => Some(Some((*one).clone())),
        [] => None,
        _ => Some(None),
    };
    let targets = spec
        .items
        .iter()
        .filter(|item| item.target == *path)
        .collect();
    if let Some(item) = only(targets) {
        return item.map(|item| item.source);
    }
    let holders = spec
        .items
        .iter()
        .filter(|item| path.starts_with(&item.target))
        .collect();
    if let Some(item) = only(holders) {
        return item.and_then(|item| below(&item.source, &item.target, path));
    }
    let (folder, name) = (path.parent()?, path.file_name()?);
    let numbered = spec
        .items
        .iter()
        .filter(|item| {
            item.target.parent().as_ref() == Some(&folder)
                && item.target.file_name().is_some_and(|base| {
                    (2..=MAX_NUMBERED)
                        .any(|n| numbered_name(base, n, made.is_dir, Os::CURRENT) == name)
                })
        })
        .collect();
    only(numbered).flatten().map(|item| item.source)
}

/// The path below `source` where `path` is below `target`.
fn below(source: &VPath, target: &VPath, path: &VPath) -> Option<VPath> {
    let chain = ancestors(path);
    let at = chain.iter().position(|folder| folder == target)?;
    chain[at + 1..]
        .iter()
        .try_fold(source.clone(), |into, step| {
            into.join(step.file_name()?).ok()
        })
}
