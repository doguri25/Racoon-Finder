//! The jobs on the trash as a place (NAV-16): putting items back and deleting them for good.
//! They go through the provider of `shell://trash` (`rf_vfs::ShellProvider` in the app), which
//! acts only on items that lie in a trash folder and refuses anything else. Emptying the trash
//! is a delete of the items its question counted, never a wipe of whatever the trash holds when
//! the job runs.
//!
//! A restore is undone by sending what came back to the trash again (`UndoRecord::Restored`); a
//! delete from the trash cannot be undone and leaves no record.

use std::io;
use std::path::{Path, PathBuf};

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath};
use rf_platform::TrashItem;
use rf_vfs::{Provider, ShellPlace};

use super::{At, Attempt, Ctx, Failed};
use crate::conflict::{Conflict, ConflictAction};
use crate::naming::{MAX_NUMBERED, numbered_name};
use crate::report::{FailureReason, Stage, Totals};
use crate::spec::ErrorPolicy;
use crate::undo::{TrashedItem, UndoRecord};

/// Every step reports its failure and goes on to the next item.
const ERRORS: ErrorPolicy = ErrorPolicy::Continue;

/// How many times a restore whose place is taken, and then free again before it could be looked
/// at, is tried before it fails: another program that keeps making and removing the item there.
const TRIES: usize = 3;

/// Counts `item` as one folder or one file in `totals`.
fn count(totals: &mut Totals, item: &TrashItem) {
    if item.is_dir {
        totals.dirs += 1;
    } else {
        totals.files += 1;
    }
}

/// The failure, at `stage`, of `error` from the trash: an item that is no longer the one listed
/// (`rf_platform::is_changed_since_listed`) has the reason `TrashItemChanged`.
fn trash_failure(stage: Stage, error: Error) -> Failed {
    if rf_platform::is_changed_since_listed(&error) {
        Failed::refused(stage, error, FailureReason::TrashItemChanged)
    } else {
        Failed::new(stage, error)
    }
}

/// The place of `item` in the trash as a local path: what its failures name as their source.
fn place_of(item: &TrashItem) -> Option<VPath> {
    VPath::local(&item.location).ok()
}

