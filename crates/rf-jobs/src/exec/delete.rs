//! Running a delete (OPS-03): each selected item to the trash, or each one for good with
//! everything in it, a folder only once it is empty. A delete only ever removes what it was
//! given and what is in it: it never follows a link, never enters another volume and never
//! turns a trash into a permanent delete.
//!
//! In sequential use, while nothing else changes the tree, that holds for every item. Against
//! another program changing the tree while it is deleted, each step of a permanent delete looks
//! at its item again, then checks that the folder the item is in is still the folder the walk
//! listed, and only then acts (see [`restat`] and [`checked`]); a folder is also checked right
//! after it is listed and right before it is removed. A leaf goes with
//! `Provider::delete_leaf`, which never removes a folder, and a folder with
//! `Provider::remove_dir`, which removes only an empty one, so nothing takes a tree with it.
//!
//! The folder is checked by `stat`. Where the file system has identities, `stat` gives one on
//! every system, also on Windows, whose listings carry none, so a swap of any folder above an
//! item for a link is caught there as on Unix: the folder itself turns into a link (another
//! kind), and the folder its path leads to below a swapped one is another item. On a file
//! system without identities (FAT, some network file systems) only the kind is left to tell:
//! a swap of the item's own folder for a link is still caught, since the folder becomes a
//! link, but one of a folder further above makes the path lead to another folder that looks
//! the same, and the step goes on there.
//!
//! What remains, on every system now, is the race Unix listings already left: a swap timed
//! between two system calls of a step, after its last check and before its delete, or undone
//! again before the next check, can still redirect one call that names its item by path, to one
//! other file or one empty folder. Closing that needs a walk relative to open folder handles
//! (`openat`, `unlinkat`), the follow-up for this.

use std::cell::Cell;
use std::ops::ControlFlow;
use std::time::SystemTime;

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath, VolumeId};
use rf_vfs::{DeleteMode, Deleted, Provider};

use super::{
    At, Attempt, Ctx, Failed, Level, MAX_DEPTH, Parent, Stopped, Walking, on_another_volume,
    other_volume, too_deep, volume_of,
};
use crate::copies::{self, Compared, CopyFound};
use crate::report::{FailureReason, Stage, Totals};
use crate::spec::{DeleteCopiesSpec, DeleteMethod, DeleteSpec, ErrorPolicy};
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
                let walk = Walk {
                    errors: spec.errors,
                    guard: None,
                    recheck: None,
                };
                let deleted = delete_entry(ctx, walk, path, entry, Level::TOP, None, None);
                if deleted.is_break() {
                    break;
                }
            }
        }
    }
}

