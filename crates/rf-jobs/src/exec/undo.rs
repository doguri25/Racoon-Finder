//! Reversing an earlier job from its record (OPS-12): an undo, and a redo, which is the undo of
//! an undo.
//!
//! An undo never destroys what the user changed after the job. What it takes away goes to the
//! trash, never for good, and never instead of the trash where there is none; only a new item
//! it removes again goes for good, while it is still an empty file or folder. An item that is
//! no longer as the job left it stays where it is, and the report says so, as a `Stage::Undo`
//! failure. An undo asks nothing: a conflict is skipped, a failure is reported and the undo
//! goes on, and the pre-flight review goes ahead as planned (`Ctx::interactive` is false for an
//! undo).
//!
//! The report's undo record reverses what the undo did, so that a redo runs it.

use std::ops::ControlFlow;
use std::time::SystemTime;

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath};
use rf_vfs::{DeleteMode, Deleted, Provider};

use super::{At, Attempt, Ctx, Failed, MAX_DEPTH, Stopped, simple, transfer};
use crate::report::{Stage, Totals};
use crate::spec::{
    ConflictPolicy, CreateKind, ErrorPolicy, SymlinkMode, TransferItem, TransferMode,
    TransferOptions, TransferSpec,
};
use crate::undo::{CreatedItem, MovedItem, TrashedItem, UndoRecord};

/// Every step of an undo reports its failure and goes on to the next item.
const ERRORS: ErrorPolicy = ErrorPolicy::Continue;

/// How the undo of a move moves its items back: an item whose old place is taken stays where
/// it is, a failure goes on to the next item, and a link moves as the link itself.
const MOVE_BACK: TransferOptions = TransferOptions {
    conflict: ConflictPolicy::Skip,
    errors: ErrorPolicy::Continue,
    symlinks: SymlinkMode::CopyLink,
};

/// Reverses the job that left `record`:
///
/// - `Copied`: see [`undo_copy`]. `Restored`: see [`trash_again`].
/// - `Moved`: see [`undo_move`].
/// - `Renamed`: the rename back, `to` to `from`, run as a rename job is (a taken `from` fails
///   it and nothing is replaced); its record renames forth again.
/// - `Created`: see [`remove_created`]. `Removed`: see [`make_again`].
/// - `Trashed`: see [`restore`].
pub(crate) fn run_undo(ctx: &mut Ctx<'_>, record: &UndoRecord) {
    match record {
        UndoRecord::Copied {
            created,
            overwritten,
            at,
        } => undo_copy(ctx, created, *overwritten, *at),
        UndoRecord::Moved {
            moves,
            created_dirs,
            removed_dirs,
        } => undo_move(ctx, moves, created_dirs, removed_dirs),
        UndoRecord::Renamed { from, to } => simple::run_rename(ctx, to, from),
        UndoRecord::Created { path, kind } => remove_created(ctx, path, *kind),
        UndoRecord::Removed { path, kind } => make_again(ctx, path, *kind),
        UndoRecord::Trashed { items } => restore(ctx, items),
        UndoRecord::Restored { items } => trash_again(ctx, items),
    }
}

/// Counts one folder (`is_dir`) or one file in `totals`.
fn count(totals: &mut Totals, is_dir: bool) {
    if is_dir {
        totals.dirs += 1;
    } else {
        totals.files += 1;
    }
}

/// Whether the item at `path`, if any, is a folder by `stat`; `false` when it cannot be told.
fn is_dir_at(ctx: &Ctx<'_>, path: Option<&VPath>) -> bool {
    path.and_then(|path| ctx.provider(path).ok()?.stat(path).ok())
        .is_some_and(|entry| entry.kind == EntryKind::Dir)
}

/// The failure of an item that an undo leaves where it is, because it is not as the job left
/// it: `Stage::Undo` with `Unsupported` and the text "{path} {why}; left in place".
fn left(path: &VPath, why: &str) -> Failed {
    let error = Error::Unsupported(format!("{path} {why}; left in place"));
    Failed::new(Stage::Undo, error)
}