/// Puts each `(item, to)` back from the trash, as one step under [`ERRORS`] (see
/// [`restore_one`]); the plan counts the items. What came back goes to the outputs at the path it
/// took, and the report's record sends it to the trash again (`UndoRecord::Restored`); `None`
/// when nothing came back. A cancel stops the job between two items.
pub(crate) fn run_restore(ctx: &mut Ctx<'_>, items: &[(TrashItem, PathBuf)]) {
    let mut totals = Totals::default();
    for (item, _) in items {
        count(&mut totals, item);
    }
    ctx.planned(totals);
    let trash = ShellPlace::Trash.path();
    let mut restored = Vec::new();
    for (item, to) in items {
        let source = place_of(item);
        let target = VPath::local(to).ok();
        let back = ctx.attempt(ERRORS, source.as_ref(), target.as_ref(), |ctx| {
            restore_one(ctx, &trash, item, to)
        });
        match back {
            Attempt::Done(Some(at)) => {
                restored.push(TrashedItem {
                    original: at.clone(),
                    location: source,
                    stamp: None,
                });
                ctx.report.outputs.push(at);
                ctx.item_done(!item.is_dir, item.is_dir);
            }
            Attempt::Done(None) => ctx.report.skipped += 1,
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
    if !restored.is_empty() {
        ctx.report.undo = Some(UndoRecord::Restored { items: restored });
    }
}

/// Puts `item` back at `to` through the provider of `trash`, and gives the path it took, or
/// `None` when the user skipped it.
///
/// Nothing is replaced. When `to` is taken, the conflict goes to the job's resolver (once per
/// job with "apply to all") with what is there and the item as the trash holds it: `Skip` leaves
/// the item in the trash, `Rename` puts it back under the first free numbered name next to `to`
/// (`a (2).txt`, by the job's naming rule; `NoFreeName` when every one is taken), `Cancel`
/// cancels the job, and any overwrite fails as `Stage::Conflict` with `RestoreNeverReplaces`.
/// Every other failure is `Stage::Restore` with the provider's error: `Unsupported` for an item
/// that is not in a trash folder, `TrashItemChanged` for one that is no longer the item listed,
/// `NotFound` with `LeftTheTrash` when the item is no longer at its place in the trash (whatever
/// the folder of `to`), with `OriginalFolderGone` when the folder of `to` is gone (nothing is
/// made), and `CrossesDevices` for a `to` on another volume than the trash.
fn restore_one(
    ctx: &mut Ctx<'_>,
    trash: &VPath,
    item: &TrashItem,
    to: &Path,
) -> Result<Option<VPath>, Failed> {
    ctx.check().at(Stage::Restore)?;
    let provider = ctx.provider(trash).at(Stage::Restore)?;
    let target = VPath::local(to).map_err(Error::from).at(Stage::Restore)?;
    ctx.working_on(&target);
    for _ in 0..TRIES {
        match provider.restore_from_trash(item, &target) {
            Ok(()) => return Ok(Some(target)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            // Another app took it out or deleted it: no other place would help.
            Err(error) if error.kind() == ErrorKind::NotFound && left_the_trash(ctx, item) => {
                return Err(Failed::refused(
                    Stage::Restore,
                    error,
                    FailureReason::LeftTheTrash,
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound && folder_gone(ctx, &target) => {
                return Err(Failed::refused(
                    Stage::Restore,
                    error,
                    FailureReason::OriginalFolderGone,
                ));
            }
            Err(error) => return Err(trash_failure(Stage::Restore, error)),
        }
        let existing = match ctx.provider(&target).and_then(|local| local.stat(&target)) {
            Ok(existing) => existing,
            // Free again: the next try takes it.
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(Failed::new(Stage::Conflict, error)),
        };
        let conflict = Conflict {
            source: place_of(item).unwrap_or_else(|| trash.clone()),
            target: target.clone(),
            incoming: incoming(ctx, item),
            existing,
        };
        return match ctx.resolve_conflict(&conflict).at(Stage::Conflict)? {
            ConflictAction::Skip => Ok(None),
            ConflictAction::Rename => restore_numbered(ctx, &*provider, item, &target).map(Some),
            ConflictAction::Overwrite
            | ConflictAction::OverwriteIfNewer
            | ConflictAction::OverwriteIfSizeDiffers => Err(never_replaces(&target)),
            // `resolve_conflict` gives `Error::Cancelled` for it.
            ConflictAction::Cancel => Err(Failed::new(Stage::Conflict, Error::Cancelled)),
        };
    }
    let error = Error::io(
        "restore",
        target.clone(),
        io::Error::from(io::ErrorKind::AlreadyExists),
    );
    Err(Failed::new(Stage::Conflict, error))
}

/// Whether `item` is no longer at its place in the trash: a `stat` there (of the item itself,
/// a link not followed) says `NotFound`.
fn left_the_trash(ctx: &Ctx<'_>, item: &TrashItem) -> bool {
    place_of(item).is_some_and(|place| {
        ctx.provider(&place)
            .and_then(|provider| provider.stat(&place))
            .is_err_and(|error| error.kind() == ErrorKind::NotFound)
    })
}

/// Whether the folder `target` was to go in is gone (its `stat` says `NotFound`), rather than
/// the item that was to go there.
fn folder_gone(ctx: &Ctx<'_>, target: &VPath) -> bool {
    target.parent().is_some_and(|folder| {
        ctx.provider(&folder)
            .and_then(|provider| provider.stat(&folder))
            .is_err_and(|error| error.kind() == ErrorKind::NotFound)
    })
}

/// The item as a conflict shows it: what the local provider finds at its place in the trash
/// (its times and size), under the name it had, or, when that cannot be read, what the trash
/// says of it.
fn incoming(ctx: &Ctx<'_>, item: &TrashItem) -> Entry {
    let found = place_of(item).and_then(|place| ctx.provider(&place).ok()?.stat(&place).ok());
    let mut entry = found.unwrap_or_else(|| {
        let kind = if item.is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let mut entry = Entry::new(item.name.clone(), kind);
        entry.size = item.size.unwrap_or(0);
        entry.modified = item.deleted;
        entry
    });
    entry.name.clone_from(&item.name);
    entry
}

/// The failure of an item whose place is taken and which the answer would have put over what is
/// there: `Stage::Conflict`, `AlreadyExists`, `RestoreNeverReplaces`.
fn never_replaces(target: &VPath) -> Failed {
    let error = Error::io(
        "restore",
        target.clone(),
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a restore from the trash never replaces an item",
        ),
    );
    Failed::refused(Stage::Conflict, error, FailureReason::RestoreNeverReplaces)
}

/// Puts `item` back under the first free numbered name next to `target` ([`numbered_name`] by
/// the job's naming rule, from 2 to [`MAX_NUMBERED`]), each one tried by the restore itself,
/// which never replaces anything.
fn restore_numbered(
    ctx: &Ctx<'_>,
    provider: &dyn Provider,
    item: &TrashItem,
    target: &VPath,
) -> Result<VPath, Failed> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        let error = Error::Unsupported(format!("no name to number: {target}"));
        return Err(Failed::refused(
            Stage::Conflict,
            error,
            FailureReason::NoTargetFolder,
        ));
    };
    for n in 2..=MAX_NUMBERED {
        ctx.check_cancelled().at(Stage::Restore)?;
        let candidate = parent
            .join(numbered_name(name, n, item.is_dir, ctx.env.naming_os))
            .map_err(Error::from)
            .at(Stage::Restore)?;
        match provider.restore_from_trash(item, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(trash_failure(Stage::Restore, error).writing(&candidate));
            }
        }
    }
    let error = Error::io(
        "restore",
        target.clone(),
        io::Error::from(io::ErrorKind::AlreadyExists),
    );
    Err(Failed::refused(
        Stage::Conflict,
        error,
        FailureReason::NoFreeName,
    ))
}

/// Deletes each of `items` from the trash for good, with its record, as one step under
/// [`ERRORS`] through the provider of `shell://trash`; the plan counts them (see
/// [`delete_each`]). There is no undo record.
pub(crate) fn run_delete(ctx: &mut Ctx<'_>, items: &[TrashItem]) {
    let mut totals = Totals::default();
    for item in items {
        count(&mut totals, item);
    }
    ctx.planned(totals);
    delete_each(ctx, items);
}

/// Deletes each of `items` from the trash for good, as one step each under [`ERRORS`]. An item
/// that is gone already is skipped. A failure is `Stage::Delete` with the provider's error
/// (`Unsupported` for an item that is not in a trash folder, `TrashItemChanged` for one that is
/// no longer the item listed), and its item stays as it is. A cancel stops between two items.
fn delete_each(ctx: &mut Ctx<'_>, items: &[TrashItem]) {
    let trash = ShellPlace::Trash.path();
    for item in items {
        let source = place_of(item);
        let deleted = ctx.attempt(ERRORS, source.as_ref(), None, |ctx| {
            ctx.check().at(Stage::Delete)?;
            let provider = ctx.provider(&trash).at(Stage::Delete)?;
            ctx.working_on(source.as_ref().unwrap_or(&trash));
            match provider.delete_from_trash(item) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
                Err(error) => Err(trash_failure(Stage::Delete, error)),
            }
        });
        match deleted {
            Attempt::Done(true) => ctx.item_done(!item.is_dir, item.is_dir),
            Attempt::Done(false) => ctx.report.skipped += 1,
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
}