/// Deletes for good each copy of `spec` that is still as its copy job left it and whose
/// original still holds everything it holds, as a permanent delete deletes an item (see
/// [`delete_entry`]), under `ErrorPolicy::Continue`. The plan counts each copy that is there,
/// with everything in it (see [`count_tree`]). A cancel stops the job between two steps.
///
/// Right before a copy goes, one step looks at it again: `stat`, then
/// `copies::check_found` on what `stat` gave, which the delete then works from. A copy that is
/// gone is skipped. One found any other way than [`CopyFound::SAME`] stays, and fails as
/// `Stage::Delete` with `Unsupported` and the reason `CopyKept`: a copy or an original changed
/// after the job was submitted, or before a job a crash cut off runs again, keeps the copy. In
/// a job that may have run before ([`DeleteCopiesSpec::resumed`]: a retry, or a job a crash cut
/// off), a copied folder found changed may be one it already partly deleted, so its reason is
/// `CopyPartlyDeleted` then.
///
/// The delete itself holds every item of the copy to the look (see [`Guard`]) at the step that
/// deletes or lists it, after that step's checkpoint: a change made while the job was paused
/// after the look, or between two steps of its walk, keeps the item and the folders above it,
/// each kept item failing with `CopyKept` (the copy changed). A folder that another program put
/// an item in after its listing fails to be removed and counts as changed in the same way.
///
/// Once the job has been paused after the look of a copy, the original may have changed too:
/// each step that then deletes an item of that copy first compares the item with its original
/// again (see [`Recheck`]). The first item whose original is gone or changed stays and fails
/// with `CopyKept` (the original changed or went); the rest of that copy stays with it. Between
/// two steps of a walk the job was not paused in, only the copy is held to the look.
pub(crate) fn run_delete_copies(ctx: &mut Ctx<'_>, spec: &DeleteCopiesSpec) {
    let mut totals = Totals::default();
    for copy in &spec.copies {
        let path = &copy.made.path;
        let found = ctx
            .provider(path)
            .and_then(|provider| Ok((provider.stat(path)?, provider)));
        let Ok((entry, provider)) = found else {
            continue;
        };
        let counted = count_tree(ctx, &*provider, path, &entry, Level::TOP, None, &mut totals);
        if counted.is_err() {
            ctx.stop(Stopped::Cancelled);
            return;
        }
    }
    ctx.planned(totals);
    for copy in &spec.copies {
        let path = &copy.made.path;
        let looked = ctx.attempt(ErrorPolicy::Continue, Some(path), None, |ctx| {
            ctx.check().at(Stage::Delete)?;
            let provider = ctx.provider(path).at(Stage::Delete)?;
            let now = match provider.stat(path) {
                Ok(now) => now,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(Failed::new(Stage::Delete, error)),
            };
            ctx.working_on(path);
            // The pauses so far: one after this look invites a change of the original.
            let pauses = ctx.env.control.pauses();
            let cancel = ctx.env.control.cancel_token();
            let found = copies::check_found(
                ctx.env.vfs,
                &*provider,
                &copy.made,
                &now,
                spec.at,
                &copy.original,
                &cancel,
            );
            // A look that a cancel cut short stops the job, rather than keep the copy for it.
            ctx.check().at(Stage::Delete)?;
            if found == CopyFound::SAME {
                Ok(Some((now, pauses)))
            } else if spec.resumed && copy.made.is_dir && found.copy != Compared::Same {
                Err(partly_deleted(path))
            } else {
                Err(copy_kept(path, found))
            }
        });
        let flow = match looked {
            Attempt::Done(Some((now, pauses))) => {
                let recheck = Recheck {
                    pauses,
                    lost: Cell::new(false),
                };
                let walk = Walk {
                    errors: ErrorPolicy::Continue,
                    guard: Some(Guard {
                        modified: now.modified,
                        size: now.size,
                        at: spec.at,
                    }),
                    recheck: Some(&recheck),
                };
                let original = Some(&copy.original);
                delete_entry(ctx, walk, path, &now, Level::TOP, None, original)
            }
            Attempt::Done(None) => {
                ctx.report.skipped += 1;
                ControlFlow::Continue(true)
            }
            other => kept(&other),
        };
        if flow.is_break() {
            break;
        }
    }
}

/// The failure of the copy `path` that a delete of copies leaves, found as `found`.
fn copy_kept(path: &VPath, found: CopyFound) -> Failed {
    let error = Error::Unsupported(format!(
        "{path} is not as it was copied, or its original is not ({found:?}); left in place"
    ));
    Failed::refused(Stage::Delete, error, FailureReason::CopyKept(found))
}

/// The failure of the item `path` of a copy that changed while a delete of copies deleted the
/// copy (see [`Guard`]): `CopyKept`, the copy changed.
fn changed_copy(path: &VPath) -> Failed {
    let found = CopyFound {
        copy: Compared::Changed,
        original: Compared::Unknown,
    };
    copy_kept(path, found)
}

/// The failure of the copied folder `path` that a resumed delete of copies finds changed, as it
/// would be after that job had deleted part of it: `CopyPartlyDeleted`.
fn partly_deleted(path: &VPath) -> Failed {
    let error = Error::Unsupported(format!(
        "{path} may have been partly deleted already; the rest cannot be compared safely and is \
         left in place"
    ));
    Failed::refused(Stage::Delete, error, FailureReason::CopyPartlyDeleted)
}