/// Undoes a copy. Each file it replaced (`overwritten`) is a `Stage::Undo` warning: its old
/// content is gone, and the file stays as the copy left it. Each item it made (`created`) goes
/// to the trash (see [`trash_item`]) only if it is still as the copy left it (see
/// [`check_copied`]); one that changed stays, with a `Stage::Undo` failure, and one that is
/// gone is skipped. The plan counts the items it made.
///
/// Without a trash (Linux, a network share, a USB stick) nothing is deleted at all: each item
/// fails as `Stage::Delete` with the trash's error and stays. The undo's record puts back what
/// went to the trash (`UndoRecord::Trashed`); `None` when nothing did.
fn undo_copy(ctx: &mut Ctx<'_>, created: &[CreatedItem], overwritten: u64, at: Option<SystemTime>) {
    let mut totals = Totals::default();
    for item in created {
        count(&mut totals, item.is_dir);
    }
    ctx.planned(totals);
    for _ in 0..overwritten {
        let error = Error::Unsupported(
            "a file was overwritten by the copy; its old content cannot be brought back".to_owned(),
        );
        ctx.warn(Failed::new(Stage::Undo, error), None, None);
    }
    let mut trashed = Vec::new();
    for item in created {
        let check = |ctx: &Ctx<'_>, provider: &dyn Provider, now: &Entry| {
            check_copied(ctx, provider, item, now, at)
        };
        if trash_item(ctx, &item.path, check, &mut trashed).is_break() {
            break;
        }
    }
    if !trashed.is_empty() {
        ctx.report.undo = Some(UndoRecord::Trashed { items: trashed });
    }
}

/// Checks that `item`, which a copy made and `stat` now finds as `now`, is still as the copy
/// left it. Anything else, or anything that cannot be told, fails as [`left`] with "changed
/// after it was copied".
///
/// Its identity must be the recorded one when both are known, and its modification time must
/// be known and the recorded one (a time before 1970 is not recorded, so such an item always
/// stays).
///
/// - A file (or a link): it must be no folder, of the recorded size.
/// - A folder: it must be a folder, the job's time `at` must be known, and nothing under it may
///   be newer than `at` (see [`untouched_since`]). The folder's own time catches an item added
///   to it or taken from it; an item changed, added or taken deeper down leaves a time after
///   `at` on itself or on its folder. A file that the copy gave a time after `at`, because its
///   source has a time in the future, makes its folder stay too.
fn check_copied(
    ctx: &Ctx<'_>,
    provider: &dyn Provider,
    item: &CreatedItem,
    now: &Entry,
    at: Option<SystemTime>,
) -> Result<(), Failed> {
    let other_item = matches!(
        (item.identity, now.identity()),
        (Some(was), Some((volume, file))) if was != (volume.0, file.0)
    );
    let same_time = matches!((item.modified, now.modified), (Some(was), Some(is)) if was == is);
    let unchanged = !other_item
        && same_time
        && if item.is_dir {
            match at {
                Some(at) if now.kind == EntryKind::Dir => {
                    untouched_since(ctx, provider, &item.path, now, at, 1)?
                }
                _ => false,
            }
        } else {
            now.kind != EntryKind::Dir && now.size == item.size
        };
    if unchanged {
        Ok(())
    } else {
        Err(left(&item.path, "changed after it was copied"))
    }
}

