//! Running a delete (OPS-03): each selected item to the trash, or each one for good with
//! everything in it, a folder only once it is empty. A delete only ever removes what it was
//! given and what is in it: it never follows a link, never enters another volume and never
//! turns a trash into a permanent delete.
//!
//! In sequential use, while nothing else changes the tree, that holds for every item. Against
//! another program changing the tree as it is deleted, a permanent delete checks again what it
//! found before it deletes a leaf, and before and after it lists a folder (see [`restat`]); a
//! leaf goes with `Provider::delete_leaf`, which never removes a folder, and a folder with
//! `Provider::remove_dir`, which removes only an empty one. So an item that turned into a
//! folder, a folder swapped for a link, and a path that leads elsewhere by then never take a
//! tree with them. A hostile race that swaps a folder of the path for a link and back between
//! two system calls of the walk can still redirect a walk that names items by path; closing
//! that needs a walk relative to open folder handles (`openat`, `unlinkat`), the follow-up for
//! this.

use std::ops::ControlFlow;

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath, VolumeId};
use rf_vfs::{DeleteMode, Deleted, Provider};

use super::{At, Attempt, Ctx, Failed, Level, MAX_DEPTH, Stopped, replaced, too_deep};
use crate::report::{Stage, Totals};
use crate::spec::{DeleteMethod, DeleteSpec, ErrorPolicy};
use crate::undo::{TrashedItem, UndoRecord};

/// Plans `spec` (see [`plan`]) and deletes each planned path by its method, under its error
/// policy: to the trash with [`trash`], for good with [`delete_entry`]. A cancel stops the job
/// between two items.
///
/// The paths are taken as given, one after the other, and not cleaned of each other: a path
/// inside another one of the job (or one given twice) goes with the first and then fails as
/// `NotFound`, while one met first is deleted on its own. Nothing is deleted twice or outside
/// what was given, so the report only shows an extra failure.
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

/// What a step of the walk on an item it found inside a folder (a `child`, not a path of the
/// job) makes of a failure that only says the item is not there any more (`NotFound`): it went
/// by itself, which is what the step was for, so `Ok(None)` and no failure. Asking the user
/// would change nothing, and a retry would meet the same. A path of the job that is gone still
/// fails, since it was asked for. Any other result is passed on.
fn unless_gone<T>(step: Result<T, Failed>, child: bool) -> Result<Option<T>, Failed> {
    match step {
        Ok(value) => Ok(Some(value)),
        Err(failed) if child && failed.cause.kind() == ErrorKind::NotFound => Ok(None),
        Err(failed) => Err(failed),
    }
}

/// Deletes `path`, which `entry` describes, for good; `level` says how far below a path of the
/// job it is, and `above` is the volume of the folder it is in (`None` for a path of the job).
/// Every step is an attempt under `errors` that checks for a pause or a cancel first, so a
/// failure, a retry and the progress go item by item. An item below a path of the job that is
/// gone by the time a step reaches it counts as gone, with no failure, and is not counted as
/// done (see [`unless_gone`]). No undo record is kept.
///
/// - Deeper than [`MAX_DEPTH`]: fails as `Stage::Plan` with the nesting message and stays (see
///   [`too_deep`]).
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
    if let Some(failed) = too_deep(ctx, level, errors, path, None) {
        return kept(&failed);
    }
    if entry.kind == EntryKind::Dir {
        delete_dir(ctx, errors, path, entry, level, above)
    } else {
        delete_leaf(ctx, errors, path, entry, level.depth() > 0)
    }
}

/// `stat`s `path` of `provider` again and gives what it finds. Fails as `Stage::Delete`,
/// leaving the item in place, when it is no longer the item `entry` described (see
/// [`replaced`]): of another kind (a file turned into a folder, a folder into a link to one)
/// or, when both identities are known, another item.
fn restat(provider: &dyn Provider, path: &VPath, entry: &Entry) -> Result<Entry, Failed> {
    let now = provider.stat(path).at(Stage::Delete)?;
    if replaced(entry, &now) {
        let error = Error::Unsupported(format!(
            "{path} changed while it was deleted; left in place"
        ));
        return Err(Failed::new(Stage::Delete, error));
    }
    Ok(now)
}

/// Deletes the file, link or other item `path`, which `entry` describes, by its name: a link
/// goes itself, never what it leads to, and a FIFO, socket or device is never opened. `child`
/// says whether it is below a path of the job.
///
/// Just before, it is checked to be still what was found (see [`restat`]): a path that leads
/// to another item by then (it was replaced, or a folder above it was swapped for a link) is
/// left. Then it goes with `Provider::delete_leaf`, which never removes a folder: an item that
/// turned into a folder after that check stays with everything in it, and so does a folder
/// that the path, swapped since, leads to. The provider's removal of a whole tree is never
/// used. What remains is the race of the module doc: a swap between that check and the delete
/// can still make the path lead to one other file, which then goes in its place.
fn delete_leaf(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    path: &VPath,
    entry: &Entry,
    child: bool,
) -> Flow {
    let deleted = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        ctx.working_on(path);
        let step = restat(&*provider, path, entry)
            .and_then(|_| provider.delete_leaf(path).at(Stage::Delete));
        unless_gone(step, child)
    });
    match deleted {
        Attempt::Done(Some(())) => {
            ctx.item_done(true, false);
            ControlFlow::Continue(true)
        }
        Attempt::Done(None) => ControlFlow::Continue(true),
        other => kept(&other),
    }
}

