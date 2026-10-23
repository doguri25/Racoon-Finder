//! Running a delete (OPS-03): each selected item to the trash, or each one for good with
//! everything in it, a folder only once it is empty. A delete only ever removes what it was
//! given and what is in it: it never follows a link, never enters another volume and never
//! turns a trash into a permanent delete.

use std::ops::ControlFlow;

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath, VolumeId};
use rf_vfs::{DeleteMode, Deleted, Provider};

use super::{At, Attempt, Ctx, Failed, Level, MAX_DEPTH, Stopped};
use crate::report::{Stage, Totals};
use crate::spec::{DeleteMethod, DeleteSpec, ErrorPolicy};
use crate::undo::{TrashedItem, UndoRecord};

/// Plans `spec` (see [`plan`]) and deletes each planned path by its method, under its error
/// policy: to the trash with [`trash`], for good with [`delete_entry`]. A cancel stops the job
/// between two items.
pub(crate) fn run_delete(ctx: &mut Ctx<'_>, spec: &DeleteSpec) {
    let Some(items) = plan(ctx, spec) else {
        return;
    };
    match spec.method {
        DeleteMethod::Trash => trash(ctx, spec.errors, &items),
        DeleteMethod::Permanent => {
            for (path, entry) in &items {
                let deleted = delete_entry(ctx, spec.errors, path, entry, Level::TOP, None);
                if deleted.is_break() {
                    break;
                }
            }
        }
    }
}

/// What the job will delete: each path of `spec` that `stat` finds (a link itself, not what it
/// leads to), with that entry. A path `stat` cannot find fails as `Stage::Plan` under the
/// job's error policy and is left out.
///
/// The totals count items, never bytes, since a delete reads no file. A trash counts each path
/// alone, as one file or one folder. A permanent delete counts everything it will meet (see
/// [`count_tree`]). `None` when the job was cancelled or aborted.
fn plan(ctx: &mut Ctx<'_>, spec: &DeleteSpec) -> Option<Vec<(VPath, Entry)>> {
    let mut found = Vec::with_capacity(spec.paths.len());
    for path in &spec.paths {
        let stat = ctx.attempt(spec.errors, Some(path), None, |ctx| {
            ctx.check().at(Stage::Plan)?;
            let provider = ctx.provider(path).at(Stage::Plan)?;
            let entry = provider.stat(path).at(Stage::Plan)?;
            Ok((provider, entry))
        });
        match stat {
            Attempt::Done((provider, entry)) => found.push((path, provider, entry)),
            Attempt::Skipped => {}
            Attempt::Stop => return None,
        }
    }
    let mut totals = Totals::default();
    let mut items = Vec::with_capacity(found.len());
    for (path, provider, entry) in found {
        match spec.method {
            DeleteMethod::Trash => count_one(&entry, &mut totals),
            DeleteMethod::Permanent => {
                let counted =
                    count_tree(ctx, &*provider, path, &entry, Level::TOP, None, &mut totals);
                if counted.is_err() {
                    ctx.stop(Stopped::Cancelled);
                    return None;
                }
            }
        }
        items.push((path.clone(), entry));
    }
    ctx.planned(totals);
    Some(items)
}

/// Counts `entry` as one folder or one file (a link and anything else included).
fn count_one(entry: &Entry, totals: &mut Totals) {
    if entry.kind == EntryKind::Dir {
        totals.dirs += 1;
    } else {
        totals.files += 1;
    }
}

/// Counts the item `path` of `provider`, which `entry` describes, and for a folder everything
/// in it, as [`delete_entry`] will meet it; `level` says how far below a path of the job it is,
/// and `above` is the volume of the folder it is in (`None` for a path of the job).
///
/// Only a real folder (`EntryKind::Dir`, as `stat` and `list` see it, never following a link)
/// is listed. An item deeper than [`MAX_DEPTH`] and a folder on another volume than `above`
/// (both volume IDs known) are not counted, nor is anything in them; a folder that cannot be
/// listed is counted alone. Fails only when the job is cancelled.
fn count_tree(
    ctx: &Ctx<'_>,
    provider: &dyn Provider,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    above: Option<VolumeId>,
    totals: &mut Totals,
) -> rf_core::Result<()> {
    if level.depth() > MAX_DEPTH {
        return Ok(());
    }
    if entry.kind != EntryKind::Dir {
        totals.files += 1;
        return Ok(());
    }
    let volume = volume_of(provider, path, entry);
    if on_another_volume(above, volume) {
        return Ok(());
    }
    totals.dirs += 1;
    ctx.check()?;
    let children = match ctx.list(provider, path) {
        Ok(children) => children,
        Err(error) if error.kind() == ErrorKind::Cancelled => return Err(error),
        Err(_) => return Ok(()),
    };
    let here = level.enter(None);
    for child in &children {
        let Ok(child_path) = path.join(&child.name) else {
            continue;
        };
        count_tree(
            ctx,
            provider,
            &child_path,
            child,
            here.below(),
            volume,
            totals,
        )?;
    }
    Ok(())
}