/// Whether nothing under the folder `dir` of `provider`, which `entry` describes, is newer than
/// `at`: every item it lists has a known modification time, none of them after `at`, and every
/// real folder among them (`EntryKind::Dir`, as the listing gives it: a link is never followed)
/// holds the same. `depth` is how far below the copied folder the listed items are (its own
/// items are at 1).
///
/// The copy went no deeper than [`MAX_DEPTH`], so a folder whose items would be deeper must be
/// empty. A folder on another volume than the one it is in (both volume IDs known) is not
/// entered and counts as a change. A folder that cannot be listed fails as `Stage::Undo` with
/// the listing's error, and a cancel of the job stops the walk.
fn untouched_since(
    ctx: &Ctx<'_>,
    provider: &dyn Provider,
    dir: &VPath,
    entry: &Entry,
    at: SystemTime,
    depth: usize,
) -> Result<bool, Failed> {
    let children = ctx.list(provider, dir).at(Stage::Undo)?;
    if depth > MAX_DEPTH {
        return Ok(children.is_empty());
    }
    for child in &children {
        if child.modified.is_none_or(|modified| modified > at) {
            return Ok(false);
        }
        if child.kind != EntryKind::Dir {
            continue;
        }
        let elsewhere = matches!(
            (entry.volume_id, child.volume_id),
            (Some(here), Some(there)) if here != there
        );
        if elsewhere {
            return Ok(false);
        }
        let path = dir
            .join(&child.name)
            .map_err(|error| Failed::new(Stage::Undo, error.into()))?;
        if !untouched_since(ctx, provider, &path, child, at, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Sends the item at `path` to the trash, as one step under [`ERRORS`], once `check` has passed
/// for what `stat` finds there; it goes to `trashed` with where the trash holds it. An item
/// that is gone is skipped; a `stat` error and a refused `check` fail as `Stage::Undo`, and the
/// trash's error (`Unsupported` where there is none) as `Stage::Delete`, and the item stays: a
/// trash never turns into a permanent delete. A provider that deleted it for good instead is a
/// `Stage::Delete` warning. `Break` once the job is cancelled.
fn trash_item<'e>(
    ctx: &mut Ctx<'e>,
    path: &VPath,
    check: impl Fn(&Ctx<'e>, &dyn Provider, &Entry) -> Result<(), Failed>,
    trashed: &mut Vec<TrashedItem>,
) -> ControlFlow<()> {
    let sent = ctx.attempt(ERRORS, Some(path), None, |ctx| {
        ctx.check().at(Stage::Undo)?;
        let provider = ctx.provider(path).at(Stage::Undo)?;
        let now = match provider.stat(path) {
            Ok(now) => now,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Failed::new(Stage::Undo, error)),
        };
        check(ctx, &*provider, &now)?;
        ctx.working_on(path);
        let deleted = provider.delete(path, DeleteMode::Trash).at(Stage::Delete)?;
        Ok(Some((deleted, now.kind == EntryKind::Dir)))
    });
    match sent {
        Attempt::Done(Some((Deleted::Trashed { location }, is_dir))) => {
            trashed.push(TrashedItem {
                original: path.clone(),
                location,
            });
            ctx.item_done(!is_dir, is_dir);
        }
        // A provider that breaks its promise: the item is gone, but no redo brings it back.
        Attempt::Done(Some((Deleted::Permanently, is_dir))) => {
            let error = Error::Unsupported(format!("{path} was deleted for good, not trashed"));
            ctx.warn(Failed::new(Stage::Delete, error), Some(path), None);
            ctx.item_done(!is_dir, is_dir);
        }
        Attempt::Done(None) => ctx.report.skipped += 1,
        Attempt::Skipped => {}
        Attempt::Stop => return ControlFlow::Break(()),
    }
    ControlFlow::Continue(())
}

/// Undoes a move, in three steps:
///
/// 1. Each source folder the move removed (`removed_dirs`, from the last: parents first) is
///    made again where it is missing, so that its items have a folder to go back to. A failure
///    is `Stage::Undo`.
/// 2. Each moved item goes back, from the last: the move executor runs from `to` to `from`
///    under [`MOVE_BACK`], with every rule of a move. An item whose old place is taken stays
///    where it is (skipped), never replacing what is there now, and a source is deleted only
///    after its copy is committed.
/// 3. Unless the job was cancelled or aborted, each folder the move made at the target
///    (`created_dirs`, from the last: children first) is removed with `remove_dir`, which
///    removes only an empty folder. One that is gone already is passed over; any other failure,
///    as for a folder with something in it, is a `Stage::Undo` warning, and the folder stays.
///
/// The undo's record moves the items forth again: what step 2 moved, the folders steps 1 and 2
/// made, and those steps 2 and 3 removed. A move back that the plan turned into a copy (DND-06:
/// the folder the items are in now cannot be written to) leaves them in both places; its
/// copies are in no record.
fn undo_move(
    ctx: &mut Ctx<'_>,
    moves: &[MovedItem],
    created_dirs: &[VPath],
    removed_dirs: &[VPath],
) {
    let mut made = Vec::new();
    for dir in removed_dirs.iter().rev() {
        match ctx.attempt(ERRORS, None, Some(dir), |ctx| make_dir_back(ctx, dir)) {
            Attempt::Done(true) => made.push(dir.clone()),
            Attempt::Done(false) | Attempt::Skipped => {}
            Attempt::Stop => {
                ctx.report.undo = moved_record(Vec::new(), made, Vec::new());
                return;
            }
        }
    }
    let back = TransferSpec {
        mode: TransferMode::Move,
        items: moves
            .iter()
            .rev()
            .map(|moved| TransferItem {
                source: moved.to.clone(),
                target: moved.from.clone(),
            })
            .collect(),
        options: MOVE_BACK,
    };
    transfer::run_transfer(ctx, &back);
    let (moved, mut made_back, mut removed) = match ctx.report.undo.take() {
        Some(UndoRecord::Moved {
            moves,
            created_dirs,
            removed_dirs,
        }) => (moves, created_dirs, removed_dirs),
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };
    if ctx.stopped == Stopped::No {
        removed.extend(remove_made_dirs(ctx, created_dirs));
    }
    made.append(&mut made_back);
    ctx.report.undo = moved_record(moved, made, removed);
}

/// Makes the folder `dir` again for [`undo_move`]: `true` when it was made, `false` when
/// something is there already. Any other error fails as `Stage::Undo`.
fn make_dir_back(ctx: &Ctx<'_>, dir: &VPath) -> Result<bool, Failed> {
    ctx.check().at(Stage::Undo)?;
    let provider = ctx.provider(dir).at(Stage::Undo)?;
    match provider.create_dir(dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(Failed::new(Stage::Undo, error)),
    }
}

/// Removes each of `dirs`, from the last, with `remove_dir`, for step 3 of [`undo_move`], and
/// gives those it removed, in that order. A cancel stops it between two folders.
fn remove_made_dirs(ctx: &mut Ctx<'_>, dirs: &[VPath]) -> Vec<VPath> {
    let mut removed = Vec::new();
    for dir in dirs.iter().rev() {
        if ctx.check().is_err() {
            ctx.stop(Stopped::Cancelled);
            break;
        }
        match ctx
            .provider(dir)
            .and_then(|provider| provider.remove_dir(dir))
        {
            Ok(()) => removed.push(dir.clone()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => ctx.warn(Failed::new(Stage::Undo, error), Some(dir), None),
        }
    }
    removed
}

/// The record of a move that did this, or `None` when it did nothing.
fn moved_record(
    moves: Vec<MovedItem>,
    created_dirs: Vec<VPath>,
    removed_dirs: Vec<VPath>,
) -> Option<UndoRecord> {
    let changed = !(moves.is_empty() && created_dirs.is_empty() && removed_dirs.is_empty());
    changed.then_some(UndoRecord::Moved {
        moves,
        created_dirs,
        removed_dirs,
    })
}

/// Removes the new empty item `path` that a create job made (a folder or a file, `kind`), if it
/// is still that: a folder with `remove_dir`, which removes only an empty one, and a file of no
/// bytes with `delete_leaf`, which never removes a folder. An item that is gone is skipped. A
/// folder with something in it fails as `Stage::Undo` with `DirectoryNotEmpty`, a file with
/// something in it as [`left`] ("is not empty"), and an item of another kind (a link, say) as
/// [`left`] ("changed after it was created"); each stays. The undo's record makes it again
/// (`UndoRecord::Removed`).
fn remove_created(ctx: &mut Ctx<'_>, path: &VPath, kind: CreateKind) {
    let is_dir = kind == CreateKind::Dir;
    let mut totals = Totals::default();
    count(&mut totals, is_dir);
    ctx.planned(totals);
    let removed = ctx.attempt(ERRORS, Some(path), None, |ctx| {
        ctx.check().at(Stage::Undo)?;
        let provider = ctx.provider(path).at(Stage::Undo)?;
        let now = match provider.stat(path) {
            Ok(now) => now,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(Failed::new(Stage::Undo, error)),
        };
        ctx.working_on(path);
        match (kind, now.kind) {
            (CreateKind::Dir, EntryKind::Dir) => provider.remove_dir(path).at(Stage::Undo)?,
            (CreateKind::File, EntryKind::File) if now.size == 0 => {
                provider.delete_leaf(path).at(Stage::Undo)?;
            }
            (CreateKind::File, EntryKind::File) => return Err(left(path, "is not empty")),
            _ => return Err(left(path, "changed after it was created")),
        }
        Ok(true)
    });
    match removed {
        Attempt::Done(true) => {
            ctx.report.undo = Some(UndoRecord::Removed {
                path: path.clone(),
                kind,
            });
            ctx.item_done(!is_dir, is_dir);
        }
        Attempt::Done(false) => ctx.report.skipped += 1,
        Attempt::Skipped | Attempt::Stop => {}
    }
}

/// Makes the empty folder or file (`kind`) that an undo removed again, at its own `path`: no
/// numbered name, and nothing is replaced, so a taken `path` fails as `Stage::Undo` with
/// `AlreadyExists`. The undo's record removes it again (`UndoRecord::Created`).
fn make_again(ctx: &mut Ctx<'_>, path: &VPath, kind: CreateKind) {
    let is_dir = kind == CreateKind::Dir;
    let mut totals = Totals::default();
    count(&mut totals, is_dir);
    ctx.planned(totals);
    let made = ctx.attempt(ERRORS, None, Some(path), |ctx| {
        ctx.check().at(Stage::Undo)?;
        let provider = ctx.provider(path).at(Stage::Undo)?;
        ctx.working_on(path);
        if is_dir {
            provider.create_dir(path).at(Stage::Undo)
        } else {
            provider.create_file(path).at(Stage::Undo)
        }
    });
    if let Attempt::Done(()) = made {
        ctx.report.outputs.push(path.clone());
        ctx.report.undo = Some(UndoRecord::Created {
            path: path.clone(),
            kind,
        });
        ctx.item_done(!is_dir, is_dir);
    }
}

/// Puts each of `items` back from the trash, at its own original place, with
/// `Provider::restore_trashed`, each as one step under [`ERRORS`]. Every failure is
/// `Stage::Undo` and leaves the item where it is: an item whose trash location is not known
/// fails with `Unsupported`, and the restore's error is passed on as it is, its path being the
/// trash location: `AlreadyExists` when the original place is taken (nothing is replaced),
/// `NotFound` when the trash no longer holds the item or the original folder is gone, and
/// `CrossesDevices` when the trash is on another volume. The plan counts each item by what is
/// at its trash location (a file when that cannot be told). What came back goes to the outputs,
/// and the undo's record trashes it again (`UndoRecord::Restored`).
fn restore(ctx: &mut Ctx<'_>, items: &[TrashedItem]) {
    let kinds: Vec<bool> = items
        .iter()
        .map(|item| is_dir_at(ctx, item.location.as_ref()))
        .collect();
    let mut totals = Totals::default();
    for &is_dir in &kinds {
        count(&mut totals, is_dir);
    }
    ctx.planned(totals);
    let mut restored = Vec::new();
    for (item, is_dir) in items.iter().zip(kinds) {
        let original = &item.original;
        let put_back = ctx.attempt(ERRORS, item.location.as_ref(), Some(original), |ctx| {
            ctx.check().at(Stage::Undo)?;
            let Some(location) = &item.location else {
                let error = Error::Unsupported(format!("trash location of {original} is unknown"));
                return Err(Failed::new(Stage::Undo, error));
            };
            let provider = ctx.provider(original).at(Stage::Undo)?;
            ctx.working_on(original);
            provider.restore_trashed(location, original).at(Stage::Undo)
        });
        match put_back {
            Attempt::Done(()) => {
                restored.push(item.clone());
                ctx.report.outputs.push(original.clone());
                ctx.item_done(!is_dir, is_dir);
            }
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
    if !restored.is_empty() {
        ctx.report.undo = Some(UndoRecord::Restored { items: restored });
    }
}

/// Sends each item an undo put back from the trash (`items`, at their original places) to the
/// trash again, as [`trash_item`] does, whatever it holds now: the trash keeps it. The plan
/// counts each item by what is at its place (a file when that cannot be told). The undo's
/// record puts back what went (`UndoRecord::Trashed`); `None` when nothing did.
fn trash_again(ctx: &mut Ctx<'_>, items: &[TrashedItem]) {
    let mut totals = Totals::default();
    for item in items {
        count(&mut totals, is_dir_at(ctx, Some(&item.original)));
    }
    ctx.planned(totals);
    let mut trashed = Vec::new();
    for item in items {
        let anything = |_: &Ctx<'_>, _: &dyn Provider, _: &Entry| Ok(());
        if trash_item(ctx, &item.original, anything, &mut trashed).is_break() {
            break;
        }
    }
    if !trashed.is_empty() {
        ctx.report.undo = Some(UndoRecord::Trashed { items: trashed });
    }
}