/// Lists the folder `path` of `provider`, which `entry` describes and which is in a folder on
/// the volume `above`, for [`delete_dir`]: its volume and its children.
///
/// It must still be a folder of its own (see [`restat`]) and on the volume `above`, unless
/// either volume is unknown: a folder on another volume (a mount point) fails as
/// `Stage::Delete` with `Unsupported`. After the listing it is checked once more against what
/// the first check found: if it was swapped for a link, or for another folder, while it was
/// listed, the listing came from somewhere else, and the step fails before any child is
/// touched.
fn open_dir(
    ctx: &mut Ctx<'_>,
    provider: &dyn Provider,
    path: &VPath,
    entry: &Entry,
    above: Option<VolumeId>,
) -> Result<(Option<VolumeId>, Vec<Entry>), Failed> {
    let before = restat(provider, path, entry)?;
    if on_another_volume(above, before.volume_id) {
        let error = Error::Unsupported(format!("{path} is on another volume"));
        return Err(Failed::new(Stage::Delete, error));
    }
    ctx.working_on(path);
    let children = ctx.list(provider, path).at(Stage::Delete)?;
    restat(provider, path, &before)?;
    Ok((before.volume_id, children))
}

/// Deletes the folder `path`, which `entry` describes, after everything in it.
///
/// One step lists it (see [`open_dir`]); a folder on another volume, or one that is no longer
/// a folder of its own, is neither entered nor removed. Each child is then deleted (see
/// [`delete_entry`]), and the folder is removed with `Provider::remove_dir`, which removes only
/// an empty folder, once each child has gone with everything in it. A folder with a child that
/// failed or stayed is kept without a failure of its own; one that another program put a file
/// in meanwhile fails to be removed (`DirectoryNotEmpty`) and keeps that file.
fn delete_dir(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    above: Option<VolumeId>,
) -> Flow {
    let below_the_job = level.depth() > 0;
    let opened = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        let step = open_dir(ctx, &*provider, path, entry, above);
        unless_gone(step, below_the_job)
    });
    let (volume, children) = match opened {
        Attempt::Done(Some(opened)) => opened,
        Attempt::Done(None) => return ControlFlow::Continue(true),
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
        unless_gone(provider.remove_dir(path).at(Stage::Delete), below_the_job)
    });
    match removed {
        Attempt::Done(Some(())) => {
            ctx.item_done(false, true);
            ControlFlow::Continue(true)
        }
        Attempt::Done(None) => ControlFlow::Continue(true),
        other => kept(&other),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rf_vfs::LocalProvider;

    use super::*;
    use crate::exec::swapping::{Swapping, When};
    use crate::exec::{ExecEnv, execute};
    use crate::id::JobId;
    use crate::report::{JobReport, Outcome};
    use crate::resolver::NonInteractive;
    use crate::spec::JobSpec;
    use crate::testkit::{tree, write_tree};

    /// Deletes `root/dir` for good while it turns into a link to `root/outside` at `when`.
    fn delete_while_swapping(root: &Path, when: When) -> JobReport {
        let dir = root.join("dir");
        let local = Arc::new(LocalProvider::new());
        let vfs = Swapping::new(local, &dir, &root.join("outside"), when).vfs();
        let job = JobSpec::Delete(DeleteSpec {
            paths: vec![VPath::local(&dir).unwrap()],
            method: DeleteMethod::Permanent,
            errors: ErrorPolicy::Continue,
        });
        let report = execute(JobId(1), &job, &ExecEnv::new(&vfs, &NonInteractive));
        assert!(dir.is_symlink(), "the swap did not happen");
        report
    }

    fn local(path: PathBuf) -> VPath {
        VPath::local(path).unwrap()
    }

    /// A folder swapped for a link while it is listed was listed through the link: the check
    /// after the listing refuses the folder before any of what it lists is touched.
    #[test]
    fn a_folder_swapped_for_a_link_as_it_is_listed_is_left() {
        let t = tempfile::tempdir().unwrap();
        write_tree(
            t.path(),
            &[
                ("dir/x.txt", "X"),
                ("dir/sub/y.txt", "Y"),
                ("outside/keep.txt", "K"),
                ("outside/deep/d.txt", "D"),
            ],
        );
        let before = tree(&t.path().join("outside"));
        // Listing 0 is the plan's; listing 1 is the run's.
        let report = delete_while_swapping(t.path(), When::Listing(1));
        assert_eq!(tree(&t.path().join("outside")), before);
        assert_eq!(report.outcome, Outcome::CompletedWithErrors);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        let failure = &report.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (
                Stage::Delete,
                ErrorKind::Unsupported,
                Some(local(t.path().join("dir")))
            )
        );
        assert!(
            failure.message.contains("changed while it was deleted"),
            "{}",
            failure.message
        );
    }

    /// A leaf whose path leads to a folder by the time it is deleted (a folder above it was
    /// swapped for a link after the leaf was checked) takes nothing with it: `delete_leaf`
    /// never removes a folder.
    #[test]
    fn a_leaf_whose_path_leads_to_a_folder_by_then_removes_nothing() {
        let t = tempfile::tempdir().unwrap();
        write_tree(t.path(), &[("dir/c", "C"), ("outside/c/keep.txt", "K")]);
        let before = tree(&t.path().join("outside"));
        let report = delete_while_swapping(t.path(), When::DeletingALeaf);
        assert_eq!(tree(&t.path().join("outside")), before);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        let failure = &report.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (
                Stage::Delete,
                ErrorKind::IsADirectory,
                Some(local(t.path().join("dir").join("c")))
            )
        );
    }
}
