//! The copies that an undo of a copy could not send to the trash (where there is none: Linux,
//! a USB stick, a network share). An undo never deletes anything for good (the undo contract
//! of rf-jobs), so they stay, and only the user may have them deleted for good. The window
//! offers that only once a worker has found each copy's original as it was
//! ([`Loader::compare_with_original`](crate::loader::Loader::compare_with_original)): the
//! danger confirmation lists every copy by its whole path, and its job is a permanent delete of
//! exactly those. Once it has deleted them all, the copy's record leaves the history: the copy
//! is undone. A child of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The originals come from the copy jobs of this run: when one ends, the state notes which
//! source each item it made came from ([`copy_origins`]). A copy whose original cannot be told
//! (a copy of an earlier run, a name two items could have taken) is never offered.

use std::collections::VecDeque;

use rf_core::{Os, VPath};
use rf_jobs::{
    CreatedItem, FailureReason, ItemFailure, JobReport, JobSpec, MAX_NUMBERED, Outcome,
    TransferSpec, UNDO_LIMIT, UndoRecord, numbered_name,
};

use super::jobs::deleted_for_good;
use super::{AppState, Effect, ToastAction};
use crate::dialogs::{Confirm, Failures, Modal};
use crate::jobs::JobEffect;
use crate::loader::Original;
use crate::pane::ancestors;
use crate::words::{self, CopyKept};

/// The items a copy job of this run made, each with the source it came from when that can be
/// told, under the job's undo record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopyOrigins {
    record: UndoRecord,
    pairs: Vec<(VPath, Option<VPath>)>,
}

/// The copies an undo could not trash, while a worker looks at their originals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopiesCheck {
    token: u64,
    /// The record of the copy the undo was to undo.
    record: UndoRecord,
    /// The copies, in the order the worker's answers come.
    copies: Vec<VPath>,
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
    /// The permanent deletes of copies the user confirmed, each with the record of the copy
    /// it undoes.
    deleting: Vec<(JobSpec, UndoRecord)>,
}

impl AppState {
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
    /// looks at them first ([`Effect::CheckOriginals`]); otherwise nothing is offered, and a
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
        let mut check = CopiesCheck {
            token: 0,
            record: record.clone(),
            copies: refused,
            other_lines,
            other_failed: others.len(),
            gone: gone.len(),
        };
        let Some(originals) = originals else {
            let unknown = vec![Original::Unknown; check.copies.len()];
            return vec![self.copies_kept(&check, &unknown)];
        };
        self.copies.token += 1;
        check.token = self.copies.token;
        let pairs = check.copies.iter().cloned().zip(originals).collect();
        let token = check.token;
        self.copies.checking = Some(check);
        vec![Effect::CheckOriginals { token, pairs }]
    }

    /// What the worker found of the originals of the check `token`, one for each copy, in
    /// order. With every one as it was, the user is asked whether the copies go for good;
    /// otherwise a toast lists them and why they stay. A check that has been replaced since, or
    /// that ends while the app ends, gives nothing.
    pub fn originals_checked(&mut self, token: u64, found: &[Original]) -> Vec<Effect> {
        let Some(check) = self.copies.checking.take_if(|check| check.token == token) else {
            return Vec::new();
        };
        if self.quitting {
            return Vec::new();
        }
        let intact =
            found.len() == check.copies.len() && found.iter().all(|&o| o == Original::Same);
        if !intact {
            return vec![self.copies_kept(&check, found)];
        }
        let mut effects = Vec::new();
        if !check.other_lines.is_empty() {
            let shown = Failures {
                title: words::failures_title().to_owned(),
                lines: check.other_lines.clone(),
            };
            let text = words::and_gone(words::items_failed(check.other_failed), check.gone);
            effects.push(self.toast(text, Some(ToastAction::Details(shown))));
        }
        self.open(Modal::Confirm(Confirm::delete_copies(
            check.copies,
            check.record,
        )));
        effects
    }

    /// Notes that the user confirmed `spec`, the permanent delete of the copies of `record`.
    pub(super) fn deleting_copies(&mut self, spec: JobSpec, record: UndoRecord) {
        self.copies.deleting.push((spec, record));
    }

    /// Takes the end of the job that gave `report` when it is a permanent delete of copies
    /// the user confirmed: with every copy gone, the copy's record leaves the history (the
    /// window has undone it) and a toast says so. `None` for any other job; for one that did
    /// not delete them all, the usual notice tells why, and the record stays to be undone again.
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
        let count = match &spec {
            JobSpec::Delete(delete) => delete.paths.len(),
            _ => 0,
        };
        self.copies.origins.retain(|noted| noted.record != record);
        Some(vec![
            Effect::Job(JobEffect::ForgetUndo(record)),
            self.toast(words::copies_deleted(count), None),
        ])
    }

    /// The toast of the copies of `check` that stay, `found` saying how each one's original
    /// is: each copy with why, after the undo's other failures.
    fn copies_kept(&mut self, check: &CopiesCheck, found: &[Original]) -> Effect {
        let refused = words::failure_reason(&FailureReason::TrashRefused);
        let mut lines = check.other_lines.clone();
        for (i, copy) in check.copies.iter().enumerate() {
            let why = match found.get(i).copied().unwrap_or(Original::Unknown) {
                // Another copy's original stands in the way: this one is as it was.
                Original::Same => None,
                Original::Missing => Some(CopyKept::OriginalMissing),
                Original::Changed => Some(CopyKept::OriginalChanged),
                Original::Unknown => Some(CopyKept::OriginalUnknown),
            };
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