/// How a permanent delete walks: its error policy, and for a delete of copies what it holds
/// each item to before the item goes.
#[derive(Clone, Copy, Debug)]
struct Walk<'a> {
    errors: ErrorPolicy,
    guard: Option<Guard>,
    recheck: Option<&'a Recheck>,
}

/// What a delete of copies holds each item of one copy to besides the [`Guard`], once the job
/// has been paused since the copy's look ([`Control::pauses`](crate::Control)): the
/// item's original must still hold it. While the job is paused anyone may delete or change the
/// original, and deleting the copy then would lose what only the copy holds. A file or a link
/// is compared with its original as the look compares them (`copies::original_beside`); a
/// copied folder, which goes only once it is empty, needs only its original to be a folder
/// still (anything put in the copy keeps it as a changed copy, see [`delete_dir`]). Without a
/// pause since the look, the original is not looked at again.
#[derive(Debug)]
struct Recheck {
    /// How many times the job had been paused when the look compared the original.
    pauses: u64,
    /// Set once an item's original was found gone or changed: the rest of the copy stays.
    lost: Cell<bool>,
}

impl Recheck {
    /// Whether the item `path` of the copy, which its step's `stat` found as `now`, may go: its
    /// original `twin` (the original of the copy, or the item of the same names below it) still
    /// holds it, unless the job has not paused since the look. Fails with `CopyKept` (the
    /// original gone or changed, or a `twin` that cannot be named) and keeps the rest of the
    /// copy.
    fn allows(
        &self,
        ctx: &Ctx<'_>,
        provider: &dyn Provider,
        path: &VPath,
        now: &Entry,
        twin: Option<&VPath>,
    ) -> Result<(), Failed> {
        if ctx.env.control.pauses() == self.pauses {
            return Ok(());
        }
        let original = match twin {
            Some(twin) if now.kind == EntryKind::Dir => still_a_folder(ctx, twin),
            Some(twin) => {
                let cancel = ctx.env.control.cancel_token();
                let vfs = ctx.env.vfs;
                let original = copies::original_beside(vfs, provider, path, now, twin, &cancel);
                // A look that a cancel cut short stops the job, rather than keep the item for it.
                ctx.check().at(Stage::Delete)?;
                original
            }
            None => Compared::Unknown,
        };
        if original == Compared::Same {
            return Ok(());
        }
        self.lost.set(true);
        let found = CopyFound {
            copy: Compared::Same,
            original,
        };
        Err(copy_kept(path, found))
    }
}

/// How the original folder `twin` of an emptied copied folder is: `Same` while it is a folder,
/// `Missing` when it is gone, `Changed` when it is something else, and `Unknown` when it cannot
/// be looked at.
fn still_a_folder(ctx: &Ctx<'_>, twin: &VPath) -> Compared {
    let Ok(provider) = ctx.provider(twin) else {
        return Compared::Unknown;
    };
    match provider.stat(twin) {
        Ok(entry) if entry.kind == EntryKind::Dir => Compared::Same,
        Ok(_) => Compared::Changed,
        Err(error) if error.kind() == ErrorKind::NotFound => Compared::Missing,
        Err(_) => Compared::Unknown,
    }
}

/// Whether the walk keeps the rest of its copy: an item's original was found gone or changed
/// ([`Recheck`]). Its items then stay without a failure of their own.
fn original_lost(walk: Walk<'_>) -> bool {
    walk.recheck.is_some_and(|recheck| recheck.lost.get())
}

/// What a delete of copies holds each item of a copy to, at the step that deletes or lists it,
/// after that step's checkpoint (where a pause waits) and on what that step's own `stat` finds.
/// The copy itself must still be as the job's look found it: of the same modification time and,
/// for a file, size. Each item below it must be no newer than the copy job's end `at`, the rule
/// of the look (`copies::as_left`): the copy gave every item the time of its source, and
/// anything changed since has a later one.
///
/// Below the copy only that time is checked, not the size, the content or the identity. So an
/// edit that also sets an item's modification time back to no later than `at` (a tool that
/// keeps times, `touch -d`) is not seen, and the item is deleted: the same limit as the look's
/// rule (`untouched_since`). The folder that holds an item added or taken away still shows it,
/// unless its own time was set back too.
#[derive(Clone, Copy, Debug)]
struct Guard {
    modified: Option<SystemTime>,
    size: u64,
    at: Option<SystemTime>,
}

