//! Running a delete (OPS-03): each selected item to the trash, or each one for good with
//! everything in it, a folder only once it is empty. A delete only ever removes what it was
//! given and what is in it: it never follows a link, never enters another volume and never
//! turns a trash into a permanent delete.
//!
//! In sequential use, while nothing else changes the tree, that holds for every item. Against
//! another program changing the tree while it is deleted, each step of a permanent delete looks
//! at its item again, then checks that the folder the item is in is still the folder the walk
//! listed, and only then acts (see [`restat`] and [`checked`]); a folder is also checked right
//! after it is listed and right before it is removed. The folder is checked by `stat`, which
//! gives an identity on every system, so a swap of any folder above an item for a link is
//! caught on Windows, whose listings carry no identities, as on Unix. A leaf goes with
//! `Provider::delete_leaf`, which never removes a folder, and a folder with
//! `Provider::remove_dir`, which removes only an empty one, so nothing takes a tree with it.
//!
//! What remains, on every system now, is the race Unix listings already left: a swap timed
//! between two system calls of a step, after its last check and before its delete, or undone
//! again before the next check, can still redirect one call that names its item by path, to one
//! other file or one empty folder. Closing that needs a walk relative to open folder handles
//! (`openat`, `unlinkat`), the follow-up for this.

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

/// The folder an item of a permanent delete is in, as the walk found it when it listed it: its
/// path, and what `stat` gave for it then. That `stat` carries the folder's identity on every
/// system, also on Windows, where a listing carries none.
#[derive(Clone, Copy, Debug)]
struct Parent<'a> {
    path: &'a VPath,
    entry: &'a Entry,
}

/// What a step on an item below a path of the job (a `child`, not a path of the job) makes of a
/// failure that only says the item is not there any more (`NotFound`): it went by itself, which
/// is what the step was for, so `Ok(None)` and no failure. Asking the user would change
/// nothing, and a retry would meet the same. A path of the job that is gone still fails, since
/// it was asked for. Any other result is passed on.
fn unless_gone<T>(step: Result<T, Failed>, child: bool) -> Result<Option<T>, Failed> {
    match step {
        Ok(value) => Ok(Some(value)),
        Err(failed) if child && failed.cause.kind() == ErrorKind::NotFound => Ok(None),
        Err(failed) => Err(failed),
    }
}

/// What a step on an item makes of `own`, its first look at the item, once it has checked the
/// folder the item is in, `parent` (`None` for a path of the job): that folder is `stat`ed
/// again and must still be what the walk listed (see [`restat`]). A swap of that folder, or of
/// any folder above it, for a link makes its path lead to another folder, with another
/// identity, so the step fails then, whatever `own` says, and on every system. Only once that
/// check has passed does a `NotFound` of `own` mean that the item went by itself (see
/// [`unless_gone`]).
fn checked<T>(
    provider: &dyn Provider,
    parent: Option<Parent<'_>>,
    own: Result<T, Failed>,
) -> Result<Option<T>, Failed> {
    if let Some(parent) = parent {
        restat(provider, parent.path, parent.entry)?;
    }
    unless_gone(own, parent.is_some())
}