/// The volume of the folder `path` of `provider`, which `entry` describes: the entry's, or the
/// one `stat` gives for a listed folder that has none (a Windows listing). `None` when neither
/// tells.
fn volume_of(provider: &dyn Provider, path: &VPath, entry: &Entry) -> Option<VolumeId> {
    entry
        .volume_id
        .or_else(|| provider.stat(path).ok()?.volume_id)
}

/// Whether a folder on `volume` in a folder on `above` is on another volume (a mount point):
/// only when both are known and differ.
fn on_another_volume(above: Option<VolumeId>, volume: Option<VolumeId>) -> bool {
    matches!((above, volume), (Some(a), Some(b)) if a != b)
}

/// Sends each of `items` to the trash, as a step of its own under `errors` (`Stage::Delete`).
///
/// A path the trash cannot take fails, `Unsupported` (no trash there, or one that refuses the
/// item) as much as any other error, and stays where it is: it is never deleted for good
/// instead (OPS-03). The report's undo record lists every path that went, with where the trash
/// holds it when the system tells (`UndoRecord::Trashed`); `None` when none went.
fn trash(ctx: &mut Ctx<'_>, errors: ErrorPolicy, items: &[(VPath, Entry)]) {
    let mut trashed = Vec::new();
    for (path, entry) in items {
        let deleted = ctx.attempt(errors, Some(path), None, |ctx| {
            ctx.check().at(Stage::Delete)?;
            let provider = ctx.provider(path).at(Stage::Delete)?;
            ctx.working_on(path);
            provider.delete(path, DeleteMode::Trash).at(Stage::Delete)
        });
        let is_dir = entry.kind == EntryKind::Dir;
        match deleted {
            Attempt::Done(Deleted::Trashed { location }) => {
                trashed.push(TrashedItem {
                    original: path.clone(),
                    location,
                });
                ctx.item_done(!is_dir, is_dir);
            }
            // A provider that breaks its promise: the item is gone, but no undo brings it back.
            Attempt::Done(Deleted::Permanently) => {
                let error = Error::Unsupported(format!("{path} was deleted for good, not trashed"));
                ctx.warn(Failed::new(Stage::Delete, error), Some(path), None);
                ctx.item_done(!is_dir, is_dir);
            }
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
    if !trashed.is_empty() {
        ctx.report.undo = Some(UndoRecord::Trashed { items: trashed });
    }
}

/// Whether the job goes on after an item of a permanent delete, `Break` once it is cancelled or
/// aborted, and whether the item is gone with everything in it (`Continue(true)`).
type Flow = ControlFlow<(), bool>;

/// The flow after an attempt that, unless it worked, leaves its item where it is.
fn kept<T>(attempt: &Attempt<T>) -> Flow {
    match attempt {
        Attempt::Stop => ControlFlow::Break(()),
        Attempt::Done(_) | Attempt::Skipped => ControlFlow::Continue(false),
    }
}

/// Deletes `path`, which `entry` describes, for good; `level` says how far below a path of the
/// job it is, and `above` is the volume of the folder it is in (`None` for a path of the job).
/// Every step is an attempt under `errors` that checks for a pause or a cancel first, so a
/// failure, a retry and the progress go item by item. No undo record is kept.
///
/// - Deeper than [`MAX_DEPTH`]: fails as `Stage::Plan` with the nesting message and stays.
/// - A real folder (`EntryKind::Dir`): see [`delete_dir`].
/// - Anything else, a link to a folder and a FIFO, socket or device included: see
///   [`delete_leaf`].
fn delete_entry(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    above: Option<VolumeId>,
) -> Flow {
    if level.depth() > MAX_DEPTH {
        let too_deep = ctx.attempt(errors, Some(path), None, |_| {
            let error =
                Error::Unsupported(format!("folder nesting deeper than {MAX_DEPTH} levels"));
            Err::<(), _>(Failed::new(Stage::Plan, error))
        });
        return kept(&too_deep);
    }
    if entry.kind == EntryKind::Dir {
        delete_dir(ctx, errors, path, entry, level, above)
    } else {
        delete_leaf(ctx, errors, path, entry)
    }
}

/// `stat`s `path` of `provider` again, just before it is deleted or listed, and gives what it
/// finds. Fails as `Stage::Delete`, leaving the item in place, when it is no longer the item
/// `entry` described: of another kind (a file turned into a folder, a folder into a link to
/// one) or, when both identities are known, another item.
fn restat(provider: &dyn Provider, path: &VPath, entry: &Entry) -> Result<Entry, Failed> {
    let now = provider.stat(path).at(Stage::Delete)?;
    let replaced = now.kind != entry.kind
        || matches!((now.identity(), entry.identity()), (Some(a), Some(b)) if a != b);
    if replaced {
        let error = Error::Unsupported(format!(
            "{path} changed while it was deleted; left in place"
        ));
        return Err(Failed::new(Stage::Delete, error));
    }
    Ok(now)
}

/// Deletes the file, link or other item `path`, which `entry` describes, by its name
/// (`Provider::delete` with `DeleteMode::Permanent`): a link goes itself, never what it leads
/// to, and a FIFO, socket or device is never opened.
///
/// Just before, it is checked to be still what was found (see [`restat`]): above all not a
/// folder, which that delete would remove with everything in it. So the provider's removal of
/// a whole tree is never used.
fn delete_leaf(ctx: &mut Ctx<'_>, errors: ErrorPolicy, path: &VPath, entry: &Entry) -> Flow {
    let deleted = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        ctx.working_on(path);
        restat(&*provider, path, entry)?;
        provider
            .delete(path, DeleteMode::Permanent)
            .map(drop)
            .at(Stage::Delete)
    });
    match deleted {
        Attempt::Done(()) => {
            ctx.item_done(true, false);
            ControlFlow::Continue(true)
        }
        other => kept(&other),
    }
}