impl Guard {
    /// Whether the item at `level` of the walk (`Level::TOP` for the copy itself), which a
    /// step's `stat` finds as `now`, may go.
    fn allows(self, level: Level<'_>, now: &Entry) -> bool {
        if level.depth() == 0 {
            now.modified.is_some()
                && now.modified == self.modified
                && (now.kind == EntryKind::Dir || now.size == self.size)
        } else {
            matches!((now.modified, self.at), (Some(modified), Some(at)) if modified <= at)
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

/// The trash of `path` by `provider`, as a step at `Stage::Delete`: a refusal of the trash
/// (`Unsupported`: no trash there, or one that refuses the item) has the reason
/// `TrashRefused`, and the item stays. It is never deleted for good instead (OPS-03).
pub(crate) fn trash_one(provider: &dyn Provider, path: &VPath) -> Result<Deleted, Failed> {
    provider.delete(path, DeleteMode::Trash).map_err(|error| {
        let refused = error.kind() == ErrorKind::Unsupported;
        let failed = Failed::new(Stage::Delete, error);
        if refused {
            failed.because(FailureReason::TrashRefused)
        } else {
            failed
        }
    })
}

/// The warning of an item `path` that a provider deleted for good when it was asked to trash
/// it: it is gone, and no undo brings it back.
pub(crate) fn deleted_not_trashed(path: &VPath) -> Failed {
    let error = Error::Unsupported(format!("{path} was deleted for good, not trashed"));
    Failed::refused(Stage::Delete, error, FailureReason::DeletedNotTrashed)
}

/// Sends each of `items` to the trash, as a step of its own under `errors` (`Stage::Delete`).
///
/// A path the trash cannot take fails, `Unsupported` (no trash there, or one that refuses the
/// item) as much as any other error, and stays where it is: it is never deleted for good
/// instead (OPS-03). The report's undo record lists every path that went, with where the trash
/// holds it when the system tells (`UndoRecord::Trashed`); `None` when none went. A provider
/// that deletes an item for good when it is asked to trash it leaves a `Stage::Delete` warning
/// (`DeletedNotTrashed`) and no undo record for that item.
fn trash(ctx: &mut Ctx<'_>, errors: ErrorPolicy, items: &[(VPath, Entry)]) {
    let mut trashed = Vec::new();
    for (path, entry) in items {
        let deleted = ctx.attempt(errors, Some(path), None, |ctx| {
            ctx.check().at(Stage::Delete)?;
            let provider = ctx.provider(path).at(Stage::Delete)?;
            ctx.working_on(path);
            trash_one(&*provider, path)
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
                ctx.warn(deleted_not_trashed(path), Some(path), None);
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
/// an attempt under the walk's error policy that checks for a pause or a cancel first, so a
/// failure, a retry and the progress go item by item. Each step looks at its item, then checks
/// its folder (see [`checked`]), then holds the item to the walk's [`Guard`] if it has one, and
/// only then acts. An item below a path of the job that is gone by the time a
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
    walk: Walk<'_>,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    parent: Option<Parent<'_>>,
    twin: Option<&VPath>,
) -> Flow {
    if let Some(failed) = too_deep(ctx, level, walk.errors, path, None) {
        return kept(&failed);
    }
    if entry.kind == EntryKind::Dir {
        delete_dir(ctx, walk, path, entry, level, parent, twin)
    } else {
        delete_leaf(ctx, walk, path, entry, level, parent, twin)
    }
}

/// `stat`s `path` of `provider` again and gives what it finds (`super::restat`). Fails as
/// `Stage::Delete`, leaving the item in place, when it is no longer the item `entry` described
/// (see `super::replaced`): of another kind (a file turned into a folder, a folder into a link
/// to one) or, when both identities are known, another item. The failure says the item
/// "changed while it was deleted" (`ChangedWhileDeleted`).
fn restat(provider: &dyn Provider, path: &VPath, entry: &Entry) -> Result<Entry, Failed> {
    super::restat(provider, path, entry, Stage::Delete, Walking::Deleting)
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
    walk: Walk<'_>,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    parent: Option<Parent<'_>>,
    twin: Option<&VPath>,
) -> Flow {
    if original_lost(walk) {
        return ControlFlow::Continue(false);
    }
    let deleted = ctx.attempt(walk.errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        ctx.working_on(path);
        let own = restat(&*provider, path, entry);
        let Some(now) = checked(&*provider, parent, own)? else {
            return Ok(None);
        };
        if let Some(guard) = walk.guard
            && !guard.allows(level, &now)
        {
            return Err(changed_copy(path));
        }
        if let Some(recheck) = walk.recheck {
            recheck.allows(ctx, &*provider, path, &now, twin)?;
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
/// came from somewhere else, and the step fails before any child is touched. Under a `guard`
/// (a delete of copies, at `level` of its walk), what `stat` finds before the listing must pass
/// it, or the folder is kept as a changed copy.
fn open_dir(
    ctx: &mut Ctx<'_>,
    provider: &dyn Provider,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    parent: Option<Parent<'_>>,
    guard: Option<Guard>,
) -> Result<Option<(Entry, Vec<Entry>)>, Failed> {
    let own = restat(provider, path, entry);
    let Some(before) = checked(provider, parent, own)? else {
        return Ok(None);
    };
    let above = parent.and_then(|parent| parent.entry.volume_id);
    if on_another_volume(above, before.volume_id) {
        return Err(other_volume(Stage::Delete, path));
    }
    if let Some(guard) = guard
        && !guard.allows(level, &before)
    {
        return Err(changed_copy(path));
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
/// (`DirectoryNotEmpty`) and keeps that file, and under a [`Guard`] it is kept as a changed
/// copy.
fn delete_dir(
    ctx: &mut Ctx<'_>,
    walk: Walk<'_>,
    path: &VPath,
    entry: &Entry,
    level: Level<'_>,
    parent: Option<Parent<'_>>,
    twin: Option<&VPath>,
) -> Flow {
    if original_lost(walk) {
        return ControlFlow::Continue(false);
    }
    let opened = ctx.attempt(walk.errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        open_dir(ctx, &*provider, path, entry, level, parent, walk.guard)
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
                let failed = ctx.attempt(walk.errors, Some(path), None, |_| {
                    Err::<(), _>(Failed::new(Stage::Delete, error.clone().into()))
                });
                kept(&failed)?;
                all_gone = false;
                continue;
            }
        };
        // The original of the child: the item of the same name in this folder's original.
        let child_twin = twin.and_then(|twin| twin.join(&child.name).ok());
        all_gone &= delete_entry(
            ctx,
            walk,
            &child_path,
            child,
            here.below(),
            Some(this),
            child_twin.as_ref(),
        )?;
    }
    if !all_gone {
        return ControlFlow::Continue(false);
    }
    if original_lost(walk) {
        return ControlFlow::Continue(false);
    }
    let removed = ctx.attempt(walk.errors, Some(path), None, |ctx| {
        ctx.check().at(Stage::Delete)?;
        let provider = ctx.provider(path).at(Stage::Delete)?;
        let own = restat(&*provider, path, &before);
        let Some(now) = checked(&*provider, parent, own)? else {
            return Ok(None);
        };
        // Emptied by now: its original must still be a folder, if the job paused since the look.
        if let Some(recheck) = walk.recheck {
            recheck.allows(ctx, &*provider, path, &now, twin)?;
        }
        // Its own time changed as its items went; only an item put in since can stop it now.
        let removed = match provider.remove_dir(path) {
            Err(error) if walk.guard.is_some() && error.kind() == ErrorKind::DirectoryNotEmpty => {
                return Err(changed_copy(path));
            }
            removed => removed,
        };
        unless_gone(removed.at(Stage::Delete), parent.is_some())
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
        assert_eq!(failure.reason, Some(FailureReason::ChangedWhileDeleted));
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