/// Deletes `path`, which `entry` describes, for good; `level` says how far below a path of the
/// job it is, and `parent` is the folder it is in (`None` for a path of the job). Every step is
/// an attempt under `errors` that checks for a pause or a cancel first, so a failure, a retry
/// and the progress go item by item. Each step looks at its item, then checks its folder (see
/// [`checked`]), and only then acts. An item below a path of the job that is gone by the time a
/// step reaches it counts as gone, with no failure, and is not counted as done. No undo record
/// is kept.
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
    parent: Option<Parent<'_>>,
) -> Flow {
    if let Some(failed) = too_deep(ctx, level, errors, path, None) {
        return kept(&failed);
    }
    if entry.kind == EntryKind::Dir {
        delete_dir(ctx, errors, path, entry, level, parent)
    } else {
        delete_leaf(ctx, errors, path, entry, parent)
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

/// Deletes the file, link or other item `path` in `parent`, which `entry` describes, by its
/// name: a link goes itself, never what it leads to, and a FIFO, socket or device is never
/// opened.
///
/// Just before, it is looked at again (see [`restat`]) and its folder is checked (see
/// [`checked`]): an item that was replaced or turned into a folder, and a path that leads
/// elsewhere because a folder above it was swapped for a link, are left, on every system. Then
/// it goes with `Provider::delete_leaf`, which never removes a folder: an item that turned into
/// one after those checks stays with everything in it, and so does a folder the path leads to
/// by then. The provider's removal of a whole tree is never used. What remains is the race of
/// the module doc: a swap between those checks and the delete can still make the path lead to
/// one other file, which then goes in its place.
fn delete_leaf(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    path: &VPath,
    entry: &Entry,
    parent: Option<Parent<'_>>,
) -> Flow {
    let deleted = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        ctx.working_on(path);
        let own = restat(&*provider, path, entry);
        if checked(&*provider, parent, own)?.is_none() {
            return Ok(None);
        }
        unless_gone(
            provider.delete_leaf(path).at(Stage::Delete),
            parent.is_some(),
        )
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

/// Lists the folder `path` in `parent`, which `entry` describes, for [`delete_dir`]: what
/// `stat` finds for it, and its children. `None` when it is gone.
///
/// It must still be a folder of its own (see [`restat`]), in a folder that is still the one
/// the walk listed (see [`checked`]), and on the volume of that folder, unless either volume is
/// unknown: a folder on another volume (a mount point) fails as `Stage::Delete` with
/// `Unsupported`. After the listing it is checked once more against what `stat` found before
/// it: if it, or a folder above it, was swapped for a link while it was listed, the listing
/// came from somewhere else, and the step fails before any child is touched.
fn open_dir(
    ctx: &mut Ctx<'_>,
    provider: &dyn Provider,
    path: &VPath,
    entry: &Entry,
    parent: Option<Parent<'_>>,
) -> Result<Option<(Entry, Vec<Entry>)>, Failed> {
    let own = restat(provider, path, entry);
    let Some(before) = checked(provider, parent, own)? else {
        return Ok(None);
    };
    let above = parent.and_then(|parent| parent.entry.volume_id);
    if on_another_volume(above, before.volume_id) {
        let error = Error::Unsupported(format!("{path} is on another volume"));
        return Err(Failed::new(Stage::Delete, error));
    }
    ctx.working_on(path);
    let child = parent.is_some();
    let listed = ctx.list(provider, path).at(Stage::Delete);
    let Some(children) = unless_gone(listed, child)? else {
        return Ok(None);
    };
    if unless_gone(restat(provider, path, &before), child)?.is_none() {
        return Ok(None);
    }
    Ok(Some((before, children)))
}

/// Deletes the folder `path` in `parent`, which `entry` describes, after everything in it.
///
/// One step lists it (see [`open_dir`]); a folder on another volume, or one that is no longer
/// a folder of its own, is neither entered nor removed. Each child is then deleted (see
/// [`delete_entry`]), checking this folder against what `stat` found when it was listed. Once
/// each child has gone with everything in it, the folder is looked at and its folder checked
/// once more, as for a leaf, and it is removed with `Provider::remove_dir`, which removes only
/// an empty folder. A folder with a child that failed or stayed is kept without a failure of
/// its own; one that another program put a file in meanwhile fails to be removed
/// (`DirectoryNotEmpty`) and keeps that file.
fn delete_dir(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    parent: Option<Parent<'_>>,
) -> Flow {
    let opened = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        open_dir(ctx, &*provider, path, entry, parent)
    });
    let (before, children) = match opened {
        Attempt::Done(Some(opened)) => opened,
        Attempt::Done(None) => return ControlFlow::Continue(true),
        other => return kept(&other),
    };
    let here = level.enter(None);
    let this = Parent {
        path,
        entry: &before,
    };
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
        all_gone &= delete_entry(ctx, errors, &child_path, child, here.below(), Some(this))?;
    }
    if !all_gone {
        return ControlFlow::Continue(false);
    }
    let removed = ctx.attempt(errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        let own = restat(&*provider, path, &before);
        if checked(&*provider, parent, own)?.is_none() {
            return Ok(None);
        }
        unless_gone(
            provider.remove_dir(path).at(Stage::Delete),
            parent.is_some(),
        )
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
    use crate::exec::{ExecEnv, ExecEvent, execute};
    use crate::id::JobId;
    use crate::report::{JobReport, Outcome};
    use crate::resolver::NonInteractive;
    use crate::spec::JobSpec;
    use crate::testkit::{tree, write_tree};

    /// Deletes `root/selected` for good while `root/dir` is swapped for a link to
    /// `root/outside` at `when`. The swapping provider is armed once the plan is made, and
    /// lists entries without identities, as the local provider does on Windows, if `no_ids`.
    fn delete_while_swapping(root: &Path, selected: &str, when: When, no_ids: bool) -> JobReport {
        let dir = root.join("dir");
        let local = Arc::new(LocalProvider::new());
        let mut swapping = Swapping::new(local, &dir, &root.join("outside"), when);
        if no_ids {
            swapping = swapping.without_listed_ids();
        }
        let swapping = Arc::new(swapping);
        let vfs = swapping.vfs();
        let on = |event: ExecEvent| {
            if matches!(event, ExecEvent::Planned { .. }) {
                swapping.arm();
            }
        };
        let job = JobSpec::Delete(DeleteSpec {
            paths: vec![VPath::local(root.join(selected)).unwrap()],
            method: DeleteMethod::Permanent,
            errors: ErrorPolicy::Continue,
        });
        let env = ExecEnv::new(&vfs, &NonInteractive).on_event(&on);
        let report = execute(JobId(1), &job, &env);
        assert!(dir.is_symlink(), "the swap did not happen");
        report
    }

    fn local(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    /// The one failure of `report` is the refusal of `source`, which changed while it was
    /// deleted, or whose folder did.
    fn refused(report: &JobReport, source: &Path) {
        assert_eq!(report.outcome, Outcome::CompletedWithErrors);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        let failure = &report.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (Stage::Delete, ErrorKind::Unsupported, Some(local(source)))
        );
        assert!(
            failure.message.contains("changed while it was deleted"),
            "{}",
            failure.message
        );
    }

    /// Where `root/dir` went when it was swapped.
    fn moved(root: &Path) -> PathBuf {
        Swapping::moved(&root.join("dir"))
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
        let report = delete_while_swapping(t.path(), "dir", When::Listing(1), false);
        assert_eq!(tree(&t.path().join("outside")), before);
        refused(&report, &t.path().join("dir"));
    }

    /// A leaf whose path leads to a folder by the time it is deleted (a folder above it was
    /// swapped for a link after the leaf was checked) takes nothing with it: `delete_leaf`
    /// never removes a folder.
    #[test]
    fn a_leaf_whose_path_leads_to_a_folder_by_then_removes_nothing() {
        let t = tempfile::tempdir().unwrap();
        write_tree(t.path(), &[("dir/c", "C"), ("outside/c/keep.txt", "K")]);
        let before = tree(&t.path().join("outside"));
        let report = delete_while_swapping(t.path(), "dir", When::DeletingALeaf, false);
        assert_eq!(tree(&t.path().join("outside")), before);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        let failure = &report.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (
                Stage::Delete,
                ErrorKind::IsADirectory,
                Some(local(&t.path().join("dir").join("c")))
            )
        );
    }

    /// One swap of a folder above an item, made after that folder's own checks and never
    /// undone, is caught by the check of the item's folder before the item is touched, also
    /// when listings carry no identities, as on Windows.
    #[test]
    fn one_swap_above_a_child_is_caught_with_or_without_listed_identities() {
        for no_ids in [false, true] {
            let t = tempfile::tempdir().unwrap();
            write_tree(
                t.path(),
                &[
                    ("dir/sub/x.txt", "X"),
                    ("outside/sub/keep1.txt", "K1"),
                    ("outside/sub/deep/keep2.txt", "K2"),
                    ("outside/top.txt", "T"),
                ],
            );
            let before = tree(&t.path().join("outside"));
            let sub = t.path().join("dir").join("sub");
            // `dir` goes right before the run first looks at `dir/sub`.
            let when = When::Stat {
                of: sub.clone(),
                nth: 0,
            };
            let report = delete_while_swapping(t.path(), "dir", when, no_ids);
            assert_eq!(tree(&t.path().join("outside")), before, "no_ids: {no_ids}");
            refused(&report, &sub);
            let x = moved(t.path()).join("sub").join("x.txt");
            assert_eq!(std::fs::read_to_string(x).unwrap(), "X");
        }
    }

    /// A child that reads as gone because a folder above it was swapped for a link, to a place
    /// where it is missing, is not taken for gone: the check of its folder comes first, so its
    /// folder is never removed through the link, and the empty folder of the same name there
    /// stays.
    #[test]
    fn a_child_seen_as_gone_through_a_swap_never_frees_its_folder() {
        for no_ids in [false, true] {
            let t = tempfile::tempdir().unwrap();
            write_tree(
                t.path(),
                &[
                    ("dir/sub/x.txt", "X"),
                    ("outside/sub/", ""),
                    ("outside/top.txt", "T"),
                ],
            );
            let before = tree(&t.path().join("outside"));
            let x = t.path().join("dir").join("sub").join("x.txt");
            let when = When::Stat {
                of: x.clone(),
                nth: 0,
            };
            let report = delete_while_swapping(t.path(), "dir", when, no_ids);
            assert_eq!(tree(&t.path().join("outside")), before, "no_ids: {no_ids}");
            refused(&report, &x);
            let left = moved(t.path()).join("sub").join("x.txt");
            assert_eq!(std::fs::read_to_string(left).unwrap(), "X");
        }
    }

    /// A folder whose path leads elsewhere by the time it is to be removed (a folder above it
    /// was swapped for a link after its children went) is kept, and the empty folder of the same
    /// name the path leads to by then stays.
    #[test]
    fn a_folder_swapped_before_its_removal_is_kept() {
        for no_ids in [false, true] {
            let t = tempfile::tempdir().unwrap();
            write_tree(
                t.path(),
                &[
                    ("dir/sub/x.txt", "X"),
                    ("outside/sub/", ""),
                    ("outside/top.txt", "T"),
                ],
            );
            let before = tree(&t.path().join("outside"));
            let sub = t.path().join("dir").join("sub");
            // The run looks at `dir/sub` when it lists it, after the listing, when it deletes
            // `x.txt`, and then right before it removes it.
            let when = When::Stat {
                of: sub.clone(),
                nth: 3,
            };
            let report = delete_while_swapping(t.path(), "dir", when, no_ids);
            assert_eq!(tree(&t.path().join("outside")), before, "no_ids: {no_ids}");
            refused(&report, &sub);
            let kept = moved(t.path()).join("sub");
            assert_eq!(tree(&kept), std::collections::BTreeMap::new());
        }
    }

    /// The same for a selected folder, which has no folder of the job above it to check: it is
    /// looked at again right before it is removed, so a swap of a folder above it never removes
    /// the empty folder of the same name its path leads to by then.
    #[test]
    fn a_selected_folder_swapped_before_its_removal_is_kept() {
        for no_ids in [false, true] {
            let t = tempfile::tempdir().unwrap();
            write_tree(
                t.path(),
                &[
                    ("dir/sub/x.txt", "X"),
                    ("outside/sub/", ""),
                    ("outside/top.txt", "T"),
                ],
            );
            let before = tree(&t.path().join("outside"));
            let sub = t.path().join("dir").join("sub");
            // As above: listed, after the listing, `x.txt`'s check, right before the removal.
            let when = When::Stat {
                of: sub.clone(),
                nth: 3,
            };
            let report = delete_while_swapping(t.path(), "dir/sub", when, no_ids);
            assert_eq!(tree(&t.path().join("outside")), before, "no_ids: {no_ids}");
            refused(&report, &sub);
            let kept = moved(t.path()).join("sub");
            assert_eq!(tree(&kept), std::collections::BTreeMap::new());
        }
    }
}