/// Deletes the folder `path`, which `entry` describes, after everything in it.
///
/// One step makes sure the folder is still a folder of its own (see [`restat`]), so that a
/// folder replaced by a link is never listed through it, and that it is on the volume of the
/// folder it is in, `above`, unless either volume is unknown: a folder on another volume (a
/// mount point) fails as `Stage::Delete` with `Unsupported` and is neither entered nor removed.
/// The same step lists its children, all of them before any is deleted. Each child is then
/// deleted (see [`delete_entry`]), and the folder is removed with `Provider::remove_dir`, which
/// removes only an empty folder, once each child has gone with everything in it. A folder with
/// a child that failed or stayed is kept without a failure of its own; one that another
/// program put a file in meanwhile fails to be removed (`DirectoryNotEmpty`) and keeps that
/// file.
fn delete_dir(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    above: Option<VolumeId>,
) -> Flow {
    let opened = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        let volume = restat(&*provider, path, entry)?.volume_id;
        if on_another_volume(above, volume) {
            let error = Error::Unsupported(format!("{path} is on another volume"));
            return Err(Failed::new(Stage::Delete, error));
        }
        ctx.working_on(path);
        let children = ctx.list(&*provider, path).at(Stage::Delete)?;
        Ok((volume, children))
    });
    let (volume, children) = match opened {
        Attempt::Done(opened) => opened,
        other => return kept(&other),
    };
    let here = level.enter(None);
    let mut all_gone = true;
    for child in &children {
        let child_path = match path.join(&child.name) {
            Ok(child_path) => child_path,
            Err(error) => {
                // No path names the child; the error names it.
                let failed = ctx.attempt(errors, Some(path), None, |_| {
                    Err::<(), _>(Failed::new(Stage::Delete, error.clone().into()))
                });
                kept(&failed)?;
                all_gone = false;
                continue;
            }
        };
        all_gone &= delete_entry(ctx, errors, &child_path, child, here.below(), volume)?;
    }
    if !all_gone {
        return ControlFlow::Continue(false);
    }
    let removed = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        provider.remove_dir(path).at(Stage::Delete)
    });
    match removed {
        Attempt::Done(()) => {
            ctx.item_done(false, true);
            ControlFlow::Continue(true)
        }
        other => kept(&other),
    }
}
